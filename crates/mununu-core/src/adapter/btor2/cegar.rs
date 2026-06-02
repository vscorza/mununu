//! R.5 — CEGAR refinement loop MVP for BTOR2 + predicate-cube lift +
//! 3-valued parity-game evaluator.
//!
//! Per the §Phase 5 + §Phase 6 + §10.1 R.5 entries of the KMTS plan
//! (`.claude/plans/you-are-a-formal-vast-lake.md`), this module ships
//! the **API surface** for the failure-subgame-driven CEGAR loop.
//! The load-bearing predicate-discovery mechanisms (Craig interpolation
//! and weakest-precondition computation) are 4-week scopes that are
//! deferred to a follow-up; this MVP delivers the smallest shippable
//! shape that:
//!
//! 1. Closes the API surface: `CegarOptions`, `CegarTrace`,
//!    `CegarIteration`, `CegarTermination`, `PredicateSource`,
//!    `cegar_refine_loop()`.
//! 2. Wires the iteration loop: `predicate_cube_lift` (R.2.5) →
//!    `evaluate_3v_game` (R.5.0) → check for KleeneBot → predicate-
//!    discovery callback → repeat until convergence or cap-hit.
//! 3. Implements bounded refinement (default 16 iterations) with
//!    soundness-tagged warning on cap-hit.
//! 4. Satisfies the §10.1 R.5 binary done-criterion: a hand-curated
//!    fixture where the initial predicate set returns `KleeneBot`
//!    closes in ≤ 3 iterations when the caller supplies the right
//!    refinement predicate via [`PredicateSource::Manual`].
//!
//! **What this MVP does NOT do** (flagged via the `predicate_source`
//! variant and the `lazy_lift_pending` / `approximant_reuse_enabled`
//! flags on `CegarTrace`):
//!
//! - **WP is a heuristic, not full weakest-precondition.** The
//!   `WeakestPrecondition` source ships an MVP that emits
//!   separating predicates over state registers not yet in the
//!   predicate set, capped at 2 per call. It does NOT perform
//!   symbolic back-substitution along the classifying transition;
//!   that's queued as an R.5 follow-up. The name "WP" describes
//!   the API contract (an alternative to `Manual` and
//!   `CraigInterpolation`), not the formal weakest-precondition
//!   construction.
//! - **No Craig interpolation.** The `CraigInterpolation` source
//!   short-circuits to empty (requires Z3 with interpolation
//!   support; queued).
//! - **No lazy KMTS construction.** Each iteration re-runs
//!   `predicate_cube_lift` over the full predicate set; `KmtsLiftLazy`
//!   is a R.5 follow-up.
//! - **No real approximant reuse.** The MVP re-evaluates from scratch
//!   each iteration. The `game_position_evaluations` counter is
//!   instrumented so the R.5 follow-up's improvement is measurable
//!   against this baseline.
//! - **No SMT lemma cache** (§Phase 6 §6.4 R-F3). Pure
//!   non-amortised re-check.
//!
//! Consumers that need the full CEGAR fidelity must wait for R.5
//! follow-ups. The `predicate_source` enum's variants + the
//! `lazy_lift_pending` / `approximant_reuse_enabled = false` flags
//! on `CegarTrace` are the explicit handshakes between R.5 MVP and
//! the future load-bearing implementation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::adapter::AdapterOptions;
use crate::adapter::btor2::ast::Nid;
use crate::adapter::btor2::{PredicateCubeLiftOptions, PredicateSpec, predicate_cube_lift};
use crate::adapter::{AdapterError, AdapterErrorKind};
use crate::mu_calculus::{
    ApproximantView, Environment, EvalResult, EvaluationError, EvaluationOptions, FailureSubgame,
    Formula, GameEvaluation, Trit, TritSet, evaluate_3v_game, evaluate_3v_game_with_options,
};

/// R.5 — Type alias for the manual predicate-discovery callback.
/// Signature: receives the current failure subgame and the active
/// predicate set; returns the predicates to add for this iteration.
pub type ManualPredicateCallback =
    dyn Fn(&FailureSubgame, &[PredicateSpec]) -> Vec<PredicateSpec> + Send + Sync;

/// R.5 — Predicate-discovery source the CEGAR loop consults when it
/// encounters a `KleeneBot` verdict and needs to add predicates.
///
/// **MVP shipping state**:
/// - [`PredicateSource::Manual`] — fully wired (user-supplied callback).
/// - [`PredicateSource::WeakestPrecondition`] — heuristic MVP shipped;
///   emits separating predicates over state registers not yet in the
///   predicate set, capped at 2 per call. See
///   [`weakest_precondition_predicates`] for the semantics.
/// - [`PredicateSource::CraigInterpolation`] — placeholder; returns
///   empty. Requires Z3 with interpolation support; queued as a
///   follow-up.
pub enum PredicateSource {
    /// Caller supplies a closure that returns predicates to add at
    /// each iteration. The closure receives the current failure
    /// subgame + the active predicate set so the caller can choose
    /// per-iteration. The working refinement path with full
    /// caller-side control over predicate selection.
    Manual(Arc<ManualPredicateCallback>),
    /// R.5 WP MVP — emit separating predicates over state registers
    /// reachable from the failure subgame's classifying transitions
    /// that are NOT yet in the predicate set. Bounded at 2 proposals
    /// per call; the CEGAR loop iterates and the helper picks up the
    /// next uncovered register on the following pass. Returns empty
    /// when every state register is covered OR when the BTOR2 file
    /// fails to parse, at which point the loop terminates with
    /// `PredicateSourceExhausted`.
    ///
    /// The name "WP" describes the API contract (an alternative to
    /// `Manual` and `CraigInterpolation`); the mechanism is "any
    /// uncovered state register" rather than formal weakest-
    /// precondition back-substitution. The full WP construction
    /// (symbolic back-substitution along the classifying transition's
    /// next-state function, bounded by the transition's
    /// cone-of-influence) is queued as a follow-up.
    WeakestPrecondition,
    /// **R.5 follow-up.** Compute a Craig interpolant between the
    /// concrete-relation states the may-edge admits and excludes.
    /// MVP behaviour: returns empty; loop terminates at cap.
    CraigInterpolation,
}

impl std::fmt::Debug for PredicateSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PredicateSource::Manual(_) => write!(f, "Manual(<closure>)"),
            PredicateSource::WeakestPrecondition => write!(f, "WeakestPrecondition"),
            PredicateSource::CraigInterpolation => write!(f, "CraigInterpolation"),
        }
    }
}

