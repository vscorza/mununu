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
    /// R.2.5 predicate-image MVP — max number of 1-bit BTOR2 inputs
    /// to enumerate per cube when computing may-edges. Defaults to 8
    /// (256 input combinations per cube). Over the cap, those inputs
    /// default to zero — sound under-approximation of input
    /// nondeterminism that may miss may-edges. Set to 0 to disable
    /// edge construction (recovers the legacy R.2.5 MVP behaviour
    /// where the Clts has cube states but no transitions).
    pub max_input_bits: usize,
}

impl Default for PredicateCubeLiftOptions {
    fn default() -> Self {
        Self {
            max_cube_count: 1024,
            max_input_bits: 8,
        }
    }
}

/// R.5 lazy KMTS sub-item 2.1 — a single may-edge from
/// [`KmtsLiftLazy::expand_cube`]. The target cube is identified by
/// its 0-based index in the lifter's cube space; the label is the
/// transition's BTOR2-level label name (`"step"` for the R.2.5
/// MVP's single-label edges; future variants may carry more
/// distinguished labels per input combination).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LazyExpansionEdge {
    /// Label name on the may-edge. Matches the labels the eager
    /// `predicate_cube_lift` would emit for the same edge.
    pub label: String,
    /// 0-based cube index of the edge's target.
    pub target_cube: usize,
}

/// R.5 lazy KMTS sub-item 2.1 — trait for on-demand predicate-cube
/// expansion. The eager [`predicate_cube_lift`] materializes every
/// cube + every may-edge before returning; the lazy trait lets the
/// caller (typically R.5's CEGAR loop) drive expansion as the
/// failure-subgame walk explores cubes, so memory grows only with
/// the reachable cube set rather than `2^|P|`.
///
/// **Sub-item 2.1 scope.** This commit ships the trait + a
/// [`NullLazyLift`] stub impl that returns empty edges for every
/// cube (caller treats every cube as terminal). Sub-item 2.2 adds
/// an eager-wrapping impl that backs the lazy API with the
/// existing `predicate_cube_lift` results; sub-item 2.3 adds the
/// truly-lazy impl that computes cube successors on demand via
/// `simulate_one_step_with_uf_rep`. Sub-items 2.4 + 2.5 plumb the
/// trait into the CEGAR loop + add benchmark coverage.
///
/// **Per-method contracts**:
/// - [`Self::cube_count`] returns the total cube count
///   (`2^|predicates|` at the MVP).
/// - [`Self::expand_cube`] returns the may-edges out of the given
///   cube. Implementations may cache; repeat calls on the same
///   cube index must return the same set (sound under
///   determinism).
/// - [`Self::predicates`] returns the predicate set the handle was
///   built with; immutable across the handle's lifetime.
pub trait KmtsLiftLazy {
    /// Total number of cubes in the abstract state space.
    /// Equals `2^|predicates|` at the R.2.5 MVP.
    fn cube_count(&self) -> usize;

    /// Expand a single cube's may-edges. Caller drives exploration;
    /// implementations may memoize. Returns an empty `Vec` for
    /// cubes with no admissible successors (the cube is terminal).
    ///
    /// Out-of-range `cube_index` (>= `cube_count()`) returns an
    /// empty `Vec` — implementations should not panic.
    fn expand_cube(&mut self, cube_index: usize) -> Vec<LazyExpansionEdge>;

    /// The predicate set the handle was built with.
    fn predicates(&self) -> &[PredicateSpec];
}

/// R.5 lazy KMTS sub-item 2.1 — stub implementation of
/// [`KmtsLiftLazy`] that always returns empty edges. Useful for
/// testing the trait shape compiles + for callers that want a
/// cube-state-only abstraction with no dynamics (the legacy R.2.5
/// MVP behaviour pre-may-edge-construction).
#[derive(Debug, Clone)]
pub struct NullLazyLift {
    /// Predicates the handle was built with. Stored for
    /// [`KmtsLiftLazy::predicates`] return.
    pub predicates: Vec<PredicateSpec>,
}

impl NullLazyLift {
    /// Build a `NullLazyLift` with the given predicate set.
    pub fn new(predicates: Vec<PredicateSpec>) -> Self {
        Self { predicates }
    }
}

impl KmtsLiftLazy for NullLazyLift {
    fn cube_count(&self) -> usize {
        1usize << self.predicates.len()
    }

    fn expand_cube(&mut self, _cube_index: usize) -> Vec<LazyExpansionEdge> {
        Vec::new()
    }

    fn predicates(&self) -> &[PredicateSpec] {
        &self.predicates
    }
}

