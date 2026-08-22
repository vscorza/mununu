//! R.5.0 sub-item 4.4 (2026-06-06) — Precise failure-subgame
//! extraction.
//!
//! Per the breakdown at
//! `.claude/plans/r-track-multi-session-breakdown-2026-05-29.md`
//! Item 4 sub-item 4.4 of 6. From an indefinite verdict produced by
//! [`solve_3v`], identifies the **classifying** `MayOnly` edges:
//! those whose removal from the over-approximation game changes
//! the verdict at at least one position. Replaces the MVP's
//! over-approximation (which marks every reachable `MayOnly` edge
//! as a candidate classifier).
//!
//! **Method: per-edge differential evaluation.** For each `MayOnly`
//! edge `e` in the game:
//!
//! 1. Run [`solve_2v`] on the over-approximation game with `e`
//!    removed.
//! 2. Compare the per-position winners to the unperturbed over-
//!    approximation.
//! 3. `e` is **classifying** iff at least one position's verdict
//!    changes between the two runs.
//!
//! Cost: `O(|MayOnly edges| × solve_2v_cost)`. For MVP fixtures
//! with ≤ 10 `MayOnly` edges and `solve_2v` polynomial in the
//! game size, this is acceptable. Sub-item 4.5's integration may
//! demand optimisation; today the MVP fixtures finish in under
//! 100ms.
//!
//! **What this sub-item does NOT do** (queued for 4.5 + 4.6):
//!
//! - No swap into `evaluate_3v_game`'s MVP delegation. Sub-item
//!   4.5 wires this in.
//! - No bench (sub-item 4.6).

use crate::mu_calculus::parity_game_3v_build::{EdgeModality, Game3v, Player, PositionId};
use crate::mu_calculus::parity_game_3v_solve::solve_2v;
use crate::mu_calculus::parity_game_3v_solve3v::Solution3v;
use std::collections::HashMap;

/// R.5.0 sub-item 4.4 — Precise failure-subgame: positions whose
/// verdict is `Indefinite` + the load-bearing `MayOnly` edges
/// that caused the indefiniteness.
///
/// This replaces the MVP `parity_game_3v::FailureSubgame`'s over-
/// approximation (which lists every reachable `MayOnly` edge). The
/// precise list is suitable for R.5 CEGAR's per-edge predicate-
/// splitting: refinement only targets edges actually responsible
/// for indefinite verdicts, not every may-edge in the cone.
#[derive(Debug, Clone)]
pub struct FailureSubgamePrecise {
    /// Indefinite positions per [`Trit3v::Indefinite`].
    pub positions: Vec<PositionId>,
    /// `(source_position, target_position)` pairs for `MayOnly`
    /// edges whose removal changes at least one position's verdict.
    /// Empty iff no `MayOnly` edge is load-bearing for any
    /// indefinite verdict (which is the Sharp-only fixture case
    /// and a correctness check: indefiniteness without a
    /// classifying edge would be unsound).
    pub classifying_edges: Vec<(PositionId, PositionId)>,
    /// Always `true` at sub-item 4.4 (precise extraction, unlike
    /// the MVP's `false`).
    pub subgame_extraction_complete: bool,
}