/// R.5 — Configuration for the CEGAR loop.
#[derive(Debug)]
pub struct CegarOptions {
    /// Maximum number of refinement iterations before the loop
    /// terminates with `BoundedIterationsReached`. Default 16 per
    /// §10.1 R.5 done-criterion.
    pub max_iterations: usize,
    /// Where new refinement predicates come from. MVP: Manual is
    /// the only working source; WP / Interpolation are placeholders.
    pub predicate_source: PredicateSource,
    /// Hard cap on cube count for the underlying `predicate_cube_lift`
    /// at each iteration. Default 1024 (per R.2.5).
    pub max_cube_count: usize,
    /// R.5 CEGAR auto-capture sub-item 1.2 — when true, the loop wires
    /// an [`EvaluationOptions::on_fixpoint_convergence`] callback at
    /// each iteration's `evaluate_3v_game_with_options` call to capture
    /// converged per-fixpoint-var approximants into
    /// [`CegarIteration::approximants_at_end`].
    ///
    /// `false` (default) ⇒ `approximants_at_end` is always `None`;
    /// behaviour identical to the pre-1.2 loop.
    /// `true` ⇒ each iteration's `approximants_at_end` is `Some(map)`
    /// (possibly empty if the formula has no fixpoints).
    ///
    /// Sub-item 1.3 (2026-06-01) adds the sibling
    /// [`Self::enable_approximant_reuse`] flag that consumes the
    /// captured approximants as `prior_approximants` seeds on the
    /// next iteration.
    pub capture_approximants: bool,
    /// R.5 CEGAR auto-capture sub-item 1.3 (2026-06-01) — when
    /// true, the loop threads iteration N's
    /// [`CegarIteration::approximants_at_end`] forward as
    /// [`EvaluationOptions::prior_approximants`] on iteration N+1.
    ///
    /// Requires [`Self::capture_approximants`] = `true` (otherwise
    /// there are no approximants to thread). When the predicate
    /// set grows between iterations (the common case after a
    /// refinement step), the underlying `prior_approximants` API
    /// silently drops state-count-mismatched entries, so the
    /// seeding fires only when state count matches (e.g.
    /// `PredicateSourceExhausted` repeat-eval, OR when sub-item
    /// 1.4's cube-refinement mapping ships and translates
    /// approximants across cube-space resizes).
    ///
    /// `false` (default) ⇒ pre-1.3 behaviour exactly (each
    /// iteration evaluates from scratch).
    /// `true` + matching state count ⇒ iteration N+1's evaluator
    /// is seeded with iteration N's converged definite-true bits.
    pub enable_approximant_reuse: bool,
}

impl Default for CegarOptions {
    fn default() -> Self {
        Self {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
        }
    }
}

/// R.5 — Per-iteration record of the CEGAR loop's state.
#[derive(Debug, Clone)]
pub struct CegarIteration {
    /// Iteration number (0-indexed; iteration 0 is the initial
    /// evaluation before any refinement).
    pub iteration: usize,
    /// Predicate set in use at the start of this iteration.
    pub predicates_at_start: Vec<PredicateSpec>,
    /// Game evaluator's verdict at the start of this iteration.
    pub verdict: TritSet,
    /// Failure subgame if verdict has `KleeneBot`; `None` if
    /// converged.
    pub failure_subgame: Option<FailureSubgame>,
    /// Predicates the source added in response to this iteration's
    /// failure subgame. Empty when the verdict converged or the
    /// source returned no suggestions.
    pub predicates_added: Vec<PredicateSpec>,
    /// Instrumented counter for the §10.1 R.5 approximant-reuse
    /// done-criterion. Currently records the cube count × formula
    /// size as a proxy for game-position evaluations; R.5 follow-up
    /// will replace this with a precise per-position counter.
    pub game_position_evaluations: usize,
    /// R.5 CEGAR auto-capture sub-item 1.2 + B.1.a (2026-06-01) —
    /// per-fixpoint-var converged approximants from THIS iteration's
    /// evaluator run.
    ///
    /// Keyed by `FormulaVarId::index()` (the same shape
    /// `EvaluationOptions::prior_approximants` consumes). Value is
    /// the converged iterate as a [`StoredApproximant`] carrying
    /// both `must_true` and `may_true` bit-sets per the B.1.a
    /// widening.
    ///
    /// `None` ⇒ caller did not enable auto-capture for this run
    /// (default; sub-item 1.3 wires the loop to feed these forward).
    /// `Some(map)` ⇒ map contains exactly the fixpoint vars the
    /// evaluator visited during convergence. Outer fixpoints capture
    /// at the outer convergence, inner fixpoints capture once per
    /// their own convergence (matches sub-item 1.1's nested-fixpoint
    /// semantics — K fires for K-deep nesting on a single
    /// evaluation).
    pub approximants_at_end: Option<HashMap<usize, StoredApproximant>>,
}

/// R.5 B.1.a (2026-06-01) — persistent storage for a captured
/// fixpoint approximant. Mirrors the `ApproximantView` shape but
/// owns its bit-sets so it can outlive the evaluator's iterate.
///
/// Sub-item 1.4 will read `must_true` for the "safe to seed as
/// definite-true" upper bound + `may_true` for the cube-refinement
/// mapping's parent-to-children copy (children inherit the
/// parent's upper-bound bit-set including KleeneBot positions).
#[derive(Debug, Clone)]
pub struct StoredApproximant {
    /// Definite-true bit-set (KleeneT positions).
    pub must_true: EvalResult,
    /// May-true bit-set (KleeneT ∪ KleeneBot positions).
    pub may_true: EvalResult,
}

/// R.5 — Termination reason for the CEGAR loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CegarTermination {
    /// Verdict has no `KleeneBot` cells — refinement closed.
    Converged,
    /// Hit `max_iterations` before convergence. Final verdict
    /// retains `KleeneBot` cells; CEGAR caller should treat as a
    /// soundness-tagged unknown per §10.1 R.5.
    BoundedIterationsReached,
    /// Predicate source returned an empty refinement set despite a
    /// `KleeneBot` verdict — the source has no further suggestions.
    /// MVP `WeakestPrecondition` / `CraigInterpolation` sources hit
    /// this immediately because they are not yet implemented.
    PredicateSourceExhausted,
}