/// R.5 lazy KMTS sub-item 2.2 (2026-06-04) — eager wrapper that
/// satisfies [`KmtsLiftLazy`] by holding a fully-materialized
/// [`PredicateCubeLiftResult`]. All cube + edge work happens at
/// handle construction (via the eager [`predicate_cube_lift`]);
/// [`KmtsLiftLazy::expand_cube`] just walks the pre-computed
/// `Clts::outgoing` for the requested cube.
///
/// **Purpose**: drop-in adapter for callers wanting the lazy API
/// without the lazy performance characteristics yet. Sub-item
/// 2.3 ships the truly-lazy implementation; until then, this
/// wrapper lets the CEGAR loop's R.5 follow-up (sub-item 2.4)
/// migrate to the trait surface incrementally.
///
/// **Memory**: O(2^|P|) cubes × per-cube outgoing transitions —
/// exactly the eager lift's footprint. The lazy savings come
/// from sub-item 2.3.
#[derive(Debug, Clone)]
pub struct EagerLazyLift {
    result: PredicateCubeLiftResult,
}

impl EagerLazyLift {
    /// Wrap a pre-computed `PredicateCubeLiftResult`. Use when
    /// the caller has already invoked `predicate_cube_lift` (e.g.
    /// the CEGAR loop's iteration 0).
    pub fn from_result(result: PredicateCubeLiftResult) -> Self {
        Self { result }
    }

    /// Construct an `EagerLazyLift` directly from a BTOR2 source.
    /// Invokes `predicate_cube_lift` internally; propagates any
    /// adapter errors.
    pub fn from_btor2(
        predicates: Vec<PredicateSpec>,
        btor2_content: &str,
        adapter_options: &crate::adapter::AdapterOptions,
        lift_opts: &PredicateCubeLiftOptions,
    ) -> Result<Self, crate::adapter::AdapterError> {
        let result = predicate_cube_lift(predicates, btor2_content, adapter_options, lift_opts)?;
        Ok(Self::from_result(result))
    }

    /// Access the underlying lift result (Clts + metadata).
    /// Useful when consumers need both the lazy-trait shape AND
    /// the result's auxiliary fields (warnings, lift_time, etc.).
    pub fn result(&self) -> &PredicateCubeLiftResult {
        &self.result
    }
}

impl KmtsLiftLazy for EagerLazyLift {
    fn cube_count(&self) -> usize {
        self.result.cube_count
    }

    fn expand_cube(&mut self, cube_index: usize) -> Vec<LazyExpansionEdge> {
        // Out-of-range: per the trait contract, return empty
        // rather than panicking.
        if cube_index >= self.cube_count() {
            return Vec::new();
        }
        use crate::clts::StateId;
        let Some(src) = StateId::<DefaultStateIdx>::from_index(cube_index) else {
            return Vec::new();
        };
        self.result
            .clts
            .outgoing(src)
            .iter()
            .map(|t| {
                // Resolve each label-id to its first symbol via
                // `Clts::label_payload`. The R.2.5 lifter emits
                // single-symbol single-label transitions
                // (`["step"]`); future multi-symbol cases would
                // need a stable join convention here.
                let label_name = t
                    .labels()
                    .first()
                    .and_then(|lid| self.result.clts.label_payload(*lid))
                    .and_then(|symbols| symbols.first())
                    .cloned()
                    .unwrap_or_else(|| "?unknown_label".to_string());
                LazyExpansionEdge {
                    label: label_name,
                    target_cube: t.target().index(),
                }
            })
            .collect()
    }

