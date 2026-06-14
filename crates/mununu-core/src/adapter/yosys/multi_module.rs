//! R-MM (KMTS multi-module composition) — the netlist-driven driver that
//! composes per-module KMTSes into one product for verification.
//!
//! Each submodule's BTOR2 is lifted independently (one `Clts` per module
//! *type*); the top module's netlist (parsed by
//! [`super::parse_instance_connections`]) says which instance port connects
//! to which net. To make two instances that share a net rendezvous under
//! [`crate::composition::compose`] — whose synchronisation is name-equality
//! on label payloads — each instance's labels are rewritten so the shared
//! net carries a single value-encoded name `<net>_<v>` on *both* sides:
//!
//! - A **reader** input port `P` connected to net `N` has labels
//!   `P_<v>` (the lift value-encodes inputs); they are renamed to `N_<v>`.
//! - A **driver** output port `P` connected to net `N` has *no* label (the
//!   lift drops outputs — see R-MM-4b); its value is surfaced as a per-state
//!   valuation `P = T/F` (R-MM-4b's `surface_output_ports`), from which we
//!   synthesise a `N_<v>` label on every transition leaving that state.
//!   This is the `annotate_driving_output_labels` analog the native path
//!   performs.
//! - Every other label (a free top-level input like `enable`/`pop`) is
//!   instance-namespaced (`<instance>__<label>`) so two instances of the
//!   *same* module type don't accidentally rendezvous on a same-named local
//!   input. The clock-step fallback `step` is kept verbatim — modules with
//!   no free inputs all step together on the clock.
//!
//! Modality (may/must), structured valuations, and 3-valued predicates are
//! preserved (the composed property reads them).

use crate::clts::{Clts, CltsResult, DefaultLabelIdx, DefaultStateIdx, LabelId, StateId};
use smallvec::SmallVec;
use std::collections::HashMap;

/// The clock-step fallback label the bit-blaster emits for a module with no
/// free (non-clock, non-reset) inputs. It is shared across such modules so
/// they advance together on the clock — never instance-namespaced.
const STEP_LABEL: &str = "step";

/// R-MM-4c — Rewrite one instance's KMTS into a composition-ready form:
/// reader inputs and driver outputs are recast to shared `<net>_<v>` labels;
/// free local inputs are instance-namespaced. See the module docs.
///
/// * `reader_ports` — input-port → net for ports this instance *reads*.
/// * `driver_ports` — output-port → net for ports this instance *drives*.
///   The output value per source state comes from that state's valuation
///   (R-MM-4b surfaced it as `<port> = T/F`); states without the valuation
///   contribute no driver label (best-effort — the e2e path asserts it is
///   present).
/// * `instance` — the instance name, used to namespace free local labels.
pub fn prepare_instance_for_composition(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    reader_ports: &HashMap<String, String>,
    driver_ports: &HashMap<String, String>,
    instance: &str,
) -> CltsResult<Clts<DefaultStateIdx, DefaultLabelIdx>> {
    let mut builder = Clts::builder();

    // 1. Copy states (names, initial flags, variables).
    let mut state_mapping: HashMap<StateId<DefaultStateIdx>, StateId<DefaultStateIdx>> =
        HashMap::new();
    for state in clts.states() {
        let name = clts.state_name(state).unwrap_or("state").to_owned();
        if let Some(new_id) = builder.state_with_name(name) {
            if clts.initial_states().contains(&state) {
                builder.initial_state_id(new_id);
            }
            let vars = clts.state_variables(state);
            if !vars.is_empty() {
                builder.with_variables_for_state(new_id, vars.iter().map(|s| s.as_str()));
            }
            state_mapping.insert(state, new_id);
        }
    }

    // 2. Copy transitions, rewriting labels + synthesising driver labels.
    for state in clts.states() {
        let &source_new = match state_mapping.get(&state) {
            Some(id) => id,
            None => continue,
        };
        // Driver labels depend only on the SOURCE state's output values
        // (Moore) or its split-variant value (Mealy, already definite per
        // split-state) — compute once per source state.
        let driver_symbols = driver_labels_for_state(clts, state, driver_ports);

        for transition in clts.outgoing(state) {
            let &target_new = match state_mapping.get(&transition.target()) {
                Some(id) => id,
                None => continue,
            };
            let mut new_labels: SmallVec<[LabelId<DefaultLabelIdx>; 4]> = SmallVec::new();
            // 2a. Rewrite the transition's own labels (reader inputs → net;
            //     step kept; free inputs namespaced).
            for &label_id in transition.labels() {
                let payload = match clts.label_payload(label_id) {
                    Some(p) => p,
                    None => continue,
                };
                let renamed: Vec<String> = payload
                    .iter()
                    .map(|sym| rewrite_label_symbol(sym, reader_ports, instance))
                    .collect();
                let new_label_id = builder
                    .labels()
                    .intern(renamed.iter().map(|s| s.as_str()))?;
                if let Some(ctrl) = clts.label_controllability(label_id) {
                    builder.set_label_controllability(new_label_id, ctrl);
                }
                new_labels.push(new_label_id);
            }
            // 2b. Add the synthesised driver-output labels for this step.
            for sym in &driver_symbols {
                let id = builder.labels().intern([sym.as_str()])?;
                new_labels.push(id);
            }
            builder.transition_ids_with_modality(
                source_new,
                &new_labels,
                target_new,
                transition.modality().clone(),
            );
        }
    }

    // 3. Copy structured valuations + 3-valued predicates verbatim.
    for state in clts.states() {
        if let (Some(&new_id), Some(valuation)) =
            (state_mapping.get(&state), clts.state_valuation(state))
        {
            builder.with_valuation_for_state(new_id, valuation.clone());
        }
    }
    for (state, predicate, verdict) in clts
        .states()
        .flat_map(|s| {
            clts.state_3valued_predicate_entries(s)
                .into_iter()
                .map(move |(p, v)| (s, p.to_string(), v))
        })
        .collect::<Vec<_>>()
    {
        if let Some(&new_id) = state_mapping.get(&state) {
            builder.with_3valued_predicate(new_id, predicate, verdict);
        }
    }

    builder.build()
}