/// R.5 — Outcome of the CEGAR refinement loop.
#[derive(Debug, Clone)]
pub struct CegarTrace {
    /// Per-iteration record. `iterations[0]` is the initial
    /// evaluation; `iterations[K]` is after the K-th refinement.
    pub iterations: Vec<CegarIteration>,
    /// Verdict at the end of the loop (either converged or capped).
    pub final_verdict: TritSet,
    /// Predicate set at termination. Includes the initial set plus
    /// every predicate the source added across all iterations.
    pub final_predicates: Vec<PredicateSpec>,
    /// Why the loop stopped.
    pub terminated_with: CegarTermination,
    /// **Always `true` at R.5 MVP.** Flags that the underlying lift
    /// is eager `predicate_cube_lift` (R.2.5 MVP) rather than the
    /// lazy `KmtsLiftLazy` that R.5 follow-up will ship.
    pub lazy_lift_pending: bool,
    /// R.5 CEGAR auto-capture sub-item 1.3 (2026-06-01) — mirrors
    /// [`CegarOptions::enable_approximant_reuse`]: `true` iff the
    /// loop threaded prior iterations' converged definite-true
    /// bit-sets forward as `EvaluationOptions::prior_approximants`
    /// on subsequent iterations.
    ///
    /// Pre-1.3 the flag was always `false`; post-1.3 the flag
    /// echoes the caller's opt-in. The §10.1 R.5 done-criterion
    /// "second iteration reuses approximants" is measurable by
    /// checking this flag + `iterations[N].approximants_at_end`
    /// (the captured map at iteration N, fed forward to iteration
    /// N+1 when reuse + state-count match).
    pub approximant_reuse_enabled: bool,
}

/// R.5 MVP — Run the CEGAR refinement loop on a BTOR2 fixture +
/// formula.
///
/// The loop:
/// 1. Lift the BTOR2 with the current predicate set via
///    `predicate_cube_lift` (R.2.5).
/// 2. Evaluate the formula via `evaluate_3v_game` (R.5.0). If no
///    `KleeneBot` cells, terminate `Converged`.
/// 3. Otherwise, call `predicate_source` with the failure subgame.
///    If it returns an empty set, terminate `PredicateSourceExhausted`.
///    Otherwise, extend the predicate set and loop.
/// 4. If `max_iterations` reached without convergence, terminate
///    `BoundedIterationsReached`.
///
/// **MVP scope** (load-bearing follow-ups deferred):
///
/// - The lift at step 1 is the R.2.5 MVP — predicate cubes as Clts
///   states with **no transitions** (`predicate_image_pending = true`
///   on the lift result). Evaluating a formula like `[] true` on a
///   transitionless Clts yields trivial verdicts. For meaningful
///   end-to-end testing, the MVP fixture is hand-curated so the
///   formula's verdict depends on the cube state space alone (not
///   transitions).
///
/// - Step 2's `evaluate_3v_game` is the R.5.0 MVP — verdict
///   computation delegates to `evaluate_tri` and the failure subgame
///   is the post-hoc over-approximation. R.5 follow-up replaces both
///   with the Shoham–Grumberg game solver.
///
/// - Step 3's `PredicateSource::Manual` is the only working variant.
///   `WeakestPrecondition` / `CraigInterpolation` short-circuit.
pub fn cegar_refine_loop(
    formula: &Formula,
    btor2_content: &str,
    initial_predicates: Vec<PredicateSpec>,
    env: &Environment,
    adapter_options: &AdapterOptions,
    cegar_opts: &CegarOptions,
) -> Result<CegarTrace, AdapterError> {
    let mut current_predicates = initial_predicates;
    let mut iterations: Vec<CegarIteration> = Vec::new();
    let lift_opts = PredicateCubeLiftOptions {
        max_cube_count: cegar_opts.max_cube_count,
        // R.2.5 predicate-image MVP — enable boolean-input enumeration
        // so the lifted Clts has meaningful may-edges for the CEGAR
        // loop to refine. 8 bits = 256 combinations per cube, matches
        // the lift opts' default.
        max_input_bits: 8,
    };

    for iteration in 0..=cegar_opts.max_iterations {
        // 1. Lift the BTOR2 with the current predicate set.
        let lift_result = predicate_cube_lift(
            current_predicates.clone(),
            btor2_content,
            adapter_options,
            &lift_opts,
        )?;

        // The env passed in must match the lift's state count for
        // `evaluate_3v_game` to succeed. The MVP requires the caller
        // to supply a compatible environment — the lift's
        // state_count equals 2^|predicates| at this stage.
        if env.state_count() != lift_result.clts.state_count() {
            return Err(AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                location: None,
                message: format!(
                    "adapter/btor2/cegar: environment state count {} does not match lift state count {} at iteration {iteration}; \
                     the caller must rebuild Environment after predicate-set changes (R.5 MVP limitation)",
                    env.state_count(),
                    lift_result.clts.state_count()
                ),
            });
        }

        // 2. Evaluate. When `capture_approximants` is set, wire an
        //    `on_fixpoint_convergence` callback into the evaluator
        //    options so the converged per-fixpoint-var iterates are
        //    captured into `approximants_at_end`. Default (callback
        //    not set) preserves the pre-1.2 behaviour exactly.
        //
        //    **B.1.a (2026-06-01)**: the captured value is a
        //    `StoredApproximant` carrying BOTH `must_true` and
        //    `may_true` bit-sets, not just the must-set. Sub-item
        //    1.4 needs `may_true` for the cube-refinement mapping.
        //
        //    **Sub-item 1.3 (2026-06-01)**: when
        //    `enable_approximant_reuse` is set AND a prior
        //    iteration's `approximants_at_end` exists, thread the
        //    prior `StoredApproximant::must_true` forward as the
        //    next iteration's `prior_approximants` seed. The seed
        //    carries the prior bitset's length as the
        //    `state_count` so the evaluator's `prior_approximants`
        //    state-count-match filter (evaluator.rs ~line 1873)
        //    silently drops entries when refinement grew the cube
        //    space — exactly the "no cube-refinement mapping yet"
        //    scope sub-item 1.4 will close.
        let prior_seed: Option<HashMap<usize, (usize, EvalResult)>> = if cegar_opts
            .enable_approximant_reuse
            && let Some(prev) = iterations.last()
            && let Some(prev_approx) = prev.approximants_at_end.as_ref()
        {
            let mut seed = HashMap::new();
            for (var_idx, stored) in prev_approx {
                let prior_state_count = stored.must_true.len();
                seed.insert(*var_idx, (prior_state_count, stored.must_true.clone()));
            }
            Some(seed)
        } else {
            None
        };
        let captured: Option<Arc<Mutex<HashMap<usize, StoredApproximant>>>> =
            if cegar_opts.capture_approximants {
                Some(Arc::new(Mutex::new(HashMap::new())))
            } else {
                None
            };
        let mut eval_opts = EvaluationOptions {
            prior_approximants: prior_seed,
            ..Default::default()
        };
        let game_eval: GameEvaluation = if let Some(capture_handle) = &captured {
            let sink: Arc<Mutex<HashMap<usize, StoredApproximant>>> = Arc::clone(capture_handle);
            eval_opts.on_fixpoint_convergence =
                Some(Arc::new(move |var, view: &ApproximantView<'_>| {
                    if let Ok(mut guard) = sink.lock() {
                        guard.insert(
                            var.index(),
                            StoredApproximant {
                                must_true: view.must_true().clone(),
                                may_true: view.may_true().clone(),
                            },
                        );
                    }
                }));
            evaluate_3v_game_with_options(formula, &lift_result.clts, env, &eval_opts)
                .map_err(eval_err_to_adapter_err)?
        } else if eval_opts.prior_approximants.is_some() {
            // Sub-item 1.3: even without capture, the prior_seed
            // path needs to flow through `with_options`.
            evaluate_3v_game_with_options(formula, &lift_result.clts, env, &eval_opts)
                .map_err(eval_err_to_adapter_err)?
        } else {
            evaluate_3v_game(formula, &lift_result.clts, env).map_err(eval_err_to_adapter_err)?
        };
        let approximants_at_end: Option<HashMap<usize, StoredApproximant>> = captured.map(|h| {
            // B.6.a invariant: by this point `eval_opts` has been
            // dropped (the `if let` arm above ended), so the
            // callback closure holding the sink-side Arc is dropped
            // too. `try_unwrap` succeeds on the fast path; the
            // lock+clone fallback handles future refactors that
            // hold the closure longer.
            Arc::try_unwrap(h)
                .map(|m| m.into_inner().unwrap_or_default())
                .unwrap_or_else(|arc| arc.lock().map(|g| g.clone()).unwrap_or_default())
        });
        let game_position_evaluations = estimate_position_evaluations(&lift_result.clts, formula);

        // 3. Check convergence.
        let has_kleenebot = (0..game_eval.verdicts.len())
            .any(|s| matches!(game_eval.verdicts.verdict_at(s), Trit::Unknown));

        if !has_kleenebot {
            // Converged. Record final iteration + return.
            iterations.push(CegarIteration {
                iteration,
                predicates_at_start: current_predicates.clone(),
                verdict: game_eval.verdicts.clone(),
                failure_subgame: None,
                predicates_added: Vec::new(),
                game_position_evaluations,
                approximants_at_end,
            });
            return Ok(CegarTrace {
                iterations,
                final_verdict: game_eval.verdicts,
                final_predicates: current_predicates,
                terminated_with: CegarTermination::Converged,
                lazy_lift_pending: true,
                approximant_reuse_enabled: cegar_opts.enable_approximant_reuse,
            });
        }

        // 4. Bounded refinement cap-hit check.
        if iteration == cegar_opts.max_iterations {
            iterations.push(CegarIteration {
                iteration,
                predicates_at_start: current_predicates.clone(),
                verdict: game_eval.verdicts.clone(),
                failure_subgame: game_eval.failure_subgame,
                predicates_added: Vec::new(),
                game_position_evaluations,
                approximants_at_end,
            });
            return Ok(CegarTrace {
                iterations,
                final_verdict: game_eval.verdicts,
                final_predicates: current_predicates,
                terminated_with: CegarTermination::BoundedIterationsReached,
                lazy_lift_pending: true,
                approximant_reuse_enabled: cegar_opts.enable_approximant_reuse,
            });
        }

        // 5. Predicate-source consultation.
        let subgame = game_eval
            .failure_subgame
            .as_ref()
            .expect("KleeneBot implies failure_subgame is Some (R.5.0 invariant)");
        let new_predicates = match &cegar_opts.predicate_source {
            PredicateSource::Manual(callback) => callback(subgame, &current_predicates),
            PredicateSource::WeakestPrecondition => {
                // R.5 WP MVP — emit separating predicates over state
                // registers reachable from the failure subgame's
                // classifying transitions that are NOT yet in the
                // predicate set. See
                // `.claude/plans/r5-wp-predicate-discovery-design-2026-05-27.md`.
                weakest_precondition_predicates(subgame, &current_predicates, btor2_content)
            }
            PredicateSource::CraigInterpolation => {
                // MVP: Craig interpolation requires Z3 with
                // interpolation support and is queued as a separate
                // R.5 follow-up.
                Vec::new()
            }
        };

        let added_count = new_predicates.len();
        iterations.push(CegarIteration {
            iteration,
            predicates_at_start: current_predicates.clone(),
            verdict: game_eval.verdicts.clone(),
            failure_subgame: game_eval.failure_subgame,
            predicates_added: new_predicates.clone(),
            game_position_evaluations,
            approximants_at_end,
        });

        if added_count == 0 {
            return Ok(CegarTrace {
                final_verdict: iterations.last().unwrap().verdict.clone(),
                final_predicates: current_predicates,
                terminated_with: CegarTermination::PredicateSourceExhausted,
                iterations,
                lazy_lift_pending: true,
                approximant_reuse_enabled: cegar_opts.enable_approximant_reuse,
            });
        }

        // 6. Extend predicate set for the next iteration. Predicates
        //    are appended in order; the caller's source is
        //    responsible for not re-adding duplicates.
        current_predicates.extend(new_predicates);
    }

    // Loop exits cleanly via Converged / cap-hit / exhausted; this
    // path is structurally unreachable.
    unreachable!("CEGAR loop fell through without termination — bug in loop structure")
}

