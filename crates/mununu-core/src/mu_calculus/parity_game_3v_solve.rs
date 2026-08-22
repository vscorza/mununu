//! R.5.0 sub-item 4.2 (2026-06-05) — Standard 2-valued Zielonka
//! recursion for parity-game solving.
//!
//! Per the breakdown at
//! `.claude/plans/r-track-multi-session-breakdown-2026-05-29.md`
//! Item 4 sub-item 4.2 of 6. Consumes the `Game3v` structure built
//! by sub-item 4.1 + a leaf-position winner assignment (`True` =
//! Eve, `False` = Adam, `Predicate(p)` = Eve iff `state ⊨ p`) and
//! returns the per-position parity-game winner.
//!
//! This is the **classical 2-valued** recursion (Zielonka, 1998 —
//! "Infinite games on finitely coloured graphs with applications to
//! automata on infinite trees"). On a KMTS containing only `Sharp`
//! transitions (and where the leaf-truth oracle is total), the
//! returned winner-map gives the same verdicts as
//! [`super::evaluate_tri`]'s 2-valued projection — Eve at a position
//! ≡ the formula holds at the position's state. The 3-valued
//! extension for `MayOnly` / `MustHyperOnly` edges + indefinite
//! winners is sub-item 4.3.
//!
//! Algorithm sketch (Zielonka 1998):
//!
//! ```text
//! solve(G):
//!   if G is empty: return (∅, ∅)
//!   d := max priority in G
//!   player := Existential iff d is odd, else Universal  // "winner if cycled"
//!   opponent := the other player
//!   U := { positions with priority d }
//!   A := player-attractor(G, U)            // positions player can force into U
//!   (W'_∃, W'_∀) := solve(G \ A)
//!   if opponent-wins-from-W' is empty:
//!     // player wins from A ∪ W'_player; opponent wins nothing
//!     return (player ↦ A ∪ W'_player, opponent ↦ ∅)
//!   else:
//!     B := opponent-attractor(G, W'_opponent)
//!     (W''_∃, W''_∀) := solve(G \ B)
//!     return (player ↦ W''_player, opponent ↦ W''_opponent ∪ B)
//! ```
//!
//! Per-iteration cost is dominated by attractor computation
//! (`O(|edges|)` worst case); the recursion bottoms out at `|G| =
//! 0`. The total runtime is exponential in alternation depth in the
//! worst case (Calude et al. STOC 2017 gave a quasi-polynomial
//! algorithm; we ship the classical Zielonka for simplicity and
//! correctness, with the QP variant left as a possible later
//! optimisation).
//!
//! **What this sub-item does NOT do** (queued):
//!
//! - No 3-valued extension. `MayOnly` and `MustHyperOnly` modalities
//!   are not yet honoured in the solver — sub-item 4.3 ships that.
//! - No predicate-oracle integration. The solver takes a pre-
//!   computed `leaf_winners` map; the future integration with the
//!   `Environment` + per-state predicate labelling lives in sub-
//!   item 4.5's swap of `evaluate_3v_game`.
//! - No CEGAR / failure-subgame interaction (sub-items 4.4 / 4.5).
//! - No benchmark (sub-item 4.6).

#[cfg(test)]
use crate::mu_calculus::parity_game_3v_build::EdgeModality;
use crate::mu_calculus::parity_game_3v_build::{Game3v, Player, Position, PositionId};
use std::collections::{BTreeSet, HashMap};

/// R.5.0 sub-item 4.2 — Per-position parity-game winner.
#[derive(Debug, Clone)]
pub struct Solution {
    /// Winner per `PositionId`. Every position in the input game
    /// appears as a key (the parity-game determinacy theorem
    /// guarantees a unique winner at every position; Mostowski 1991).
    pub winners: HashMap<PositionId, Player>,
}

impl Solution {
    /// Look up the winner at a position. Returns `None` only when
    /// the position is outside the solved game's universe (which
    /// the standard [`solve_2v`] call never produces).
    pub fn winner(&self, pid: PositionId) -> Option<Player> {
        self.winners.get(&pid).copied()
    }

    /// Count positions won by a given player.
    pub fn winner_count(&self, player: Player) -> usize {
        self.winners.values().filter(|&&w| w == player).count()
    }
}

