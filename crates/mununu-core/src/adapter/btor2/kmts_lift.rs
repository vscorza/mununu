//! R.2 — BTOR2 → KMTS lifter.
//!
//! Per the KMTS architecture
//! (`docs/design/native-sv-abstraction.md` §6, §7;
//! `docs/design/kmts-theory.md` §6;
//! `docs/design/predicate-abstraction-recipe.md` §2),
//! this module promotes the existing 2-valued
//! [`Btor2Adapter`](super::Btor2Adapter) output into a KMTS-aware
//! shape consumable by the post-R.3 `KleeneDomain` evaluator.
//!
//! **R.2 scope (MVP).** The lifter is a *post-hoc enrichment* over
//! the existing BTOR2 adapter — it does not change the CLTS shape
//! today, it adds a parallel `predicate_labellings` side-map keyed
//! by `(automaton, state, predicate_name) → Tristate`. Each entry
//! is derived from the BTOR2 bit-blaster's per-state register
//! valuations: every `(register, value)` pair becomes a predicate
//! `<register>=<value>` whose `KleeneT` set is the states where the
//! register has that value, and whose `KleeneF` set is every other
//! enumerated state. **No `KleeneBot` values are produced at R.2** —
//! the bit-blaster's explicit-state enumeration is exact, so every
//! predicate has a definite verdict at every state.
//!
//! Modality is also uniform at R.2: every transition the bit-blaster
//! emits has both a may-witness (the abstraction admits the edge)
//! and a must-witness (the bit-blaster's exact enumeration computes
//! the concrete reachability), so every edge is `Sharp`. This
//! matches the legacy semantics exactly; the R.2 lifter produces a
//! KMTS that is *vacuously* 3-valued — the same verdicts as the
//! 2-valued evaluator on every fixture today.
//!
//! **Where R.2 stops being a no-op.** When R.5 (CEGAR) + R.5b (UF
//! abstraction) land, the lifter's predicate-image construction
//! will introduce `KleeneBot` valuations (predicates the abstraction
//! cannot decide) and `MayOnly` transitions (over-approximation
//! edges from UF-abstracted operators). The R.2 surface is the
//! interface those phases plug into; today it ships the enrichment
//! shape and a fixture-sweep regression so the post-R.3 evaluator
//! has something to read.

use std::collections::{BTreeMap, HashMap};

use crate::adapter::AdapterOptions;
use crate::adapter::btor2::Btor2Adapter;
use crate::adapter::{AdapterError, AdapterOutput, FormatAdapter};
use crate::clts::Tristate;

/// Per-(automaton, state) 3-valued labelling map.
/// Outer key: automaton name. Middle key: state name. Inner map:
/// predicate name → Kleene verdict at that state.
pub type LabellingMap = HashMap<String, HashMap<String, BTreeMap<String, Tristate>>>;

/// R.2 — Options controlling the BTOR2 → KMTS lift.
#[derive(Debug, Clone, Default)]
pub struct KmtsLiftOptions {
    /// When `true`, the lifter wraps an explicit "no-state-matched"
    /// predicate `__no_state__` that is KleeneT at exactly the
    /// states where every other predicate is KleeneF. Useful for
    /// debugging predicate completeness; off by default.
    pub emit_no_state_predicate: bool,
    /// Cap on the number of predicates synthesised. Each
    /// `(register, value)` pair in the bit-blaster's state
    /// valuations becomes one predicate; designs with wide
    /// registers and many enumerated values can produce hundreds.
    /// `None` (default) means no cap; the R.5+ CEGAR loop is
    /// where predicate cardinality becomes a real concern.
    pub max_predicates: Option<usize>,
}

/// R.2 — One synthesised predicate the lifter produces per
/// `(register, value)` pair found in the bit-blaster's state
/// valuations. The CLTS-layer 3-valued labelling field
/// [`crate::clts::Clts::state_3valued_predicates`] uses the
/// `name` here as the predicate identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LiftedPredicate {
    /// Predicate name in the form `<register>=<value>`. Round-trips
    /// directly as the predicate identifier in
    /// `Clts::state_3valued_predicate(state, name)`.
    pub name: String,
    /// Source register / signal the predicate is anchored on.
    pub register: String,
    /// The display value the predicate witnesses.
    pub value: String,
}