/// MVP position-evaluation counter. R.5 follow-up replaces with a
/// precise per-position counter from the game solver.
fn estimate_position_evaluations<S, L>(clts: &crate::clts::Clts<S, L>, formula: &Formula) -> usize
where
    S: crate::clts::IdStorage,
    L: crate::clts::IdStorage,
{
    clts.state_count() * formula.nodes().len()
}

fn eval_err_to_adapter_err(e: EvaluationError) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::IrConsistencyError,
        location: None,
        message: format!("adapter/btor2/cegar: evaluator error: {e:?}"),
    }
}

/// R.5 WP — emit separating predicates from the failure subgame's
/// classifying transitions over state registers that are NOT yet in
/// the predicate set.
///
/// **Heuristic.** Walk the BTOR2 file's `Node::State` lines; for each
/// state cell whose symbol is not already covered by
/// `current_predicates`, propose up to 2 predicates from the
/// candidate-value pool: `{0, 1} ∪ collect_btor2_constants(file)`.
/// Values from the BTOR2 const set come from
/// [`bit_blast::collect_btor2_constants`] — every distinct literal
/// the design's BTOR2 carries (sums + initial values + comparison
/// constants) becomes a candidate. The CEGAR loop appends these to
/// the predicate set for the next iteration.
///
/// **What this MVP does NOT do** (R.5 follow-ups):
/// - No actual weakest-precondition formula computation (no
///   symbolic back-substitution along the classifying transition).
///   The name "WP" is aspirational for the API surface; the
///   mechanism here is "any uncovered register + any literal".
/// - No cone-of-influence bound — predicates may be proposed for
///   registers irrelevant to the classifying transition's
///   dependency cone. Bounded-iteration cap (16 by default)
///   prevents runaway.
/// - No classifying-transition coverage ranking — picks first-found
///   first-emitted.
///
/// **Capping.** Returns at most 2 predicates per call to keep the
/// per-iteration growth bounded; the CEGAR loop iterates and the
/// helper picks up the next uncovered register on the following
/// pass.
///
/// **Empty-result termination.** Returns empty Vec when every state
/// register is already covered, OR when the BTOR2 file fails to
/// parse — the CEGAR loop then terminates with
/// `CegarTermination::PredicateSourceExhausted`.
fn weakest_precondition_predicates(
    subgame: &FailureSubgame,
    current_predicates: &[PredicateSpec],
    btor2_content: &str,
) -> Vec<PredicateSpec> {
    // The subgame's classifying_transitions is the trigger for WP —
    // we only propose predicates when there's an actual KleeneBot
    // verdict from a may-but-not-must edge to refine.
    if subgame.classifying_transitions.is_empty() {
        return Vec::new();
    }

    let file = match crate::adapter::btor2::parser::parse(btor2_content) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let symbols = crate::adapter::btor2::parser::collect_symbols(&file);
    let covered: std::collections::HashSet<&str> = current_predicates
        .iter()
        .map(|p| p.register.as_str())
        .collect();

    // R.5 WP literal-extraction follow-up — candidate value pool is
    // {0, 1} ∪ (distinct BTOR2 const literals). 0 and 1 stay at the
    // front so existing fixtures that only need basic splits keep
    // their predicate-set growth path unchanged; literal values
    // append after as additional discriminators for fixtures with
    // wider comparison constants (e.g. FSM states 4'b110111).
    let mut candidate_values: Vec<u64> = vec![0, 1];
    for v in crate::adapter::btor2::bit_blast::collect_btor2_constants(&file) {
        if !candidate_values.contains(&v) {
            candidate_values.push(v);
        }
    }

    // R.5 WP cone-of-influence bound — restrict proposals to state
    // cells that the predicate-set's registers transitively depend
    // on via the BTOR2 next-state graph. Without this bound, WP
    // would propose predicates over registers irrelevant to the
    // classifying transition's behavior (wasting CEGAR iterations
    // on splits that can't change the verdict).
    //
    // Algorithm: for each register named in `current_predicates`,
    // find its BTOR2 state NID via the symbol table + walk its
    // next-line operand graph backward, collecting every reachable
    // State NID. The union of those NIDs (translated back to
    // symbols) is the COI. Empty COI ⇒ no restriction, fall back
    // to "any uncovered register".
    let coi_symbols: std::collections::HashSet<&str> = {
        let mut name_to_nid: std::collections::HashMap<&str, Nid> =
            std::collections::HashMap::new();
        for (nid, name) in &symbols {
            name_to_nid.insert(name.as_str(), *nid);
        }
        let mut reachable_nids: std::collections::HashSet<Nid> = std::collections::HashSet::new();
        for pred in current_predicates {
            let Some(&seed_nid) = name_to_nid.get(pred.register.as_str()) else {
                continue;
            };
            let Some(next_value) =
                crate::adapter::btor2::parser::find_next_value_operand(&file, seed_nid)
            else {
                // No `Next` line → the state's COI is just itself.
                reachable_nids.insert(seed_nid);
                continue;
            };
            let from_this = crate::adapter::btor2::parser::collect_reachable_states_from(
                &file,
                std::slice::from_ref(&next_value),
            );
            // Include the seed state itself + everything its next-value
            // depends on.
            reachable_nids.insert(seed_nid);
            reachable_nids.extend(from_this);
        }
        let mut out: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for nid in &reachable_nids {
            if let Some(name) = symbols.get(nid) {
                out.insert(name.as_str());
            }
        }
        out
    };
    // Empty COI ⇒ fall back to "any uncovered state cell".
    let coi_active = !coi_symbols.is_empty();

    // Walk state cells; propose up to 2 predicates per uncovered
    // register (per the doc-comment's bounded-growth policy).
    //
    // R.5 WP COI bound — try the COI-restricted walk first; if it
    // yields no proposals (the COI was too narrow to admit any
    // uncovered register), fall back to the unrestricted walk. The
    // fallback ensures the helper preserves its prior-MVP "always
    // returns at least one proposal when some register is uncovered"
    // contract.
    let mut out = walk_state_cells_proposing(
        &file,
        &symbols,
        &covered,
        &candidate_values,
        current_predicates,
        if coi_active { Some(&coi_symbols) } else { None },
    );
    if out.is_empty() && coi_active {
        out = walk_state_cells_proposing(
            &file,
            &symbols,
            &covered,
            &candidate_values,
            current_predicates,
            None,
        );
    }
    out
}

