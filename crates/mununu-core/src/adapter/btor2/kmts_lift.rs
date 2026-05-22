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
use std::time::{Duration, Instant};

use crate::adapter::AdapterOptions;
use crate::adapter::btor2::Btor2Adapter;
use crate::adapter::{AdapterError, AdapterOutput, FormatAdapter, SourceFormat, SourceInfo};
use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, Tristate};

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

// ---------------------------------------------------------------------------
// R.2.5 — predicate-cube lift API
// ---------------------------------------------------------------------------
//
// Per the §Phase 5 R.2.5 / §Phase 6 §6.3 / §10.1 plan entries, the R.2
// post-hoc lifter inherits the bit-blaster's `MAX_STATE_BITS = 20` cap.
// R.2.5 ships an alternative API whose abstract states are predicate
// cubes (2^|P|), bypassing the bit-blast enumeration entirely.
//
// **R.2.5 MVP scope** (this commit): the API surface, structural
// state-space construction (2^|P| cubes as `Clts` states with the
// matching `state_3valued_predicates` labelling), the binary capability
// test (a synthetic BTOR2 fixture with > MAX_STATE_BITS total state
// bits but |P| ≤ 10 lifts where R.2 errors), and an explicit
// `predicate_image_pending` flag marking the structural debt that the
// load-bearing SMT-driven must/may edge construction (R.5 / R.5b) will
// close.
//
// **What this MVP does NOT do**: no SMT predicate-image queries; no
// must-edges; no may-edges (the returned Clts has the cube state set
// but no transitions). Verdicts computed over the R.2.5 output today
// are useless (every property evaluates over an isolated set of cube
// states with no dynamics), but the *binary capability* — lifting a
// cap-exceeding fixture into a predicate-cube state space — is real
// and verifiable. The done-criterion in §10.1 R.2.5 is explicitly
// binary; verdict correctness lands when R.5's `KmtsLiftLazy` ships.

/// R.2.5 — A single predicate the cube lifter understands. Today's
/// minimal shape is a register-equality predicate `<register> = <value>`;
/// future iterations can extend this enum to arbitrary BTOR2-expression
/// predicates as SMT integration matures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateSpec {
    /// Display name for the predicate; surfaces in `state_3valued_predicates`
    /// labellings as the key.
    pub name: String,
    /// Source register / signal name (as it appears in the BTOR2 symbol
    /// table). The lifter validates this names a real BTOR2 state.
    pub register: String,
    /// The value the predicate checks against (current MVP: literal
    /// equality only; future extensions could carry an operator).
    pub value: u64,
}

/// R.2.5 — Options for [`predicate_cube_lift`].
#[derive(Debug, Clone)]
pub struct PredicateCubeLiftOptions {
    /// Hard cap on cube count. Defaults to 1024 (|P| ≤ 10). Larger
    /// values are accepted but the wall-clock + memory bound from the
    /// §10.1 R.2.5 done-criterion (< 10 s, < 256 MB) applies only at
    /// the default cap.
    pub max_cube_count: usize,
}

impl Default for PredicateCubeLiftOptions {
    fn default() -> Self {
        Self {
            max_cube_count: 1024,
        }
    }
}

/// R.2.5 — Result of lifting one BTOR2 source through the predicate-
/// cube path. Carries a `Clts` whose state count is bounded by 2^|P|
/// (NOT 2^|Registers|), plus the predicate set and timing
/// information for the §10.1 R.2.5 binary capability check.
#[derive(Debug, Clone)]
pub struct PredicateCubeLiftResult {
    /// Abstract state space — `Clts` whose state count equals the
    /// number of satisfiable predicate cubes. At the R.2.5 MVP every
    /// cube is treated as satisfiable (no SMT check yet); the Clts
    /// has no transitions (R.5's `KmtsLiftLazy` populates them).
    pub clts: Clts<DefaultStateIdx, DefaultLabelIdx>,
    /// The predicate set the lifter consumed, in the order the cube
    /// bits index. `predicates[i]` controls bit i of each cube.
    pub predicates: Vec<PredicateSpec>,
    /// Total number of cubes the lifter materialized (`2^predicates.len()`
    /// at the MVP; future iterations may prune via SMT-satisfiability).
    pub cube_count: usize,
    /// Source-info metadata mirroring `AdapterOutput.source_info` for
    /// downstream consumers (e.g. CLI summary).
    pub source_info: SourceInfo,
    /// Wall-clock duration of the lift. The §10.1 R.2.5 done-criterion
    /// requires this to be < 10 s for the cap-exceeding fixture.
    pub lift_time: Duration,
    /// **Always `true` at R.2.5.** Flags that the must/may transition
    /// relation has not been populated — the Clts has the cube state
    /// set but no edges. R.5's `KmtsLiftLazy` shipping closes this
    /// debt; consumers that need verdicts on the cube space must wait
    /// for that integration.
    pub predicate_image_pending: bool,
}