/// R.2 — Result of lifting one BTOR2 source through the KMTS-aware
/// shape. Carries the existing 2-valued [`AdapterOutput`] unchanged
/// plus the predicate side-map keyed by `(automaton, state, predicate)`.
#[derive(Debug, Clone)]
pub struct KmtsLiftResult {
    /// The unchanged 2-valued output the legacy BTOR2 adapter would
    /// have produced. Existing CTXDSL emission, state_valuations,
    /// sidecars, partition summaries all survive.
    pub adapter_output: AdapterOutput,
    /// The set of predicates the lifter synthesised from the
    /// bit-blaster's state valuations. Deterministic order
    /// (sorted by name).
    pub predicates: Vec<LiftedPredicate>,
    /// Per-(automaton, state) 3-valued labelling map. The post-R.3
    /// `KleeneDomain` evaluator reads this when the
    /// `Clts::state_3valued_predicates` field is `None` (the legacy
    /// CLTS path today; the lifter intentionally does *not* mutate
    /// the Clts struct at R.2, that hook lands when the evaluator
    /// is on).
    pub predicate_labellings: LabellingMap,
}

impl KmtsLiftResult {
    /// Total number of `(automaton, state, predicate)` triples
    /// the labelling map carries. Used as the R.2 done-criterion
    /// proxy: a fixture "produces KMTS" iff this count is > 0
    /// (the lifter inferred at least one predicate from the
    /// bit-blaster's state valuations).
    pub fn labelling_count(&self) -> usize {
        self.predicate_labellings
            .values()
            .flat_map(|per_state| per_state.values())
            .map(BTreeMap::len)
            .sum()
    }
}

/// R.2 — Lift one BTOR2 source through the KMTS-aware shape.
///
/// Runs the existing [`Btor2Adapter::translate`] to produce the
/// 2-valued output, then walks `state_valuations` to synthesise
/// per-`(register, value)` predicates and per-state labellings.
/// All transitions are implicitly `Sharp` (the bit-blaster's
/// exact-enumeration semantics — both may and must witnesses
/// exist for every emitted edge) and the existing Clts is
/// returned unmodified inside `adapter_output`.
///
/// Errors only when the underlying BTOR2 adapter errors —
/// post-translation enrichment is infallible (or returns an empty
/// labelling map when the bit-blaster did not populate
/// `state_valuations`, which is the case for very small fixtures
/// where every state has a trivial valuation).
pub fn lift_btor2_to_kmts(
    content: &str,
    options: &AdapterOptions,
    lift_opts: &KmtsLiftOptions,
) -> Result<KmtsLiftResult, AdapterError> {
    let adapter_output = Btor2Adapter::translate(content, options).map_err(|mut e| {
        e.message = format!("adapter/btor2/kmts_lift: {}", e.message);
        e
    })?;

    let (predicates, predicate_labellings) =
        synthesise_predicates_and_labellings(&adapter_output, lift_opts);

    Ok(KmtsLiftResult {
        adapter_output,
        predicates,
        predicate_labellings,
    })
}