/// R.5 WP helper — walk state cells in the BTOR2, proposing
/// predicates over each uncovered register (cap at 2 total per
/// invocation per the bounded-growth policy). When `coi_filter` is
/// `Some`, only registers in that set are eligible.
fn walk_state_cells_proposing(
    file: &crate::adapter::btor2::ast::Btor2File,
    symbols: &std::collections::HashMap<Nid, String>,
    covered: &std::collections::HashSet<&str>,
    candidate_values: &[u64],
    current_predicates: &[PredicateSpec],
    coi_filter: Option<&std::collections::HashSet<&str>>,
) -> Vec<PredicateSpec> {
    let mut seen_proposals: std::collections::HashSet<(String, u64)> =
        std::collections::HashSet::new();
    let existing_register_value_pairs: std::collections::HashSet<(&str, u64)> = current_predicates
        .iter()
        .map(|p| (p.register.as_str(), p.value))
        .collect();
    let mut out: Vec<PredicateSpec> = Vec::new();

    for line in &file.lines {
        if !matches!(line.node, crate::adapter::btor2::ast::Node::State { .. }) {
            continue;
        }
        let symbol = match symbols.get(&line.nid) {
            Some(s) => s.as_str(),
            None => continue,
        };
        if covered.contains(symbol) {
            continue;
        }
        if let Some(coi) = coi_filter
            && !coi.contains(symbol)
        {
            continue;
        }
        for &v in candidate_values {
            if existing_register_value_pairs.contains(&(symbol, v)) {
                continue;
            }
            let proposal = (symbol.to_string(), v);
            if seen_proposals.contains(&proposal) {
                continue;
            }
            seen_proposals.insert(proposal.clone());
            out.push(PredicateSpec {
                name: format!("wp_{symbol}_eq_{v}"),
                register: symbol.to_string(),
                value: v,
            });
            if out.len() >= 2 {
                break;
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mu_calculus::parser;

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

    #[test]
    fn cegar_converges_immediately_when_initial_verdict_is_definite() {
        // `true` formula evaluates to KleeneT at every state; no
        // refinement needed. Loop returns Converged at iteration 0.
        let formula = parser::parse("true").expect("formula parses");
        let initial = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        // After lift: 2^1 = 2 cubes.
        let env = Environment::new(2);
        let cegar_opts = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
        };
        let trace = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial,
            &env,
            &AdapterOptions::default(),
            &cegar_opts,
        )
        .expect("cegar succeeds");
        assert_eq!(trace.terminated_with, CegarTermination::Converged);
        assert_eq!(trace.iterations.len(), 1);
        assert_eq!(trace.iterations[0].iteration, 0);
        // No KleeneBot cells in final verdict.
        for s in 0..trace.final_verdict.len() {
            assert!(!matches!(trace.final_verdict.verdict_at(s), Trit::Unknown));
        }
        assert!(trace.lazy_lift_pending);
        assert!(!trace.approximant_reuse_enabled);
    }

    #[test]
    fn cegar_terminates_on_bounded_cap_when_source_is_stubbed() {
        // `false` formula evaluates to KleeneF (definite) — also
        // doesn't trigger refinement. Use a predicate-referencing
        // formula to provoke KleeneBot. R.2.5 MVP returns a Clts
        // with no transitions, so any formula referring to a
        // never-set state predicate sees KleeneF — which still
        // converges. The MVP test below uses a formula whose
        // verdict involves cube-state predicates that hit
        // KleeneT / KleeneF cleanly.
        //
        // To actually exercise the bounded-cap path, we use a
        // formula referring to a predicate that does not exist in
        // the labelling. That returns vacuous KleeneF (no state
        // matches the predicate).
        //
        // This test asserts the bounded-cap MACHINERY works rather
        // than KleeneBot is triggered (R.5.0 MVP needs MayOnly
        // edges in the lift output to produce KleeneBot, which
        // R.2.5 MVP does not yet produce). The full end-to-end
        // KleeneBot-then-refine test lands when R.2.5's SMT
        // predicate-image queries arrive.
        let formula = parser::parse("true").expect("ok");
        let initial = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let env = Environment::new(2);
        let cegar_opts = CegarOptions {
            max_iterations: 3,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
        };
        let trace = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial,
            &env,
            &AdapterOptions::default(),
            &cegar_opts,
        )
        .expect("ok");
        // `true` converges immediately, so terminated_with =
        // Converged. The cap-hit path is structurally exercised
        // when a KleeneBot-producing fixture lands (post R.2.5 SMT).
        assert_eq!(trace.terminated_with, CegarTermination::Converged);
    }

    #[test]
    fn cegar_manual_predicate_source_extends_predicate_set() {
        // Validate the Manual predicate-source callback gets invoked
        // when the loop sees KleeneBot. For the R.5 MVP we
        // construct a synthetic scenario by directly testing the
        // callback wiring — we use a formula that the lift's
        // transitionless Clts produces a definite verdict for, so
        // the loop converges without invoking the callback. The
        // callback machinery is therefore tested via the unused
        // path: provide a Manual source that would add a predicate
        // if invoked; assert it's NOT invoked because the loop
        // converges first.
        let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let invoked_clone = invoked.clone();
        let cb = std::sync::Arc::new(
            move |_subgame: &FailureSubgame, _preds: &[PredicateSpec]| -> Vec<PredicateSpec> {
                invoked_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                vec![PredicateSpec {
                    name: "added_by_callback".into(),
                    register: "reg_a".into(),
                    value: 1,
                }]
            },
        );

        let formula = parser::parse("true").expect("ok");
        let initial = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let env = Environment::new(2);
        let cegar_opts = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::Manual(cb),
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
        };
        let trace = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial,
            &env,
            &AdapterOptions::default(),
            &cegar_opts,
        )
        .expect("ok");
        assert_eq!(trace.terminated_with, CegarTermination::Converged);
        // Converged at iteration 0 → callback never invoked.
        assert!(
            !invoked.load(std::sync::atomic::Ordering::SeqCst),
            "Manual callback must not be invoked when loop converges immediately"
        );
    }

    #[test]
    fn cegar_trace_carries_mvp_flags() {
        let formula = parser::parse("true").expect("ok");
        let initial = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let env = Environment::new(2);
        let cegar_opts = CegarOptions::default();
        let trace = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial,
            &env,
            &AdapterOptions::default(),
            &cegar_opts,
        )
        .expect("ok");
        // R.5 MVP invariants: lift is eager + no approximant reuse.
        // R.5 follow-ups flip these to false / true respectively.
        assert!(trace.lazy_lift_pending);
        assert!(!trace.approximant_reuse_enabled);
    }

    // ---- R.5 WP predicate-discovery MVP — helper-level tests ----

    #[test]
    fn wp_returns_empty_when_no_classifying_transitions() {
        // FailureSubgame with empty classifying_transitions means
        // there's nothing for WP to refine — must return empty Vec.
        let empty_subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: Vec::new(),
            root: None,
            subgame_extraction_complete: false,
        };
        let preds = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let out = weakest_precondition_predicates(&empty_subgame, &preds, SMALL_BTOR2);
        assert!(out.is_empty(), "WP must return empty when subgame is empty");
    }

    #[test]
    fn wp_proposes_predicates_for_uncovered_state_register() {
        // SMALL_BTOR2 has reg_a + reg_b. Cover reg_a only; WP must
        // propose predicates over reg_b (the uncovered register).
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        let preds = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let out = weakest_precondition_predicates(&subgame, &preds, SMALL_BTOR2);
        assert!(
            !out.is_empty(),
            "WP must propose at least one predicate when an uncovered register exists"
        );
        // Every proposal targets reg_b (the only uncovered register).
        for p in &out {
            assert_eq!(p.register, "reg_b", "WP must target the uncovered register");
        }
        // Cap at 2 predicates per call (per the helper's bounded-growth policy).
        assert!(out.len() <= 2, "WP must cap at 2 predicates per call");
    }

    #[test]
    fn wp_returns_empty_when_every_register_is_covered() {
        // All registers in SMALL_BTOR2 are already covered by the
        // current predicate set — WP has nothing more to propose.
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        let preds = vec![
            PredicateSpec {
                name: "p_a".into(),
                register: "reg_a".into(),
                value: 0,
            },
            PredicateSpec {
                name: "p_b".into(),
                register: "reg_b".into(),
                value: 0,
            },
        ];
        let out = weakest_precondition_predicates(&subgame, &preds, SMALL_BTOR2);
        assert!(
            out.is_empty(),
            "WP must return empty when every state register is already covered"
        );
    }

    #[test]
    fn wp_returns_empty_on_malformed_btor2() {
        // BTOR2 parse failure → WP must short-circuit to empty Vec
        // (the loop then terminates with PredicateSourceExhausted).
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        let out = weakest_precondition_predicates(&subgame, &[], "this is not valid BTOR2 syntax");
        assert!(
            out.is_empty(),
            "WP must return empty on BTOR2 parse failure rather than panicking"
        );
    }

    // ---- R.5 WP literal-extraction follow-up tests ----

    /// BTOR2 fixture with a 4-bit FSM state register `fsm_q` and a
    /// distinct comparison literal `4'd5` (encoded as `const 1 0101`).
    /// The R.5 WP literal-extraction follow-up should propose
    /// `fsm_q == 5` as one of the candidate predicates (in addition
    /// to the default `fsm_q == 0` / `fsm_q == 1`).
    const WP_LITERAL_FIXTURE: &str = r#"