/// Rewrite one label symbol for [`prepare_instance_for_composition`]:
/// a reader port's `<port>_<v>` → `<net>_<v>`; the `step` clock label is
/// kept; anything else is instance-namespaced.
fn rewrite_label_symbol(
    symbol: &str,
    reader_ports: &HashMap<String, String>,
    instance: &str,
) -> String {
    if symbol == STEP_LABEL {
        return STEP_LABEL.to_string();
    }
    if let Some(net_symbol) = relabel_reader_symbol(symbol, reader_ports) {
        return net_symbol;
    }
    format!("{instance}__{symbol}")
}

/// If `symbol` is `<port>_<suffix>` for some reader `port` (longest match
/// wins), return `<net>_<suffix>`; otherwise `None`.
fn relabel_reader_symbol(symbol: &str, reader_ports: &HashMap<String, String>) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (port, net) in reader_ports {
        if let Some(suffix) = symbol
            .strip_prefix(port.as_str())
            .and_then(|rest| rest.strip_prefix('_'))
            && best.as_ref().is_none_or(|(len, _)| port.len() > *len)
        {
            best = Some((port.len(), format!("{net}_{suffix}")));
        }
    }
    best.map(|(_, s)| s)
}

/// The synthesised `<net>_<v>` driver labels for a source state: for each
/// output port this instance drives, read the port's value from the state
/// valuation (R-MM-4b's `T`/`F`, normalised to `1`/`0`) and form the shared
/// net label. States missing the valuation contribute nothing.
fn driver_labels_for_state(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    state: StateId<DefaultStateIdx>,
    driver_ports: &HashMap<String, String>,
) -> Vec<String> {
    if driver_ports.is_empty() {
        return Vec::new();
    }
    let Some(valuation) = clts.state_valuation(state) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (port, net) in driver_ports {
        if let Some(raw) = valuation.get(port) {
            out.push(format!("{net}_{}", normalize_driver_value(raw)));
        }
    }
    out
}

