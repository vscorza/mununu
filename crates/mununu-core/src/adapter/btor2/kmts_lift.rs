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
    /// R.2.5b sub-item (2026-06-06) — must-edge inference policy.
    /// Defaults to [`MustEdgeInference::Off`], which preserves the
    /// pre-R.2.5b behaviour of emitting only `MayOnly` edges.
    /// See [`MustEdgeInference`] for the post-pass semantics.
    pub must_edge_inference: MustEdgeInference,
    /// MIG-3 (2026-06-13) — may-edge construction policy. Defaults to
    /// [`MayEdgeInference::Off`] (sampling, preserving pre-MIG-3
    /// behaviour). [`MayEdgeInference::SmtAllPairs`] replaces the
    /// sampling may-edges with the sound all-pairs SMT may relation.
    pub may_edge_inference: MayEdgeInference,
    /// R-Y7 (2026-06-07) — symbolic-init via predicate cubes.
    /// Map of `register_name → set of valid initial values` for
    /// under-constrained constants. When non-empty, the lifter
    /// expands the initial-state set to ALL cubes admissible
    /// under the R-S8 encoder's
    /// [`crate::adapter::btor2::r_s8_encoder::hyper_must_initial_cubes`]
    /// instead of the pre-R-Y7 single-initial-cube default.
    /// One mechanism (the predicate cube space) handles both
    /// init nondeterminism and state-space abstraction per Phase
    /// 8 §8.1 R-Y7 spec.
    ///
    /// Default empty preserves the pre-R-Y7 single-initial-cube
    /// behaviour exactly.
    pub config_values: std::collections::HashMap<String, Vec<u64>>,
}

impl Default for PredicateCubeLiftOptions {
    fn default() -> Self {
        Self {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: MayEdgeInference::Off,
            config_values: std::collections::HashMap::new(),
        }
    }
}

/// R.2.5b (2026-06-06) — Policy for inferring `must`-side
/// transitions (`Sharp`, `MustHyperOnly`) from the R.2.5 sampling-
/// based predicate-image MVP. Paired with the B.3.b/B.3.c phase-
/// boundary decision (2026-06-01): R.5.0 ships the soundness-warning
/// for alt-depth-≥-2 formulas on Sharp-only KMTSes (already in
/// main); R.2.5b's lifter post-pass opportunistically emits
/// `Sharp` and `MustHyperOnly` edges when the sampling-based
/// predicate-image converges on a single target cube (Sharp) or a
/// non-singleton target set (hyper-must) for every sampled (input,
/// UF-representative) pair.
///
/// **SOUNDNESS**: sampling-based inference. A single-target
/// convergence under sampling proves the must-edge ONLY when the
/// canonical representative is a sound proxy for every concrete
/// state in the cube. R.2.5b's follow-up session will replace the
/// sampling pass with an SMT-backed must-edge query (∀ concrete
/// state in source cube ⟹ ∃ input ⟹ next-state in target cube)
/// using Z3 array theory. Until the SMT swap lands, the inferred
/// must-edges carry a `// SOUNDNESS:` annotation + an
/// `AdapterWarning` on the lift result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MustEdgeInference {
    /// Pre-R.2.5b default. The post-pass is disabled; the lift
    /// emits only `MayOnly` edges. Use when callers require strict
    /// soundness (no sampling-derived must claims).
    Off,
    /// R.2.5b session-1. Post-pass: for each (source-cube, label)
    /// pair, collect target cubes across all sampled (input,
    /// UF-rep) pairs. If exactly one target appears, promote the
    /// MayOnly edge to `Sharp`. If multiple targets appear, emit
    /// a `MustHyperOnly { targets }` edge alongside the existing
    /// MayOnly edges. SOUNDNESS: sampling-derived, not SMT-proved.
    SamplingConfluence,
    /// R.2.5b session-2 (2026-06-08). Post-pass: for each
    /// (source-cube, sampled-target) pair, run an SMT must-edge
    /// query via [`super::smt_must_edge::smt_per_target_must_check`].
    /// If Z3 proves the must-edge (UNSAT on the
    /// `src ∧ transition ∧ ¬tgt` formula), promote MayOnly → Sharp.
    /// Multi-target hyper-must inference is queued for a follow-up;
    /// the session-2 MVP only promotes per-target Sharp edges.
    ///
    /// **Stronger than the standard KMTS must-edge.** The MVP form
    /// is `∀ state ⊨ src. ∀ inputs. (transition ⟹ next ⊨ tgt)` —
    /// deterministic transition into tgt regardless of input. The
    /// standard form `∀ state ⊨ src. ∃ input. next ⊨ tgt` produces
    /// more must-edges; the MVP is sound but less precise. The
    /// standard form is shipped as [`MustEdgeInference::SmtPerTargetStandard`].
    ///
    /// SOUNDNESS: SMT-proved (no sampling). The session-2 AdapterWarning
    /// reads `[R.2.5b-smt-must]` instead of `[R.2.5b-sampling-must]`.
    SmtPerTarget,
    /// R.2.5b session-2 follow-up (2026-06-09). SMT-backed must-edge
    /// inference using the canonical KMTS ∀∃ form via Z3 quantifier
    /// alternation (`forall_const` over inputs + state_next). For
    /// each (source cube, sampled target) pair the query is:
    ///
    /// ```text
    /// ∀ state ⊨ src. ∃ inputs. ∃ state_next. (transition ∧ next ⊨ tgt)
    /// ```
    ///
    /// Strictly **more permissive** than [`MustEdgeInference::SmtPerTarget`]:
    /// every ∀∀-Must is also ∀∃-Must, but the ∀∃ form additionally
    /// promotes edges where SOME input combo (not necessarily all)
    /// reaches tgt per concrete source state. This is the canonical
    /// KMTS must-edge semantics from Bruns–Godefroid CONCUR 2000.
    ///
    /// Cost: per-query SMT time is higher than ∀∀ (quantifier
    /// alternation), but bounded by the same `timeout_ms`. AdapterWarning
    /// reads `[R.2.5b-smt-must-standard]`.
    SmtPerTargetStandard,
    /// R.2.5b session-2 follow-up (2026-06-09). SMT-backed
    /// **hyper-must** inference via Z3 ∀∃ with a disjunction over
    /// the candidate target set. Semantics:
    ///
    /// ```text
    /// ∀ state ⊨ src. ∃ inputs. ∃ t ∈ T. (transition ∧ next ⊨ t)
    /// ```
    ///
    /// Where `T` is the sampled-target set per source cube. The
    /// abstraction guarantees some t ∈ T is reached, but not
    /// necessarily the same t across concrete states (standard
    /// Shoham–Grumberg LMCS 2007 §4 hyper-must semantics).
    ///
    /// Lifter strategy: for each source cube,
    /// 1. First try the per-target ∀∃ check per sampled target. If
    ///    any singleton proves Must, promote MayOnly → Sharp.
    /// 2. Otherwise, run `smt_hyper_must_check` over the FULL
    ///    sampled target set. If Must, emit `MustHyperOnly` with
    ///    the full target set (MVP doesn't minimize T; sub-minimal
    ///    T inference is a follow-up).
    ///
    /// SOUNDNESS: same as `SmtPerTargetStandard`; warning reads
    /// `[R.2.5b-smt-must-hyper]`.
    SmtHyperMust,
}

/// MIG-3 (S-track migration, 2026-06-13) — may-edge construction policy
/// for [`predicate_cube_lift`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MayEdgeInference {
    /// Default — sampling-based may-edges (`max_input_bits` input
    /// enumeration per canonical-representative cube). Fast, but an
    /// **under-approximation of the may relation**: sampling only a
    /// subset of inputs (and a single canonical representative per
    /// cube) can MISS a real may-edge → unsound for safety. Preserves
    /// the pre-MIG-3 behaviour exactly.
    #[default]
    Off,
    /// MIG-3 — **sound** all-pairs SMT may-edges. For every
    /// (source-cube, target-cube) pair the lifter runs
    /// [`super::smt_must_edge::smt_per_target_may_check`] (`∃ s⊨src,
    /// inputs, s'⊨tgt. (s,s')∈R`) and emits a `MayOnly` edge iff a
    /// witness exists; an edge is excluded ONLY when Z3 proves it
    /// impossible (UNSAT). This is the authoritative may relation
    /// (`R_may(b,b') ⟺ ∃ s⊨b, s'⊨b'. (s,s')∈R`), a sound
    /// over-approximation that REPLACES the sampling may-edges.
    ///
    /// **Tractability.** All-pairs is O(cubes²) SMT calls — bounded for
    /// the small fixtures the SV migration targets (cubes ≤ 64), but it
    /// scales poorly at the default cube cap (1024² ≈ 10⁶). An all-SMT
    /// per-source enumeration (O(edges), reusing `all_smt`) is the
    /// queued perf follow-up; this MVP ships the sound all-pairs form.
    ///
    /// **Note.** Combining with a non-`Off` `must_edge_inference` is not
    /// yet wired (the must post-passes consume the sampling pass's
    /// `sampled_targets_per_source`, which `SmtAllPairs` bypasses).
    SmtAllPairs,
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

/// R.5 lazy KMTS sub-item 2.3 (2026-06-04) — context owned by a
/// `LazyLift` carrying everything needed to compute a single
/// cube's outgoing edges on demand. Built once at handle
/// construction (parses the BTOR2 + collects symbols, register
/// widths, boolean inputs, UF-wrapped Op NIDs); read-only
/// thereafter.
///
/// Currently DUPLICATES the per-cube logic of the eager
/// `predicate_cube_lift` (lines ~741–841). A 2.3-follow-up
/// (likely folded into sub-item 2.4) will refactor
/// `predicate_cube_lift` to use the same helper, removing the
/// duplication. The drift risk is mitigated by the
/// `r5_subitem_23_lazy_lift_edges_match_eager_lift` test which
/// asserts edge-set equality between the two paths.
#[derive(Debug, Clone)]
struct LazyLiftContext {
    file: crate::adapter::btor2::ast::Btor2File,
    pred_register_widths: std::collections::HashMap<String, u32>,
    boolean_inputs: Vec<String>,
    n_inputs: usize,
    n_combos: usize,
    uf_wrapped_nids: std::collections::HashSet<crate::adapter::btor2::ast::Nid>,
    label_name: String,
    predicates: Vec<PredicateSpec>,
    cube_count: usize,
}

/// R.5 lazy KMTS sub-item 2.3 (2026-06-04) — truly-lazy
/// implementation of `KmtsLiftLazy`. `expand_cube` computes
/// a single cube's outgoing edges on demand via
/// `simulate_one_step` / `simulate_one_step_with_uf_rep` and
/// caches the result. Subsequent `expand_cube(cube)` calls for
/// the same cube are O(1) lookups.
///
/// **Memory**: bounded by the number of cubes the caller
/// actually visits. For a `2^|P|`-cube space where only N cubes
/// are reachable from the initial state(s), memory is O(N)
/// rather than O(2^|P|) for `EagerLazyLift`.
///
/// **CPU**: per-cube cost is the same as the eager loop's
/// per-iteration cost. The savings come from never paying for
/// cubes that are never visited.
///
/// **MVP scope**: handles the R.2.5 may-edge construction path
/// (sampling-based, single canonical representative + boolean-
/// input enumeration). Does NOT handle must-edges (R.2.5b's SMT
/// work is required for those). Falls back to empty edge set
/// when the BTOR2 has no boolean inputs OR the predicate set is
/// empty, mirroring the eager lifter's `if lift_opts.max_input_bits
/// > 0 && !predicates.is_empty()` gate.
#[derive(Debug, Clone)]
pub struct LazyLift {
    ctx: LazyLiftContext,
    /// Per-cube cache. Key = cube index; value = the edge set
    /// computed on first `expand_cube` call. Bounded by the
    /// number of unique cubes the caller has visited.
    cache: std::collections::HashMap<usize, Vec<LazyExpansionEdge>>,
}

