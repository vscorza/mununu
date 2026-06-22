//! `Clts` → [`AdapterIR`] → CTXDSL bridge.
//!
//! The format adapters build an [`AdapterIR`] from their source AST and the
//! shared [`crate::adapter::emit`] emitter renders it to CTXDSL text. The
//! *predicate-abstraction* path (the BTOR2 predicate-cube lift, the R-MM
//! multi-module KMTS composition) instead produces a realized
//! [`Clts`] directly — there is no source AST to translate. This module is
//! the inverse seam: it reconstructs an [`AdapterIR`] from a realized `Clts`
//! so the same emitter produces faithful CTXDSL for those paths too.
//!
//! What survives the round-trip (`clts_to_ctxdsl` → `parse` → `realize`):
//! states (name + initial flag), transitions (flattened multi-label
//! payloads + modality `[may]` / `[must]` + hyper-must additional targets),
//! controllability (controllable / internal label partition), per-state
//! display `valuations`, and — the CTXDSL Phase 1b gap fix — the per-state
//! **3-valued (Kleene) predicate labels** (`state_3valued_predicates`),
//! emitted as `predicates_3v { p = true|false|unknown; }` blocks. The
//! Kleene labels are what make a predicate-cube KMTS round-trippable; before
//! Phase 1a/1b there was no CTXDSL syntax for them, so routing a cube `Clts`
//! through the emitter silently dropped the labelling.
//!
//! Not carried: mu-calculus property declarations (the cube/KMTS carries the
//! model, not the formulae — the caller supplies those separately) and the
//! `state_variable_bitset` 2-valued predicate set (it has no standalone
//! per-state CTXDSL surface; it is reconstructed by the realize step from
//! state names).

use crate::adapter::AdapterError;
use crate::adapter::SourceFormat;
use crate::adapter::emit::emit;
use crate::adapter::ir::{AdapterIR, AutomatonSpec, Metadata, StateSpec, TransitionSpec};
use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelId};
use crate::context_dsl::ast::TransitionModalitySpec;
use std::collections::HashSet;

/// Render a realized `Clts` (a single automaton) as CTXDSL text via the
/// shared [`crate::adapter::emit`] emitter.
///
/// `automaton_name` names the emitted `automaton <name> { … }`;
/// `context_name` names the wrapping `context <name> { … }`.
///
/// Carries states + transitions + controllability + modality + per-state
/// `valuations` **and** per-state 3-valued (Kleene) predicate labels
/// (`predicates_3v { … }`). For a 2-valued CLTS (no 3-valued labels) the
/// output is identical to the pre-Phase-1b emitter — the `predicates_3v`
/// block only appears for states that carry Kleene labels.
pub fn clts_to_ctxdsl(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    automaton_name: &str,
    context_name: &str,
) -> Result<String, AdapterError> {
    let state_name = |s| clts.state_name(s).unwrap_or("state").to_string();

    let states: Vec<StateSpec> = clts
        .states()
        .map(|s| {
            // CTXDSL Phase 1b — carry the per-state 3-valued labels so the
            // emitter writes a `predicates_3v { … }` block. Empty ⇒ `None`
            // ⇒ no block emitted (the 2-valued path stays byte-for-byte).
            let entries = clts.state_3valued_predicate_entries(s);
            let three_valued = if entries.is_empty() {
                None
            } else {
                Some(
                    entries
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                )
            };
            StateSpec {
                name: state_name(s),
                is_initial: clts.initial_states().contains(&s),
                valuations: clts.state_valuation(s).cloned(),
                three_valued,
            }
        })
        .collect();

    let mut transitions: Vec<TransitionSpec> = Vec::new();
    for s in clts.states() {
        let source = state_name(s);
        for t in clts.outgoing(s) {
            let mut labels: Vec<String> = Vec::new();
            for &lid in t.labels() {
                if let Some(payload) = clts.label_payload(lid) {
                    labels.extend(payload.iter().cloned());
                }
            }
            let (modality, additional_targets) = modality_to_spec(clts, t);
            transitions.push(TransitionSpec {
                source: source.clone(),
                target: state_name(t.target()),
                labels,
                modality,
                additional_targets,
            });
        }
    }

    let label_names = |set: &HashSet<LabelId<DefaultLabelIdx>>| -> Vec<String> {
        let mut v: Vec<String> = set
            .iter()
            .filter_map(|&l| clts.label_payload(l))
            .flatten()
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    };

    let automaton = AutomatonSpec {
        name: automaton_name.to_string(),
        states,
        transitions,
        controllable_labels: label_names(clts.controllable_alphabet()),
        internal_labels: label_names(clts.internal_alphabet()),
    };

    let ir = AdapterIR {
        metadata: Metadata {
            title: context_name.to_string(),
            source_format: SourceFormat::SystemVerilog,
            description: None,
            game_semantics: None,
            known_status: None,
        },
        signals: Vec::new(),
        automata: vec![automaton],
        compositions: Vec::new(),
        properties: Vec::new(),
        controller: None,
    };

    emit(&ir).map(|r| r.ctxdsl)
}