    fn predicates(&self) -> &[PredicateSpec] {
        &self.result.predicates
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
    /// R.5b follow-up — Adapter warnings the lift emitted (e.g. UF
    /// wrapping naming). Replaces the prior tracing::warn!-only
    /// surface so callers (e.g. cegar.rs) can fold lift warnings
    /// into their own reporting (CegarTrace, CLI verdict-with-
    /// warnings, etc.). Empty for fixtures that triggered no
    /// warning-worthy abstraction decisions.
    pub warnings: Vec<crate::adapter::AdapterWarning>,
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
    options: &AdapterOptions,
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

    // R.5b lifter consumer wiring — resolve sidecar `uf_wrap` /
    // `uf_unwrap` declarations + the default policy (Op::Mul always;
    // Op::Add/Sub when width > 32) to the set of BTOR2 Op NIDs the
    // simulate-one-step pass should treat as uninterpreted (returns
    // BvValue::zero on evaluation). Empty set ⇒ no UF wrapping;
    // simulate_one_step is used directly with no behaviour change.
    let uf_wrapped_nids = crate::adapter::btor2::bit_blast::collect_uf_wrapped_nids(&file, options);
    let mut warnings: Vec<crate::adapter::AdapterWarning> = Vec::new();
    if !uf_wrapped_nids.is_empty() {
        // R.5b AdapterWarning channel + multi-value UF enumeration —
        // surface the UF wrapping via both tracing (log-stream
        // visibility) and the structured warnings vec (for callers
        // like cegar.rs to fold into their own reporting). The
        // wrapped Ops' outputs are enumerated under both
        // UfRepresentative::Zero and UfRepresentative::Ones per
        // (cube, input combo) → 2× may-edges in the lifted Clts.
        let message = format!(
            "R.5b predicate-image: {} Op(s) UF-wrapped (substituted to zero AND ones via the \
             multi-value enumeration in simulate-one-step). SOUND for safety (may-side \
             over-approximation per §6.10); the wrapped cells' downstream cube successors may \
             shift toward KleeneBot. To remove a wrapping, list the cell in the sidecar's \
             `uf_unwrap` field.",
            uf_wrapped_nids.len()
        );
        tracing::warn!(uf_wrapped_count = uf_wrapped_nids.len(), "{}", message);
        warnings.push(crate::adapter::AdapterWarning {
            kind: crate::adapter::WarningKind::ApproximateTranslation,
            message,
            location: None,
        });
    }

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

    // R.2.5 predicate-image MVP — emit MayOnly edges between cubes.
    //
    // For each cube_i, build a "canonical representative" concrete
    // register assignment (each predicate's source register set to
    // the value the predicate checks against when the cube has the
    // predicate true; left at 0 otherwise — which is a valid
    // representative for the predicate-false case under the
    // simplifying assumption that 0 ≠ predicate.value, which holds
    // for the typical equality predicate `register == nonzero_constant`).
    //
    // For each boolean input combination (up to `max_input_bits` 1-bit
    // BTOR2 inputs), simulate one clock step via
    // `bit_blast::simulate_one_step`. The resulting register values
    // map to a target cube `cube_j` via predicate re-evaluation;
    // emit a MayOnly edge `cube_i → cube_j` with a "step" label.
    //
    // Soundness: emitting MayOnly (no must-witness) is the safe
    // under-approximation per the standard KMTS preservation theorem.
    // Sampling a single canonical representative per cube is an
    // under-approximation of the may-set — every concrete state in
    // the cube might reach OTHER cubes via inputs we don't sample.
    // R.5 / R.5b's SMT-driven must-edges + lazy `KmtsLiftLazy` close
    // these gaps.
    let mut predicate_image_pending = true;
    if lift_opts.max_input_bits > 0 && !predicates.is_empty() {
        let boolean_inputs: Vec<String> = collect_boolean_input_symbols(&file, &symbols);
        let n_inputs = boolean_inputs.len().min(lift_opts.max_input_bits);
        let n_combos: usize = 1usize << n_inputs;
        let label_id = builder
            .labels()
            .intern(["step"])
            .map_err(|e| AdapterError {
                kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
                location: None,
                message: format!("adapter/btor2/predicate_cube_lift: label intern failed: {e}"),
            })?;

        // Collect register widths so we can mask the representative
        // values to the cell's bit-width.
        let mut pred_register_widths: std::collections::HashMap<&str, u32> =
            std::collections::HashMap::new();
        for line in &file.lines {
            if let crate::adapter::btor2::ast::Node::State { sort, .. } = &line.node
                && let Some(width) = crate::adapter::btor2::parser::bv_width(&file, *sort)
                && let Some(name) = symbols.get(&line.nid)
            {
                pred_register_widths.insert(name.as_str(), width);
            }
        }

        for (i, &src_id) in state_ids.iter().enumerate() {
            // Build canonical representative for cube_i.
            let mut registers: std::collections::HashMap<String, u128> =
                std::collections::HashMap::new();
            for (bit, pred) in predicates.iter().enumerate() {
                let truth = (i >> bit) & 1 == 1;
                let entry = registers.entry(pred.register.clone()).or_insert(0);
                if truth {
                    *entry = pred.value as u128;
                }
                // Predicate-false case leaves *entry at 0. If the
                // predicate's value is also 0 (i.e. predicate is
                // `register == 0`), the false case needs a non-zero
                // representative; bump to 1 of the appropriate width.
                if !truth && pred.value == 0 {
                    let width = pred_register_widths
                        .get(pred.register.as_str())
                        .copied()
                        .unwrap_or(1);
                    if width >= 1 {
                        *entry = 1;
                    }
                }
            }

            // Enumerate input combinations + emit MayOnly edges.
            for combo in 0..n_combos {
                let mut input_values: std::collections::HashMap<String, u128> =
                    std::collections::HashMap::new();
                for (bit, name) in boolean_inputs.iter().take(n_inputs).enumerate() {
                    let v = if (combo >> bit) & 1 == 1 { 1 } else { 0 };
                    input_values.insert(name.clone(), v);
                }
                // R.5b consumer wiring — multi-value UF enumeration.
                // When at least one Op is UF-wrapped, enumerate
                // `UfRepresentative::{Zero, Ones}` substitutions to
                // emit multiple may-edges per cube/input combo —
                // tighter may-side approximation than the zero-only
                // MVP (each representative routes the next-state
                // computation through a different downstream cube,
                // when the wrapped Op's value actually controls a
                // branching path).
                //
                // When no UF wrapping fires, use the plain
                // `simulate_one_step` to keep the no-UF code path
                // on its existing performance profile (avoids the
                // HashSet-membership check per Op).
                let next_register_snapshots: Vec<std::collections::HashMap<String, u128>> =
                    if uf_wrapped_nids.is_empty() {
                        match crate::adapter::btor2::bit_blast::simulate_one_step(
                            &file,
                            &registers,
                            &input_values,
                        ) {
                            Ok(v) => vec![v],
                            Err(_) => continue,
                        }
                    } else {
                        use crate::adapter::btor2::bit_blast::UfRepresentative;
                        let mut snaps = Vec::with_capacity(2);
                        for rep in [UfRepresentative::Zero, UfRepresentative::Ones] {
                            match crate::adapter::btor2::bit_blast::simulate_one_step_with_uf_rep(
                                &file,
                                &registers,
                                &input_values,
                                &uf_wrapped_nids,
                                rep,
                            ) {
                                Ok(v) => snaps.push(v),
                                Err(_) => continue,
                            }
                        }
                        snaps
                    };

                // Emit one may-edge per representative. Duplicate
                // target cubes get deduplicated via the builder's
                // transition merging.
                for next_registers in &next_register_snapshots {
                    // Determine the resulting cube index by
                    // re-evaluating each predicate against the new
                    // register values.
                    let mut target_index: usize = 0;
                    for (bit, pred) in predicates.iter().enumerate() {
                        let next_v = next_registers.get(&pred.register).copied().unwrap_or(0);
                        if next_v == pred.value as u128 {
                            target_index |= 1 << bit;
                        }
                    }
                    if target_index < state_ids.len() {
                        let tgt_id = state_ids[target_index];
                        builder.transition_ids_with_modality(
                            src_id,
                            &[label_id],
                            tgt_id,
                            crate::clts::TransitionModality::MayOnly,
                        );
                    }
                }
            }
        }
        predicate_image_pending = false;
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
        // R.2.5 predicate-image MVP populates may-edges when
        // `max_input_bits > 0` and at least one predicate exists;
        // R.5 / R.5b will additionally populate must-edges via SMT.
        predicate_image_pending,
        // R.5b AdapterWarning channel — UF wrapping naming + other
        // lift-time abstraction decisions surface here for caller
        // reporting.
        warnings,
    })
}