/// R.5.0 sub-item 4.2 — Solve a 2-valued parity game.
///
/// Inputs:
/// - `game`: the `Game3v` built by [`crate::mu_calculus::parity_game_3v_build::build_game`].
/// - `leaf_winners`: pre-computed winner at every leaf position
///   (`True`, `False`, `Predicate(_)`). The caller is responsible
///   for evaluating the leaf truth against the CLTS + Environment
///   (sub-item 4.5 ships the integration). Leaf positions correspond
///   to nodes whose `Game3v::successors[pid.index()]` is empty.
///
/// Returns: `Solution` mapping every `PositionId` in the game to its
/// parity-game winner per Zielonka 1998.
///
/// **Correctness invariant**: on a Sharp-only KMTS with a total
/// leaf-truth oracle, the solver's verdict at `(state, formula_root)`
/// matches [`super::evaluate_tri`]'s 2-valued projection at `state`
/// — Eve wins iff the formula holds at the state.
pub fn solve_2v(game: &Game3v, leaf_winners: &HashMap<PositionId, Player>) -> Solution {
    // R.5.0 sub-item 4.2 — Pre-resolve every position that the
    // caller's truth oracle has explicitly classified. Zielonka
    // recurses on the remaining positions (including non-leaves
    // and any positions made stuck via edge filtering — those are
    // resolved by the propagation pass's "owner stuck loses"
    // rule, not by pre-resolution).
    //
    // R.5.0 sub-item 4.3 (2026-06-06) — Only oracle-classified
    // positions get pre-resolution. A non-leaf that becomes stuck
    // via `solve_3v`'s edge-filter (under-approximation) is NOT in
    // leaf_winners and must be resolved via propagation, so its
    // owner's no-move loss is honoured (instead of defaulting to
    // an arbitrary winner).
    let mut winners: HashMap<PositionId, Player> = HashMap::with_capacity(game.positions.len());
    for (pid, w) in leaf_winners {
        if pid.index() < game.positions.len() {
            winners.insert(*pid, *w);
        }
    }

    let mut active: BTreeSet<PositionId> = (0..game.positions.len())
        .map(PositionId::new)
        .filter(|pid| !winners.contains_key(pid))
        .collect();

    // R.5.0 sub-item 4.2 — Propagate winners from pre-resolved
    // successors. A non-leaf position whose successors are ALL
    // resolved can be settled directly: Eve picks an Eve-winning
    // successor if any (else loses); Adam picks an Adam-winning
    // successor if any (else loses). This pre-pass handles the
    // degenerate case where Zielonka's "max priority = 0
    // everywhere" recursion can't distinguish between positions —
    // standard Zielonka assumes priorities differentiate positions
    // or the game has no terminals; in the mu-calculus encoding
    // both assumptions can fail at the leaves' boundary.
    propagate_from_resolved(game, &mut active, &mut winners);

    zielonka(game, &active, &mut winners);
    Solution { winners }
}