/// R.2.5 — Lift one BTOR2 source through the predicate-cube path.
///
/// **Does NOT call `Btor2Adapter::translate`** — the bit-blaster is
/// the cap source we are bypassing. Parses BTOR2 directly to validate
/// each predicate's register name, then enumerates 2^|P| cubes as
/// `Clts` states with matching `state_3valued_predicates` labellings.
///
/// **Binary capability test** (§10.1 R.2.5 done-criterion): the
/// returned `PredicateCubeLiftResult.clts` exists and the cube count
/// matches `2^predicates.len()`. The fixture lifting is what we are
/// validating; the must/may transition relation is left to R.5's
/// `KmtsLiftLazy` integration (the `predicate_image_pending` flag
/// flags this explicitly).
///
/// Errors when:
/// - The BTOR2 content fails to parse (delegated to the existing
///   `parser::parse`).
/// - A predicate names a register that does not exist in the BTOR2
///   source's symbol table.
/// - The cube count exceeds `lift_opts.max_cube_count`.
pub fn predicate_cube_lift(
    predicates: Vec<PredicateSpec>,
    btor2_content: &str,
    _options: &AdapterOptions,
    lift_opts: &PredicateCubeLiftOptions,
) -> Result<PredicateCubeLiftResult, AdapterError> {
    let start = Instant::now();

    // 1. Parse the BTOR2 source to validate predicate register names
    //    against the symbol table. Bypasses bit_blast.
    let file = crate::adapter::btor2::parser::parse(btor2_content).map_err(|mut e| {
        e.message = format!("adapter/btor2/predicate_cube_lift: {}", e.message);
        e
    })?;
    let symbols = crate::adapter::btor2::parser::collect_symbols(&file);
    let known_registers: std::collections::HashSet<String> = symbols.values().cloned().collect();

    for pred in &predicates {
        if !known_registers.contains(&pred.register) {
            return Err(AdapterError {
                kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
                location: None,
                message: format!(
                    "adapter/btor2/predicate_cube_lift: predicate `{}` references unknown register `{}` (known: {:?})",
                    pred.name, pred.register, known_registers
                ),
            });
        }
    }

    // 2. Cube count check — `2^|P|` must fit `max_cube_count`. For
    //    |P| > 63 the shift overflows so we cap at 63 explicitly.
    let p = predicates.len();
    if p > 63 {
        return Err(AdapterError {
            kind: crate::adapter::AdapterErrorKind::StateSpaceOverflow,
            location: None,
            message: format!(
                "adapter/btor2/predicate_cube_lift: |P| = {p} exceeds 63 (cube count would overflow usize)"
            ),
        });
    }
    let cube_count: usize = 1usize << p;
    if cube_count > lift_opts.max_cube_count {
        return Err(AdapterError {
            kind: crate::adapter::AdapterErrorKind::StateSpaceOverflow,
            location: None,
            message: format!(
                "adapter/btor2/predicate_cube_lift: cube count 2^{p} = {cube_count} exceeds max_cube_count = {}",
                lift_opts.max_cube_count
            ),
        });
    }

    // 3. Build the Clts: one state per cube. State name `cube_<i>`
    //    where i is the bit pattern. State_3valued_predicate at each
    //    state labels each predicate KleeneT if the corresponding bit
    //    is set in i, KleeneF otherwise. Predicates are populated on
    //    the builder *before* `build()` because `with_3valued_predicate`
    //    is a builder-side mutator.
    let mut builder = Clts::builder();
    let mut state_ids = Vec::with_capacity(cube_count);
    for i in 0..cube_count {
        let name = format!("cube_{i}");
        let id = builder
            .state_id_or_insert(&name)
            .ok_or_else(|| AdapterError {
                kind: crate::adapter::AdapterErrorKind::StateSpaceOverflow,
                location: None,
                message: format!(
                    "adapter/btor2/predicate_cube_lift: state id overflow at cube {i} / {cube_count}"
                ),
            })?;
        state_ids.push(id);
    }
    // Mark cube_0 (all-predicates-false) as initial. A future iteration
    // can pick the cube matching the BTOR2 `init` values.
    if let Some(initial) = state_ids.first() {
        builder.initial_state_id(*initial);
    }

    // Populate state_3valued_predicates per cube *on the builder*.
    for (i, &state_id) in state_ids.iter().enumerate() {
        for (bit, pred) in predicates.iter().enumerate() {
            let verdict = if (i >> bit) & 1 == 1 {
                Tristate::KleeneT
            } else {
                Tristate::KleeneF
            };
            builder.with_3valued_predicate(state_id, &pred.name, verdict);
        }
    }

    let clts = builder.build().map_err(|e| AdapterError {
        kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
        location: None,
        message: format!("adapter/btor2/predicate_cube_lift: builder.build failed: {e}"),
    })?;

    let elapsed = start.elapsed();

    Ok(PredicateCubeLiftResult {
        clts,
        predicates,
        cube_count,
        source_info: SourceInfo {
            format: SourceFormat::Btor2,
            title: None,
            signal_count: known_registers.len(),
            state_count: cube_count,
            property_count: 0,
        },
        lift_time: elapsed,
        // R.5's KmtsLiftLazy will set this to false when it ships.
        predicate_image_pending: true,
    })
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

    // ---- R.2.5 — predicate-cube lift tests ----

    /// Small BTOR2 fixture with 2 state registers (1 bit each) for
    /// API-shape testing. Below the bit-blaster cap; serves as the
    /// "happy path" sanity check.
    const SMALL_BTOR2: &str = "\
1 sort bitvec 1
2 state 1 reg_a
3 state 1 reg_b
4 zero 1
5 init 1 2 4
6 init 1 3 4
7 next 1 2 4
8 next 1 3 4
";

    /// R.2.5 binary capability test — synthetic BTOR2 fixture with
    /// more than MAX_STATE_BITS total state bits (6 registers × 4
    /// bits = 24, vs `MAX_STATE_BITS = 20`). The R.2 lifter errors
    /// on this fixture with "BTOR2 design has 24 state bits"; the
    /// R.2.5 lifter must succeed with a small predicate set.
    const CAP_EXCEEDING_BTOR2: &str = "\
1 sort bitvec 4
2 state 1 reg_0
3 state 1 reg_1
4 state 1 reg_2
5 state 1 reg_3
6 state 1 reg_4
7 state 1 reg_5
8 zero 1
9 init 1 2 8
10 init 1 3 8
11 init 1 4 8
12 init 1 5 8
13 init 1 6 8
14 init 1 7 8
15 next 1 2 8
16 next 1 3 8
17 next 1 4 8
18 next 1 5 8
19 next 1 6 8
20 next 1 7 8
";

    #[test]
    fn predicate_cube_lift_validates_predicate_register_names() {
        // Predicate references a register that does not exist.
        let preds = vec![PredicateSpec {
            name: "bogus".into(),
            register: "nonexistent_reg".into(),
            value: 0,
        }];
        let result = predicate_cube_lift(
            preds,
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("nonexistent_reg"),
            "error message should name the unknown register; got: {msg}"
        );
    }

    #[test]
    fn predicate_cube_lift_respects_max_cube_count() {
        // |P| = 4 → cube_count = 16; max set to 8 → should error.
        let preds = (0..4)
            .map(|i| PredicateSpec {
                name: format!("p_{i}"),
                register: "reg_a".into(),
                value: i,
            })
            .collect();
        let opts = PredicateCubeLiftOptions { max_cube_count: 8 };
        let result = predicate_cube_lift(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts);
        assert!(result.is_err());
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("cube count") && msg.contains("16"),
            "error should mention cube count overflow; got: {msg}"
        );
    }

    #[test]
    fn predicate_cube_lift_emits_2_to_p_states() {
        // |P| = 3 → cube_count = 8 → 8 states in the resulting Clts.
        let preds = vec![
            PredicateSpec {
                name: "p0".into(),
                register: "reg_a".into(),
                value: 0,
            },
            PredicateSpec {
                name: "p1".into(),
                register: "reg_a".into(),
                value: 1,
            },
            PredicateSpec {
                name: "p2".into(),
                register: "reg_b".into(),
                value: 0,
            },
        ];
        let result = predicate_cube_lift(
            preds,
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        )
        .expect("predicate_cube_lift succeeds on valid input");
        assert_eq!(result.cube_count, 8);
        assert_eq!(result.clts.state_count(), 8);
        assert_eq!(result.predicates.len(), 3);
        // R.2.5 MVP flag: must/may edges not yet populated.
        assert!(result.predicate_image_pending);
        // Each cube state must have all 3 predicate verdicts populated.
        for state in result.clts.states() {
            for pred in &result.predicates {
                let verdict = result.clts.state_3valued_predicate(state, &pred.name);
                assert!(
                    verdict.is_some(),
                    "cube state {state:?} missing predicate `{}`",
                    pred.name
                );
                let v = verdict.unwrap();
                assert!(
                    matches!(v, Tristate::KleeneT | Tristate::KleeneF),
                    "R.2.5 MVP cubes carry definite verdicts only (no KleeneBot)"
                );
            }
        }
    }

    #[test]
    fn predicate_cube_lift_state_3valued_predicates_match_cube_bit_pattern() {
        // |P| = 2 → 4 cubes. cube_i's bit pattern matches predicate
        // verdicts: cube 0 = both F; cube 1 = p0 T; cube 2 = p1 T;
        // cube 3 = both T.
        let preds = vec![
            PredicateSpec {
                name: "p0".into(),
                register: "reg_a".into(),
                value: 0,
            },
            PredicateSpec {
                name: "p1".into(),
                register: "reg_b".into(),
                value: 0,
            },
        ];
        let result = predicate_cube_lift(
            preds,
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        )
        .expect("ok");
        assert_eq!(result.cube_count, 4);

        // cube_0: bit 0 = 0 → p0 KleeneF; bit 1 = 0 → p1 KleeneF
        let s0 = result.clts.state_id("cube_0").expect("cube_0 exists");
        assert_eq!(
            result.clts.state_3valued_predicate(s0, "p0"),
            Some(Tristate::KleeneF)
        );
        assert_eq!(
            result.clts.state_3valued_predicate(s0, "p1"),
            Some(Tristate::KleeneF)
        );
        // cube_3: bit 0 = 1 → p0 KleeneT; bit 1 = 1 → p1 KleeneT
        let s3 = result.clts.state_id("cube_3").expect("cube_3 exists");
        assert_eq!(
            result.clts.state_3valued_predicate(s3, "p0"),
            Some(Tristate::KleeneT)
        );
        assert_eq!(
            result.clts.state_3valued_predicate(s3, "p1"),
            Some(Tristate::KleeneT)
        );
    }

    #[test]
    fn r2_5_binary_capability_test_cap_exceeding_fixture_lifts() {
        // R.2 errors on CAP_EXCEEDING_BTOR2 (24 state bits >
        // MAX_STATE_BITS = 20). R.2.5 must succeed with a small
        // predicate set.
        let r2_result = lift_btor2_to_kmts(
            CAP_EXCEEDING_BTOR2,
            &AdapterOptions::default(),
            &KmtsLiftOptions::default(),
        );
        assert!(
            r2_result.is_err(),
            "R.2 lifter must error on the cap-exceeding fixture (it inherits MAX_STATE_BITS)"
        );

        // R.2.5 with |P| = 4 cubes should lift in < 10s and < 256MB
        // (the §10.1 R.2.5 done-criterion bounds). Wall-clock here
        // measured by the lift itself; memory is not instrumented at
        // the MVP but the trivial state-construction cost dominates.
        let preds = vec![
            PredicateSpec {
                name: "reg_0_eq_0".into(),
                register: "reg_0".into(),
                value: 0,
            },
            PredicateSpec {
                name: "reg_1_eq_0".into(),
                register: "reg_1".into(),
                value: 0,
            },
            PredicateSpec {
                name: "reg_2_eq_0".into(),
                register: "reg_2".into(),
                value: 0,
            },
            PredicateSpec {
                name: "reg_3_eq_0".into(),
                register: "reg_3".into(),
                value: 0,
            },
        ];
        let r2_5_result = predicate_cube_lift(
            preds,
            CAP_EXCEEDING_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        );
        assert!(
            r2_5_result.is_ok(),
            "R.2.5 lifter must succeed on the cap-exceeding fixture; got {:?}",
            r2_5_result.err()
        );
        let result = r2_5_result.unwrap();
        assert_eq!(result.cube_count, 16);
        assert_eq!(result.clts.state_count(), 16);
        // Wall-clock bound (§10.1 R.2.5 done-criterion: < 10 s). The
        // MVP enumeration is O(|cubes| × |predicates|) bit operations
        // + |cubes| HashMap inserts — should complete in milliseconds.
        assert!(
            result.lift_time < std::time::Duration::from_secs(10),
            "R.2.5 done-criterion wall-clock bound exceeded: {:?}",
            result.lift_time
        );
        // R.2.5 MVP explicitly does NOT populate must/may edges.
        // Flag must be true so callers know not to evaluate verdicts.
        assert!(
            result.predicate_image_pending,
            "R.2.5 MVP must flag predicate_image_pending = true"
        );
    }
}