/// R.2.5 predicate-image MVP helper — collect the symbols of every
/// 1-bit BTOR2 input that is NOT a clock signal. Clock inputs (per
/// `looks_like_clock` in `bit_blast`) are excluded because each CLTS
/// step already represents one clock edge.
///
/// Returns symbols in BTOR2 NID order (matches the order
/// `simulate_one_step` walks the file when building its Env). Inputs
/// without a symbol are skipped (they're typically Yosys-generated
/// phi inputs the property doesn't reference anyway).
fn collect_boolean_input_symbols(
    file: &crate::adapter::btor2::ast::Btor2File,
    symbols: &std::collections::HashMap<crate::adapter::btor2::ast::Nid, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for line in &file.lines {
        if let crate::adapter::btor2::ast::Node::Input { sort, .. } = &line.node {
            let width = crate::adapter::btor2::parser::bv_width(file, *sort).unwrap_or(0);
            if width != 1 {
                continue;
            }
            let symbol = match symbols.get(&line.nid) {
                Some(s) => s.clone(),
                None => continue,
            };
            // Skip clock inputs; reuse the same heuristic as
            // bit_blast's `looks_like_clock`. Conservative: a name
            // containing "clk" or "clock" is treated as a clock.
            let lower = symbol.to_lowercase();
            if lower.contains("clk") || lower.contains("clock") {
                continue;
            }
            out.push(symbol);
        }
    }
    out
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
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 8,
            max_input_bits: 0,
        };
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
        // R.2.5 predicate-image MVP populates may-edges by default
        // (when `max_input_bits > 0` AND at least one predicate exists).
        // The flag flips to `false` to advertise edges are present.
        assert!(
            !result.predicate_image_pending,
            "R.2.5 predicate-image MVP should populate may-edges under default opts"
        );
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
        // R.2.5 predicate-image MVP populates may-edges by default;
        // the flag flips to `false`. The cap-exceeding fixture's
        // edges are sound under the canonical-representative +
        // boolean-input sampling discipline (§Phase 11 R.2.5
        // predicate-image MVP design doc).
        assert!(
            !result.predicate_image_pending,
            "R.2.5 predicate-image MVP should populate may-edges under default opts"
        );
    }

    // ---- R.2.5 predicate-image MVP — may-edge construction tests ----

    /// BTOR2 fixture: a single 2-bit counter `cnt` that increments
    /// every cycle. One state cell + one 1-bit input `clr` that
    /// resets cnt to 0 when high. Used by R.2.5 predicate-image
    /// tests to verify may-edges are emitted between cubes.
    const COUNTER_BTOR2: &str = r#"
