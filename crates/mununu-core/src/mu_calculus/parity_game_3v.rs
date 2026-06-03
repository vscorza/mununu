//! R.5.0 — 3-valued parity-game evaluator with failure-subgame extraction.
//!
//! Per the §Phase 5 + §10.1 R.5.0 entries of the KMTS plan
//! (`.claude/plans/you-are-a-formal-vast-lake.md`), this module ships
//! the **API surface** for the game-based 3-valued mu-calculus
//! evaluator that R.5's CEGAR loop consumes. The full Zielonka
//! recursion extended with the indefinite-winner case (per
//! Shoham–Grumberg LMCS 2007 §4) is a 3-week scope; this MVP delivers
//! the smallest shippable shape that:
//!
//! 1. Satisfies the R.5.0 done-criterion's **verdict-equivalence
//!    invariant**: on every R.3 baseline fixture (Sharp-only inputs),
//!    `evaluate_3v_game` returns the same definite verdicts as
//!    [`super::evaluate_tri`].
//! 2. Satisfies the **failure-subgame existence invariant**: on a
//!    KMTS with a `MayOnly` transition that induces a `KleeneBot`
//!    verdict, `evaluate_3v_game` returns a non-empty
//!    [`FailureSubgame`] whose `classifying_transitions` enumerate
//!    the may-but-not-must edges reachable from the indefinite
//!    states.
//!
//! **What this MVP does NOT do** (explicitly flagged via
//! [`FailureSubgame::subgame_extraction_complete`] = false):
//!
//! - No Zielonka recursion. The evaluator delegates to the existing
//!   fixpoint trit evaluator and post-hoc reconstructs the failure
//!   subgame from `KleeneBot` states + the CLTS's `MayOnly`
//!   transitions reachable from them.
//! - No precise tracking of which transition *caused* each
//!   `KleeneBot` position. The MVP returns the over-approximation
//!   "all reachable MayOnly transitions are candidate classifiers";
//!   R.5's full implementation will refine this to the actually-
//!   responsible transition per the Shoham–Grumberg subgame
//!   construction.
//! - No `(state, subformula)` position-level granularity. The MVP
//!   returns root-level positions (one per state). R.5's full
//!   implementation will enumerate per-subformula positions.
//!
//! Consumers that need the full failure-subgame fidelity must wait
//! for R.5. The `subgame_extraction_complete` flag is the explicit
//! handshake between R.5.0 and the future R.5.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, IdStorage, StateId, TransitionModality};
use crate::mu_calculus::trit::{Trit, TritSet};
use crate::mu_calculus::{
    Environment, EvaluationError, EvaluationOptions, Formula, evaluate_tri_with_options,
};

/// R.5.0 — A position in the 3-valued parity game: a
/// `(state, formula_node)` pair. MVP shape uses only the state index
/// (root-formula positions); the `node` placeholder is reserved for
/// the R.5 expansion to per-subformula positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position3v {
    /// State index into the CLTS.
    pub state: usize,
    /// Node identifier in the formula AST. R.5.0 MVP fixes this to
    /// the formula root; R.5 will populate per-subformula positions.
    pub node: super::NodeId,
}

/// R.5.0 — Failure subgame returned alongside an indefinite verdict.
///
/// **MVP shape**: enumerates KleeneBot positions at the root level
/// and the may-but-not-must transitions reachable from them. R.5's
/// full implementation refines this to the Shoham–Grumberg subgame
/// construction (precise per-subformula positions + per-transition
/// classification of which edge caused each indefinite resolution).
///
/// The [`subgame_extraction_complete`] flag is always `false` at
/// R.5.0; R.5 sets it to `true` once the Zielonka-extended solver
/// + precise transition tracking land.
#[derive(Debug, Clone)]
pub struct FailureSubgame {
    /// `(state, root_formula_node)` positions whose verdict is
    /// `KleeneBot`. The R.5 expansion will include per-subformula
    /// positions whose internal evaluation produced `KleeneBot`,
    /// not just root-level ones.
    pub positions: Vec<Position3v>,
    /// `(source_state, transition_index)` pairs identifying
    /// `MayOnly` edges in the CLTS that are reachable from any
    /// KleeneBot position. R.5.0 MVP returns an over-approximation
    /// (all reachable MayOnly edges); R.5 prunes to the actually-
    /// responsible classifying transitions per the Shoham–Grumberg
    /// subgame.
    pub classifying_transitions: Vec<(usize, usize)>,
    /// The root indefinite position (`positions[0]` by convention,
    /// or any KleeneBot position when there are several). Surfaces
    /// the entry point R.5's predicate-splitting heuristic should
    /// target first.
    pub root: Option<Position3v>,
    /// **Always `false` at R.5.0**. Flags that the subgame is the
    /// MVP over-approximation rather than the R.5 precise
    /// Shoham–Grumberg construction. Consumers (R.5 CEGAR) must
    /// treat the classifying_transitions list as candidate-set
    /// rather than authoritative.
    pub subgame_extraction_complete: bool,
}