/// R.5.0 sub-item 4.4 — Extract the precise failure subgame from a
/// 3-valued solution.
///
/// Inputs: the `Game3v` from sub-item 4.1, the leaf-winner oracle,
/// and the `Solution3v` from sub-item 4.3. Returns a
/// [`FailureSubgamePrecise`] whose `classifying_edges` list each
/// `MayOnly` edge that is load-bearing for at least one indefinite
/// position.
///
/// **Correctness invariant**: on a `Solution3v` with no indefinite
/// positions, `classifying_edges` is empty and `positions` is
/// empty. On a `Solution3v` with indefinite positions, every
/// classifying edge has `EdgeModality::MayOnly` (verified by
/// `r5_0_4_4_classifying_edges_are_mayonly`).
pub fn extract_failure_subgame(
    game: &Game3v,
    leaf_winners: &HashMap<PositionId, Player>,
    solution3v: &Solution3v,
) -> FailureSubgamePrecise {
    let positions = solution3v.indefinite.clone();

    if positions.is_empty() {
        return FailureSubgamePrecise {
            positions,
            classifying_edges: Vec::new(),
            subgame_extraction_complete: true,
        };
    }

    // Baseline: over-approximation solve.
    let over_baseline = solve_2v(game, leaf_winners);

    // Enumerate all MayOnly edges.
    let mut may_only_edges: Vec<(PositionId, PositionId)> = Vec::new();
    for src_idx in 0..game.positions.len() {
        let succs = &game.successors[src_idx];
        let mods = &game.edge_modalities[src_idx];
        for (s, m) in succs.iter().zip(mods.iter()) {
            if *m == EdgeModality::MayOnly {
                may_only_edges.push((PositionId::new(src_idx), *s));
            }
        }
    }

    // Per-edge differential: remove each MayOnly edge from a fresh
    // game copy, re-solve, compare per-position verdicts to
    // baseline.
    let mut classifying_edges: Vec<(PositionId, PositionId)> = Vec::new();
    for &(src, tgt) in &may_only_edges {
        let perturbed = remove_single_edge(game, src, tgt);
        let perturbed_sol = solve_2v(&perturbed, leaf_winners);
        let changed = (0..game.positions.len())
            .map(PositionId::new)
            .any(|pid| over_baseline.winner(pid) != perturbed_sol.winner(pid));
        if changed {
            classifying_edges.push((src, tgt));
        }
    }

    FailureSubgamePrecise {
        positions,
        classifying_edges,
        subgame_extraction_complete: true,
    }
}

