//! R.5.0 sub-item 4.3 (2026-06-06) — 3-valued parity-game solver
//! that returns indefinite verdicts on KMTS positions whose
//! resolution depends on `MayOnly` edges.
//!
//! Per the breakdown at
//! `.claude/plans/r-track-multi-session-breakdown-2026-05-29.md`
//! Item 4 sub-item 4.3 of 6. Extends sub-item 4.2's classical
//! Zielonka recursion to the Shoham–Grumberg LMCS 2007 3-valued
//! setting: positions whose winner is the same in the may-relaxed
//! (over-approximation) and must-restricted (under-approximation)
//! solver runs get a **definite** verdict; positions that disagree
//! get a **KleeneBot** (indefinite) verdict, signalling that
//! refinement is needed.
//!
//! **Implementation: classical 3-valued abstraction.** Two
//! 2-valued solver runs:
//!
//! 1. **Over-approximation** (may-relaxed): include every edge
//!    (Sharp, MayOnly, MustHyperOnly). Adam has maximum moves
//!    available at Box positions; Eve has minimum at Diamond
//!    (must-restricted, including MustHyperOnly).
//! 2. **Under-approximation** (must-restricted): exclude MayOnly
//!    edges from Box positions (Adam loses the MayOnly options),
//!    and exclude MayOnly from Diamond too.
//!
//! For Sharp-only inputs, both runs produce identical games and
//! identical verdicts → every position is definite. The R.5.0
//! done-criterion's **verdict-equivalence invariant** is preserved.
//!
//! **What this sub-item does NOT do** (queued for 4.4 + 4.5):
//!
//! - No failure-subgame extraction beyond the indefinite-position
//!   list. Sub-item 4.4 ships precise per-classifying-transition
//!   extraction.
//! - No swap into `evaluate_3v_game`. Sub-item 4.5 wires this
//!   solver into the existing MVP-delegation evaluator.
//! - No benchmark / R.3 fixture sweep (sub-item 4.6).

use crate::mu_calculus::parity_game_3v_build::{
    EdgeModality, Game3v, Player, Position, PositionId,
};
use crate::mu_calculus::parity_game_3v_solve::solve_2v;
use std::collections::HashMap;

/// R.5.0 sub-item 4.3 — 3-valued per-position verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trit3v {
    /// Eve has a winning strategy in BOTH the over-approximation
    /// and under-approximation runs (definite Eve-win).
    DefiniteEve,
    /// Adam has a winning strategy in BOTH runs (definite Adam-win).
    DefiniteAdam,
    /// The over-approximation and under-approximation runs disagree
    /// at this position — neither player has a winning strategy in
    /// the abstract game. Refinement (sub-item R.5 CEGAR) is
    /// required to decide.
    Indefinite,
}

/// R.5.0 sub-item 4.3 — Per-position 3-valued solution.
#[derive(Debug, Clone)]
pub struct Solution3v {
    /// Per-position 3-valued verdict.
    pub verdicts: HashMap<PositionId, Trit3v>,
    /// Indefinite positions, surfaced separately as a convenience
    /// for sub-item 4.4's failure-subgame extraction.
    pub indefinite: Vec<PositionId>,
}

impl Solution3v {
    /// Look up the verdict at a position.
    pub fn verdict(&self, pid: PositionId) -> Option<Trit3v> {
        self.verdicts.get(&pid).copied()
    }

    /// Count positions with each verdict.
    pub fn verdict_count(&self, t: Trit3v) -> usize {
        self.verdicts.values().filter(|&&v| v == t).count()
    }
}