/// Walk `AdapterOutput.state_valuations` and produce
/// `(predicates, labellings)`. Each `(register, value)` pair found
/// in any state's valuation becomes one predicate; the labelling
/// at each state is `KleeneT` for predicates matching that state's
/// valuation and `KleeneF` for the rest.
fn synthesise_predicates_and_labellings(
    out: &AdapterOutput,
    lift_opts: &KmtsLiftOptions,
) -> (Vec<LiftedPredicate>, LabellingMap) {
    // First pass: collect the universe of (register, value) pairs
    // observed across all automata × states. Deduplicate and sort
    // for determinism — the lifter's output must be stable across
    // runs to keep regression baselines clean.
    let mut universe: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for per_state in out.state_valuations.values() {
        for valuation in per_state.values() {
            for (reg, val) in valuation {
                universe.insert((reg.clone(), val.clone()));
            }
        }
    }

    let mut predicates: Vec<LiftedPredicate> = universe
        .into_iter()
        .map(|(register, value)| LiftedPredicate {
            name: format!("{register}={value}"),
            register,
            value,
        })
        .collect();

    if let Some(cap) = lift_opts.max_predicates
        && predicates.len() > cap
    {
        predicates.truncate(cap);
    }

    if lift_opts.emit_no_state_predicate {
        predicates.push(LiftedPredicate {
            name: "__no_state__".to_string(),
            register: "__synthetic__".to_string(),
            value: "no_state".to_string(),
        });
    }

    // Second pass: for each (automaton, state), build a BTreeMap of
    // predicate_name → Tristate. The bit-blaster's enumeration is
    // exact, so every predicate has a definite verdict (KleeneT or
    // KleeneF) — no KleeneBot at R.2.
    let mut labellings: LabellingMap = HashMap::new();
    for (aut, per_state) in &out.state_valuations {
        let mut per_state_map: HashMap<String, BTreeMap<String, Tristate>> = HashMap::new();
        for (state, valuation) in per_state {
            let mut verdicts: BTreeMap<String, Tristate> = BTreeMap::new();
            for pred in &predicates {
                if pred.register == "__synthetic__" {
                    // The `__no_state__` synthetic predicate is
                    // KleeneT iff every other predicate is KleeneF.
                    // Filled in after the main loop below.
                    continue;
                }
                let verdict = if valuation.get(&pred.register) == Some(&pred.value) {
                    Tristate::KleeneT
                } else {
                    Tristate::KleeneF
                };
                verdicts.insert(pred.name.clone(), verdict);
            }
            // `__no_state__` post-fill: KleeneT iff every other
            // predicate at this state is KleeneF. This happens iff
            // the state's valuation has no matching `(register, value)`
            // in the predicate set — which today is rare (the
            // predicate set is derived from the valuation universe).
            if lift_opts.emit_no_state_predicate {
                let all_false = verdicts.values().all(|v| *v == Tristate::KleeneF);
                verdicts.insert(
                    "__no_state__".to_string(),
                    if all_false {
                        Tristate::KleeneT
                    } else {
                        Tristate::KleeneF
                    },
                );
            }
            per_state_map.insert(state.clone(), verdicts);
        }
        labellings.insert(aut.clone(), per_state_map);
    }

    (predicates, labellings)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixture: (automaton, [(state, [(var, value)])]).
    type StateVarPair<'a> = (&'a str, &'a str);
    type StateVals<'a> = (&'a str, Vec<StateVarPair<'a>>);
    type AutomatonVals<'a> = (&'a str, Vec<StateVals<'a>>);

    fn make_output_with_valuations(valuations: Vec<AutomatonVals<'_>>) -> AdapterOutput {
        use crate::adapter::{SourceFormat, SourceInfo};
        let mut state_valuations = HashMap::new();
        for (aut, states) in valuations {
            let mut per_state = HashMap::new();
            for (state, vars) in states {
                let bmap: BTreeMap<String, String> = vars
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                per_state.insert(state.to_string(), bmap);
            }
            state_valuations.insert(aut.to_string(), per_state);
        }
        AdapterOutput {
            ctxdsl: String::new(),
            warnings: Vec::new(),
            source_info: SourceInfo {
                format: SourceFormat::Btor2,
                title: None,
                signal_count: 0,
                state_count: 0,
                property_count: 0,
            },
            sidecars: Vec::new(),
            state_valuations,
            transition_observations: Default::default(),
            partition_summary: Default::default(),
        }
    }

    #[test]
    fn synthesise_predicates_sorted_and_deduplicated() {
        let out = make_output_with_valuations(vec![(
            "M",
            vec![
                ("s0", vec![("cnt", "0"), ("flag", "true")]),
                ("s1", vec![("cnt", "1"), ("flag", "true")]),
                ("s2", vec![("cnt", "1"), ("flag", "false")]),
            ],
        )]);
        let (preds, _) = synthesise_predicates_and_labellings(&out, &KmtsLiftOptions::default());
        let names: Vec<&str> = preds.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["cnt=0", "cnt=1", "flag=false", "flag=true"],
            "predicates must be deterministic + deduplicated"
        );
    }

    #[test]
    fn labelling_assigns_kleenet_at_matching_state() {
        let out = make_output_with_valuations(vec![(
            "M",
            vec![("s0", vec![("cnt", "0")]), ("s1", vec![("cnt", "1")])],
        )]);
        let (_, lab) = synthesise_predicates_and_labellings(&out, &KmtsLiftOptions::default());
        let s0 = &lab["M"]["s0"];
        assert_eq!(s0["cnt=0"], Tristate::KleeneT);
        assert_eq!(s0["cnt=1"], Tristate::KleeneF);
        let s1 = &lab["M"]["s1"];
        assert_eq!(s1["cnt=0"], Tristate::KleeneF);
        assert_eq!(s1["cnt=1"], Tristate::KleeneT);
    }

    #[test]
    fn labelling_no_kleenebot_at_r2() {
        // R.2 invariant: every predicate has a definite verdict at
        // every enumerated state. KleeneBot would only arise from
        // CEGAR refinement (R.5) or UF-abstracted predicate-image
        // queries (R.5b), neither of which are wired yet.
        let out =
            make_output_with_valuations(vec![("M", vec![("s0", vec![("a", "0"), ("b", "x")])])]);
        let (_, lab) = synthesise_predicates_and_labellings(&out, &KmtsLiftOptions::default());
        for state_lab in lab["M"].values() {
            for verdict in state_lab.values() {
                assert!(
                    *verdict != Tristate::KleeneBot,
                    "R.2 must not produce KleeneBot"
                );
            }
        }
    }

    #[test]
    fn empty_valuations_yields_empty_labellings() {
        // Bit-blaster did not populate state_valuations (e.g. a
        // degenerate single-state fixture). The lifter must
        // gracefully produce zero predicates + zero labellings
        // rather than erroring.
        let out = make_output_with_valuations(vec![]);
        let (preds, lab) = synthesise_predicates_and_labellings(&out, &KmtsLiftOptions::default());
        assert!(preds.is_empty());
        assert!(lab.is_empty());
    }

    #[test]
    fn max_predicates_cap_truncates() {
        let out = make_output_with_valuations(vec![(
            "M",
            vec![
                ("s0", vec![("a", "0"), ("b", "0"), ("c", "0")]),
                ("s1", vec![("a", "1"), ("b", "1"), ("c", "1")]),
            ],
        )]);
        let lift_opts = KmtsLiftOptions {
            max_predicates: Some(2),
            ..Default::default()
        };
        let (preds, _) = synthesise_predicates_and_labellings(&out, &lift_opts);
        assert_eq!(preds.len(), 2);
    }

    #[test]
    fn lifted_result_labelling_count_sums_correctly() {
        let out = make_output_with_valuations(vec![(
            "M",
            vec![("s0", vec![("x", "0")]), ("s1", vec![("x", "1")])],
        )]);
        let (preds, lab) = synthesise_predicates_and_labellings(&out, &KmtsLiftOptions::default());
        let result = KmtsLiftResult {
            adapter_output: AdapterOutput {
                ctxdsl: String::new(),
                warnings: Vec::new(),
                source_info: crate::adapter::SourceInfo {
                    format: crate::adapter::SourceFormat::Btor2,
                    title: None,
                    signal_count: 0,
                    state_count: 0,
                    property_count: 0,
                },
                sidecars: Vec::new(),
                state_valuations: HashMap::new(),
                transition_observations: Default::default(),
                partition_summary: Default::default(),
            },
            predicates: preds,
            predicate_labellings: lab,
        };
        // 2 states × 2 predicates = 4 (state, predicate) entries.
        assert_eq!(result.labelling_count(), 4);
    }
}