/// Normalise a surfaced output value to the same encoding readers use:
/// the bit-blaster surfaces 1-bit combinational outputs as `T`/`F`, but
/// input labels are `<sig>_<integer>`, so map `T`→`1`, `F`→`0`. A raw
/// integer (multi-bit, future) passes through unchanged.
fn normalize_driver_value(raw: &str) -> String {
    match raw {
        "T" => "1".to_string(),
        "F" => "0".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{
        Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability, TransitionModality,
    };

    /// Helper: alphabet (label payload symbols) of a CLTS.
    fn alphabet(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> Vec<String> {
        clts.alphabet()
    }

    #[test]
    fn reader_input_ports_are_relabelled_to_net_names() {
        // A consumer-like instance reading port `push` (value-encoded
        // labels push_0 / push_1) connected to net `valid`.
        let mut b = Clts::builder();
        let s0 = b.state_with_name("s0".into()).unwrap();
        let s1 = b.state_with_name("s1".into()).unwrap();
        b.initial_state_id(s0);
        let push0 = b.labels().intern(["push_0"]).unwrap();
        let push1 = b.labels().intern(["push_1"]).unwrap();
        b.transition_ids(s0, &[push0], s0);
        b.transition_ids(s0, &[push1], s1);
        let clts = b.build().unwrap();

        let mut readers = HashMap::new();
        readers.insert("push".to_string(), "valid".to_string());
        let out =
            prepare_instance_for_composition(&clts, &readers, &HashMap::new(), "u_buffer").unwrap();

        let alpha = alphabet(&out);
        assert!(alpha.contains(&"valid_0".to_string()), "push_0 → valid_0");
        assert!(alpha.contains(&"valid_1".to_string()), "push_1 → valid_1");
        assert!(
            !alpha.contains(&"push_0".to_string()),
            "old reader label gone"
        );
    }

    #[test]
    fn driver_output_synthesises_net_label_from_valuation() {
        // A producer-like instance driving output `valid` (net `valid`).
        // s0: valid=F → leaving transitions carry valid_0; s1: valid=T →
        // valid_1. The free input `enable` is instance-namespaced.
        let mut b = Clts::builder();
        let s0 = b.state_with_name("s0".into()).unwrap();
        let s1 = b.state_with_name("s1".into()).unwrap();
        b.initial_state_id(s0);
        let en0 = b.labels().intern(["enable_0"]).unwrap();
        let en1 = b.labels().intern(["enable_1"]).unwrap();
        b.transition_ids(s0, &[en1], s1); // from s0 (valid=F)
        b.transition_ids(s1, &[en0], s0); // from s1 (valid=T)
        let mut v0 = std::collections::BTreeMap::new();
        v0.insert("valid".to_string(), "F".to_string());
        b.with_valuation_for_state(s0, v0);
        let mut v1 = std::collections::BTreeMap::new();
        v1.insert("valid".to_string(), "T".to_string());
        b.with_valuation_for_state(s1, v1);
        let clts = b.build().unwrap();

        let mut drivers = HashMap::new();
        drivers.insert("valid".to_string(), "valid".to_string());
        let out = prepare_instance_for_composition(&clts, &HashMap::new(), &drivers, "u_producer")
            .unwrap();

        // The transition from s0 (valid=F) carries valid_0; from s1, valid_1.
        let s0n = out.state_id("s0").unwrap();
        let s1n = out.state_id("s1").unwrap();
        let labels_of = |st| {
            out.outgoing(st)
                .iter()
                .flat_map(|t| t.labels())
                .filter_map(|&l| out.label_payload(l))
                .flatten()
                .cloned()
                .collect::<Vec<String>>()
        };
        let from_s0 = labels_of(s0n);
        assert!(
            from_s0.contains(&"valid_0".to_string()),
            "s0 drives valid_0"
        );
        assert!(
            from_s0.contains(&"u_producer__enable_1".to_string()),
            "free input enable is instance-namespaced"
        );
        let from_s1 = labels_of(s1n);
        assert!(
            from_s1.contains(&"valid_1".to_string()),
            "s1 drives valid_1"
        );
    }

    #[test]
    fn step_label_is_kept_verbatim() {
        let mut b = Clts::builder();
        let s0 = b.state_with_name("s0".into()).unwrap();
        b.initial_state_id(s0);
        let step = b.labels().intern(["step"]).unwrap();
        b.set_label_controllability(step, LabelControllability::Uncontrollable);
        b.transition_ids(s0, &[step], s0);
        let clts = b.build().unwrap();

        let out = prepare_instance_for_composition(&clts, &HashMap::new(), &HashMap::new(), "u_x")
            .unwrap();
        assert!(
            alphabet(&out).contains(&"step".to_string()),
            "step kept verbatim"
        );
    }

    #[test]
    fn modality_and_predicates_are_preserved() {
        use crate::clts::Tristate;
        let mut b = Clts::builder();
        let s0 = b.state_with_name("s0".into()).unwrap();
        let s1 = b.state_with_name("s1".into()).unwrap();
        b.initial_state_id(s0);
        let lab = b.labels().intern(["enable_1"]).unwrap();
        b.transition_ids_with_modality(s0, &[lab], s1, TransitionModality::MayOnly);
        b.with_3valued_predicate(s1, "ready", Tristate::KleeneBot);
        let clts = b.build().unwrap();

        let out = prepare_instance_for_composition(&clts, &HashMap::new(), &HashMap::new(), "u_x")
            .unwrap();
        let s0n = out.state_id("s0").unwrap();
        let modality = out.outgoing(s0n)[0].modality().clone();
        assert_eq!(modality, TransitionModality::MayOnly, "modality preserved");
        let s1n = out.state_id("s1").unwrap();
        assert_eq!(
            out.state_3valued_predicate(s1n, "ready"),
            Some(Tristate::KleeneBot),
            "3-valued predicate preserved"
        );
    }
}