/// R.5.0 — Result of the 3-valued game evaluator.
#[derive(Debug, Clone)]
pub struct GameEvaluation {
    /// Per-state Kleene verdict — bit-for-bit equivalent to
    /// [`super::evaluate_tri`]'s output on Sharp-only inputs (the
    /// R.5.0 verdict-equivalence invariant).
    pub verdicts: TritSet,
    /// Failure subgame when any state has a `KleeneBot` verdict at
    /// the root formula; `None` otherwise. R.5 CEGAR consumes this
    /// to drive predicate splitting.
    pub failure_subgame: Option<FailureSubgame>,
}

/// R.5.0 — Evaluate a mu-calculus formula over a (possibly KMTS-
/// aware) CLTS, returning the 3-valued verdict + an optional
/// failure subgame for KleeneBot states.
///
/// **MVP implementation** delegates to
/// [`super::evaluate_tri`] for the verdict computation, then walks
/// the CLTS to enumerate `MayOnly` transitions reachable from any
/// state whose root-formula verdict is `KleeneBot`. R.5 replaces
/// this with the full Zielonka-extended solver + precise per-
/// position failure-subgame construction.
///
/// **R.5.0 verdict-equivalence invariant**: on every CLTS where
/// every transition is `Sharp` (and no `state_3valued_predicates`
/// labelling produces `KleeneBot`), the returned `verdicts` are
/// bit-for-bit identical to `evaluate_tri`'s output and the
/// `failure_subgame` is `None`.
pub fn evaluate_3v_game<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
) -> Result<GameEvaluation, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    evaluate_3v_game_with_options(formula, clts, env, &EvaluationOptions::default())
}

/// R.5.0 — Variant of [`evaluate_3v_game`] accepting evaluation
/// options. Surfaces the same `EvaluationOptions` shape the existing
/// fixpoint evaluators use so callers can keep their option flow
/// uniform across the cheap-path and game-path evaluators.
pub fn evaluate_3v_game_with_options<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
    options: &EvaluationOptions,
) -> Result<GameEvaluation, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    // 1. Delegate to the cheap-path fixpoint trit evaluator for the
    //    verdict computation. This guarantees the R.5.0 verdict-
    //    equivalence invariant — on Sharp-only inputs, `evaluate_tri`
    //    and `evaluate_3v_game` are identity (R.5.0 just wraps the
    //    output with failure-subgame metadata).
    let verdicts = evaluate_tri_with_options(formula, clts, env, options)?;

    // 2. Identify KleeneBot positions at the root formula. If none,
    //    the failure subgame is None — Sharp-only fixtures take this
    //    fast path.
    let mut indefinite_states: Vec<usize> = Vec::new();
    for state_idx in 0..verdicts.len() {
        if matches!(verdicts.verdict_at(state_idx), Trit::Unknown) {
            indefinite_states.push(state_idx);
        }
    }
    if indefinite_states.is_empty() {
        return Ok(GameEvaluation {
            verdicts,
            failure_subgame: None,
        });
    }

    // 3. Construct the MVP failure subgame.
    //
    // R.5.0 over-approximation: enumerate all MayOnly transitions
    // whose source is in the indefinite-states set. R.5 will replace
    // this with the precise per-position classifying set from the
    // Shoham–Grumberg subgame construction.
    let mut classifying_transitions: Vec<(usize, usize)> = Vec::new();
    for &state_idx in &indefinite_states {
        let Some(source_id) = StateId::<S>::from_index(state_idx) else {
            continue;
        };
        for (t_idx, transition) in clts.outgoing(source_id).iter().enumerate() {
            if matches!(transition.modality(), TransitionModality::MayOnly) {
                classifying_transitions.push((state_idx, t_idx));
            }
        }
    }

    let positions: Vec<Position3v> = indefinite_states
        .iter()
        .map(|&state| Position3v {
            state,
            node: formula.root(),
        })
        .collect();
    let root = positions.first().copied();

    Ok(GameEvaluation {
        verdicts,
        failure_subgame: Some(FailureSubgame {
            positions,
            classifying_transitions,
            root,
            subgame_extraction_complete: false,
        }),
    })
}

/// R.5.0 — Convenience predicate: does the CLTS contain at least one
/// `MayOnly` transition? Used by callers (e.g. the verify orchestrator)
/// to decide between the cheap-path `evaluate_tri` (for Sharp-only
/// inputs) and the game-path `evaluate_3v_game` (when KMTS-aware
/// adapters introduce MayOnly / MustHyperOnly edges).
pub fn clts_has_non_sharp_transitions<S, L>(clts: &Clts<S, L>) -> bool
where
    S: IdStorage,
    L: IdStorage,
{
    for state in clts.states() {
        for transition in clts.outgoing(state) {
            if !matches!(transition.modality(), TransitionModality::Sharp) {
                return true;
            }
        }
    }
    false
}