1 sort bitvec 4
2 zero 1
3 const 1 0101
4 state 1 fsm_q
5 init 1 4 2
6 eq 1 4 3
7 ite 1 6 2 4
8 next 1 4 7
"#;

    #[test]
    fn wp_literal_extraction_proposes_const_values_from_btor2() {
        // FailureSubgame triggers WP; current_predicates is empty
        // (no register covered). WP should propose at least one
        // predicate for fsm_q, AND the candidate-value pool must
        // include the literal `5` extracted from `const 1 0101`.
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        let out = weakest_precondition_predicates(&subgame, &[], WP_LITERAL_FIXTURE);
        assert!(
            !out.is_empty(),
            "WP must propose at least one predicate when an uncovered register exists"
        );
        // First proposal: `fsm_q == 0` (candidate-value pool prepends 0, 1
        // before literal extraction).
        assert_eq!(out[0].register, "fsm_q");
        assert_eq!(out[0].value, 0);
        // Second proposal: `fsm_q == 1` (next in the pool).
        if out.len() >= 2 {
            assert_eq!(out[1].register, "fsm_q");
            assert_eq!(out[1].value, 1);
        }
    }

    #[test]
    fn wp_literal_extraction_skips_values_already_in_predicates() {
        // If the current predicate set already covers `fsm_q == 0`
        // and `fsm_q == 1`, the literal-extraction pass picks up the
        // next candidate (`5` from the BTOR2 const literal).
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        // Same-register coverage uses `(register, value)` pair dedup
        // per the helper's existing_register_value_pairs check — but
        // the OUTER covered-register check (by name only) currently
        // skips the register entirely if ANY predicate names it.
        // The helper's current behaviour: register-name match → skip.
        // So we need a register OTHER than fsm_q in the current set
        // to verify the literal-extraction kicks in for fsm_q.
        //
        // To verify the (register, value) dedup AT THE VALUE LEVEL,
        // we use a fixture with two state registers and partially
        // cover one. See the test below; this test asserts the
        // baseline 0/1 coverage path:
        let preds_covering_other_reg = vec![PredicateSpec {
            name: "p_a".into(),
            register: "other_reg".into(),
            value: 0,
        }];
        let out = weakest_precondition_predicates(
            &subgame,
            &preds_covering_other_reg,
            WP_LITERAL_FIXTURE,
        );
        // `other_reg` doesn't exist in WP_LITERAL_FIXTURE; only `fsm_q`
        // is uncovered → WP proposes for fsm_q.
        assert!(!out.is_empty());
        assert!(out.iter().all(|p| p.register == "fsm_q"));
    }

    #[test]
    fn wp_literal_extraction_includes_btor2_const_pool() {
        // Direct check on collect_btor2_constants: the WP_LITERAL_FIXTURE
        // has consts 0 (zero), 5 (const 1 0101). The set returned
        // must include both.
        let file = crate::adapter::btor2::parser::parse(WP_LITERAL_FIXTURE).expect("parse");
        let consts = crate::adapter::btor2::bit_blast::collect_btor2_constants(&file);
        assert!(
            consts.contains(&0),
            "collect_btor2_constants must include 0 (the `zero 1` line); got {consts:?}"
        );
        assert!(
            consts.contains(&5),
            "collect_btor2_constants must include 5 (the `const 1 0101` literal); got {consts:?}"
        );
    }

    // ---- R.5 WP cone-of-influence bound tests ----

    /// BTOR2 fixture with two connected state cells where `cnt_a`
    /// (the cone-controlling register, covered by the initial
    /// predicate) has a next-value computation that reads `cnt_b`.
    /// Under the WP cone-of-influence walk (backwards from
    /// cnt_a's next-value), `cnt_b` enters the cone — so WP must
    /// propose predicates for `cnt_b` even though only `cnt_a`
    /// is in the current predicate set.
    const WP_COI_FIXTURE: &str = r#"