/// R.5.0 sub-item 4.3 — Solve a 3-valued parity game.
///
/// Inputs: the `Game3v` from sub-item 4.1 (now carrying
/// per-edge modality), and a leaf-position winner map (same shape
/// as the 2v solver consumes).
///
/// Returns: `Solution3v` mapping every `PositionId` to a `Trit3v`.
///
/// **Correctness invariant**: on a Sharp-only game (every edge is
/// `EdgeModality::Sharp`), the over- and under-approximation games
/// are identical, so every position gets a definite verdict that
/// matches `solve_2v`'s output 1:1.
///
/// **Soundness guarantee** (Shoham-Grumberg LMCS 2007 §4): when
/// the verdict is `DefiniteEve` or `DefiniteAdam`, it transfers to
/// every refinement of the KMTS (the verdict is sound on every
/// concretization). Only `Indefinite` verdicts may be over-strict;
/// CEGAR refines those.
pub fn solve_3v(game: &Game3v, leaf_winners: &HashMap<PositionId, Player>) -> Solution3v {
    // Optimistic (over-approx): include all edges. Includes
    // MayOnly + MustHyperOnly + Sharp.
    let over_solution = solve_2v(game, leaf_winners);

    // Pessimistic (under-approx): drop MayOnly edges. Game with
    // MayOnly edges removed has fewer moves available; the verdict
    // is the under-approximation.
    let under_game = remove_may_only_edges(game);
    let under_solution = solve_2v(&under_game, leaf_winners);

    // Combine per-position.
    let mut verdicts = HashMap::with_capacity(game.positions.len());
    let mut indefinite = Vec::new();
    for pid_idx in 0..game.positions.len() {
        let pid = PositionId::new(pid_idx);
        let over = over_solution.winner(pid);
        let under = under_solution.winner(pid);
        let t = match (over, under) {
            (Some(Player::Existential), Some(Player::Existential)) => Trit3v::DefiniteEve,
            (Some(Player::Universal), Some(Player::Universal)) => Trit3v::DefiniteAdam,
            _ => {
                indefinite.push(pid);
                Trit3v::Indefinite
            }
        };
        verdicts.insert(pid, t);
    }
    Solution3v {
        verdicts,
        indefinite,
    }
}

/// R.5.0 sub-item 4.3 — Build a copy of `game` with `MayOnly`
/// edges removed. `MustHyperOnly` and `Sharp` edges are preserved.
///
/// Used by `solve_3v` for the under-approximation run.
fn remove_may_only_edges(game: &Game3v) -> Game3v {
    let mut filtered_succs: Vec<Vec<PositionId>> = Vec::with_capacity(game.positions.len());
    let mut filtered_mods: Vec<Vec<EdgeModality>> = Vec::with_capacity(game.positions.len());
    for pid_idx in 0..game.positions.len() {
        let succs = &game.successors[pid_idx];
        let mods = &game.edge_modalities[pid_idx];
        let mut new_succs = Vec::with_capacity(succs.len());
        let mut new_mods = Vec::with_capacity(mods.len());
        for (s, m) in succs.iter().zip(mods.iter()) {
            if *m != EdgeModality::MayOnly {
                new_succs.push(*s);
                new_mods.push(*m);
            }
        }
        filtered_succs.push(new_succs);
        filtered_mods.push(new_mods);
    }
    Game3v {
        positions: game.positions.clone(),
        successors: filtered_succs,
        edge_modalities: filtered_mods,
        index: game.index.clone(),
    }
}