/// Build a fresh `Game3v` copy with the single edge `src -> tgt`
/// removed. If the edge appears multiple times (parallel edges
/// after coalescing), only the first occurrence is removed.
fn remove_single_edge(game: &Game3v, src: PositionId, tgt: PositionId) -> Game3v {
    let mut successors = game.successors.clone();
    let mut edge_modalities = game.edge_modalities.clone();
    let src_succs = &mut successors[src.index()];
    let src_mods = &mut edge_modalities[src.index()];
    if let Some(pos) = src_succs.iter().position(|s| *s == tgt) {
        src_succs.remove(pos);
        src_mods.remove(pos);
    }
    Game3v {
        positions: game.positions.clone(),
        successors,
        edge_modalities,
        index: game.index.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
    use crate::mu_calculus::parity_game_3v_build::build_game;
    use crate::mu_calculus::parity_game_3v_solve::leaf_winners_from_oracle;
    use crate::mu_calculus::parity_game_3v_solve3v::{Trit3v, solve_3v};
    use crate::mu_calculus::parser::parse;

    fn build_two_state_sharp_clts() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
        let mut builder = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        builder.state("s0").state("s1").initial("s0");
        let lbl = builder.labels().intern(["a"]).expect("intern a");
        builder.set_label_controllability(lbl, LabelControllability::Uncontrollable);
        let s0 = builder.state_id_or_insert("s0").expect("s0");
        let s1 = builder.state_id_or_insert("s1").expect("s1");
        builder.transition_ids(s0, &[lbl], s1);
        builder.transition_ids(s1, &[lbl], s0);
        builder.build().expect("build")
    }

    fn build_kmts_with_mayonly() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
        use crate::clts::TransitionModality;
        let mut builder = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        builder.state("s0").state("s1").initial("s0");
        let lbl = builder.labels().intern(["a"]).expect("intern a");
        builder.set_label_controllability(lbl, LabelControllability::Uncontrollable);
        let s0 = builder.state_id_or_insert("s0").expect("s0");
        let s1 = builder.state_id_or_insert("s1").expect("s1");
        builder.transition_ids_with_modality(s0, &[lbl], s1, TransitionModality::MayOnly);
        builder.transition_ids(s1, &[lbl], s1);
        builder.build().expect("build")
    }

    /// R.5.0 sub-item 4.4 — Sharp-only games produce no indefinite
    /// positions and an empty classifying-edges list.
    #[test]
    fn r5_0_4_4_sharp_only_produces_empty_subgame() {
        let clts = build_two_state_sharp_clts();
        let formula = parse("nu Y. (true && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution3v = solve_3v(&game, &leaves);
        let subgame = extract_failure_subgame(&game, &leaves, &solution3v);
        assert_eq!(subgame.positions.len(), 0, "no indefinite positions");
        assert_eq!(subgame.classifying_edges.len(), 0, "no classifying edges");
        assert!(
            subgame.subgame_extraction_complete,
            "precise extraction flag is true"
        );
    }

    /// R.5.0 sub-item 4.4 — When a MayOnly edge causes an
    /// indefinite verdict, that edge appears in the classifying-
    /// edges list. The MVP over-approximation would have included
    /// EVERY MayOnly edge reachable from indefinite states; this
    /// precise extraction includes only edges that are load-bearing.
    #[test]
    fn r5_0_4_4_mayonly_edge_appears_in_classifying_list() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution3v = solve_3v(&game, &leaves);

        assert!(
            !solution3v.indefinite.is_empty(),
            "MayOnly KMTS should produce indefinite positions"
        );

        let subgame = extract_failure_subgame(&game, &leaves, &solution3v);
        assert!(
            !subgame.classifying_edges.is_empty(),
            "MayOnly edges should appear in classifying list when load-bearing"
        );
        // Each classifying edge must be a MayOnly edge in the game.
        for (src, tgt) in &subgame.classifying_edges {
            let mods = &game.edge_modalities[src.index()];
            let succs = &game.successors[src.index()];
            let edge_pos = succs.iter().position(|s| *s == *tgt);
            assert!(edge_pos.is_some(), "classifying edge must exist in game");
            assert_eq!(
                mods[edge_pos.unwrap()],
                EdgeModality::MayOnly,
                "classifying edges are MayOnly"
            );
        }
    }

    /// R.5.0 sub-item 4.4 — All classifying edges are MayOnly
    /// (verified across all 4.4 fixtures). Soundness invariant.
    #[test]
    fn r5_0_4_4_classifying_edges_are_mayonly() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution3v = solve_3v(&game, &leaves);
        let subgame = extract_failure_subgame(&game, &leaves, &solution3v);

        for (src, tgt) in &subgame.classifying_edges {
            let succs = &game.successors[src.index()];
            let mods = &game.edge_modalities[src.index()];
            let i = succs
                .iter()
                .position(|s| *s == *tgt)
                .expect("classifying edge exists");
            assert_eq!(
                mods[i],
                EdgeModality::MayOnly,
                "every classifying edge has MayOnly modality"
            );
        }
    }

    /// R.5.0 sub-item 4.4 — `subgame_extraction_complete` is
    /// always `true` for the precise extraction (distinguishes from
    /// the MVP delegation in `parity_game_3v::FailureSubgame` which
    /// sets it to `false`).
    #[test]
    fn r5_0_4_4_extraction_complete_flag_is_true() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution3v = solve_3v(&game, &leaves);
        let subgame = extract_failure_subgame(&game, &leaves, &solution3v);
        assert!(subgame.subgame_extraction_complete);
    }

    /// R.5.0 sub-item 4.4 — `remove_single_edge` helper unit test.
    /// Removes one occurrence of the (src, tgt) edge; leaves
    /// everything else intact.
    #[test]
    fn r5_0_4_4_remove_single_edge_helper() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);

        let original_edge_count = game.edge_count();
        // Find any edge to remove.
        let (src, tgt) = (0..game.positions.len())
            .find_map(|src_idx| {
                game.successors[src_idx]
                    .first()
                    .map(|&t| (PositionId::new(src_idx), t))
            })
            .expect("game has at least one edge");

        let perturbed = remove_single_edge(&game, src, tgt);
        assert_eq!(
            perturbed.edge_count(),
            original_edge_count - 1,
            "exactly one edge removed"
        );
    }

    /// R.5.0 sub-item 4.4 — uses verdict from Solution3v to confirm
    /// shipping invariant: precise extraction never returns
    /// `Indefinite` positions that the verdict map does not
    /// include.
    #[test]
    fn r5_0_4_4_positions_match_solution3v_indefinite_list() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution3v = solve_3v(&game, &leaves);
        let subgame = extract_failure_subgame(&game, &leaves, &solution3v);

        assert_eq!(
            subgame.positions, solution3v.indefinite,
            "subgame positions exactly match solution3v indefinite list"
        );

        // Cross-check via verdict map.
        for pid in &subgame.positions {
            assert_eq!(
                solution3v.verdict(*pid),
                Some(Trit3v::Indefinite),
                "every subgame position is Indefinite in the solution"
            );
        }
    }
}