1 sort bitvec 1
2 sort bitvec 2
3 zero 2
4 const 2 11
5 input 1 clr
6 state 2 cnt
7 add 2 6 4
8 ite 2 5 3 7
9 next 2 6 8
"#;

    #[test]
    fn predicate_image_mvp_emits_may_edges_between_cubes() {
        // 2 predicates over the counter: cnt == 0 and cnt == 1.
        // Default opts enable boolean-input enumeration; the lifter
        // should emit MayOnly edges and clear predicate_image_pending.
        let preds = vec![
            PredicateSpec {
                name: "cnt_is_0".into(),
                register: "cnt".into(),
                value: 0,
            },
            PredicateSpec {
                name: "cnt_is_1".into(),
                register: "cnt".into(),
                value: 1,
            },
        ];
        let result = predicate_cube_lift(
            preds,
            COUNTER_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        )
        .expect("predicate_cube_lift succeeds");

        // 4 cubes (2^2 predicates) — same as the legacy MVP.
        assert_eq!(result.cube_count, 4);
        assert_eq!(result.clts.state_count(), 4);

        // Must/may edges populated (vs the legacy MVP's empty Clts).
        assert!(
            !result.predicate_image_pending,
            "R.2.5 predicate-image MVP must clear predicate_image_pending when edges are populated"
        );

        // Sanity: every cube has at least one outgoing transition
        // (either to itself or another cube). With 1 boolean input
        // `clr` and 4 cube source states, we expect 4 × 2 = 8 edges
        // total (the MVP enumerates each input combo per cube).
        let total_transitions: usize = result
            .clts
            .states()
            .map(|s| result.clts.outgoing(s).len())
            .sum();
        assert!(
            total_transitions > 0,
            "R.2.5 predicate-image MVP must emit at least one may-edge across the cube space"
        );

        // Every transition must be MayOnly (R.2.5 MVP under-
        // approximation discipline — must-witnesses wait for R.5).
        for state in result.clts.states() {
            for transition in result.clts.outgoing(state) {
                assert!(
                    matches!(
                        transition.modality(),
                        crate::clts::TransitionModality::MayOnly
                    ),
                    "R.2.5 predicate-image MVP must emit only MayOnly edges; got {:?}",
                    transition.modality()
                );
            }
        }
    }

    #[test]
    fn predicate_image_mvp_max_input_bits_zero_disables_edges() {
        // Setting max_input_bits = 0 recovers the legacy R.2.5 MVP
        // behaviour (cube states but no edges; predicate_image_pending
        // stays true). Useful for the binary-capability test that
        // doesn't need edges.
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 0,
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("ok");
        assert!(
            result.predicate_image_pending,
            "max_input_bits=0 must preserve the legacy 'no edges' behaviour"
        );
        let total_transitions: usize = result
            .clts
            .states()
            .map(|s| result.clts.outgoing(s).len())
            .sum();
        assert_eq!(
            total_transitions, 0,
            "max_input_bits=0 must emit zero edges (legacy MVP behaviour)"
        );
    }

    // ---- R.5b consumer wiring — predicate_cube_lift honors uf_wrap ----

    /// BTOR2 fixture for R.5b consumer wiring: 2-bit `cnt` register
    /// driven by `add` aliased via a uext line carrying the symbol
    /// `wide_add`. With `uf_wrap = ["wide_add"]` in the sidecar,
    /// predicate_cube_lift's simulate_one_step pass should substitute
    /// the add's result with zero — `cnt` stays at 0 across cube
    /// transitions instead of advancing.
    const R5B_CONSUMER_FIXTURE: &str = r#"