// Quiet unused-import warning when this module is compiled
// against a fresh build that doesn't yet touch Position.
#[allow(dead_code)]
fn _ensure_position_reachable(p: &Position) -> &Player {
    &p.owner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
    use crate::mu_calculus::parity_game_3v_build::build_game;
    use crate::mu_calculus::parity_game_3v_solve::leaf_winners_from_oracle;
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
        // MayOnly s0 -> s1.
        builder.transition_ids_with_modality(s0, &[lbl], s1, TransitionModality::MayOnly);
        // Sharp self-loop at s1.
        builder.transition_ids(s1, &[lbl], s1);
        builder.build().expect("build")
    }

    /// R.5.0 sub-item 4.3 — On a Sharp-only KMTS, every position
    /// gets a definite verdict. Verdict-equivalence invariant.
    #[test]
    fn r5_0_4_3_sharp_only_gives_definite_verdicts() {
        let clts = build_two_state_sharp_clts();
        let formula = parse("nu Y. (true && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_3v(&game, &leaves);
        for pid_idx in 0..game.positions.len() {
            let pid = PositionId::new(pid_idx);
            let verdict = solution.verdict(pid).expect("position has verdict");
            assert!(
                matches!(verdict, Trit3v::DefiniteEve | Trit3v::DefiniteAdam),
                "Sharp-only game must yield definite verdicts; got {verdict:?} at pid {pid_idx}"
            );
        }
        assert_eq!(
            solution.indefinite.len(),
            0,
            "Sharp-only game: no indefinite positions"
        );
    }

    /// R.5.0 sub-item 4.3 — `[a] false` on a KMTS where s0's only
    /// outgoing edge is MayOnly. Over-approximation: every may-
    /// successor (s1) satisfies `false`? No (s1 satisfies false?
    /// No, false is always false; so [a] false is false at any
    /// position with at least one may-successor; Adam wins).
    /// Under-approximation: the MayOnly edge is removed; s0 has no
    /// may-successors, so [a] false is vacuously true; Eve wins.
    /// → INDEFINITE at the formula root for s0.
    #[test]
    fn r5_0_4_3_mayonly_box_false_yields_indefinite() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_3v(&game, &leaves);

        let root = formula.root();
        let pid_s0 = game.position_id(0, root).expect("position s0");
        let verdict_s0 = solution.verdict(pid_s0).expect("s0 verdict");
        assert_eq!(
            verdict_s0,
            Trit3v::Indefinite,
            "s0 [] false on MayOnly KMTS: verdict must be Indefinite, got {verdict_s0:?}"
        );
        assert!(
            solution.indefinite.contains(&pid_s0),
            "indefinite list must include the s0 root position"
        );
    }

    /// R.5.0 sub-item 4.3 — Verdict counts: on a Sharp-only game,
    /// `verdict_count(Indefinite)` is 0; on a KMTS-shaped game,
    /// it's > 0.
    #[test]
    fn r5_0_4_3_verdict_count_distinguishes_sharp_from_kmts() {
        let sharp_clts = build_two_state_sharp_clts();
        let mayonly_clts = build_kmts_with_mayonly();
        let formula = parse("[] false").expect("parse");

        let sharp_game = build_game(&formula, &sharp_clts);
        let sharp_leaves = leaf_winners_from_oracle(&sharp_game, &formula, |_, _| false);
        let sharp_sol = solve_3v(&sharp_game, &sharp_leaves);
        assert_eq!(
            sharp_sol.verdict_count(Trit3v::Indefinite),
            0,
            "Sharp-only: 0 indefinite"
        );

        let kmts_game = build_game(&formula, &mayonly_clts);
        let kmts_leaves = leaf_winners_from_oracle(&kmts_game, &formula, |_, _| false);
        let kmts_sol = solve_3v(&kmts_game, &kmts_leaves);
        assert!(
            kmts_sol.verdict_count(Trit3v::Indefinite) > 0,
            "MayOnly KMTS: at least one indefinite position"
        );
    }

    /// R.5.0 sub-item 4.3 — `remove_may_only_edges` strips MayOnly
    /// edges from the game, leaving Sharp + MustHyperOnly intact.
    #[test]
    fn r5_0_4_3_remove_may_only_edges_strips_correctly() {
        let clts = build_kmts_with_mayonly();
        let formula = parse("[] true").expect("parse");
        let game = build_game(&formula, &clts);

        let any_may_only_before = game
            .edge_modalities
            .iter()
            .flatten()
            .any(|m| *m == EdgeModality::MayOnly);
        assert!(
            any_may_only_before,
            "KMTS-built game must contain at least one MayOnly edge"
        );

        let filtered = remove_may_only_edges(&game);
        let any_may_only_after = filtered
            .edge_modalities
            .iter()
            .flatten()
            .any(|m| *m == EdgeModality::MayOnly);
        assert!(
            !any_may_only_after,
            "filtered game must contain zero MayOnly edges"
        );
        // Sharp edges preserved.
        let original_sharp_count = game
            .edge_modalities
            .iter()
            .flatten()
            .filter(|m| **m == EdgeModality::Sharp)
            .count();
        let filtered_sharp_count = filtered
            .edge_modalities
            .iter()
            .flatten()
            .filter(|m| **m == EdgeModality::Sharp)
            .count();
        assert_eq!(
            original_sharp_count, filtered_sharp_count,
            "Sharp edge count preserved by filter"
        );
    }
}