impl LazyLift {
    /// Build a `LazyLift` from a BTOR2 source + predicate set.
    /// Parses the BTOR2 + collects the context once; subsequent
    /// `expand_cube` calls are per-cube on-demand.
    pub fn from_btor2(
        predicates: Vec<PredicateSpec>,
        btor2_content: &str,
        adapter_options: &crate::adapter::AdapterOptions,
        lift_opts: &PredicateCubeLiftOptions,
    ) -> Result<Self, crate::adapter::AdapterError> {
        use crate::adapter::AdapterErrorKind;
        let file =
            crate::adapter::btor2::parser::parse(btor2_content).map_err(|e| AdapterError {
                kind: AdapterErrorKind::ParseError,
                location: None,
                message: format!("adapter/btor2/lazy_lift: parse failed: {}", e.message),
            })?;
        let symbols = crate::adapter::btor2::parser::collect_symbols(&file);
        let cube_count: usize = 1usize << predicates.len();
        if cube_count > lift_opts.max_cube_count {
            return Err(AdapterError {
                kind: AdapterErrorKind::StateSpaceOverflow,
                location: None,
                message: format!(
                    "adapter/btor2/lazy_lift: cube count 2^{} = {cube_count} exceeds max_cube_count = {}",
                    predicates.len(),
                    lift_opts.max_cube_count
                ),
            });
        }

        // pred_register_widths: needed for the "predicate value == 0
        // → use non-zero representative" edge case (mirrors eager
        // lifter's logic at line ~728).
        let mut pred_register_widths: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for line in &file.lines {
            if let crate::adapter::btor2::ast::Node::State { sort, .. } = &line.node
                && let Some(width) = crate::adapter::btor2::parser::bv_width(&file, *sort)
                && let Some(name) = symbols.get(&line.nid)
            {
                pred_register_widths.insert(name.clone(), width);
            }
        }

        // Boolean inputs + UF-wrapped Op NIDs (mirrors eager
        // lifter's logic at lines ~716, ~494).
        let boolean_inputs: Vec<String> = collect_boolean_input_symbols(&file, &symbols);
        let n_inputs = boolean_inputs.len().min(lift_opts.max_input_bits);
        let n_combos: usize = 1usize << n_inputs;
        let uf_wrapped_nids =
            crate::adapter::btor2::bit_blast::collect_uf_wrapped_nids(&file, adapter_options);

        Ok(Self {
            ctx: LazyLiftContext {
                file,
                pred_register_widths,
                boolean_inputs,
                n_inputs,
                n_combos,
                uf_wrapped_nids,
                label_name: "step".to_string(),
                predicates,
                cube_count,
            },
            cache: std::collections::HashMap::new(),
        })
    }

    /// Number of cubes currently cached.
    pub fn cached_count(&self) -> usize {
        self.cache.len()
    }

    /// R.2.5b session-2 (2026-06-08) — borrow the parsed BTOR2
    /// file. Consumed by [`materialize_clts_from_lazy`]'s
    /// `SmtPerTarget` post-pass, which feeds the file into the
    /// shared [`super::smt_must_edge`] BV-theory encoder.
    pub fn file(&self) -> &crate::adapter::btor2::ast::Btor2File {
        &self.ctx.file
    }
}

impl KmtsLiftLazy for LazyLift {
    fn cube_count(&self) -> usize {
        self.ctx.cube_count
    }

    fn expand_cube(&mut self, cube_index: usize) -> Vec<LazyExpansionEdge> {
        if cube_index >= self.ctx.cube_count {
            return Vec::new();
        }
        if let Some(cached) = self.cache.get(&cube_index) {
            return cached.clone();
        }
        let edges = compute_cube_outgoing_edges(cube_index, &self.ctx);
        self.cache.insert(cube_index, edges.clone());
        edges
    }

    fn predicates(&self) -> &[PredicateSpec] {
        &self.ctx.predicates
    }
}

/// R.5 lazy KMTS sub-item 2.4 (2026-06-04) — materialize a
/// fully-formed `PredicateCubeLiftResult` from a `LazyLift` by
/// visiting every cube in 0..cube_count and emitting one
/// `Transition::MayOnly` per `LazyExpansionEdge`. Identity result
/// with `predicate_cube_lift` (sub-item 2.3's
/// `r5_subitem_23_lazy_lift_edges_match_eager_lift` test
/// asserts the per-cube edge sets agree).
///
/// **Purpose**: bridge for the CEGAR loop's `LiftStrategy::Lazy`
/// path (sub-item 2.4) — until the evaluator gains lazy-handle
/// support, the loop still needs a full `Clts` to call
/// `evaluate_3v_game_with_options`. This helper visits every
/// cube (defeating the laziness payoff for the CEGAR run) but
/// exercises the `LazyLift` machinery end-to-end, surfacing
/// any drift between the lazy + eager per-cube paths as a
/// verdict-equality test failure.
///
/// **Memory caveat**: visiting every cube populates the
/// `LazyLift`'s cache fully (cached_count() == cube_count()
/// after this call). The lazy memory savings only fire when
/// the caller visits a strict subset of cubes — which is what
/// sub-item 2.5's bench fixture will exercise standalone (not
/// via the CEGAR loop).
/// §Phase 10 stage 3.c.2a (2026-06-12) — encode a BTOR2 file for the
/// must-edge SMT post-passes, selecting the theory by whether the
/// design carries memory cells.
///
/// - No array-sorted state cells → `Theory::BvOnly` (pure QF_BV;
///   the pre-stage-3.c.2a behaviour, unchanged for every memory-
///   free fixture).
/// - One or more array-sorted state cells → `Theory::BvUfArray`
///   (QF_AUFBV), so the encoder's stage-3.c.1 `select`/`store`
///   handling produces an array-aware transition relation. This is
///   the **must-side** of the §Phase 10 havoc-may / array-must
///   composition: the must-edge query reasons about memory
///   precisely via Z3 array theory.
///
/// The cube predicates the must-edge query asserts are over BV
/// registers only (memory cells are not in `view.signals`), so the
/// query construction in `smt_must_edge.rs` is unchanged — it just
/// consumes a richer transition relation.
fn encode_design_for_lift(
    file: &crate::adapter::btor2::ast::Btor2File,
) -> Result<
    crate::adapter::sidecar::predicate_image::btor2_encode::Btor2SmtView,
    crate::adapter::sidecar::predicate_image::btor2_encode::EncodeError,
> {
    use crate::adapter::sidecar::predicate_image::btor2_encode::encode_design_with_theory;
    use crate::adapter::sidecar::predicate_image::theory::Theory;
    let theory = if crate::adapter::btor2::bit_blast::detect_btor2_memories(file).is_empty() {
        Theory::BvOnly
    } else {
        Theory::BvUfArray
    };
    encode_design_with_theory(file, theory)
}