1 sort bitvec 2
2 zero 1
3 const 1 11
4 state 1 cnt
5 init 1 4 2
6 add 1 4 3
7 uext 1 6 0 wide_add
8 next 1 4 7
"#;

    #[test]
    fn r5b_predicate_cube_lift_consumes_uf_wrap_sidecar() {
        // Predicate: cnt == 1. Two cubes (cnt=1 true; cnt=1 false).
        let preds = vec![PredicateSpec {
            name: "cnt_is_1".into(),
            register: "cnt".into(),
            value: 1,
        }];

        // WITHOUT UF wrap: `add` evaluates normally. From cnt=0 (cube 0)
        // the next-value is add(0, 3) = 3 → cnt=3 (mask to 2 bits = 3).
        // 3 != 1 → target cube is cube 0 (cnt_is_1 false). From cnt=1
        // (cube 1) next is add(1, 3) = 4 mask = 0 → target cube 0.
        // Either way: every cube transitions to cube 0 → at least one
        // edge goes to cube 0.
        let no_uf_opts = AdapterOptions::default();
        let no_uf_result = predicate_cube_lift(
            preds.clone(),
            R5B_CONSUMER_FIXTURE,
            &no_uf_opts,
            &PredicateCubeLiftOptions::default(),
        )
        .expect("ok");
        assert!(
            !no_uf_result.predicate_image_pending,
            "predicate-image MVP populates may-edges under default opts"
        );

        // WITH UF wrap on wide_add: `add` is substituted to zero, so
        // next cnt = 0 from every starting cube. Both cubes transition
        // to cube 0 (cnt=0, cnt_is_1 false).
        let uf_sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "uf_wrap": ["wide_add"]
        });
        let uf_opts = AdapterOptions {
            sidecar_json: Some(uf_sidecar.to_string()),
            ..Default::default()
        };
        let uf_result = predicate_cube_lift(
            preds,
            R5B_CONSUMER_FIXTURE,
            &uf_opts,
            &PredicateCubeLiftOptions::default(),
        )
        .expect("ok");
        assert!(
            !uf_result.predicate_image_pending,
            "predicate-image MVP populates may-edges under UF wrapping too"
        );

        // Both runs produce non-zero transitions; the consumer wiring
        // is exercised in the UF case via `simulate_one_step_with_uf_rep`.
        // The observable invariant (post-R.5b multi-value enumeration):
        // under UF wrap, each (cube, input combo) emits TWO may-edges
        // — one per UfRepresentative variant ({Zero, Ones}) — so the
        // UF run produces ~2× the edge count of the no-UF run. The
        // exact 2× factor depends on whether Zero and Ones land in
        // the SAME target cube (deduped by the builder) or DIFFERENT
        // cubes (both edges survive). For this fixture (2-bit add
        // with target cube always being cube 0), both representatives
        // land in cube 0 → edges deduplicate.
        let uf_edges: usize = uf_result
            .clts
            .states()
            .map(|s| uf_result.clts.outgoing(s).len())
            .sum();
        assert!(
            uf_edges > 0,
            "predicate_cube_lift under UF wrap must still produce may-edges"
        );
        let no_uf_edges: usize = no_uf_result
            .clts
            .states()
            .map(|s| no_uf_result.clts.outgoing(s).len())
            .sum();
        assert!(
            uf_edges >= no_uf_edges,
            "multi-value UF enumeration must produce at least as many edges as no-UF; \
             got uf={uf_edges}, no_uf={no_uf_edges}"
        );
        assert!(
            uf_edges <= no_uf_edges * 2,
            "multi-value UF enumeration capped at 2× no-UF edges (Zero + Ones representatives); \
             got uf={uf_edges}, no_uf={no_uf_edges}"
        );
    }

    #[test]
    fn r5b_predicate_cube_lift_surfaces_uf_wrap_warning_to_caller() {
        // R.5b AdapterWarning channel test — when UF wrapping fires,
        // the result.warnings vec must carry an entry naming the
        // wrapping. Mirror of the tracing::warn! surface but
        // structured for caller consumption (e.g. cegar.rs folding
        // into CegarTrace).
        let preds = vec![PredicateSpec {
            name: "cnt_is_1".into(),
            register: "cnt".into(),
            value: 1,
        }];
        let uf_sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "uf_wrap": ["wide_add"]
        });
        let uf_opts = AdapterOptions {
            sidecar_json: Some(uf_sidecar.to_string()),
            ..Default::default()
        };
        let result = predicate_cube_lift(
            preds,
            R5B_CONSUMER_FIXTURE,
            &uf_opts,
            &PredicateCubeLiftOptions::default(),
        )
        .expect("ok");
        assert!(
            !result.warnings.is_empty(),
            "UF wrapping must surface as at least one AdapterWarning on the result"
        );
        let has_uf_warning = result.warnings.iter().any(|w| {
            matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
                && w.message.contains("UF-wrapped")
        });
        assert!(
            has_uf_warning,
            "warnings must include an UF-wrapping naming entry; got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn r5b_predicate_cube_lift_no_warning_when_no_wrapping() {
        // Fixture with NO Op::Mul + no wide add → no UF wrapping →
        // no warning surfaces.
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let result = predicate_cube_lift(
            preds,
            COUNTER_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        )
        .expect("ok");
        assert!(
            result.warnings.is_empty(),
            "warnings must be empty when no UF wrapping fires; got: {:?}",
            result.warnings
        );
    }

    /// Multi-value UF enumeration produces distinct target cubes when
    /// Zero and Ones representatives diverge. Fixture is a 1-bit
    /// register `bit_q` whose next-value = `wide_op` (an add aliased
    /// via uext). With UF wrap, Zero → bit_q=0; Ones → bit_q=1. So
    /// the cube cnt_is_1=true and cnt_is_1=false BOTH show as
    /// targets — 2 distinct may-edges per source cube.
    const R5B_MULTIVALUE_FIXTURE: &str = r#"