/// Map a transition's [`crate::clts::TransitionModality`] to the CTXDSL
/// emitter's [`TransitionModalitySpec`] + the additional hyper-must target
/// names (targets beyond the primary). `Sharp` / `MayOnly` carry no extra
/// targets; a `MustHyperOnly` set maps to `MustOnly` with the targets after
/// the primary surfaced as `additional_targets`.
fn modality_to_spec(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    t: &crate::clts::Transition<DefaultStateIdx, DefaultLabelIdx>,
) -> (TransitionModalitySpec, Vec<String>) {
    use crate::clts::TransitionModality;
    match t.modality() {
        TransitionModality::Sharp => (TransitionModalitySpec::Sharp, Vec::new()),
        TransitionModality::MayOnly => (TransitionModalitySpec::MayOnly, Vec::new()),
        TransitionModality::MustHyperOnly(_) => {
            let targets = t.modality().must_target_set(t.target());
            let additional: Vec<String> = targets
                .iter()
                .skip(1)
                .filter_map(|&st| clts.state_name(st).map(str::to_string))
                .collect();
            (TransitionModalitySpec::MustOnly, additional)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::TransitionModality;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability, Tristate};
    use crate::context_dsl::parse;
    use crate::context_dsl::realize::realize;

    /// CTXDSL Phase 1b — a predicate-cube-shaped `Clts` carrying per-state
    /// 3-valued (Kleene) labels round-trips through CTXDSL: the bridge emits
    /// `predicates_3v { … }` blocks, the parser accepts them (Phase 1a), and
    /// `realize` re-registers the Kleene verdicts on the rebuilt CLTS.
    #[test]
    fn cube_three_valued_labels_round_trip_through_ctxdsl() {
        // Build a tiny 2-cube KMTS: one Sharp edge, one MayOnly self-loop,
        // and a spread of Kleene labels (T / F / ⊥) on the cube states.
        let mut b = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        let c0 = b.state_id_or_insert("cube_0").expect("cube_0 id");
        let c1 = b.state_id_or_insert("cube_1").expect("cube_1 id");
        b.initial_state_id(c0);
        let tick = b.labels().intern(["tick"]).expect("tick label");
        b.set_label_controllability(tick, LabelControllability::Uncontrollable);
        b.transition_ids(c0, &[tick], c1); // Sharp
        b.transition_ids_with_modality(c1, &[tick], c1, TransitionModality::MayOnly);
        b.with_3valued_predicate(c0, "boot_idle", Tristate::KleeneT);
        b.with_3valued_predicate(c0, "boot_done", Tristate::KleeneF);
        b.with_3valued_predicate(c0, "ctr_overflow", Tristate::KleeneBot);
        b.with_3valued_predicate(c1, "boot_done", Tristate::KleeneT);
        let clts = b.build().expect("build cube clts");

        let ctxdsl = clts_to_ctxdsl(&clts, "M", "tri_roundtrip").expect("emit ctxdsl");

        // Emission: the `predicates_3v` block + all three Kleene literals +
        // the `[may]` modality suffix appear in the output.
        assert!(
            ctxdsl.contains("predicates_3v {"),
            "missing block:\n{ctxdsl}"
        );
        assert!(ctxdsl.contains("boot_idle = true;"), "{ctxdsl}");
        assert!(ctxdsl.contains("boot_done = false;"), "{ctxdsl}");
        assert!(ctxdsl.contains("ctr_overflow = unknown;"), "{ctxdsl}");
        assert!(
            ctxdsl.contains("[may]"),
            "MayOnly modality must round-trip:\n{ctxdsl}"
        );

        // Round-trip: parse → realize → the Kleene labels survive.
        let doc = parse(&ctxdsl).expect("emitted ctxdsl parses");
        let realized = realize(&doc, &[]).expect("emitted ctxdsl realizes");
        let r = realized.context.clts("M").expect("automaton M exists");
        assert!(
            r.has_3valued_predicates(),
            "3-valued labels must survive the round-trip"
        );
        let rs0 = r.state_id("cube_0").expect("cube_0 exists");
        assert_eq!(
            r.state_3valued_predicate(rs0, "boot_idle"),
            Some(Tristate::KleeneT)
        );
        assert_eq!(
            r.state_3valued_predicate(rs0, "boot_done"),
            Some(Tristate::KleeneF)
        );
        assert_eq!(
            r.state_3valued_predicate(rs0, "ctr_overflow"),
            Some(Tristate::KleeneBot)
        );
        let rs1 = r.state_id("cube_1").expect("cube_1 exists");
        assert_eq!(
            r.state_3valued_predicate(rs1, "boot_done"),
            Some(Tristate::KleeneT)
        );
    }

    /// A 2-valued CLTS (no Kleene labels) emits no `predicates_3v` block —
    /// the Phase 1b field is strictly additive for the legacy path.
    #[test]
    fn two_valued_clts_emits_no_predicates_3v_block() {
        let mut b = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        let s0 = b.state_id_or_insert("s0").expect("s0 id");
        let s1 = b.state_id_or_insert("s1").expect("s1 id");
        b.initial_state_id(s0);
        let tick = b.labels().intern(["tick"]).expect("tick label");
        b.transition_ids(s0, &[tick], s1);
        let clts = b.build().expect("build clts");

        let ctxdsl = clts_to_ctxdsl(&clts, "M", "two_valued").expect("emit ctxdsl");
        assert!(
            !ctxdsl.contains("predicates_3v"),
            "no Kleene labels ⇒ no predicates_3v block:\n{ctxdsl}"
        );
        // Still parses + realizes (sanity).
        let doc = parse(&ctxdsl).expect("parses");
        realize(&doc, &[]).expect("realizes");
    }
}