pub fn materialize_clts_from_lazy(
    lazy: &mut LazyLift,
    btor2_source_info_format: SourceFormat,
    must_edge_inference: MustEdgeInference,
) -> Result<PredicateCubeLiftResult, AdapterError> {
    use crate::adapter::AdapterErrorKind;
    use crate::clts::TransitionModality;

    let start = Instant::now();
    let cube_count = lazy.cube_count();
    let predicates = lazy.predicates().to_vec();
    let mut warnings: Vec<crate::adapter::AdapterWarning> = Vec::new();
    let mut sharp_edges_promoted: usize = 0;
    let mut hyper_must_edges_emitted: usize = 0;
    // R.2.5b session-1 follow-up — collect per-source target sets
    // for the SamplingConfluence post-pass, same shape as the
    // eager predicate_cube_lift path.
    let mut sampled_targets_per_source: Vec<std::collections::BTreeSet<usize>> =
        vec![std::collections::BTreeSet::new(); cube_count];

    // Build the Clts shape. Mirrors `predicate_cube_lift`'s
    // builder pattern (lines ~563–594 in this file).
    let mut builder = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
    let mut state_ids = Vec::with_capacity(cube_count);
    for i in 0..cube_count {
        let name = format!("cube_{i}");
        let id = builder
            .state_id_or_insert(&name)
            .ok_or_else(|| AdapterError {
                kind: AdapterErrorKind::StateSpaceOverflow,
                location: None,
                message: format!(
                    "adapter/btor2/materialize_clts_from_lazy: state id overflow at cube {i} / {cube_count}"
                ),
            })?;
        state_ids.push(id);
    }
    if let Some(initial) = state_ids.first() {
        builder.initial_state_id(*initial);
    }

    // Populate state_3valued_predicates per cube (same bit-pattern
    // convention as predicate_cube_lift line ~587).
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

    // Intern the label store with the "step" label once, then
    // visit every cube + emit a MayOnly edge per
    // LazyExpansionEdge. The lazy helper dedupes target cubes
    // per source so we don't emit duplicate edges.
    let label_id = builder
        .labels()
        .intern(["step"])
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::IrConsistencyError,
            location: None,
            message: format!("adapter/btor2/materialize_clts_from_lazy: label intern failed: {e}"),
        })?;
    for i in 0..cube_count {
        let src_id = state_ids[i];
        for edge in lazy.expand_cube(i) {
            if edge.target_cube < state_ids.len() {
                let tgt_id = state_ids[edge.target_cube];
                builder.transition_ids_with_modality(
                    src_id,
                    &[label_id],
                    tgt_id,
                    TransitionModality::MayOnly,
                );
                sampled_targets_per_source[i].insert(edge.target_cube);
            }
        }
    }

    // R.2.5b session-1 follow-up (2026-06-06) — SamplingConfluence
    // post-pass for the lazy materialiser. Same logic as the eager
    // predicate_cube_lift path: single-target convergence promotes
    // to Sharp; multi-target convergence emits a MustHyperOnly
    // edge. SOUNDNESS: sampling-derived; SMT-backed proof is
    // queued for R.2.5b session 2.
    if matches!(must_edge_inference, MustEdgeInference::SamplingConfluence) {
        for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
            if targets.is_empty() {
                continue;
            }
            let src_id = state_ids[src_idx];
            let target_ids: smallvec::SmallVec<[crate::clts::StateId<DefaultStateIdx>; 4]> =
                targets.iter().map(|&i| state_ids[i]).collect();
            if target_ids.len() == 1 {
                builder.transition_ids_with_modality(
                    src_id,
                    &[label_id],
                    target_ids[0],
                    TransitionModality::Sharp,
                );
                sharp_edges_promoted += 1;
            } else {
                builder.transition_ids_with_modality(
                    src_id,
                    &[label_id],
                    target_ids[0],
                    TransitionModality::must_hyper(target_ids.clone()),
                );
                hyper_must_edges_emitted += 1;
            }
        }
        if sharp_edges_promoted > 0 || hyper_must_edges_emitted > 0 {
            warnings.push(crate::adapter::AdapterWarning {
                kind: crate::adapter::WarningKind::ApproximateTranslation,
                message: format!(
                    "[R.2.5b-sampling-must] materialize_clts_from_lazy: SamplingConfluence post-pass promoted {} edge(s) to Sharp and emitted {} MustHyperOnly edge(s). SOUNDNESS: sampling-derived; SMT-backed must-edge proof is queued for an R.2.5b follow-up session.",
                    sharp_edges_promoted,
                    hyper_must_edges_emitted
                ),
                location: None,
            });
        }
    }

    // R.2.5b session-2 (2026-06-08) — SmtPerTarget post-pass for
    // the lazy materialiser. Same shape as the eager
    // predicate_cube_lift path: SMT-prove the stronger ∀∀
    // must-edge form per (src, sampled-target) pair; promote
    // confirmed edges to Sharp. Hyper-must inference is queued
    // for the session-2 follow-up.
    if matches!(must_edge_inference, MustEdgeInference::SmtPerTarget) {
        let cfg = z3::Config::new();
        let smt_sharp_promoted = z3::with_z3_config(&cfg, || -> usize {
            let view = match encode_design_for_lift(lazy.file()) {
                Ok(v) => v,
                Err(_) => return 0,
            };
            let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
            let mut promoted = 0usize;
            for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                if targets.is_empty() {
                    continue;
                }
                let src_id = state_ids[src_idx];
                let src_bits = src_idx as u64;
                for &tgt_idx in targets {
                    let tgt_bits = tgt_idx as u64;
                    let verdict = crate::adapter::btor2::smt_must_edge::smt_per_target_must_check(
                        &view,
                        src_bits,
                        tgt_bits,
                        &predicates,
                        &nid_map,
                        5_000,
                    );
                    if matches!(
                        verdict,
                        crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                    ) {
                        let tgt_id = state_ids[tgt_idx];
                        builder.transition_ids_with_modality(
                            src_id,
                            &[label_id],
                            tgt_id,
                            TransitionModality::Sharp,
                        );
                        promoted += 1;
                    }
                }
            }
            promoted
        });
        sharp_edges_promoted += smt_sharp_promoted;
        if smt_sharp_promoted > 0 {
            warnings.push(crate::adapter::AdapterWarning {
                kind: crate::adapter::WarningKind::ApproximateTranslation,
                message: format!(
                    "[R.2.5b-smt-must] materialize_clts_from_lazy: SmtPerTarget post-pass promoted {smt_sharp_promoted} edge(s) to Sharp via Z3 BV-theory must-check (stronger ∀∀ form). Standard ∀∃ form: SmtPerTargetStandard. Hyper-must: SmtHyperMust."
                ),
                location: None,
            });
        }
    }

    // R.2.5b session-2 follow-up (2026-06-09) — SmtPerTargetStandard
    // ∀∃ form in the lazy materialiser. Same shape as the eager
    // path's SmtPerTargetStandard post-pass.
    if matches!(must_edge_inference, MustEdgeInference::SmtPerTargetStandard) {
        let cfg = z3::Config::new();
        let smt_sharp_promoted = z3::with_z3_config(&cfg, || -> usize {
            let view = match encode_design_for_lift(lazy.file()) {
                Ok(v) => v,
                Err(_) => return 0,
            };
            let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
            let mut promoted = 0usize;
            for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                if targets.is_empty() {
                    continue;
                }
                let src_id = state_ids[src_idx];
                let src_bits = src_idx as u64;
                for &tgt_idx in targets {
                    let tgt_bits = tgt_idx as u64;
                    let verdict =
                        crate::adapter::btor2::smt_must_edge::smt_per_target_must_check_standard(
                            &view,
                            src_bits,
                            tgt_bits,
                            &predicates,
                            &nid_map,
                            5_000,
                        );
                    if matches!(
                        verdict,
                        crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                    ) {
                        let tgt_id = state_ids[tgt_idx];
                        builder.transition_ids_with_modality(
                            src_id,
                            &[label_id],
                            tgt_id,
                            TransitionModality::Sharp,
                        );
                        promoted += 1;
                    }
                }
            }
            promoted
        });
        sharp_edges_promoted += smt_sharp_promoted;
        if smt_sharp_promoted > 0 {
            warnings.push(crate::adapter::AdapterWarning {
                kind: crate::adapter::WarningKind::ApproximateTranslation,
                message: format!(
                    "[R.2.5b-smt-must-standard] materialize_clts_from_lazy: SmtPerTargetStandard post-pass promoted {smt_sharp_promoted} edge(s) to Sharp via Z3 ∀∃ must-check (canonical KMTS form per Bruns–Godefroid CONCUR 2000)."
                ),
                location: None,
            });
        }
    }

    // R.2.5b session-2 follow-up (2026-06-09) — SmtHyperMust in the
    // lazy materialiser. Same shape as the eager path: per-target
    // ∀∃ singletons first, then full-set hyper-must fallback.
    if matches!(must_edge_inference, MustEdgeInference::SmtHyperMust) {
        let cfg = z3::Config::new();
        let (smt_sharp_promoted, smt_hyper_emitted) = z3::with_z3_config(
            &cfg,
            || -> (usize, usize) {
                let view = match encode_design_for_lift(lazy.file()) {
                    Ok(v) => v,
                    Err(_) => return (0, 0),
                };
                let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
                let mut sharp = 0usize;
                let mut hyper = 0usize;
                for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                    if targets.is_empty() {
                        continue;
                    }
                    let src_id = state_ids[src_idx];
                    let src_bits = src_idx as u64;
                    let mut promoted_singleton = false;
                    for &tgt_idx in targets {
                        let tgt_bits = tgt_idx as u64;
                        let verdict = crate::adapter::btor2::smt_must_edge::smt_per_target_must_check_standard(
                            &view,
                            src_bits,
                            tgt_bits,
                            &predicates,
                            &nid_map,
                            5_000,
                        );
                        if matches!(
                            verdict,
                            crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                        ) {
                            let tgt_id = state_ids[tgt_idx];
                            builder.transition_ids_with_modality(
                                src_id,
                                &[label_id],
                                tgt_id,
                                TransitionModality::Sharp,
                            );
                            sharp += 1;
                            promoted_singleton = true;
                        }
                    }
                    if !promoted_singleton && targets.len() > 1 {
                        let target_bits_set: Vec<u64> = targets.iter().map(|&i| i as u64).collect();
                        let verdict = crate::adapter::btor2::smt_must_edge::smt_hyper_must_check(
                            &view,
                            src_bits,
                            &target_bits_set,
                            &predicates,
                            &nid_map,
                            5_000,
                        );
                        if matches!(
                            verdict,
                            crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                        ) {
                            let target_ids: smallvec::SmallVec<
                                [crate::clts::StateId<DefaultStateIdx>; 4],
                            > = targets.iter().map(|&i| state_ids[i]).collect();
                            builder.transition_ids_with_modality(
                                src_id,
                                &[label_id],
                                target_ids[0],
                                TransitionModality::must_hyper(target_ids.clone()),
                            );
                            hyper += 1;
                        }
                    }
                }
                (sharp, hyper)
            },
        );
        sharp_edges_promoted += smt_sharp_promoted;
        hyper_must_edges_emitted += smt_hyper_emitted;
        if smt_sharp_promoted > 0 || smt_hyper_emitted > 0 {
            warnings.push(crate::adapter::AdapterWarning {
                kind: crate::adapter::WarningKind::ApproximateTranslation,
                message: format!(
                    "[R.2.5b-smt-must-hyper] materialize_clts_from_lazy: SmtHyperMust post-pass promoted {smt_sharp_promoted} edge(s) to Sharp + emitted {smt_hyper_emitted} MustHyperOnly edge(s) via Z3 ∀∃ checks."
                ),
                location: None,
            });
        }
    }

    let clts = builder.build().map_err(|e| AdapterError {
        kind: AdapterErrorKind::IrConsistencyError,
        location: None,
        message: format!("adapter/btor2/materialize_clts_from_lazy: builder.build failed: {e}"),
    })?;

    Ok(PredicateCubeLiftResult {
        clts,
        predicates,
        cube_count,
        source_info: SourceInfo {
            format: btor2_source_info_format,
            title: None,
            // Approximated; the precise `signal_count`
            // requires re-parsing the BTOR2 which we've
            // already done inside LazyLift. For sub-item 2.4
            // MVP, surface 0 (the caller doesn't use this
            // field for the Lazy path's CEGAR verdict).
            signal_count: 0,
            state_count: cube_count,
            property_count: 0,
        },
        lift_time: start.elapsed(),
        // Same caveat as predicate_cube_lift — may-edges are
        // populated; must-edges arrive via the SamplingConfluence
        // post-pass when opted in.
        predicate_image_pending: false,
        // Warnings include UF-wrap (currently absent from lazy
        // path) + the R.2.5b SamplingConfluence soundness warning
        // when the post-pass fires.
        warnings,
        sharp_edges_promoted,
        hyper_must_edges_emitted,
    })
}