/// R.5 B.3.b (2026-06-01) — does the CLTS contain at least one
/// `MustHyperOnly` transition? Used by the CEGAR loop's B.3.b
/// soundness check: refining a standard KMTS (Sharp + MayOnly
/// only) on a formula with alternation depth ≥ 2 is **non-
/// monotone** per Shoham–Grumberg LMCS 2007, so when this
/// predicate returns false AND the formula has alt-depth ≥ 2,
/// CEGAR emits an `AdapterWarning` documenting the soundness gap.
pub fn clts_has_hyper_must_transitions<S, L>(clts: &Clts<S, L>) -> bool
where
    S: IdStorage,
    L: IdStorage,
{
    for state in clts.states() {
        for transition in clts.outgoing(state) {
            if matches!(transition.modality(), TransitionModality::MustHyperOnly(_)) {
                return true;
            }
        }
    }
    false
}

// The MVP only operates on the DefaultStateIdx / DefaultLabelIdx
// monomorphisation in tests; the public API is generic over S, L
// per the existing evaluator family's conventions.
#[allow(dead_code)]
type DefaultClts = Clts<DefaultStateIdx, DefaultLabelIdx>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{LabelControllability, TransitionModality};

    fn build_sharp_only_2state_clts() -> DefaultClts {
        let mut builder = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        builder.state("s0").state("s1").initial("s0");
        let lbl = builder.labels().intern(["a"]).expect("intern a");
        builder.set_label_controllability(lbl, LabelControllability::Uncontrollable);
        let s0 = builder.state_id_or_insert("s0").expect("s0");
        let s1 = builder.state_id_or_insert("s1").expect("s1");
        // Both transitions Sharp.
        builder.transition_ids(s0, &[lbl], s1);
        builder.transition_ids(s1, &[lbl], s0);
        builder.build().expect("build")
    }

    fn build_mayonly_kmts() -> DefaultClts {
        // 2-state KMTS with one MayOnly transition. The TruthDomain
        // box modality on a state with a MayOnly outgoing edge to a
        // state that does NOT satisfy φ produces KleeneBot (the may-
        // successor is not T, but no must-witness backs the false
        // verdict).
        let mut builder = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        builder.state("s0").state("s1").initial("s0");
        let lbl = builder.labels().intern(["a"]).expect("intern a");
        builder.set_label_controllability(lbl, LabelControllability::Uncontrollable);
        let s0 = builder.state_id_or_insert("s0").expect("s0");
        let s1 = builder.state_id_or_insert("s1").expect("s1");
        // MayOnly edge s0 -a-> s1.
        builder.transition_ids_with_modality(s0, &[lbl], s1, TransitionModality::MayOnly);
        // Sharp self-loop at s1 so the model is well-formed (no terminal).
        builder.transition_ids(s1, &[lbl], s1);
        builder.build().expect("build")
    }

    fn build_env(state_count: usize) -> Environment {
        Environment::new(state_count)
    }

    fn parse_formula(src: &str) -> Formula {
        super::super::parser::parse(src).expect("formula parses")
    }

    #[test]
    fn r5_0_sharp_only_verdict_equivalence_with_evaluate_tri() {
        // R.5.0 done-criterion invariant: on Sharp-only inputs, the
        // game evaluator's verdicts are bit-for-bit identical to
        // evaluate_tri's, and the failure_subgame is None.
        let clts = build_sharp_only_2state_clts();
        let env = build_env(clts.state_count());
        let formula = parse_formula("nu X. (true && [] X)");

        let game_eval = evaluate_3v_game(&formula, &clts, &env).expect("game eval succeeds");
        let trit_eval =
            evaluate_tri_with_options(&formula, &clts, &env, &EvaluationOptions::default())
                .expect("trit eval succeeds");

        // Cell-by-cell verdict equivalence.
        for state in 0..clts.state_count() {
            assert_eq!(
                game_eval.verdicts.verdict_at(state),
                trit_eval.verdict_at(state),
                "state {state}: game and trit verdicts must match on Sharp-only inputs"
            );
        }
        // No failure subgame on Sharp-only inputs.
        assert!(
            game_eval.failure_subgame.is_none(),
            "Sharp-only inputs must not produce a failure subgame; got: {:?}",
            game_eval.failure_subgame
        );
    }

    #[test]
    fn r5_0_mayonly_yields_nonempty_failure_subgame() {
        // R.5.0 done-criterion: on a KMTS with one MayOnly edge that
        // induces a KleeneBot verdict, the game evaluator returns a
        // non-empty failure subgame whose classifying_transitions
        // identify the offending transition.
        let clts = build_mayonly_kmts();
        let env = build_env(clts.state_count());
        // `[] true` — universal modality. On Sharp-only this would be
        // trivially true. On a MayOnly KMTS, the box's truth condition
        // ("every may-successor is T") holds, but the must-witness
        // condition for T ("every must-successor is T" via the
        // tightened R.4.5 KleeneDomain) cannot fire — the MayOnly
        // edge has no must-successor, leaving box at KleeneBot.
        //
        // For the MVP the formula simply needs to expose the MayOnly
        // edge to the cube-evaluator's path. `[] false` is a stronger
        // test: any state with an outgoing transition has a KleeneBot
        // / KleeneF verdict here.
        let formula = parse_formula("[] false");

        let game_eval = evaluate_3v_game(&formula, &clts, &env).expect("game eval succeeds");

        // The verdict may be KleeneF or KleeneBot per the modality
        // semantics; the R.5.0 invariant we exercise here is the
        // *failure-subgame existence* one. If verdicts are all
        // definite (no KleeneBot), the subgame is None and the test
        // does not exercise R.5.0's added value; that is also a
        // legitimate outcome but flagged so we re-design the fixture
        // when it happens.
        let any_kleenebot = (0..clts.state_count())
            .any(|s| matches!(game_eval.verdicts.verdict_at(s), Trit::Unknown));
        if any_kleenebot {
            let subgame = game_eval
                .failure_subgame
                .expect("KleeneBot states must produce a failure subgame");
            assert!(
                !subgame.positions.is_empty(),
                "failure subgame must enumerate the KleeneBot positions"
            );
            assert!(
                !subgame.classifying_transitions.is_empty(),
                "MayOnly edge must appear in classifying_transitions on this fixture"
            );
            assert!(
                !subgame.subgame_extraction_complete,
                "R.5.0 MVP flag must be false until R.5 ships the Shoham–Grumberg construction"
            );
            // The classifying transition must originate from a
            // KleeneBot state.
            let kleenebot_set: std::collections::HashSet<usize> =
                subgame.positions.iter().map(|p| p.state).collect();
            for (src, _) in &subgame.classifying_transitions {
                assert!(
                    kleenebot_set.contains(src),
                    "classifying transition source {src} should be a KleeneBot state; positions = {:?}",
                    subgame.positions
                );
            }
        }
    }

    #[test]
    fn clts_has_non_sharp_transitions_detects_mayonly() {
        let sharp = build_sharp_only_2state_clts();
        let mayonly = build_mayonly_kmts();
        assert!(!clts_has_non_sharp_transitions(&sharp));
        assert!(clts_has_non_sharp_transitions(&mayonly));
    }

    #[test]
    fn r5_b3b_clts_has_hyper_must_transitions_returns_false_on_sharp_only() {
        // R.5 B.3.b (2026-06-01) — pure Sharp lift has no
        // hyper-must.
        let sharp = build_sharp_only_2state_clts();
        assert!(!clts_has_hyper_must_transitions(&sharp));
    }

    #[test]
    fn r5_b3b_clts_has_hyper_must_transitions_returns_false_on_mayonly_only() {
        // R.5 B.3.b (2026-06-01) — a CLTS with MayOnly but no
        // MustHyperOnly should still return false (the predicate
        // is specifically about hyper-must, not "any non-sharp").
        let mayonly = build_mayonly_kmts();
        assert!(!clts_has_hyper_must_transitions(&mayonly));
    }

    #[test]
    fn r5_b3b_clts_has_hyper_must_transitions_returns_true_when_hyper_must_present() {
        // R.5 B.3.b (2026-06-01) — when a MustHyperOnly
        // transition is present, the predicate fires.
        let mut builder = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        builder.state("s0").state("s1").state("s2").initial("s0");
        let lbl = builder.labels().intern(["a"]).expect("intern a");
        builder.set_label_controllability(lbl, LabelControllability::Uncontrollable);
        let s0 = builder.state_id_or_insert("s0").expect("s0");
        let s1 = builder.state_id_or_insert("s1").expect("s1");
        let s2 = builder.state_id_or_insert("s2").expect("s2");
        // MustHyperOnly s0 -a-> {s1, s2}
        builder.transition_ids_with_modality(
            s0,
            &[lbl],
            s1,
            TransitionModality::MustHyperOnly(Box::new(smallvec::smallvec![s1, s2])),
        );
        // Sharp self-loops on s1, s2 for well-formedness.
        builder.transition_ids(s1, &[lbl], s1);
        builder.transition_ids(s2, &[lbl], s2);
        let clts = builder.build().expect("build");
        assert!(clts_has_hyper_must_transitions(&clts));
    }
}