/// R.5.0 sub-item 4.2 — Greedy resolution pass: settle active
/// positions whose winner can be decided directly from already-
/// resolved successors. Two cases:
///
/// 1. **Owner has a winning move**: if the position's owner has at
///    least one resolved successor whose winner equals the owner,
///    the owner takes that move and wins immediately. Pop the
///    position out of active.
/// 2. **All successors resolved, owner has no winning move**: the
///    owner must move (no choice avoiding the loss); whatever
///    move they pick, the resolved successor wins is the opponent.
///    Opponent wins. Pop the position out of active.
///
/// Positions where the owner has no winning move yet AND some
/// successor is still unresolved remain in `active` for Zielonka.
///
/// Saturates until no more positions can be settled. Strictly
/// sound because each settle step uses only locally-decisive
/// information (an already-resolved successor's winner).
fn propagate_from_resolved(
    game: &Game3v,
    active: &mut BTreeSet<PositionId>,
    winners: &mut HashMap<PositionId, Player>,
) {
    loop {
        let mut changed = false;
        let candidates: Vec<PositionId> = active.iter().copied().collect();
        for pid in candidates {
            let pos = &game.positions[pid.index()];
            let succs = &game.successors[pid.index()];
            let owner = pos.owner;
            let opponent = other_player(owner);

            // Case 1: owner has a winning move (resolved succ →
            // owner). Take it.
            let owner_winning_move = succs.iter().any(|s| winners.get(s).copied() == Some(owner));
            if owner_winning_move {
                winners.insert(pid, owner);
                active.remove(&pid);
                changed = true;
                continue;
            }

            // Case 2: all succs resolved, none are owner-winning →
            // every move loses → opponent wins.
            let all_resolved = succs.iter().all(|s| winners.contains_key(s));
            if all_resolved {
                winners.insert(pid, opponent);
                active.remove(&pid);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Zielonka 1998 recursion. Mutates `winners` to record the
/// per-position parity-game winner.
///
/// `active` is the set of non-leaf positions currently in the
/// sub-game. Leaf positions are pre-resolved in `winners` before
/// the call. The recursion threads `winners` (read by attractor as
/// the "known winners" table for off-active successors).
fn zielonka(
    game: &Game3v,
    active: &BTreeSet<PositionId>,
    winners: &mut HashMap<PositionId, Player>,
) {
    // R.5.0 sub-item 4.2 — Settle positions whose successors are
    // all resolved (via leaves or parent recursion) before applying
    // Zielonka's main recursion. This is essential when the active
    // sub-game has only priority-0 positions (or has terminals at
    // its boundary): the classical Zielonka trivial case
    // ("max-priority positions go to player") is unsound in those
    // cases because the actual play resolution depends on
    // already-resolved successors, not on the parity-cycle
    // assumption.
    let mut active_mut = active.clone();
    propagate_from_resolved(game, &mut active_mut, winners);
    let active = &active_mut;

    if active.is_empty() {
        return;
    }

    // Find max priority in the active sub-game. The "player to
    // win if cycled" is determined by the parity of d.
    let mut d: u32 = 0;
    for pid in active {
        let p = game.positions[pid.index()].priority;
        if p > d {
            d = p;
        }
    }
    // Standard parity convention (Calude et al. STOC 2017,
    // Apt-Grädel textbook): Eve wins iff the highest priority
    // cycled is EVEN; Adam wins iff it is ODD. The mu-calculus
    // encoding (sub-item 4.1) assigns Nu = even (gfp → cycle
    // means invariant holds → Eve wins) and Mu = odd (lfp → cycle
    // means never terminates → Adam wins).
    let player = if d.is_multiple_of(2) {
        Player::Existential
    } else {
        Player::Universal
    };
    let opponent = other_player(player);

    // U := { positions with priority d in active set }.
    let u: BTreeSet<PositionId> = active
        .iter()
        .copied()
        .filter(|pid| game.positions[pid.index()].priority == d)
        .collect();

    // A := player-attractor(active, U). Attractor reads `winners`
    // for off-active successors (pre-resolved leaves + resolved
    // positions from parent recursion calls).
    let a = attractor(game, active, &u, player, winners);

    // Recurse on active \ A.
    let active_minus_a: BTreeSet<PositionId> = active.difference(&a).copied().collect();

    if active_minus_a.is_empty() {
        // Trivial case: every active position is in A. By the
        // attractor's property, player wins from all of A.
        for pid in &a {
            winners.insert(*pid, player);
        }
        return;
    }

    // Provisionally resolve A as player-wins so the sub-recursion's
    // attractor reads A-successors as player-winning. If the
    // sub-recursion later reveals opponent has a strategy in
    // active \ A, we'll roll back A and recompute.
    let mut a_winners_snapshot: Vec<(PositionId, Option<Player>)> = Vec::with_capacity(a.len());
    for pid in &a {
        a_winners_snapshot.push((*pid, winners.get(pid).copied()));
        winners.insert(*pid, player);
    }

    zielonka(game, &active_minus_a, winners);

    // W'_opponent := positions in (active \ A) that opponent wins.
    let w_opp: BTreeSet<PositionId> = active_minus_a
        .iter()
        .filter(|pid| winners.get(pid).copied() == Some(opponent))
        .copied()
        .collect();

    if w_opp.is_empty() {
        // Player wins from A ∪ W'_player. The provisional A-
        // resolution sticks; active \ A was resolved by the recursive
        // call (and is all player-wins).
        return;
    }

    // Opponent has a strategy. Roll back A's provisional resolution
    // AND any active_minus_a resolutions — we need a clean slate to
    // compute opponent-attractor in the FULL active set.
    for (pid, prev) in &a_winners_snapshot {
        match prev {
            Some(p) => {
                winners.insert(*pid, *p);
            }
            None => {
                winners.remove(pid);
            }
        }
    }
    // Also roll back resolved positions in active_minus_a so
    // recomputed attractor doesn't read stale player-resolutions.
    let mut sub_resolved: HashMap<PositionId, Option<Player>> =
        HashMap::with_capacity(active_minus_a.len());
    for pid in &active_minus_a {
        sub_resolved.insert(*pid, winners.remove(pid));
    }

    let b = attractor(game, active, &w_opp, opponent, winners);

    // Recurse on active \ B with B provisionally resolved as
    // opponent-wins.
    let active_minus_b: BTreeSet<PositionId> = active.difference(&b).copied().collect();
    for pid in &b {
        winners.insert(*pid, opponent);
    }
    zielonka(game, &active_minus_b, winners);

    // Drop any stale sub-resolved entries (they were rolled back
    // above so the recursion fills them fresh; nothing more to do
    // — winners already reflects the new resolution).
    let _ = sub_resolved;
}

/// Player-attractor: non-leaf positions from which `player` can
/// force the play into the target set `target`. The standard
/// definition:
///
/// - `target ∩ active` ⊆ `attractor`.
/// - For a player-owned position `p`: `p ∈ attractor` iff at least
///   one of `p`'s successors is **player-winning** (either in
///   `attractor`, or resolved to `player` in the `resolved` map —
///   e.g. a pre-resolved leaf or a position resolved in an enclosing
///   recursion call).
/// - For an opponent-owned position `p`: `p ∈ attractor` iff EVERY
///   successor is player-winning under the same definition, AND
///   `p` has at least one successor (a stuck opponent loses, so
///   it's in player's attractor — but that case fires for non-leaf
///   positions whose successors have all been removed from active +
///   resolved as opponent-wins; we model that by checking
///   resolved-as-opponent successors as "not player-winning").
///
/// Returns: the set of non-leaf positions in `active` that
/// `player` can force into `target`.
fn attractor(
    game: &Game3v,
    active: &BTreeSet<PositionId>,
    target: &BTreeSet<PositionId>,
    player: Player,
    resolved: &HashMap<PositionId, Player>,
) -> BTreeSet<PositionId> {
    let mut result: BTreeSet<PositionId> = target.intersection(active).copied().collect();
    let is_player_winning = |succ: PositionId, result: &BTreeSet<PositionId>| -> bool {
        result.contains(&succ) || resolved.get(&succ).copied() == Some(player)
    };

    // Saturate.
    loop {
        let mut changed = false;
        let candidates: Vec<PositionId> = active.difference(&result).copied().collect();
        for pid in candidates {
            let pos = &game.positions[pid.index()];
            let succs = &game.successors[pid.index()];
            // Active positions are non-leaves (succs non-empty) by
            // `solve_2v`'s construction. Still defend against empty
            // succs for robustness.
            if succs.is_empty() {
                continue;
            }
            let in_attractor = if pos.owner == player {
                // Player-owned: ∃ player-winning successor.
                succs.iter().any(|&s| is_player_winning(s, &result))
            } else {
                // Opponent-owned: ∀ successors are player-winning.
                succs.iter().all(|&s| is_player_winning(s, &result))
            };
            if in_attractor {
                result.insert(pid);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    result
}

fn other_player(p: Player) -> Player {
    match p {
        Player::Existential => Player::Universal,
        Player::Universal => Player::Existential,
    }
}

/// R.5.0 sub-item 4.2 — Compute leaf-position winners for a
/// `Game3v` given a per-leaf truth oracle.
///
/// Convention:
/// - `True` leaf → Eve (Existential) wins (the formula is true here).
/// - `False` leaf → Adam (Universal) wins.
/// - `Predicate(p)` leaf → Eve wins iff `oracle(state, p) == true`.
///
/// The caller supplies a closure that resolves `(state, predicate)`
/// to a boolean. Sub-item 4.5 will wire this to the existing
/// `Environment` + per-state CLTS predicate labelling.
pub fn leaf_winners_from_oracle<F>(
    game: &Game3v,
    formula: &crate::mu_calculus::Formula,
    mut oracle: F,
) -> HashMap<PositionId, Player>
where
    F: FnMut(usize, &str) -> bool,
{
    use crate::mu_calculus::Node;
    let mut result = HashMap::new();
    for (pid_idx, pos) in game.positions.iter().enumerate() {
        let pid = PositionId::new(pid_idx);
        match formula.node(pos.node) {
            Node::True => {
                result.insert(pid, Player::Existential);
            }
            Node::False => {
                result.insert(pid, Player::Universal);
            }
            Node::Predicate(name) => {
                let winner = if oracle(pos.state, name) {
                    Player::Existential
                } else {
                    Player::Universal
                };
                result.insert(pid, winner);
            }
            _ => {}
        }
    }
    result
}

// Position struct re-export for callers that want the field
// accessors without re-importing the build module.
pub use crate::mu_calculus::parity_game_3v_build::Position as _Position;

#[allow(dead_code)]
fn _ensure_position_type_exported(p: &Position) -> &Player {
    &p.owner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
    use crate::mu_calculus::parity_game_3v_build::build_game;
    use crate::mu_calculus::parser::parse;

    fn build_two_state_clts() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
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

    /// R.5.0 sub-item 4.2 — `true` over a 2-state CLTS: Eve wins at
    /// the formula root in every state.
    #[test]
    fn r5_0_4_2_solve_true_constant_eve_wins_everywhere() {
        let clts = build_two_state_clts();
        let formula = parse("true").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Existential),
                "Eve wins `true` at state {state}"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — `false` over a 2-state CLTS: Adam wins.
    #[test]
    fn r5_0_4_2_solve_false_constant_adam_wins_everywhere() {
        let clts = build_two_state_clts();
        let formula = parse("false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Universal),
                "Adam wins `false` at state {state}"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — `[] true` over a fully-connected 2-state
    /// CLTS: every reachable next-state satisfies `true`, so the box
    /// holds and Eve wins.
    #[test]
    fn r5_0_4_2_solve_box_true_eve_wins() {
        let clts = build_two_state_clts();
        let formula = parse("[] true").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Existential),
                "[]true: Eve wins at state {state}"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — `[] false` over a CLTS where every state
    /// has an outgoing transition: every successor is false, so the
    /// box reduces to false, so Adam wins. Tests the universal-
    /// owner-with-all-moves-into-Adam case.
    #[test]
    fn r5_0_4_2_solve_box_false_adam_wins() {
        let clts = build_two_state_clts();
        let formula = parse("[] false").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Universal),
                "[]false: Adam wins at state {state}"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — `<> true` over a CLTS with outgoing
    /// transitions: Eve picks the successor; some successor satisfies
    /// `true`, so the diamond holds.
    #[test]
    fn r5_0_4_2_solve_diamond_true_eve_wins() {
        let clts = build_two_state_clts();
        let formula = parse("<> true").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(solution.winner(pid), Some(Player::Existential));
        }
    }

    /// R.5.0 sub-item 4.2 — Classical "reachability" formula
    /// `mu X. (p || <> X)` solved on a CLTS where state 1 satisfies
    /// `p` and state 0 doesn't. From state 0, Eve can reach state 1
    /// in one step, so Eve wins. From state 1, the disjunct
    /// immediately holds, so Eve wins. Both states: Eve wins.
    #[test]
    fn r5_0_4_2_solve_reachability_mu_fixpoint() {
        let clts = build_two_state_clts();
        let formula = parse("mu X. (p || <> X)").expect("parse");
        let game = build_game(&formula, &clts);
        // Oracle: p holds at state 1 only.
        let leaves =
            leaf_winners_from_oracle(&game, &formula, |state, name| name == "p" && state == 1);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Existential),
                "reachability: Eve wins at state {state}"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — Safety formula `nu Y. (p && [] Y)` on a
    /// CLTS where `p` holds at state 0 only. From state 1, `p` is
    /// false so the conjunct fails immediately (Adam wins at state
    /// 1). From state 0, the box-move to state 1 reaches a state
    /// where the gfp fails (Adam wins from state 0 too; the cycle
    /// can't sustain).
    #[test]
    fn r5_0_4_2_solve_safety_nu_fixpoint_fails() {
        let clts = build_two_state_clts();
        let formula = parse("nu Y. (p && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves =
            leaf_winners_from_oracle(&game, &formula, |state, name| name == "p" && state == 0);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Universal),
                "safety: Adam wins at state {state} (p fails to invariate)"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — Safety formula `nu Y. (true && [] Y)`
    /// (always-true): the gfp invariates trivially. Eve wins
    /// everywhere.
    #[test]
    fn r5_0_4_2_solve_safety_nu_fixpoint_holds_trivially() {
        let clts = build_two_state_clts();
        let formula = parse("nu Y. (true && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);
        let leaves = leaf_winners_from_oracle(&game, &formula, |_, _| false);
        let solution = solve_2v(&game, &leaves);

        let root = formula.root();
        for state in 0..clts.state_count() {
            let pid = game.position_id(state, root).expect("position");
            assert_eq!(
                solution.winner(pid),
                Some(Player::Existential),
                "trivial gfp: Eve wins at state {state}"
            );
        }
    }

    /// R.5.0 sub-item 4.2 — Hand-built 4-position parity game from
    /// Zielonka's textbook example (Apt-Grädel "Lectures in Game
    /// Theory for Computer Scientists" §3, fig 3.1 shape).
    ///
    /// 4 positions: p0 (∃, priority 1), p1 (∀, priority 2), p2 (∃,
    /// priority 0 leaf), p3 (∀, priority 0 leaf). Edges:
    /// p0 → p1, p1 → p0 (cycle); p1 → p2 (leaf for Adam); p0 → p3
    /// (leaf for Eve).
    ///
    /// We construct this game by hand to validate the solver
    /// independently of the build-step's formula → game mapping.
    #[test]
    fn r5_0_4_2_solve_hand_built_parity_game() {
        use crate::mu_calculus::NodeId;
        use std::collections::HashMap;
        // Fake position vector — node ids are placeholders since the
        // solver only reads owner + priority + successors.
        let positions = vec![
            // p0: ∃, priority 1, has moves
            Position {
                state: 0,
                node: NodeId(0),
                owner: Player::Existential,
                priority: 1,
            },
            // p1: ∀, priority 2, has moves
            Position {
                state: 0,
                node: NodeId(1),
                owner: Player::Universal,
                priority: 2,
            },
            // p2: leaf for Adam
            Position {
                state: 0,
                node: NodeId(2),
                owner: Player::Universal,
                priority: 0,
            },
            // p3: leaf for Eve
            Position {
                state: 0,
                node: NodeId(3),
                owner: Player::Universal,
                priority: 0,
            },
        ];
        let successors = vec![
            vec![PositionId::new(1), PositionId::new(3)], // p0 → p1, p3
            vec![PositionId::new(0), PositionId::new(2)], // p1 → p0, p2
            vec![],                                       // p2: leaf
            vec![],                                       // p3: leaf
        ];
        let mut index = HashMap::new();
        for (i, p) in positions.iter().enumerate() {
            index.insert((p.state, p.node), PositionId::new(i));
        }
        // R.5.0 sub-item 4.3 — All hand-built game edges default to
        // Sharp modality (the test pre-dates 4.3's modality
        // tracking; vanilla 2v solving uses Sharp regardless).
        let edge_modalities: Vec<Vec<EdgeModality>> = successors
            .iter()
            .map(|s| vec![EdgeModality::Sharp; s.len()])
            .collect();
        let game = Game3v {
            positions,
            successors,
            edge_modalities,
            index,
        };
        // Leaf assignment: p2 → Adam, p3 → Eve.
        let mut leaf_winners = HashMap::new();
        leaf_winners.insert(PositionId::new(2), Player::Universal);
        leaf_winners.insert(PositionId::new(3), Player::Existential);

        let solution = solve_2v(&game, &leaf_winners);

        // Eve always has the p0 → p3 move available, immediately
        // winning. Adam at p1 can pick p2 (Adam-leaf), also winning.
        // So: p0 → Eve, p1 → Adam, p2 → Adam, p3 → Eve.
        assert_eq!(
            solution.winner(PositionId::new(0)),
            Some(Player::Existential)
        );
        assert_eq!(solution.winner(PositionId::new(1)), Some(Player::Universal));
        assert_eq!(solution.winner(PositionId::new(2)), Some(Player::Universal));
        assert_eq!(
            solution.winner(PositionId::new(3)),
            Some(Player::Existential)
        );
    }
}