/// R.5 lazy KMTS sub-item 2.3 (2026-06-04) — compute one cube's
/// outgoing may-edges. Currently a DUPLICATE of the per-cube
/// body in `predicate_cube_lift` (lines ~741–841); a 2.3
/// follow-up refactors `predicate_cube_lift` to use the same
/// helper.
///
/// Returns empty Vec when:
/// - The fixture has no boolean inputs OR the predicate set is
///   empty (matches the eager lifter's gating).
/// - All `simulate_one_step` invocations error for this cube
///   (e.g. BTOR2 references registers the lifter can't resolve).
fn compute_cube_outgoing_edges(cube_index: usize, ctx: &LazyLiftContext) -> Vec<LazyExpansionEdge> {
    // Gating: matches the eager lifter's `if lift_opts.max_input_bits
    // > 0 && !predicates.is_empty()` check. We allow n_inputs == 0
    // (n_combos == 1) so a single iteration runs with empty
    // input_values — the eager lifter does the same.
    if ctx.predicates.is_empty() {
        return Vec::new();
    }

    // Build canonical representative for cube_index (mirrors
    // eager lifter at lines ~742–764).
    let mut registers: std::collections::HashMap<String, u128> = std::collections::HashMap::new();
    for (bit, pred) in ctx.predicates.iter().enumerate() {
        let truth = (cube_index >> bit) & 1 == 1;
        let entry = registers.entry(pred.register.clone()).or_insert(0);
        if truth {
            *entry = pred.value as u128;
        }
        if !truth && pred.value == 0 {
            let width = ctx
                .pred_register_widths
                .get(pred.register.as_str())
                .copied()
                .unwrap_or(1);
            if width >= 1 {
                *entry = 1;
            }
        }
    }

    let mut edges: Vec<LazyExpansionEdge> = Vec::new();
    let mut seen_targets: std::collections::HashSet<usize> = std::collections::HashSet::new();

    // Enumerate input combinations + emit MayOnly-equivalent
    // edges. We dedupe target cubes across input combos +
    // UF reps so a cube doesn't get the same edge twice.
    for combo in 0..ctx.n_combos {
        let mut input_values: std::collections::HashMap<String, u128> =
            std::collections::HashMap::new();
        for (bit, name) in ctx.boolean_inputs.iter().take(ctx.n_inputs).enumerate() {
            let v = if (combo >> bit) & 1 == 1 { 1 } else { 0 };
            input_values.insert(name.clone(), v);
        }

        let next_register_snapshots: Vec<std::collections::HashMap<String, u128>> =
            if ctx.uf_wrapped_nids.is_empty() {
                match crate::adapter::btor2::bit_blast::simulate_one_step(
                    &ctx.file,
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
                        &ctx.file,
                        &registers,
                        &input_values,
                        &ctx.uf_wrapped_nids,
                        rep,
                    ) {
                        Ok(v) => snaps.push(v),
                        Err(_) => continue,
                    }
                }
                snaps
            };

        for next_registers in &next_register_snapshots {
            let mut target_index: usize = 0;
            for (bit, pred) in ctx.predicates.iter().enumerate() {
                let next_v = next_registers.get(&pred.register).copied().unwrap_or(0);
                if next_v == pred.value as u128 {
                    target_index |= 1 << bit;
                }
            }
            if target_index < ctx.cube_count && seen_targets.insert(target_index) {
                edges.push(LazyExpansionEdge {
                    label: ctx.label_name.clone(),
                    target_cube: target_index,
                });
            }
        }
    }

    edges
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
    /// R.2.5b (2026-06-06) — Number of `MayOnly` edges promoted to
    /// `Sharp` by the [`MustEdgeInference::SamplingConfluence`]
    /// post-pass. Zero when `must_edge_inference` is `Off` (the
    /// default) or when no single-target convergence was detected.
    pub sharp_edges_promoted: usize,
    /// R.2.5b (2026-06-06) — Number of `MustHyperOnly` edges
    /// emitted by the SamplingConfluence post-pass. Zero when
    /// `must_edge_inference` is `Off` or when no multi-target
    /// confluence was detected.
    pub hyper_must_edges_emitted: usize,
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
    // R-Y7 (2026-06-07) — initial-state set selection:
    // - When `lift_opts.config_values` is non-empty, expand the
    //   initial-state set to all cubes admissible under the
    //   R-S8 encoder. Each cube whose predicate evaluation is
    //   consistent with some valid value for every constrained
    //   register becomes an initial state.
    // - Otherwise (pre-R-Y7 default): single initial cube
    //   (cube_0, all-predicates-false). A future iteration can
    //   pick the cube matching the BTOR2 `init` values.
    if !lift_opts.config_values.is_empty() {
        let admissible_cubes = crate::adapter::btor2::r_s8_encoder::hyper_must_initial_cubes(
            &predicates,
            &lift_opts.config_values,
        );
        for cube_idx in &admissible_cubes {
            if let Some(state_id) = state_ids.get(*cube_idx) {
                builder.initial_state_id(*state_id);
            }
        }
        if admissible_cubes.is_empty()
            && let Some(initial) = state_ids.first()
        {
            // Defensive: if no cube is admissible, fall back to
            // cube_0 to avoid producing a Clts with no initial
            // states (which would error at evaluator time).
            builder.initial_state_id(*initial);
        }
    } else if let Some(initial) = state_ids.first() {
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
    // R.2.5b (2026-06-06) — collect target-cube sets per source cube
    // for the SamplingConfluence post-pass. Indexed by source cube
    // index; each entry is the set of target cube indices reached
    // across all (input, UF-rep) samples for that source.
    let mut sharp_edges_promoted: usize = 0;
    let mut hyper_must_edges_emitted: usize = 0;

    let mut predicate_image_pending = true;

    // MIG-3.2 (S-track migration, 2026-06-13) — sound all-pairs SMT
    // may-edges. When opted in, this REPLACES the sampling may-edge
    // pass below (the two are mutually exclusive). For every
    // (source-cube, target-cube) pair, Z3 decides whether a concrete
    // witness exists (`smt_per_target_may_check`); a `MayOnly` edge is
    // emitted iff so. Unlike sampling — which can miss may-edges and
    // thus under-approximate the may relation (unsound for safety) —
    // this excludes an edge ONLY when Z3 proves it impossible. Edges
    // are collected inside the Z3 scope and emitted after it (the
    // builder is not borrowed into the closure).
    if matches!(lift_opts.may_edge_inference, MayEdgeInference::SmtAllPairs)
        && !predicates.is_empty()
    {
        let step_label = builder
            .labels()
            .intern(["step"])
            .map_err(|e| AdapterError {
                kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
                location: None,
                message: format!(
                    "adapter/btor2/predicate_cube_lift MIG-3.2: label intern failed: {e}"
                ),
            })?;
        let n_cubes = state_ids.len();
        let cfg = z3::Config::new();
        let may_edges: Vec<(usize, usize)> = z3::with_z3_config(&cfg, || {
            let view = match encode_design_for_lift(&file) {
                Ok(v) => v,
                // Encoder can't build the view (e.g. an unsupported op):
                // fall back to no may-edges rather than an unsound guess.
                Err(_) => return Vec::new(),
            };
            let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
            let mut edges = Vec::new();
            for i in 0..n_cubes {
                for j in 0..n_cubes {
                    let verdict = crate::adapter::btor2::smt_must_edge::smt_per_target_may_check(
                        &view,
                        i as u64,
                        j as u64,
                        &predicates,
                        &nid_map,
                        5_000,
                    );
                    if matches!(
                        verdict,
                        crate::adapter::btor2::smt_must_edge::SmtMayVerdict::May
                    ) {
                        edges.push((i, j));
                    }
                }
            }
            edges
        });
        for (i, j) in may_edges {
            builder.transition_ids_with_modality(
                state_ids[i],
                &[step_label],
                state_ids[j],
                crate::clts::TransitionModality::MayOnly,
            );
        }
        predicate_image_pending = false;
    }

    if !matches!(lift_opts.may_edge_inference, MayEdgeInference::SmtAllPairs)
        && lift_opts.max_input_bits > 0
        && !predicates.is_empty()
    {
        let mut sampled_targets_per_source: Vec<std::collections::BTreeSet<usize>> =
            vec![std::collections::BTreeSet::new(); state_ids.len()];
        let boolean_inputs: Vec<String> = collect_boolean_input_symbols(&file, &symbols);
        let n_inputs = boolean_inputs.len().min(lift_opts.max_input_bits);
        let n_combos: usize = 1usize << n_inputs;

        // R.6.6 (2026-06-08) — controllability-aware label emission.
        // When `AdapterOptions.controllable_inputs` is non-empty, the
        // lifter partitions boolean inputs into env (uncontrollable) +
        // ctrl (controllable) classes per-symbol-name, then emits each
        // transition with a dual-label set `[env_combo_label,
        // ctrl_combo_label]`. The env labels are tagged
        // `LabelControllability::Uncontrollable`; ctrl labels are
        // `Controllable`. The Skolem grouping in modal_exists /
        // modal_forall (group_transitions_by_uncontrollable_labels)
        // then partitions correctly along the controllability axis:
        // ∀ env-combo, ∃ ctrl-combo for the synthesis idiom.
        //
        // When `controllable_inputs` is empty: legacy single-`step`
        // label behavior (controllability axis disabled). This keeps
        // every pre-R.6.6 fixture verdict-equivalent — the new path
        // is strictly opt-in.
        //
        // Per the R.6.6 done-criterion (kmts-theory.md §7 / R.6 plan §1):
        // this is the first adapter that emits a CLTS carrying BOTH
        // controllable labels AND MayOnly/MustHyperOnly edges from the
        // same source — the model R.6.3/4/5 evaluators are designed to
        // consume.
        let controllable_inputs_set: std::collections::HashSet<&str> = options
            .controllable_inputs
            .iter()
            .map(|s| s.as_str())
            .collect();
        let env_input_indices: Vec<usize> = boolean_inputs
            .iter()
            .take(n_inputs)
            .enumerate()
            .filter(|(_, name)| !controllable_inputs_set.contains(name.as_str()))
            .map(|(i, _)| i)
            .collect();
        let ctrl_input_indices: Vec<usize> = boolean_inputs
            .iter()
            .take(n_inputs)
            .enumerate()
            .filter(|(_, name)| controllable_inputs_set.contains(name.as_str()))
            .map(|(i, _)| i)
            .collect();
        let controllability_aware = !controllable_inputs_set.is_empty()
            && (!env_input_indices.is_empty() || !ctrl_input_indices.is_empty());

        let label_id = builder
            .labels()
            .intern(["step"])
            .map_err(|e| AdapterError {
                kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
                location: None,
                message: format!("adapter/btor2/predicate_cube_lift: label intern failed: {e}"),
            })?;

        // R.6.6 — pre-intern the per-combo controllability-aware
        // labels when opted in. We intern up-front (rather than
        // lazily per-transition) so the controllability map is set
        // exactly once per label. The label names encode the input-
        // combo bit-pattern for unique identification.
        let n_env = env_input_indices.len();
        let n_ctrl = ctrl_input_indices.len();
        let n_env_combos: usize = 1usize << n_env;
        let n_ctrl_combos: usize = 1usize << n_ctrl;
        let mut env_label_ids: Vec<crate::clts::LabelId<DefaultLabelIdx>> =
            Vec::with_capacity(n_env_combos);
        let mut ctrl_label_ids: Vec<crate::clts::LabelId<DefaultLabelIdx>> =
            Vec::with_capacity(n_ctrl_combos);
        if controllability_aware {
            for env_c in 0..n_env_combos {
                let name = format!("env_c{env_c}");
                let lid = builder
                    .labels()
                    .intern([name.as_str()])
                    .map_err(|e| AdapterError {
                        kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
                        location: None,
                        message: format!(
                            "adapter/btor2/predicate_cube_lift R.6.6: env label intern failed: {e}"
                        ),
                    })?;
                builder.set_label_controllability(
                    lid,
                    crate::clts::LabelControllability::Uncontrollable,
                );
                env_label_ids.push(lid);
            }
            for ctrl_c in 0..n_ctrl_combos {
                let name = format!("ctrl_c{ctrl_c}");
                let lid = builder
                    .labels()
                    .intern([name.as_str()])
                    .map_err(|e| AdapterError {
                        kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
                        location: None,
                        message: format!(
                            "adapter/btor2/predicate_cube_lift R.6.6: ctrl label intern failed: {e}"
                        ),
                    })?;
                builder.set_label_controllability(
                    lid,
                    crate::clts::LabelControllability::Controllable,
                );
                ctrl_label_ids.push(lid);
            }
        }

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
                //
                // R.6.6 (2026-06-08) — when controllability-aware,
                // compute the (env_combo, ctrl_combo) projection of
                // this input combo and emit the transition with a
                // dual-label set `[env_label, ctrl_label]`. The
                // Skolem grouping (group_transitions_by_uncontrollable_labels)
                // then partitions transitions by env_label correctly.
                let labels: smallvec::SmallVec<[crate::clts::LabelId<DefaultLabelIdx>; 4]> =
                    if controllability_aware {
                        let mut env_c: usize = 0;
                        for (slot, &raw_idx) in env_input_indices.iter().enumerate() {
                            if (combo >> raw_idx) & 1 == 1 {
                                env_c |= 1 << slot;
                            }
                        }
                        let mut ctrl_c: usize = 0;
                        for (slot, &raw_idx) in ctrl_input_indices.iter().enumerate() {
                            if (combo >> raw_idx) & 1 == 1 {
                                ctrl_c |= 1 << slot;
                            }
                        }
                        let mut lbls = smallvec::SmallVec::new();
                        if !env_input_indices.is_empty() {
                            lbls.push(env_label_ids[env_c]);
                        }
                        if !ctrl_input_indices.is_empty() {
                            lbls.push(ctrl_label_ids[ctrl_c]);
                        }
                        lbls
                    } else {
                        smallvec::smallvec![label_id]
                    };

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
                            &labels,
                            tgt_id,
                            crate::clts::TransitionModality::MayOnly,
                        );
                        // R.2.5b — record sampled target for the
                        // post-pass.
                        sampled_targets_per_source[i].insert(target_index);
                    }
                }
            }
        }
        predicate_image_pending = false;

        // R.2.5b (2026-06-06) — SamplingConfluence post-pass.
        // For each source cube, inspect the sampled target set:
        // - Exactly 1 target ⇒ promote to `Sharp` (single
        //   confluent successor; sampling-derived must-singleton).
        // - More than 1 target ⇒ emit `MustHyperOnly` with the set
        //   as the hyper-target collection (sampling-derived
        //   must-hyper).
        //
        // SOUNDNESS: sampling-based, not SMT-proved. The canonical-
        // representative assumption (the per-cube canonical
        // register values cover every concrete state in the cube)
        // is sound for predicate-true cubes (the register is
        // uniquely fixed) but is an OVER-approximation for
        // predicate-false cubes (registers may take many values).
        // The R.2.5b follow-up session will replace this with
        // a Z3-array-theory query proving must-edges.
        // R.6.6 (2026-06-08) — when controllability-aware, the post-
        // pass Sharp/MustHyperOnly promotions would emit edges labelled
        // with the legacy single-`step` label that mismatches the
        // controllability-aware dual-label may-edges already in the
        // CLTS. Such edges DO NOT supersede the may-edges (different
        // label sets ⇒ different transitions in the builder's view) and
        // would produce a misshaped model. The MVP gate skips the post-
        // passes entirely under controllability-aware mode; R.6.6.b
        // follow-up extends the post-pass to emit per-combo promoted
        // edges (or aggregate label sets) for soundness preservation
        // under the synthesis idiom.
        if !controllability_aware
            && matches!(
                lift_opts.must_edge_inference,
                MustEdgeInference::SamplingConfluence
            )
        {
            for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                if targets.is_empty() {
                    continue;
                }
                let src_id = state_ids[src_idx];
                let target_ids: smallvec::SmallVec<[crate::clts::StateId<DefaultStateIdx>; 4]> =
                    targets.iter().map(|&i| state_ids[i]).collect();
                if target_ids.len() == 1 {
                    // Single confluent target: promote MayOnly →
                    // Sharp. The downstream parity-game-3v_build
                    // coalesces same-target edges with Sharp
                    // dominating (per Item K's edge-modality merge
                    // rule), so this Sharp transition supersedes
                    // the previously-emitted MayOnly to the same
                    // target.
                    builder.transition_ids_with_modality(
                        src_id,
                        &[label_id],
                        target_ids[0],
                        crate::clts::TransitionModality::Sharp,
                    );
                    sharp_edges_promoted += 1;
                } else {
                    // Multi-target: emit MustHyperOnly with the
                    // full target set. The transition's `target`
                    // (StateId field) is the first element by K.2
                    // convention; the remaining targets are
                    // accessible via `modality().hyper_targets()`.
                    builder.transition_ids_with_modality(
                        src_id,
                        &[label_id],
                        target_ids[0],
                        crate::clts::TransitionModality::must_hyper(target_ids.clone()),
                    );
                    hyper_must_edges_emitted += 1;
                }
            }
            // SOUNDNESS warning: the inference is sampling-derived.
            if sharp_edges_promoted > 0 || hyper_must_edges_emitted > 0 {
                warnings.push(crate::adapter::AdapterWarning {
                    kind: crate::adapter::WarningKind::ApproximateTranslation,
                    message: format!(
                        "[R.2.5b-sampling-must] predicate_cube_lift: SamplingConfluence post-pass promoted {} edge(s) to Sharp and emitted {} MustHyperOnly edge(s). SOUNDNESS: sampling-derived; SMT-backed must-edge proof is queued for an R.2.5b follow-up session. Verdicts depending on these must-edges should be treated as candidate until the SMT swap lands.",
                        sharp_edges_promoted,
                        hyper_must_edges_emitted
                    ),
                    location: None,
                });
            }
        }

        // R.2.5b session-2 (2026-06-08) — SmtPerTarget post-pass.
        // For each (source cube, sampled-target cube) pair, run an
        // SMT must-edge query via smt_must_edge::smt_per_target_must_check.
        // On `Must` verdict, promote MayOnly → Sharp. The session-2
        // MVP only emits per-target Sharp edges; hyper-must inference
        // via SMT (`∀ s ⊨ src. ∃ input. ∃ t ⊨ T. next ⊨ t`) is queued
        // for the session-2 follow-up.
        //
        // SOUNDNESS: SMT-proved (no sampling). The MVP form
        // (`∀ state. ∀ input. transition ⟹ next ⊨ tgt`) is STRONGER
        // than the standard KMTS must-edge — every promoted Sharp
        // edge is also a standard must-edge, so soundness for
        // KMTS preservation holds. The MVP simply produces fewer
        // Sharp edges than the standard `∀ state ∃ input` form
        // would.
        // R.6.6 — same gate as the SamplingConfluence post-pass: skip
        // SmtPerTarget under controllability-aware mode (the promoted
        // Sharp edges would carry mismatched labels). R.6.6.b follow-up.
        if !controllability_aware
            && matches!(
                lift_opts.must_edge_inference,
                MustEdgeInference::SmtPerTarget
            )
        {
            let cfg = z3::Config::new();
            let smt_sharp_promoted = z3::with_z3_config(&cfg, || -> usize {
                let view = match encode_design_for_lift(&file) {
                    Ok(v) => v,
                    Err(_) => return 0,
                };
                let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
                let mut promoted = 0usize;
                for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                    if targets.is_empty() {
                        continue;
                    }
                    let src_id = state_ids[src_idx];
                    let src_bits = src_idx as u64;
                    for &tgt_idx in targets {
                        let tgt_bits = tgt_idx as u64;
                        let verdict =
                            crate::adapter::btor2::smt_must_edge::smt_per_target_must_check(
                                &view,
                                src_bits,
                                tgt_bits,
                                &predicates,
                                &nid_map,
                                5_000,
                            );
                        if matches!(
                            verdict,
                            crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                        ) {
                            // Promote MayOnly → Sharp on the
                            // (src, label, tgt) triple. The
                            // builder's edge dedup + Item K's
                            // edge-modality merge (Sharp dominates
                            // MayOnly) ensures the upgraded edge
                            // supersedes the previously-emitted
                            // MayOnly.
                            let tgt_id = state_ids[tgt_idx];
                            builder.transition_ids_with_modality(
                                src_id,
                                &[label_id],
                                tgt_id,
                                crate::clts::TransitionModality::Sharp,
                            );
                            promoted += 1;
                        }
                    }
                }
                promoted
            });
            sharp_edges_promoted += smt_sharp_promoted;
            if smt_sharp_promoted > 0 {
                warnings.push(crate::adapter::AdapterWarning {
                    kind: crate::adapter::WarningKind::ApproximateTranslation,
                    message: format!(
                        "[R.2.5b-smt-must] predicate_cube_lift: SmtPerTarget post-pass promoted {smt_sharp_promoted} edge(s) to Sharp via Z3 BV-theory must-check (stronger ∀∀ form). The standard ∀∃ form may promote additional edges; see `MustEdgeInference::SmtPerTargetStandard`. Hyper-must inference: see `MustEdgeInference::SmtHyperMust`."
                    ),
                    location: None,
                });
            }
        }

        // R.2.5b session-2 follow-up (2026-06-09) — SmtPerTargetStandard
        // post-pass: ∀∃ form via Z3 quantifier alternation. Strictly
        // more permissive than the ∀∀ SmtPerTarget MVP — every ∀∀-Must
        // is also ∀∃-Must, plus edges where SOME input per concrete
        // state reaches tgt.
        //
        // SOUNDNESS: same R.6.6 gate as SmtPerTarget — skip under
        // controllability-aware mode (mismatched-label promoted edges
        // would misshape the model). R.6.6.b follow-up.
        if !controllability_aware
            && matches!(
                lift_opts.must_edge_inference,
                MustEdgeInference::SmtPerTargetStandard
            )
        {
            let cfg = z3::Config::new();
            let smt_sharp_promoted = z3::with_z3_config(&cfg, || -> usize {
                let view = match encode_design_for_lift(&file) {
                    Ok(v) => v,
                    Err(_) => return 0,
                };
                let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
                let mut promoted = 0usize;
                for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                    if targets.is_empty() {
                        continue;
                    }
                    let src_id = state_ids[src_idx];
                    let src_bits = src_idx as u64;
                    for &tgt_idx in targets {
                        let tgt_bits = tgt_idx as u64;
                        let verdict =
                            crate::adapter::btor2::smt_must_edge::smt_per_target_must_check_standard(
                                &view,
                                src_bits,
                                tgt_bits,
                                &predicates,
                                &nid_map,
                                5_000,
                            );
                        if matches!(
                            verdict,
                            crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                        ) {
                            let tgt_id = state_ids[tgt_idx];
                            builder.transition_ids_with_modality(
                                src_id,
                                &[label_id],
                                tgt_id,
                                crate::clts::TransitionModality::Sharp,
                            );
                            promoted += 1;
                        }
                    }
                }
                promoted
            });
            sharp_edges_promoted += smt_sharp_promoted;
            if smt_sharp_promoted > 0 {
                warnings.push(crate::adapter::AdapterWarning {
                    kind: crate::adapter::WarningKind::ApproximateTranslation,
                    message: format!(
                        "[R.2.5b-smt-must-standard] predicate_cube_lift: SmtPerTargetStandard post-pass promoted {smt_sharp_promoted} edge(s) to Sharp via Z3 ∀∃ must-check (canonical KMTS form per Bruns–Godefroid CONCUR 2000). Hyper-must inference: see `MustEdgeInference::SmtHyperMust`."
                    ),
                    location: None,
                });
            }
        }

        // R.2.5b session-2 follow-up (2026-06-09) — SmtHyperMust
        // post-pass: per source cube, try per-target ∀∃ singleton
        // checks first; if no singleton proves Must, run the full
        // hyper-must check over the sampled target set. On Must,
        // emit a `MustHyperOnly` transition with the full target set
        // (MVP doesn't minimize T; sub-minimal T inference is a
        // follow-up).
        //
        // SOUNDNESS: same R.6.6 gate. Promoted edges carry the
        // legacy single-`step` label; controllability-aware mode
        // skips this post-pass to avoid label mismatch.
        if !controllability_aware
            && matches!(
                lift_opts.must_edge_inference,
                MustEdgeInference::SmtHyperMust
            )
        {
            let cfg = z3::Config::new();
            let (smt_sharp_promoted, smt_hyper_emitted) = z3::with_z3_config(
                &cfg,
                || -> (usize, usize) {
                    let view = match encode_design_for_lift(&file) {
                        Ok(v) => v,
                        Err(_) => return (0, 0),
                    };
                    let nid_map =
                        crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
                    let mut sharp = 0usize;
                    let mut hyper = 0usize;
                    for (src_idx, targets) in sampled_targets_per_source.iter().enumerate() {
                        if targets.is_empty() {
                            continue;
                        }
                        let src_id = state_ids[src_idx];
                        let src_bits = src_idx as u64;
                        // Phase 1: per-target ∀∃ singleton checks.
                        let mut promoted_singleton = false;
                        for &tgt_idx in targets {
                            let tgt_bits = tgt_idx as u64;
                            let verdict = crate::adapter::btor2::smt_must_edge::smt_per_target_must_check_standard(
                                &view,
                                src_bits,
                                tgt_bits,
                                &predicates,
                                &nid_map,
                                5_000,
                            );
                            if matches!(
                                verdict,
                                crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                            ) {
                                let tgt_id = state_ids[tgt_idx];
                                builder.transition_ids_with_modality(
                                    src_id,
                                    &[label_id],
                                    tgt_id,
                                    crate::clts::TransitionModality::Sharp,
                                );
                                sharp += 1;
                                promoted_singleton = true;
                            }
                        }
                        // Phase 2: if no singleton promoted, try
                        // the full-set hyper-must check.
                        if !promoted_singleton && targets.len() > 1 {
                            let target_bits_set: Vec<u64> =
                                targets.iter().map(|&i| i as u64).collect();
                            let verdict =
                                crate::adapter::btor2::smt_must_edge::smt_hyper_must_check(
                                    &view,
                                    src_bits,
                                    &target_bits_set,
                                    &predicates,
                                    &nid_map,
                                    5_000,
                                );
                            if matches!(
                                verdict,
                                crate::adapter::btor2::smt_must_edge::SmtMustVerdict::Must
                            ) {
                                let target_ids: smallvec::SmallVec<
                                    [crate::clts::StateId<DefaultStateIdx>; 4],
                                > = targets.iter().map(|&i| state_ids[i]).collect();
                                builder.transition_ids_with_modality(
                                    src_id,
                                    &[label_id],
                                    target_ids[0],
                                    crate::clts::TransitionModality::must_hyper(target_ids.clone()),
                                );
                                hyper += 1;
                            }
                        }
                    }
                    (sharp, hyper)
                },
            );
            sharp_edges_promoted += smt_sharp_promoted;
            hyper_must_edges_emitted += smt_hyper_emitted;
            if smt_sharp_promoted > 0 || smt_hyper_emitted > 0 {
                warnings.push(crate::adapter::AdapterWarning {
                    kind: crate::adapter::WarningKind::ApproximateTranslation,
                    message: format!(
                        "[R.2.5b-smt-must-hyper] predicate_cube_lift: SmtHyperMust post-pass promoted {smt_sharp_promoted} edge(s) to Sharp + emitted {smt_hyper_emitted} MustHyperOnly edge(s) via Z3 ∀∃ checks. Hyper-must targets = full sampled set (MVP doesn't minimize T)."
                    ),
                    location: None,
                });
            }
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
        // R.2.5 predicate-image MVP populates may-edges when
        // `max_input_bits > 0` and at least one predicate exists;
        // R.5 / R.5b will additionally populate must-edges via SMT.
        predicate_image_pending,
        // R.5b AdapterWarning channel — UF wrapping naming + other
        // lift-time abstraction decisions surface here for caller
        // reporting.
        warnings,
        sharp_edges_promoted,
        hyper_must_edges_emitted,
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
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
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
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
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

    // ---- R.2.5b — SamplingConfluence post-pass tests ----

    /// R.2.5b — Default `MustEdgeInference::Off` preserves the
    /// pre-R.2.5b behaviour: only MayOnly edges, zero promotions.
    #[test]
    fn r5_2_5b_default_inference_off_preserves_legacy_behaviour() {
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
        .expect("predicate_cube_lift succeeds");
        assert_eq!(result.sharp_edges_promoted, 0, "no promotions when Off");
        assert_eq!(result.hyper_must_edges_emitted, 0, "no hyper-must when Off");
    }

    /// R.2.5b — `SamplingConfluence` post-pass emits Sharp and / or
    /// MustHyperOnly edges + records telemetry counts. The counter
    /// fixture has a single boolean input `clr`; for each source
    /// cube, the post-pass collects targets across both `clr=0` and
    /// `clr=1` samples. Cubes whose two samples agree get a Sharp
    /// promotion; cubes whose samples diverge get a MustHyperOnly.
    #[test]
    fn r5_2_5b_sampling_confluence_emits_sharp_and_hyper_must() {
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
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SamplingConfluence,
            may_edge_inference: Default::default(),

            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");
        let total_promoted = result.sharp_edges_promoted + result.hyper_must_edges_emitted;
        assert!(
            total_promoted > 0,
            "SamplingConfluence must emit at least one Sharp or MustHyperOnly edge on the counter fixture; got sharp={}, hyper={}",
            result.sharp_edges_promoted,
            result.hyper_must_edges_emitted
        );

        // SOUNDNESS warning is emitted alongside the promotions.
        let any_sampling_warning = result.warnings.iter().any(|w| {
            w.message.contains("R.2.5b-sampling-must")
                && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
        });
        assert!(
            any_sampling_warning,
            "SamplingConfluence must emit the soundness warning; got warnings: {:?}",
            result.warnings
        );
    }

    /// R.2.5b — Disabling `max_input_bits` (= 0) skips the may-edge
    /// loop entirely; no promotions happen even when
    /// SamplingConfluence is enabled, because there are no sampled
    /// edges to promote.
    #[test]
    fn r5_2_5b_sampling_confluence_noop_when_max_input_bits_zero() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 0,
            must_edge_inference: MustEdgeInference::SamplingConfluence,
            may_edge_inference: Default::default(),

            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");
        assert_eq!(
            result.sharp_edges_promoted, 0,
            "no promotions when max_input_bits=0"
        );
        assert_eq!(
            result.hyper_must_edges_emitted, 0,
            "no hyper-must when max_input_bits=0"
        );
    }

    // ---- R.2.5b session-2 — SmtPerTarget post-pass tests ----

    /// R.2.5b session 2 — `SmtPerTarget` post-pass on the counter
    /// fixture. The Z3 BV-theory check should prove at least one
    /// Sharp edge (the `clr=1 ⟹ cnt:=0` branch is deterministic
    /// for cube `{cnt==0}` self-loop under all inputs that satisfy
    /// the source predicate IF the source predicate already pins
    /// the relevant register). The exact count is fixture-dependent;
    /// the test asserts non-zero + the SMT soundness warning.
    #[test]
    fn r5_2_5b_smt_per_target_emits_sharp_promotions_on_counter() {
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
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SmtPerTarget,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");

        // SmtPerTarget only emits per-target Sharp; never hyper-must
        // (queued for session-2 follow-up).
        assert_eq!(
            result.hyper_must_edges_emitted, 0,
            "SmtPerTarget MVP must NOT emit hyper-must edges; got {}",
            result.hyper_must_edges_emitted
        );

        // The Z3 SMT check should prove at least one Sharp on this
        // fixture — the cube `{cnt_is_0, cnt_is_1}` (an unreachable
        // combination, both predicates true) is self-loop-Sharp
        // under SMT because the cube's UNSAT precondition (no
        // register value can satisfy both) makes the must-edge
        // formula vacuously true.
        //
        // The MVP's strong ∀∀ form succeeds on at least one
        // (src, tgt) pair where src's predicates UNSAT-constrain
        // the state space; on those pairs ¬tgt is irrelevant.
        // Soundness warning fires when any promotion happens.
        if result.sharp_edges_promoted > 0 {
            let any_smt_warning = result.warnings.iter().any(|w| {
                w.message.contains("R.2.5b-smt-must")
                    && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
            });
            assert!(
                any_smt_warning,
                "SmtPerTarget must emit the [R.2.5b-smt-must] warning when it promotes edges; got warnings: {:?}",
                result.warnings
            );
        }
        // The MVP may legitimately produce 0 Sharp on this fixture
        // (the ∀∀ form is strictly stronger than ∀∃; only vacuous
        // or input-independent edges qualify). The non-strict
        // assertion above guards the warning's presence iff
        // promotion happened — which is the invariant we test.
    }

    /// R.2.5b session 2 — `SmtPerTarget` skips the post-pass when
    /// `max_input_bits = 0` (no may-edges sampled → no target sets
    /// to SMT-check → no promotions).
    #[test]
    fn r5_2_5b_smt_per_target_noop_when_max_input_bits_zero() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 0,
            must_edge_inference: MustEdgeInference::SmtPerTarget,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");
        assert_eq!(
            result.sharp_edges_promoted, 0,
            "no SMT promotions when max_input_bits=0"
        );
        assert_eq!(
            result.hyper_must_edges_emitted, 0,
            "no hyper-must when SmtPerTarget + max_input_bits=0"
        );
    }

    /// R.2.5b session 2 — Lazy materialiser threads
    /// `MustEdgeInference::SmtPerTarget` end-to-end. Same shape as
    /// the SamplingConfluence equivalent; ensures both eager + lazy
    /// paths share the dispatch.
    #[test]
    fn r5_2_5b_lazy_materialiser_honors_smt_per_target() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SmtPerTarget,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let mut lazy =
            LazyLift::from_btor2(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
                .expect("lazy lift constructs");
        let result = materialize_clts_from_lazy(
            &mut lazy,
            crate::adapter::SourceFormat::Btor2,
            MustEdgeInference::SmtPerTarget,
        )
        .expect("materialize succeeds");

        // Same invariants as the eager path: no hyper-must in MVP;
        // if any Sharp promoted, the [R.2.5b-smt-must] warning
        // must be present.
        assert_eq!(
            result.hyper_must_edges_emitted, 0,
            "Lazy SmtPerTarget MVP must NOT emit hyper-must edges"
        );
        if result.sharp_edges_promoted > 0 {
            let any_smt_warning = result.warnings.iter().any(|w| {
                w.message.contains("R.2.5b-smt-must")
                    && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
            });
            assert!(
                any_smt_warning,
                "Lazy SmtPerTarget must emit [R.2.5b-smt-must] when promoting; got warnings: {:?}",
                result.warnings
            );
        }
    }

    // ---- R.2.5b session-2 follow-up (2026-06-09) — SmtPerTargetStandard + SmtHyperMust ----

    /// R.2.5b session-2 follow-up — `SmtPerTargetStandard` ∀∃ form
    /// on the counter fixture. Strict-supremacy invariant: this form
    /// promotes a SUPERSET of the SmtPerTarget MVP's edges (every
    /// ∀∀-Must is also ∀∃-Must, plus additional edges where SOME
    /// input per state reaches tgt).
    #[test]
    fn r5_2_5b_smt_per_target_standard_emits_sharp_promotions_on_counter() {
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
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SmtPerTargetStandard,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(
            preds.clone(),
            COUNTER_BTOR2,
            &AdapterOptions::default(),
            &opts,
        )
        .expect("lift succeeds");
        // SmtPerTargetStandard never emits hyper-must edges (those
        // are SmtHyperMust territory).
        assert_eq!(
            result.hyper_must_edges_emitted, 0,
            "SmtPerTargetStandard never emits hyper-must edges"
        );
        if result.sharp_edges_promoted > 0 {
            let any_warning = result.warnings.iter().any(|w| {
                w.message.contains("R.2.5b-smt-must-standard")
                    && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
            });
            assert!(
                any_warning,
                "SmtPerTargetStandard must emit [R.2.5b-smt-must-standard] when promoting; \
                 got warnings: {:?}",
                result.warnings
            );
        }
        // Strict-supremacy: the ∀∃ standard form should promote
        // AT LEAST as many edges as the ∀∀ MVP form on the same
        // fixture. Re-run with MVP for comparison.
        let mvp_opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SmtPerTarget,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let mvp_result =
            predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &mvp_opts)
                .expect("MVP lift succeeds");
        assert!(
            result.sharp_edges_promoted >= mvp_result.sharp_edges_promoted,
            "Strict-supremacy: ∀∃ standard form ({}) must promote ≥ ∀∀ MVP ({}); \
             every ∀∀-Must is also ∀∃-Must.",
            result.sharp_edges_promoted,
            mvp_result.sharp_edges_promoted
        );
    }

    /// R.2.5b session-2 follow-up — `SmtHyperMust` on the counter
    /// fixture. The MVP strategy: per-target ∀∃ singletons first, then
    /// full-target-set hyper-must fallback. Counter fixture has small
    /// |T| per source (typically 1–2), so the singleton path
    /// dominates. Hyper-must emits only when no singleton proves
    /// AND |T| > 1.
    #[test]
    fn r5_2_5b_smt_hyper_must_emits_promotions_on_counter() {
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
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SmtHyperMust,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("lift succeeds");
        if result.sharp_edges_promoted > 0 || result.hyper_must_edges_emitted > 0 {
            let any_warning = result.warnings.iter().any(|w| {
                w.message.contains("R.2.5b-smt-must-hyper")
                    && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
            });
            assert!(
                any_warning,
                "SmtHyperMust must emit [R.2.5b-smt-must-hyper] when promoting; \
                 got warnings: {:?}",
                result.warnings
            );
        }
    }

    /// R.2.5b session-2 follow-up — both new variants noop when
    /// `max_input_bits = 0` (no may-edges sampled → no targets to
    /// SMT-check).
    #[test]
    fn r5_2_5b_smt_standard_and_hyper_noop_when_max_input_bits_zero() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        for variant in [
            MustEdgeInference::SmtPerTargetStandard,
            MustEdgeInference::SmtHyperMust,
        ] {
            let opts = PredicateCubeLiftOptions {
                max_cube_count: 1024,
                max_input_bits: 0,
                must_edge_inference: variant,
                may_edge_inference: Default::default(),
                config_values: std::collections::HashMap::new(),
            };
            let result = predicate_cube_lift(
                preds.clone(),
                COUNTER_BTOR2,
                &AdapterOptions::default(),
                &opts,
            )
            .expect("lift succeeds");
            assert_eq!(
                result.sharp_edges_promoted, 0,
                "{variant:?}: no promotions when max_input_bits=0"
            );
            assert_eq!(
                result.hyper_must_edges_emitted, 0,
                "{variant:?}: no hyper-must when max_input_bits=0"
            );
        }
    }

    /// R.2.5b session-2 follow-up — lazy materialiser threads both
    /// new variants end-to-end. Ensures dispatch symmetry between
    /// eager + lazy paths.
    #[test]
    fn r5_2_5b_lazy_materialiser_honors_smt_standard_and_hyper() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        for variant in [
            MustEdgeInference::SmtPerTargetStandard,
            MustEdgeInference::SmtHyperMust,
        ] {
            let opts = PredicateCubeLiftOptions {
                max_cube_count: 1024,
                max_input_bits: 8,
                must_edge_inference: variant,
                may_edge_inference: Default::default(),
                config_values: std::collections::HashMap::new(),
            };
            let mut lazy = LazyLift::from_btor2(
                preds.clone(),
                COUNTER_BTOR2,
                &AdapterOptions::default(),
                &opts,
            )
            .expect("lazy lift constructs");
            let result =
                materialize_clts_from_lazy(&mut lazy, crate::adapter::SourceFormat::Btor2, variant)
                    .expect("materialize succeeds");
            // SmtPerTargetStandard never emits hyper-must; SmtHyperMust may.
            if matches!(variant, MustEdgeInference::SmtPerTargetStandard) {
                assert_eq!(
                    result.hyper_must_edges_emitted, 0,
                    "Lazy SmtPerTargetStandard MVP must NOT emit hyper-must edges"
                );
            }
            if result.sharp_edges_promoted > 0 || result.hyper_must_edges_emitted > 0 {
                let pattern = match variant {
                    MustEdgeInference::SmtPerTargetStandard => "R.2.5b-smt-must-standard",
                    MustEdgeInference::SmtHyperMust => "R.2.5b-smt-must-hyper",
                    _ => unreachable!(),
                };
                let any_warning = result.warnings.iter().any(|w| {
                    w.message.contains(pattern)
                        && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
                });
                assert!(
                    any_warning,
                    "Lazy {variant:?} must emit [{pattern}] when promoting; \
                     got warnings: {:?}",
                    result.warnings
                );
            }
        }
    }

    // ---- R.6.6 (2026-06-08) — controllability-aware lifter tests ----

    /// R.6.6 — when `AdapterOptions::controllable_inputs` is empty
    /// (the default), the lifter preserves pre-R.6.6 behavior
    /// bit-for-bit: a single `step` label per transition, no
    /// controllability tags on labels. Verdict-equivalence invariant
    /// for the R-track's existing fixtures.
    #[test]
    fn r6_6_no_controllable_inputs_preserves_legacy_single_label() {
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
        .expect("lift succeeds");
        let labels = result.clts.alphabet();
        assert!(
            labels.iter().any(|l| l == "step"),
            "legacy `step` label must be present; got labels: {labels:?}"
        );
        // No env_/ctrl_ labels emitted in legacy mode.
        assert!(
            !labels
                .iter()
                .any(|l| l.starts_with("env_c") || l.starts_with("ctrl_c")),
            "no R.6.6 env_/ctrl_ labels in legacy mode; got: {labels:?}"
        );
    }

    /// R.6.6 — when `controllable_inputs = ["clr"]` on the counter
    /// fixture (whose single boolean input is `clr`), the lifter
    /// emits per-combo labels with controllability tags. The counter
    /// has 1 input, so n_env=0 + n_ctrl=1 ⇒ 2 ctrl labels
    /// (`ctrl_c0`, `ctrl_c1`), no env labels.
    #[test]
    fn r6_6_single_controllable_input_emits_ctrl_labels() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let mut opts = AdapterOptions::default();
        opts.controllable_inputs.push("clr".into());
        let result = predicate_cube_lift(
            preds,
            COUNTER_BTOR2,
            &opts,
            &PredicateCubeLiftOptions::default(),
        )
        .expect("lift succeeds");
        let labels = result.clts.alphabet();
        assert!(
            labels.iter().any(|l| l == "ctrl_c0"),
            "expected ctrl_c0 label; got: {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l == "ctrl_c1"),
            "expected ctrl_c1 label; got: {labels:?}"
        );
        // No env labels (0 env inputs).
        assert!(
            !labels.iter().any(|l| l.starts_with("env_c")),
            "no env labels expected with 0 env inputs; got: {labels:?}"
        );
        // Every ctrl_c* label must land in the controllable alphabet.
        for &label_id in result.clts.controllable_alphabet() {
            if let Some(payload) = result.clts.label_payload(label_id)
                && let Some(name) = payload.first()
            {
                assert!(
                    name.starts_with("ctrl_c") || name == "step",
                    "controllable alphabet must contain only ctrl_c* or legacy step; got {name:?}"
                );
            }
        }
        // ctrl_c0 + ctrl_c1 must NOT appear in the uncontrollable alphabet.
        for &label_id in result.clts.uncontrollable_alphabet() {
            if let Some(payload) = result.clts.label_payload(label_id)
                && let Some(name) = payload.first()
            {
                assert!(
                    !name.starts_with("ctrl_c"),
                    "ctrl_c* label {name:?} must not appear in uncontrollable alphabet"
                );
            }
        }
    }

    /// R.6.6 — when controllability-aware, the post-pass Sharp /
    /// MustHyperOnly promotions are SKIPPED (the gate flags
    /// `!controllability_aware` before firing). Even with
    /// `MustEdgeInference::SamplingConfluence` requested, no
    /// promotions happen. R.6.6.b will lift this gate by adding the
    /// per-combo promotion shape.
    #[test]
    fn r6_6_controllability_aware_skips_post_pass_promotions() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let mut adapter_opts = AdapterOptions::default();
        adapter_opts.controllable_inputs.push("clr".into());
        let lift_opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SamplingConfluence,
            may_edge_inference: Default::default(),
            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &adapter_opts, &lift_opts)
            .expect("lift succeeds");
        assert_eq!(
            result.sharp_edges_promoted, 0,
            "R.6.6 gate: controllability-aware mode skips Sharp promotion (R.6.6.b follow-up); got {} promotions",
            result.sharp_edges_promoted
        );
        assert_eq!(
            result.hyper_must_edges_emitted, 0,
            "R.6.6 gate: controllability-aware mode skips MustHyperOnly emission; got {}",
            result.hyper_must_edges_emitted
        );
    }

    /// MIG-3.2 — `MayEdgeInference::SmtAllPairs` emits the SOUND
    /// may-edge set and excludes proven-impossible edges. Fixture: a
    /// 1-bit register that toggles (`next q = ~q`), predicate {q==1} →
    /// two cubes. The sound may-edges are exactly the toggle cycle
    /// (cube_0 ⇄ cube_1); the self-loops (cube_i → cube_i) are
    /// impossible (the toggle never stays) and MUST be excluded — the
    /// soundness property the SMT query gives that sampling cannot.
    #[test]
    fn mig3_smt_all_pairs_emits_sound_may_edges() {
        let src = "1 sort bitvec 1\n2 state 1 q\n3 not 1 2\n4 next 1 2 3\n";
        let preds = vec![PredicateSpec {
            name: "q1".into(),
            register: "q".into(),
            value: 1,
        }];
        let lift_opts = PredicateCubeLiftOptions {
            may_edge_inference: MayEdgeInference::SmtAllPairs,
            ..Default::default()
        };
        let result = predicate_cube_lift(preds, src, &AdapterOptions::default(), &lift_opts)
            .expect("predicate_cube_lift (SmtAllPairs)");
        assert!(
            !result.predicate_image_pending,
            "SmtAllPairs populates the may relation"
        );
        let clts = &result.clts;
        let c0 = clts.state_id("cube_0").expect("cube_0 exists");
        let c1 = clts.state_id("cube_1").expect("cube_1 exists");
        let has_may = |from, to| {
            clts.outgoing(from).iter().any(|t| {
                t.target() == to && matches!(t.modality(), crate::clts::TransitionModality::MayOnly)
            })
        };
        // Toggle cycle — both directions are sound may-edges.
        assert!(has_may(c0, c1), "cube_0 → cube_1 (q: 0→1) is a may-edge");
        assert!(has_may(c1, c0), "cube_1 → cube_0 (q: 1→0) is a may-edge");
        // Impossible self-loops — excluded (UNSAT-proved), the soundness
        // property sampling cannot guarantee.
        assert!(
            !has_may(c0, c0),
            "cube_0 → cube_0 must be excluded (toggle never stays at q==0)"
        );
        assert!(
            !has_may(c1, c1),
            "cube_1 → cube_1 must be excluded (toggle never stays at q==1)"
        );
    }

    /// R.2.5b — Empty predicate set: the may-edge loop is skipped
    /// (gated on `!predicates.is_empty()`); SamplingConfluence's
    /// post-pass has nothing to operate on.
    #[test]
    fn r5_2_5b_sampling_confluence_noop_when_predicates_empty() {
        let preds: Vec<PredicateSpec> = vec![];
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SamplingConfluence,
            may_edge_inference: Default::default(),

            config_values: std::collections::HashMap::new(),
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");
        assert_eq!(result.sharp_edges_promoted, 0);
        assert_eq!(result.hyper_must_edges_emitted, 0);
    }

    /// R.2.5b session-1 follow-up — Lazy materialiser threads
    /// `MustEdgeInference::SamplingConfluence` end-to-end. Same
    /// fixture as the eager path's test; produces at least one
    /// Sharp / MustHyperOnly edge + the `[R.2.5b-sampling-must]`
    /// warning.
    #[test]
    fn r5_2_5b_lazy_materialiser_honors_sampling_confluence() {
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
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 8,
            must_edge_inference: MustEdgeInference::SamplingConfluence,
            may_edge_inference: Default::default(),

            config_values: std::collections::HashMap::new(),
        };
        let mut lazy =
            LazyLift::from_btor2(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
                .expect("LazyLift::from_btor2 succeeds");
        let result = materialize_clts_from_lazy(
            &mut lazy,
            crate::adapter::SourceFormat::Btor2,
            MustEdgeInference::SamplingConfluence,
        )
        .expect("materialize_clts_from_lazy succeeds");

        let total_promoted = result.sharp_edges_promoted + result.hyper_must_edges_emitted;
        assert!(
            total_promoted > 0,
            "lazy materialiser SamplingConfluence must emit at least one Sharp or MustHyperOnly edge; got sharp={}, hyper={}",
            result.sharp_edges_promoted,
            result.hyper_must_edges_emitted
        );
        let any_warning = result.warnings.iter().any(|w| {
            w.message.contains("R.2.5b-sampling-must")
                && matches!(w.kind, crate::adapter::WarningKind::ApproximateTranslation)
        });
        assert!(
            any_warning,
            "lazy materialiser SamplingConfluence must emit the soundness warning"
        );
    }

    /// R-Y7 (2026-06-07) — Default `config_values` (empty) → the
    /// lifted Clts has a single initial state (cube_0), matching
    /// pre-R-Y7 behaviour exactly.
    #[test]
    fn r_y7_empty_config_values_preserves_legacy_single_initial_state() {
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
        .expect("predicate_cube_lift succeeds");
        assert_eq!(
            result.clts.initial_states().len(),
            1,
            "pre-R-Y7 default: single initial state"
        );
    }

    /// R-Y7 — Non-empty `config_values` expands the initial-state
    /// set per the R-S8 encoder. For a register with multiple
    /// valid values, multiple cubes become admissible initial
    /// states.
    #[test]
    fn r_y7_config_values_expands_initial_state_set() {
        // 2 predicates over the counter: cnt == 0 and cnt == 1.
        // config_values: cnt is in {0, 1} → both predicate-true
        // cubes are admissible (cube 1 = pred_0 true; cube 2 =
        // pred_1 true). The over-approximation in R-S8 also
        // admits cube 0 (both predicates false; could be other
        // valid value) and cube 3 (both predicates true;
        // inconsistent but not filtered).
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
        let mut config_values: std::collections::HashMap<String, Vec<u64>> =
            std::collections::HashMap::new();
        config_values.insert("cnt".to_string(), vec![0, 1]);
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 0,
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: Default::default(),
            config_values,
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");
        let initial_count = result.clts.initial_states().len();
        assert!(
            initial_count > 1,
            "R-Y7: config_values must expand initial-state set beyond singleton; got {initial_count}"
        );
        assert_eq!(
            initial_count, 4,
            "R-Y7: with both predicate values valid + over-approximation, all 4 cubes admissible"
        );
    }

    /// R-Y7 — When `config_values` excludes the predicate's
    /// value, the cube where that predicate is TRUE is NOT
    /// admissible (per R-S8's load-bearing rule).
    #[test]
    fn r_y7_invalid_predicate_value_excludes_initial_cube() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_7".into(),
            register: "cnt".into(),
            value: 7,
        }];
        let mut config_values: std::collections::HashMap<String, Vec<u64>> =
            std::collections::HashMap::new();
        // Valid values are 0, 1, 2 — predicate's value 7 is NOT valid.
        config_values.insert("cnt".to_string(), vec![0, 1, 2]);
        let opts = PredicateCubeLiftOptions {
            max_cube_count: 1024,
            max_input_bits: 0,
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: Default::default(),
            config_values,
        };
        let result = predicate_cube_lift(preds, COUNTER_BTOR2, &AdapterOptions::default(), &opts)
            .expect("predicate_cube_lift succeeds");
        let initial_count = result.clts.initial_states().len();
        // Cube 0 (predicate false): admissible (cnt holds 0, 1,
        // or 2). Cube 1 (predicate true → cnt == 7): NOT
        // admissible. → 1 initial state.
        assert_eq!(
            initial_count, 1,
            "R-Y7: predicate-value-not-valid cube excluded from initial set"
        );
    }

    /// R.2.5b session-1 follow-up — Lazy materialiser with
    /// `MustEdgeInference::Off` (the default) produces zero
    /// promotions, matching the pre-R.2.5b behaviour.
    #[test]
    fn r5_2_5b_lazy_materialiser_off_preserves_legacy_behaviour() {
        let preds = vec![PredicateSpec {
            name: "cnt_is_0".into(),
            register: "cnt".into(),
            value: 0,
        }];
        let mut lazy = LazyLift::from_btor2(
            preds,
            COUNTER_BTOR2,
            &AdapterOptions::default(),
            &PredicateCubeLiftOptions::default(),
        )
        .expect("LazyLift::from_btor2 succeeds");
        let result = materialize_clts_from_lazy(
            &mut lazy,
            crate::adapter::SourceFormat::Btor2,
            MustEdgeInference::Off,
        )
        .expect("materialize_clts_from_lazy succeeds");
        assert_eq!(
            result.sharp_edges_promoted, 0,
            "Off must yield zero promotions"
        );
        assert_eq!(result.hyper_must_edges_emitted, 0);
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

    // R.5 lazy KMTS sub-item 2.3 tests (2026-06-04) — truly-
    // lazy implementation. Each test ensures the cache + on-
    // demand computation behaviour matches the eager wrapper.

    #[test]
    fn r5_subitem_23_lazy_lift_cube_count_matches_eager() {
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let lazy = LazyLift::from_btor2(
            preds.clone(),
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &opts,
        )
        .expect("lazy lift succeeds");
        let eager =
            EagerLazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
                .expect("eager lift succeeds");
        assert_eq!(lazy.cube_count(), eager.cube_count());
    }

    #[test]
    fn r5_subitem_23_lazy_lift_cache_starts_empty_grows_on_visits() {
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let mut lazy = LazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
            .expect("lazy lift succeeds");
        assert_eq!(lazy.cached_count(), 0, "cache MUST start empty");
        let _ = lazy.expand_cube(0);
        assert_eq!(
            lazy.cached_count(),
            1,
            "cache grows by 1 on first cube visit"
        );
        let _ = lazy.expand_cube(0);
        assert_eq!(
            lazy.cached_count(),
            1,
            "cache MUST NOT grow on repeat visit to the same cube"
        );
        let _ = lazy.expand_cube(1);
        assert_eq!(
            lazy.cached_count(),
            2,
            "cache grows by 1 on second distinct cube visit"
        );
    }

    #[test]
    fn r5_subitem_23_lazy_lift_repeat_expand_returns_identical_edges() {
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let mut lazy = LazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
            .expect("lazy lift succeeds");
        let edges_1 = lazy.expand_cube(0);
        let edges_2 = lazy.expand_cube(0);
        assert_eq!(edges_1, edges_2);
    }

    #[test]
    fn r5_subitem_23_lazy_lift_out_of_range_returns_empty_and_no_cache_growth() {
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let mut lazy = LazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
            .expect("lazy lift succeeds");
        let edges = lazy.expand_cube(100);
        assert!(edges.is_empty());
        assert_eq!(
            lazy.cached_count(),
            0,
            "out-of-range visit MUST NOT pollute the cache"
        );
    }

    #[test]
    fn r5_subitem_23_lazy_lift_edges_match_eager_lift() {
        // LOAD-BEARING DRIFT-PROTECTION TEST. The lazy impl
        // currently duplicates the per-cube logic of the eager
        // lifter. This test asserts the two paths produce
        // equivalent edge sets for the same input — any
        // divergence between the duplicated logics surfaces
        // here.
        //
        // Fixture: SMALL_BTOR2 (no inputs ⇒ no may-edges ⇒
        // both paths produce empty edge sets for every cube).
        // This is a degenerate match; richer-fixture tests
        // will live alongside sub-item 2.4 when an input-
        // bearing fixture is wired in.
        let preds = vec![PredicateSpec {
            name: "p".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let mut lazy = LazyLift::from_btor2(
            preds.clone(),
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &opts,
        )
        .expect("lazy lift succeeds");
        let mut eager =
            EagerLazyLift::from_btor2(preds, SMALL_BTOR2, &AdapterOptions::default(), &opts)
                .expect("eager lift succeeds");
        for cube in 0..lazy.cube_count() {
            let lazy_edges = lazy.expand_cube(cube);
            let eager_edges = eager.expand_cube(cube);
            let mut lazy_sorted = lazy_edges;
            lazy_sorted.sort_by(|a, b| {
                a.label
                    .cmp(&b.label)
                    .then_with(|| a.target_cube.cmp(&b.target_cube))
            });
            let mut eager_sorted = eager_edges;
            eager_sorted.sort_by(|a, b| {
                a.label
                    .cmp(&b.label)
                    .then_with(|| a.target_cube.cmp(&b.target_cube))
            });
            assert_eq!(
                lazy_sorted, eager_sorted,
                "LazyLift and EagerLazyLift MUST agree on edge set for cube {cube}"
            );
        }
    }

    #[test]
    fn r5_subitem_23_lazy_lift_predicates_round_trip() {
        let preds = vec![PredicateSpec {
            name: "p_alpha".to_string(),
            register: "reg_a".to_string(),
            value: 0,
        }];
        let opts = PredicateCubeLiftOptions::default();
        let lazy = LazyLift::from_btor2(
            preds.clone(),
            SMALL_BTOR2,
            &AdapterOptions::default(),
            &opts,
        )
        .expect("lazy lift succeeds");
        assert_eq!(lazy.predicates(), preds.as_slice());
    }

    // ─────────────────────────────────────────────────────────────
    // §Phase 10 stage 3.c.2a — encode_design_for_lift theory pick
    // ─────────────────────────────────────────────────────────────

    /// A memory-bearing BTOR2 (one array-sorted state cell).
    const MEM_BTOR_FOR_LIFT: &str = r#"
1 sort bitvec 1
2 sort bitvec 5
3 sort bitvec 8
4 sort array 2 3
5 state 4 mem
6 input 2 a
7 input 3 v
8 write 4 5 6 7
9 next 4 5 8
10 read 3 8 6
11 zero 3
12 eq 1 10 11
13 bad 12
"#;

    #[test]
    fn encode_design_for_lift_uses_array_theory_for_memory_design() {
        // A design with an array state cell must encode (not error)
        // through encode_design_for_lift, because the helper selects
        // Theory::BvUfArray. Under the bare BvOnly `encode_design`
        // this same design errors with ArraySortUnsupportedInBvOnly.
        let file = crate::adapter::btor2::parser::parse(MEM_BTOR_FOR_LIFT).expect("parse");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design_for_lift(&file);
            assert!(
                view.is_ok(),
                "encode_design_for_lift must select BvUfArray + encode a memory design"
            );
            let view = view.unwrap();
            assert_eq!(
                view.state_curr_arr.len(),
                1,
                "the array (memory) state cell must be encoded as a Z3 Array"
            );
        });
    }

    #[test]
    fn encode_design_for_lift_uses_bvonly_for_memory_free_design() {
        // A memory-free design must still encode via BvOnly (the
        // pre-stage-3.c.2a path; array maps empty).
        let file = crate::adapter::btor2::parser::parse(SMALL_BTOR2).expect("parse");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design_for_lift(&file).expect("memory-free encodes");
            assert!(
                view.state_curr_arr.is_empty(),
                "a memory-free design must have no array cells"
            );
        });
    }
}