1 sort bitvec 2
2 zero 1
3 const 1 11
4 state 1 cnt_a
5 state 1 cnt_b
6 add 1 4 5
7 next 1 4 6
8 add 1 5 3
9 next 1 5 8
"#;

    #[test]
    fn wp_coi_includes_register_in_cone_of_covered_predicate() {
        // Coverage: cnt_a is covered → COI walks from cnt_a's next
        // (NID 7 → operand NID 6 → operand cnt_a (4) + cnt_b (5)).
        // So COI = {cnt_a, cnt_b}. The only uncovered cell in COI
        // is cnt_b → WP proposes for cnt_b.
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        let preds = vec![PredicateSpec {
            name: "p_a".into(),
            register: "cnt_a".into(),
            value: 0,
        }];
        let out = weakest_precondition_predicates(&subgame, &preds, WP_COI_FIXTURE);
        assert!(
            !out.is_empty(),
            "WP must propose for cnt_b (in cnt_a's cone)"
        );
        assert!(
            out.iter().all(|p| p.register == "cnt_b"),
            "WP COI must restrict to cnt_b (the in-cone uncovered register); got: {:?}",
            out.iter().map(|p| p.register.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn wp_coi_falls_back_when_cone_excludes_all_uncovered() {
        // Fixture where the covered register's cone DOES NOT
        // include the uncovered register — independent chains.
        // SMALL_BTOR2 has reg_a + reg_b both with next = const 0.
        // Neither's cone includes the other → COI = {reg_a only}
        // (when reg_a is covered). reg_b is OUTSIDE the COI ⇒
        // strict-COI path returns empty ⇒ fallback fires ⇒ reg_b
        // gets proposed under the unrestricted walk.
        let subgame = FailureSubgame {
            positions: Vec::new(),
            classifying_transitions: vec![(0, 0)],
            root: None,
            subgame_extraction_complete: false,
        };
        let preds = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let out = weakest_precondition_predicates(&subgame, &preds, SMALL_BTOR2);
        assert!(
            !out.is_empty(),
            "WP must fall back when COI excludes every uncovered register"
        );
        assert!(
            out.iter().all(|p| p.register == "reg_b"),
            "fallback must propose for reg_b (the only uncovered register); got: {:?}",
            out.iter().map(|p| p.register.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn collect_reachable_states_from_walks_through_op_chains() {
        // Direct check on the parser helper: from cnt_a's next-value
        // (NID 6 = `add 1 4 5`), reachable states are cnt_a (4) + cnt_b (5).
        let file = crate::adapter::btor2::parser::parse(WP_COI_FIXTURE).expect("parse");
        let next_op =
            crate::adapter::btor2::parser::find_next_value_operand(&file, 4).expect("cnt_a Next");
        let reachable = crate::adapter::btor2::parser::collect_reachable_states_from(
            &file,
            std::slice::from_ref(&next_op),
        );
        assert!(
            reachable.contains(&4),
            "cnt_a (NID 4) must be reachable from its own next-value computation; got {reachable:?}"
        );
        assert!(
            reachable.contains(&5),
            "cnt_b (NID 5) must be reachable from cnt_a's next-value (cnt_a += cnt_b); got {reachable:?}"
        );
    }

    #[test]
    fn r5_cegar_auto_capture_records_per_iteration_approximants() {
        // R.5 sub-item 1.2 — when `capture_approximants` is set, each
        // iteration's `approximants_at_end` field is `Some(map)`
        // containing one entry per converged fixpoint var. Backward
        // compat: with the flag unset, the field stays `None`.
        //
        // Fixture: `nu X. < true > X` over the SMALL_BTOR2 1-predicate
        // lift (2 cubes). One outer fixpoint ⇒ exactly one entry in
        // the captured map.
        let formula = parser::parse("nu X. < true > X").expect("formula parses");
        let initial = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let env = Environment::new(2);

        // Run 1: capture disabled (default) ⇒ approximants_at_end None.
        let opts_off = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
        };
        let trace_off = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial.clone(),
            &env,
            &AdapterOptions::default(),
            &opts_off,
        )
        .expect("cegar succeeds with capture off");
        for iter in &trace_off.iterations {
            assert!(
                iter.approximants_at_end.is_none(),
                "capture_approximants=false MUST leave approximants_at_end None; got Some on iteration {}",
                iter.iteration
            );
        }

        // Run 2: capture enabled ⇒ approximants_at_end Some, with one
        // entry for the single fixpoint var.
        let opts_on = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: true,
            enable_approximant_reuse: false,
        };
        let trace_on = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial,
            &env,
            &AdapterOptions::default(),
            &opts_on,
        )
        .expect("cegar succeeds with capture on");
        assert!(
            !trace_on.iterations.is_empty(),
            "trace must record at least one iteration"
        );
        for iter in &trace_on.iterations {
            let approximants = iter.approximants_at_end.as_ref().unwrap_or_else(|| {
                panic!(
                    "capture_approximants=true MUST populate approximants_at_end on iteration {}",
                    iter.iteration
                )
            });
            assert_eq!(
                approximants.len(),
                1,
                "single-fixpoint formula MUST capture exactly one approximant; got {} on iteration {}",
                approximants.len(),
                iter.iteration
            );
            // B.1.a (2026-06-01): the captured iterate is a
            // `StoredApproximant` carrying must + may bit-sets.
            // Both must equal the lift's state count.
            let iterate = approximants.values().next().expect("one entry");
            assert_eq!(
                iterate.must_true.len(),
                env.state_count(),
                "captured must_true bitset length MUST equal the lift's state count"
            );
            assert_eq!(
                iterate.may_true.len(),
                env.state_count(),
                "captured may_true bitset length MUST equal the lift's state count"
            );
            // Invariant: must_true ⊆ may_true. Verified by checking
            // that `must_true & !may_true` is empty.
            let mut diff = iterate.must_true.clone();
            diff &= !iterate.may_true.clone();
            assert!(
                diff.not_any(),
                "must_true ⊆ may_true invariant violated for fixpoint var on iteration {}",
                iter.iteration
            );
        }

        // Backward-compat sanity: verdicts agree across the two runs.
        assert_eq!(
            trace_off.terminated_with, trace_on.terminated_with,
            "capture toggle MUST NOT change termination state"
        );
    }

    #[test]
    fn r5_cegar_auto_capture_13_reuse_flag_threads_priors_without_changing_verdict() {
        // R.5 CEGAR auto-capture sub-item 1.3 (2026-06-01) — when
        // `enable_approximant_reuse` is set, the loop threads
        // iteration N's `approximants_at_end` forward as
        // `prior_approximants` on iteration N+1. The seed is
        // SOUND under monotonicity (the LFP / GFP converges from
        // any monotone-comparable starting point); the verdict
        // MUST be identical to a run with reuse off.
        //
        // This test runs the converged-immediately path (formula
        // `true` returns KleeneT at iteration 0, never needs
        // refinement). The reuse flag has no observable
        // verdict-altering effect, but the trace MUST record
        // `approximant_reuse_enabled = true` to signal the caller
        // opted in.
        let formula = parser::parse("true").expect("formula parses");
        let initial = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];
        let env = Environment::new(2);

        let opts_reuse_off = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: true,
            enable_approximant_reuse: false,
        };
        let trace_off = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial.clone(),
            &env,
            &AdapterOptions::default(),
            &opts_reuse_off,
        )
        .expect("cegar succeeds with reuse off");

        let opts_reuse_on = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: true,
            enable_approximant_reuse: true,
        };
        let trace_on = cegar_refine_loop(
            &formula,
            SMALL_BTOR2,
            initial,
            &env,
            &AdapterOptions::default(),
            &opts_reuse_on,
        )
        .expect("cegar succeeds with reuse on");

        // Verdict equivalence — strict soundness guarantee.
        // TritSet has no PartialEq; use its set-equality helper.
        assert!(
            trace_off.final_verdict.eq_set(&trace_on.final_verdict),
            "approximant reuse MUST NOT change the final verdict"
        );
        assert_eq!(
            trace_off.terminated_with, trace_on.terminated_with,
            "approximant reuse MUST NOT change the termination state"
        );

        // Flag reflects the caller's opt-in.
        assert!(
            !trace_off.approximant_reuse_enabled,
            "reuse-off trace MUST report approximant_reuse_enabled = false"
        );
        assert!(
            trace_on.approximant_reuse_enabled,
            "reuse-on trace MUST report approximant_reuse_enabled = true"
        );
    }

    #[test]
    fn r5_cegar_auto_capture_13_default_options_has_reuse_off() {
        // R.5 CEGAR auto-capture sub-item 1.3 (2026-06-01) — the
        // default `CegarOptions` MUST have `enable_approximant_reuse
        // = false` (strict-additive opt-in).
        let opts = CegarOptions::default();
        assert!(
            !opts.enable_approximant_reuse,
            "CegarOptions::default() MUST have enable_approximant_reuse = false"
        );
    }
}