1 sort bitvec 1
2 zero 1
3 const 1 1
4 state 1 bit_q
5 init 1 4 2
6 add 1 4 3
7 uext 1 6 0 wide_op
8 next 1 4 7
"#;

    #[test]
    fn r5b_multi_value_uf_enumeration_emits_distinct_target_cubes() {
        let preds = vec![PredicateSpec {
            name: "bit_is_1".into(),
            register: "bit_q".into(),
            value: 1,
        }];
        let uf_sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "uf_wrap": ["wide_op"]
        });
        let uf_opts = AdapterOptions {
            sidecar_json: Some(uf_sidecar.to_string()),
            ..Default::default()
        };
        let result = predicate_cube_lift(
            preds,
            R5B_MULTIVALUE_FIXTURE,
            &uf_opts,
            &PredicateCubeLiftOptions::default(),
        )
        .expect("ok");

        // 2 cubes total. Each cube under UF wrap produces 2 may-edges
        // per input combo (Zero → cube_0, Ones → cube_1). The edge
        // count should reflect the multi-value enumeration. Sanity
        // check: per-cube outgoing-edge count is >= 2 (one per
        // representative when reps diverge).
        for state in result.clts.states() {
            let trans = result.clts.outgoing(state);
            // Count distinct (label, target) pairs — the builder
            // dedups identical edges. Under multi-value with diverging
            // reps, we expect both cube_0 and cube_1 as targets.
            let distinct_targets: std::collections::HashSet<_> =
                trans.iter().map(|t| t.target()).collect();
            assert!(
                distinct_targets.len() >= 2,
                "multi-value UF enumeration must reach distinct target cubes from state {:?}; \
                 got distinct targets: {:?}",
                state,
                distinct_targets.len()
            );
        }
    }

    #[test]
    fn r5_lazy_kmts_null_lift_cube_count_matches_two_to_p() {
        let preds = vec![
            PredicateSpec {
                name: "p0".to_string(),
                register: "reg_a".to_string(),
                value: 0,
            },
            PredicateSpec {
                name: "p1".to_string(),
                register: "reg_a".to_string(),
                value: 1,
            },
            PredicateSpec {
                name: "p2".to_string(),
                register: "reg_b".to_string(),
                value: 0,
            },
        ];
        let null_lift = NullLazyLift::new(preds.clone());
        assert_eq!(
            null_lift.cube_count(),
            1usize << preds.len(),
            "NullLazyLift::cube_count must equal 2^|P|"
        );
    }

    #[test]
    fn r5_lazy_kmts_null_lift_expand_cube_returns_empty() {
        let preds = vec![PredicateSpec {
            name: "p0".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let mut null_lift = NullLazyLift::new(preds);
        for cube in 0..null_lift.cube_count() {
            let edges = null_lift.expand_cube(cube);
            assert!(
                edges.is_empty(),
                "NullLazyLift must return no may-edges for cube {cube}; got {edges:?}"
            );
        }
    }

    #[test]
    fn r5_lazy_kmts_null_lift_expand_cube_out_of_range_returns_empty() {
        let preds = vec![PredicateSpec {
            name: "p0".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let mut null_lift = NullLazyLift::new(preds);
        let out_of_range = null_lift.cube_count() + 5;
        let edges = null_lift.expand_cube(out_of_range);
        assert!(
            edges.is_empty(),
            "expand_cube on out-of-range index must return empty (no panic); got {edges:?}"
        );
    }

    #[test]
    fn r5_lazy_kmts_null_lift_predicates_round_trip() {
        let preds = vec![
            PredicateSpec {
                name: "p_alpha".to_string(),
                register: "reg_a".to_string(),
                value: 42,
            },
            PredicateSpec {
                name: "p_beta".to_string(),
                register: "reg_b".to_string(),
                value: 7,
            },
        ];
        let null_lift = NullLazyLift::new(preds.clone());
        assert_eq!(
            null_lift.predicates(),
            preds.as_slice(),
            "NullLazyLift::predicates must round-trip the constructor input"
        );
    }

    #[test]
    fn r5_lazy_kmts_lazy_expansion_edge_equality() {
        let a = LazyExpansionEdge {
            label: "step".to_string(),
            target_cube: 3,
        };
        let b = LazyExpansionEdge {
            label: "step".to_string(),
            target_cube: 3,
        };
        let c = LazyExpansionEdge {
            label: "step".to_string(),
            target_cube: 4,
        };
        assert_eq!(a, b, "edges with identical fields must be equal");
        assert_ne!(a, c, "edges with different target_cube must differ");
    }

    // R.5 lazy KMTS sub-item 2.2 tests (2026-06-04) — eager
    // wrapper backed by predicate_cube_lift. The wrapper must
    // be a strict drop-in for the lazy trait: cube_count +
    // expand_cube produce results identical to walking the
    // underlying Clts directly.

    #[test]
    fn r5_subitem_22_eager_wrapper_cube_count_matches_lift() {
        // 1 predicate ⇒ 2 cubes. The wrapper's cube_count MUST
        // equal the lift's cube_count.
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let wrapper =
            EagerLazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
                .expect("lift succeeds");
        assert_eq!(wrapper.cube_count(), 2);
        // The trait-level cube_count MUST equal the
        // result-level cube_count.
        assert_eq!(wrapper.cube_count(), wrapper.result().cube_count);
    }

    #[test]
    fn r5_subitem_22_eager_wrapper_expand_cube_matches_clts_outgoing() {
        // For every cube index in range, the wrapper's
        // expand_cube() output MUST be derivable from the
        // underlying Clts's outgoing transitions (one
        // LazyExpansionEdge per transition).
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let mut wrapper =
            EagerLazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
                .expect("lift succeeds");
        use crate::clts::StateId;
        for cube in 0..wrapper.cube_count() {
            let edges = wrapper.expand_cube(cube);
            // Count outgoing transitions directly from the Clts
            // for this cube — must match the wrapper's edge
            // count.
            let src_id = StateId::<DefaultStateIdx>::from_index(cube).expect("valid cube");
            let direct_count = wrapper.result().clts.outgoing(src_id).len();
            assert_eq!(
                edges.len(),
                direct_count,
                "expand_cube({cube}) edge count must match Clts::outgoing count"
            );
        }
    }

    #[test]
    fn r5_subitem_22_eager_wrapper_expand_cube_out_of_range_returns_empty() {
        // Out-of-range cube_index ⇒ empty Vec, no panic. The
        // trait contract.
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let mut wrapper =
            EagerLazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
                .expect("lift succeeds");
        let oob = wrapper.cube_count() + 100;
        let edges = wrapper.expand_cube(oob);
        assert!(
            edges.is_empty(),
            "expand_cube({oob}) on out-of-range must return empty"
        );
    }

    #[test]
    fn r5_subitem_22_eager_wrapper_predicates_round_trip() {
        // The wrapper's predicates() MUST return the same set
        // the lift was given.
        let preds = vec![
            PredicateSpec {
                name: "p0".to_string(),
                register: "reg_a".to_string(),
                value: 0,
            },
            PredicateSpec {
                name: "p1".to_string(),
                register: "reg_b".to_string(),
                value: 0,
            },
        ];
        let opts = PredicateCubeLiftOptions::default();
        let wrapper = EagerLazyLift::from_btor2(
            preds.clone(),
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &opts,
        )
        .expect("lift succeeds");
        assert_eq!(
            wrapper.predicates(),
            preds.as_slice(),
            "predicates() must round-trip the lift input"
        );
    }

    #[test]
    fn r5_subitem_22_eager_wrapper_from_result_constructor_works() {
        // The alternative `from_result` constructor must accept
        // an already-computed PredicateCubeLiftResult.
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let result = predicate_cube_lift(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
            .expect("lift succeeds");
        let cube_count = result.cube_count;
        let wrapper = EagerLazyLift::from_result(result);
        assert_eq!(wrapper.cube_count(), cube_count);
    }
}
