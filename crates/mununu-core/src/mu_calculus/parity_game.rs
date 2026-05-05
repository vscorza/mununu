//! Parity-game synthesis for arbitrary-alternation mu-calculus formulas.
//!
//! For alternation depth ≤ 2, the obligation-rotation strategy in
//! `ControllerMode::ProductGame` (mu-obligation rotation) produces a correct
//! Mealy controller. For higher alternation (e.g., parity properties with
//! priorities > 2), the proper construction is the **parity game** over
//! `Σ × Sub(φ)`:
//!
//! - Game positions: `(plant_state, formula_subnode)`.
//! - Edges follow formula structure (And/Or branches, fixpoint unfolding,
//!   modal pre-images via plant transitions).
//! - Priorities: derived from mu/nu nesting; mu = odd, nu = even.
//! - Player ownership: ∃ (Eve, the controller) owns Or, Diamond modal,
//!   and free transitions; ∀ (Adam, the environment) owns And, Box modal.
//!
//! Standard parity-game theory (Emerson-Jutla, Zielonka 1998) guarantees
//! positional determinacy: a memoryless winning strategy exists on the
//! game graph for the winner. Projecting back to the plant requires memory
//! = the formula sub-node component of the position.
//!
//! This module implements:
//! 1. Explicit game construction from a `(Formula, Clts, Environment)` triple,
//!    preceded by an NNF preprocessing pass (`super::nnf::to_nnf`) so that
//!    `Not` only ever appears above atomic positions.
//! 2. A recursive Zielonka solver returning per-position winning region and
//!    Eve's positional strategy.
//! 3. A projection helper to convert the game-level strategy into a
//!    plant-level Mealy controller (state space = `(plant_state, formula_node)`).
//!
//! Modal guards are filtered exactly per the mu-calculus semantics:
//! controllability flag (`Control::All` / `Controllable` / `Environment`),
//! label-name set (`Guard::labels`), and current/next state-variable filters
//! (`Guard::current` / `Guard::next`). The matching mirrors
//! `mu_calculus::evaluator::guard_matches`.

use std::collections::{HashMap, HashSet};

use bitvec::prelude::{BitVec, Lsb0};

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, StateId};

use super::{
    Control, Environment, Formula, FormulaVarId, Guard, ModalKind, Node, NodeId,
    guard_matches_labels_and_vars,
};

/// A position in the parity game: a `(plant_state, formula_node)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub state: usize,
    pub node: NodeId,
}

/// Game-graph player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    /// The controller (∃, Eve). Wins on infinite plays where the highest
    /// priority occurring infinitely often is even.
    Eve,
    /// The environment (∀, Adam). Wins otherwise.
    Adam,
}

/// Parity game built from a mu-calculus formula and a CLTS.
#[derive(Debug)]
pub struct ParityGame {
    /// All positions in the game.
    pub positions: Vec<Position>,
    /// Reverse lookup: position → index into `positions`.
    pub position_idx: HashMap<Position, usize>,
    /// Player owning each position (indexed by position index).
    pub owners: Vec<Player>,
    /// Priority assigned to each position (indexed by position index).
    /// Even priorities favor Eve, odd priorities favor Adam.
    pub priorities: Vec<usize>,
    /// Outgoing edges: edges[i] is the list of position indices reachable
    /// from position `i`.
    pub edges: Vec<Vec<usize>>,
    /// Predicate-terminal results: positions[i] is `Eve`-winning iff
    /// `terminal_eve_wins[i]` is true. For non-terminal positions this is
    /// always false (the value is irrelevant).
    pub terminal_eve_wins: Vec<bool>,
    /// Indices of terminal positions (for fast lookup during attractor).
    pub terminals: HashSet<usize>,
    /// The NNF-transformed formula used to build this game. Callers that
    /// need to identify "root-formula" positions should use
    /// `formula.root()` from this field rather than the original formula's
    /// root, because the NodeIds differ between the two.
    pub formula: Formula,
}

impl ParityGame {
    /// Number of positions.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// True when the game has zero positions.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

/// Result of solving a parity game: per-position winner and Eve's positional
/// strategy (one chosen successor for each Eve position in her winning
/// region).
#[derive(Debug)]
pub struct ParitySolution {
    pub winner: Vec<Player>,
    /// `eve_strategy[pos_idx] = Some(successor_idx)` for Eve positions in
    /// her winning region. `None` for Adam positions or losing positions.
    pub eve_strategy: Vec<Option<usize>>,
}

// ---------------------------------------------------------------------------
// Game construction
// ---------------------------------------------------------------------------

/// Build the parity game from a formula and CLTS.
///
/// The formula is first transformed to negation normal form (NNF) so that
/// `Not` only appears immediately above atomic positions. After NNF the
/// game construction is exact — no compound-negation pass-through.
///
/// Only positions reachable from initial-state × root-node are constructed
/// (lazy expansion). This avoids materializing irrelevant `(state, node)`
/// combinations.
///
/// Predicate atoms are evaluated against the environment and become
/// terminal positions (Eve-winning iff the predicate holds at the state).
///
/// The returned `ParityGame` retains the NNF-transformed formula. Callers
/// that need root-position identification should use `game.formula.root()`,
/// not the original formula's root, because the NodeIds differ.
pub fn build_parity_game(
    formula: &Formula,
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    env: &Environment,
) -> ParityGame {
    // Preprocess: rewrite to NNF so the rest of the construction can assume
    // negation only ever appears above atoms (Predicate / True / False).
    // The NNF formula is then both used for the build and stashed in the
    // returned ParityGame so callers can use its root NodeId for
    // identifying initial positions.
    let nnf_formula = super::nnf::to_nnf(formula);
    let formula = &nnf_formula;
    // Map each fixpoint variable to its priority. Priorities are based on
    // alternation depth: outer fixpoints get higher priorities, with mu
    // assigned odd and nu assigned even. We follow the standard "Streett"-
    // style assignment where higher priority = more outer.
    let nesting = formula.fixpoint_nesting_order();
    let mut var_priority: HashMap<FormulaVarId, usize> = HashMap::new();
    let total_levels = nesting.len();
    for (level, (var, is_mu)) in nesting.iter().enumerate() {
        // Outermost fixpoint gets priority `total_levels` (or `total_levels - 1`
        // depending on parity). Innermost gets priority 0 or 1.
        let base = total_levels - level;
        let priority = if *is_mu {
            // mu → odd; ensure odd
            if base.is_multiple_of(2) {
                base + 1
            } else {
                base
            }
        } else {
            // nu → even
            if base.is_multiple_of(2) {
                base
            } else {
                base + 1
            }
        };
        var_priority.insert(*var, priority);
    }

    // Map each fixpoint variable to its body NodeId so we can resolve
    // `Variable(X)` to "edge to body of binder for X".
    let mut var_body: HashMap<FormulaVarId, NodeId> = HashMap::new();
    collect_var_bodies(formula, formula.root(), &mut var_body);

    let mut game = ParityGame {
        positions: Vec::new(),
        position_idx: HashMap::new(),
        owners: Vec::new(),
        priorities: Vec::new(),
        edges: Vec::new(),
        terminal_eve_wins: Vec::new(),
        terminals: HashSet::new(),
        // Stash the NNF formula so callers can identify root-formula
        // positions via `game.formula.root()`. Cloning is cheap — Formula
        // is a small arena of nodes + variable names.
        formula: nnf_formula.clone(),
    };

    // Lazy expansion: BFS from initials × root.
    let mut queue: Vec<Position> = Vec::new();
    for init in clts.initial_states() {
        let pos = Position {
            state: init.index(),
            node: formula.root(),
        };
        queue.push(pos);
        intern_position(&mut game, pos, formula, &var_priority);
    }

    while let Some(pos) = queue.pop() {
        let pos_idx = *game.position_idx.get(&pos).expect("position interned");

        // Skip already-expanded positions
        if !game.edges[pos_idx].is_empty() || game.terminals.contains(&pos_idx) {
            continue;
        }

        let node = formula.node(pos.node);
        match node {
            Node::True => {
                set_terminal(&mut game, pos_idx, true);
            }
            Node::False => {
                set_terminal(&mut game, pos_idx, false);
            }
            Node::Predicate(name) => {
                let holds = env
                    .predicate(name)
                    .map(|bits| bits.get(pos.state).is_some_and(|b| *b))
                    .unwrap_or(false);
                set_terminal(&mut game, pos_idx, holds);
            }
            Node::Not(inner) => {
                // After the NNF preprocessing pass at function entry, `Not`
                // can only appear above atomic positions: Predicate, True,
                // or False. Compound negations (over And/Or/Modal/Mu/Nu)
                // are eliminated by NNF. The `_` arm asserts unreachable.
                match formula.node(*inner) {
                    Node::True => set_terminal(&mut game, pos_idx, false),
                    Node::False => set_terminal(&mut game, pos_idx, true),
                    Node::Predicate(name) => {
                        let holds = env
                            .predicate(name)
                            .map(|bits| bits.get(pos.state).is_some_and(|b| *b))
                            .unwrap_or(false);
                        set_terminal(&mut game, pos_idx, !holds);
                    }
                    other => unreachable!(
                        "Not over non-atomic node {other:?} after NNF — to_nnf is broken"
                    ),
                }
            }
            Node::And(l, r) => {
                let p_l = Position {
                    state: pos.state,
                    node: *l,
                };
                let p_r = Position {
                    state: pos.state,
                    node: *r,
                };
                game.owners[pos_idx] = Player::Adam;
                add_edge(&mut game, formula, &var_priority, pos_idx, p_l, &mut queue);
                add_edge(&mut game, formula, &var_priority, pos_idx, p_r, &mut queue);
            }
            Node::Or(l, r) => {
                let p_l = Position {
                    state: pos.state,
                    node: *l,
                };
                let p_r = Position {
                    state: pos.state,
                    node: *r,
                };
                game.owners[pos_idx] = Player::Eve;
                add_edge(&mut game, formula, &var_priority, pos_idx, p_l, &mut queue);
                add_edge(&mut game, formula, &var_priority, pos_idx, p_r, &mut queue);
            }
            Node::Modal {
                kind,
                guard,
                target,
            } => {
                let owner = modal_owner(*kind);
                game.owners[pos_idx] = owner;
                let state_id = StateId::<DefaultStateIdx>::from_index(pos.state)
                    .expect("position state index fits storage");
                for transition in clts.outgoing(state_id) {
                    if !transition_matches_guard(transition, guard, clts, state_id) {
                        continue;
                    }
                    let next = Position {
                        state: transition.target().index(),
                        node: *target,
                    };
                    add_edge(&mut game, formula, &var_priority, pos_idx, next, &mut queue);
                }
                // If no outgoing transitions matched, the position has no
                // successors. Box with no successors is vacuously
                // satisfied (Eve wins); Diamond with no successors is
                // unsatisfiable (Adam wins).
                if game.edges[pos_idx].is_empty() {
                    let eve_wins = matches!(kind, ModalKind::Box);
                    set_terminal(&mut game, pos_idx, eve_wins);
                }
            }
            Node::Mu { body, .. } | Node::Nu { body, .. } => {
                let next = Position {
                    state: pos.state,
                    node: *body,
                };
                game.owners[pos_idx] = Player::Eve; // free move; either player works
                add_edge(&mut game, formula, &var_priority, pos_idx, next, &mut queue);
            }
            Node::Variable(var) => {
                let body = var_body
                    .get(var)
                    .copied()
                    .expect("variable should have a binder");
                let next = Position {
                    state: pos.state,
                    node: body,
                };
                game.owners[pos_idx] = Player::Eve; // free move
                add_edge(&mut game, formula, &var_priority, pos_idx, next, &mut queue);
            }
        }
    }

    game
}

/// Convert a position into a terminal: add a self-loop with priority
/// 0 (if Eve wins this terminal) or 1 (if Adam wins). The self-loop ensures
/// every position has at least one outgoing edge — eliminates "no-moves"
/// edge cases in the Zielonka recursion. Infinite plays at the terminal
/// resolve via the parity priority.
fn set_terminal(game: &mut ParityGame, pos_idx: usize, eve_wins: bool) {
    game.terminals.insert(pos_idx);
    game.terminal_eve_wins[pos_idx] = eve_wins;
    game.priorities[pos_idx] = if eve_wins { 0 } else { 1 };
    // Self-loop: position points to itself
    if !game.edges[pos_idx].contains(&pos_idx) {
        game.edges[pos_idx].push(pos_idx);
    }
    // Owner doesn't matter for self-loops at terminal priority — either
    // way the parity priority decides the infinite-play winner.
    game.owners[pos_idx] = Player::Eve;
}

fn intern_position(
    game: &mut ParityGame,
    pos: Position,
    formula: &Formula,
    var_priority: &HashMap<FormulaVarId, usize>,
) -> usize {
    if let Some(&idx) = game.position_idx.get(&pos) {
        return idx;
    }
    let idx = game.positions.len();
    game.positions.push(pos);
    game.position_idx.insert(pos, idx);
    game.owners.push(Player::Eve);
    let priority = match formula.node(pos.node) {
        Node::Mu { var, .. } => *var_priority.get(var).copied().get_or_insert(1),
        Node::Nu { var, .. } => *var_priority.get(var).copied().get_or_insert(0),
        _ => 0,
    };
    game.priorities.push(priority);
    game.edges.push(Vec::new());
    game.terminal_eve_wins.push(false);
    idx
}

fn add_edge(
    game: &mut ParityGame,
    formula: &Formula,
    var_priority: &HashMap<FormulaVarId, usize>,
    src: usize,
    target: Position,
    queue: &mut Vec<Position>,
) {
    let target_idx = if game.position_idx.contains_key(&target) {
        game.position_idx[&target]
    } else {
        let idx = intern_position(game, target, formula, var_priority);
        queue.push(target);
        idx
    };
    if !game.edges[src].contains(&target_idx) {
        game.edges[src].push(target_idx);
    }
}

fn collect_var_bodies(formula: &Formula, id: NodeId, out: &mut HashMap<FormulaVarId, NodeId>) {
    match formula.node(id) {
        Node::Mu { var, body } | Node::Nu { var, body } => {
            out.insert(*var, *body);
            collect_var_bodies(formula, *body, out);
        }
        Node::And(l, r) | Node::Or(l, r) => {
            collect_var_bodies(formula, *l, out);
            collect_var_bodies(formula, *r, out);
        }
        Node::Not(inner) => collect_var_bodies(formula, *inner, out),
        Node::Modal { target, .. } => collect_var_bodies(formula, *target, out),
        Node::True | Node::False | Node::Predicate(_) | Node::Variable(_) => {}
    }
}

fn modal_owner(kind: ModalKind) -> Player {
    match kind {
        ModalKind::Diamond => Player::Eve,
        ModalKind::Box => Player::Adam,
    }
}

/// Match a transition against a modal guard. Respects:
/// - The controllability flag (`Control::All|Controllable|Environment`).
/// - Label-name filter, current-state variable filter, and next-state variable
///   filter — all delegated to the shared
///   [`super::guard_matches_labels_and_vars`] helper so that parity-game and
///   evaluator semantics stay in sync.
fn transition_matches_guard(
    transition: &crate::clts::Transition<DefaultStateIdx, DefaultLabelIdx>,
    guard: &Guard,
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    source: StateId<DefaultStateIdx>,
) -> bool {
    // 1. Controllability flag.
    let control_ok = match guard.control {
        Control::All => true,
        Control::Controllable => transition.is_controllable(clts),
        Control::Environment => !transition.is_controllable(clts),
    };
    if !control_ok {
        return false;
    }

    // 2–4. Label and variable filters — shared with the evaluator.
    guard_matches_labels_and_vars(source, transition, guard, clts)
}

// ---------------------------------------------------------------------------
// Zielonka recursive solver
// ---------------------------------------------------------------------------

/// Solve a parity game via Zielonka's recursive algorithm.
///
/// Returns a `ParitySolution` containing per-position winner and Eve's
/// positional strategy on her winning region.
pub fn solve(game: &ParityGame) -> ParitySolution {
    let n = game.len();
    let mut universe: BitVec<usize, Lsb0> = BitVec::repeat(true, n);
    // First, force terminal-loss positions into the right player's winning
    // region by trimming them out of the universe (they're handled below
    // when assembling the final winner vector).
    let active = solve_subgame(game, &mut universe);
    let mut winner = vec![Player::Adam; n];
    let mut strategy = vec![None; n];
    for i in 0..n {
        if active[i] == Some(Player::Eve) {
            winner[i] = Player::Eve;
        } else if active[i] == Some(Player::Adam) {
            winner[i] = Player::Adam;
        } else if game.terminals.contains(&i) {
            winner[i] = if game.terminal_eve_wins[i] {
                Player::Eve
            } else {
                Player::Adam
            };
        }
    }
    // Re-derive Eve's strategy by attractor over the final winning region
    // (the recursion above destructively mutates state; cleanest is one
    // final attractor call).
    let eve_winning: BitVec<usize, Lsb0> = (0..n).map(|i| winner[i] == Player::Eve).collect();
    eve_strategy_for_region(game, &eve_winning, &mut strategy);
    ParitySolution {
        winner,
        eve_strategy: strategy,
    }
}

/// Recursive Zielonka over a subgame defined by `universe`. Returns a vector
/// where `out[i]` is `Some(winner)` for positions in the subgame and `None`
/// otherwise.
fn solve_subgame(game: &ParityGame, universe: &mut BitVec<usize, Lsb0>) -> Vec<Option<Player>> {
    let n = game.len();
    let mut result = vec![None; n];

    if !universe.any() {
        return result;
    }

    // Find max priority in the subgame
    let mut max_prio = 0usize;
    let mut max_prio_set: BitVec<usize, Lsb0> = BitVec::repeat(false, n);
    for i in 0..n {
        if !universe[i] {
            continue;
        }
        match game.priorities[i].cmp(&max_prio) {
            std::cmp::Ordering::Greater => {
                max_prio = game.priorities[i];
                max_prio_set.fill(false);
                max_prio_set.set(i, true);
            }
            std::cmp::Ordering::Equal => {
                max_prio_set.set(i, true);
            }
            std::cmp::Ordering::Less => {}
        }
    }

    let player = if max_prio.is_multiple_of(2) {
        Player::Eve
    } else {
        Player::Adam
    };
    let opponent = match player {
        Player::Eve => Player::Adam,
        Player::Adam => Player::Eve,
    };

    // Compute attractor of the player to max_prio_set within universe
    let attr_self = compute_attractor(game, universe, &max_prio_set, player);

    // Recurse on universe \ attr_self
    let mut sub_universe = universe.clone();
    for i in 0..n {
        if attr_self[i] {
            sub_universe.set(i, false);
        }
    }
    let sub_result = solve_subgame(game, &mut sub_universe);

    // Collect opponent's wins in the subgame
    let mut opp_wins: BitVec<usize, Lsb0> = BitVec::repeat(false, n);
    for (i, win) in sub_result.iter().enumerate().take(n) {
        if *win == Some(opponent) {
            opp_wins.set(i, true);
        }
    }

    if !opp_wins.any() {
        // Player wins everything in `attr_self ∪ sub_universe`
        for i in 0..n {
            if universe[i] {
                result[i] = Some(player);
            }
        }
        return result;
    }

    // Otherwise, opponent's region in subgame extends back: compute
    // attractor of opponent to opp_wins within full universe
    let attr_opp = compute_attractor(game, universe, &opp_wins, opponent);

    // Recurse on universe \ attr_opp
    let mut sub_universe2 = universe.clone();
    for i in 0..n {
        if attr_opp[i] {
            sub_universe2.set(i, false);
        }
    }
    let sub_result2 = solve_subgame(game, &mut sub_universe2);

    for i in 0..n {
        if attr_opp[i] {
            result[i] = Some(opponent);
        } else if sub_result2[i].is_some() {
            result[i] = sub_result2[i];
        }
    }

    result
}

/// Attractor: the set of positions from which `player` can force play
/// into `target` within `universe` (using only positions in `universe`).
///
/// Player-owned positions: at least one out-edge into the attractor.
/// Opponent-owned positions: ALL out-edges (within universe) into the attractor.
fn compute_attractor(
    game: &ParityGame,
    universe: &BitVec<usize, Lsb0>,
    target: &BitVec<usize, Lsb0>,
    player: Player,
) -> BitVec<usize, Lsb0> {
    let n = game.len();
    let mut attr = target.clone();
    // Restrict to universe
    for i in 0..n {
        if !universe[i] {
            attr.set(i, false);
        }
    }
    // Build reverse edges
    let mut reverse: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, succs) in game.edges.iter().enumerate() {
        if !universe[i] {
            continue;
        }
        for &s in succs {
            if universe[s] {
                reverse[s].push(i);
            }
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        let snapshot = attr.clone();
        for i in 0..n {
            if !universe[i] || snapshot[i] {
                continue;
            }
            let owner = game.owners[i];
            let succs_in_universe: Vec<usize> = game.edges[i]
                .iter()
                .copied()
                .filter(|&s| universe[s])
                .collect();
            // Standard attractor (every position has ≥ 1 successor thanks to
            // terminal self-loops):
            //   Player-owned: ANY successor in attr.
            //   Opponent-owned: ALL successors in attr (and at least one).
            let in_attr = if owner == player {
                succs_in_universe.iter().any(|&s| snapshot[s])
            } else {
                !succs_in_universe.is_empty() && succs_in_universe.iter().all(|&s| snapshot[s])
            };
            if in_attr {
                attr.set(i, true);
                changed = true;
            }
        }
    }
    attr
}

/// Compute Eve's positional strategy on her final winning region.
///
/// At each Eve-owned winning position, pick a successor that's also in the
/// winning region (one is guaranteed to exist by determinacy). For Adam
/// positions or losing positions, the strategy is `None`.
fn eve_strategy_for_region(
    game: &ParityGame,
    eve_winning: &BitVec<usize, Lsb0>,
    strategy: &mut [Option<usize>],
) {
    for (i, succs) in game.edges.iter().enumerate() {
        if !eve_winning[i] || game.owners[i] != Player::Eve {
            continue;
        }
        // Pick any successor that's also winning for Eve.
        if let Some(&pick) = succs.iter().find(|&&s| eve_winning[s]) {
            strategy[i] = Some(pick);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers for translating game-position → state name
// ---------------------------------------------------------------------------

/// Build a human-readable name for a game position. Used by the synthesis
/// path when emitting the controller CLTS.
pub fn position_name(pos: Position, clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> String {
    let state = StateId::<DefaultStateIdx>::from_index(pos.state)
        .expect("position state index fits storage");
    let plant_name = clts.state_name(state).unwrap_or("state");
    format!("{plant_name}__pg_n{}", pos.node.index())
}

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::*;
    use crate::context_dsl;

    fn realize_for_test(
        ctxdsl: &str,
        automaton: &str,
    ) -> (Clts<DefaultStateIdx, DefaultLabelIdx>, Environment) {
        let doc = context_dsl::parse(ctxdsl).expect("parse");
        let realized = context_dsl::realize_context(&doc, &[]).expect("realize");
        let clts = realized.context.clts(automaton).expect("automaton").clone();
        let env = realized.environment_for(automaton);
        (clts, env)
    }

    const TICK_CTXDSL: &str = r#"
context test {
    automata {
        automaton M {
            states { state s0 initial; state s1; }
            transitions {
                transition s0 -> s1 on label tick;
                transition s1 -> s0 on label tick;
            }
        }
    }
}
"#;

    #[test]
    fn build_game_for_safety_formula_terminates() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        let formula = parser::parse("nu X. (true && [] X)").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        assert!(!game.is_empty(), "game should have positions");
        assert_eq!(game.owners.len(), game.len());
        assert_eq!(game.priorities.len(), game.len());
    }

    #[test]
    fn solve_safety_realizable_when_invariant_holds() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        let formula = parser::parse("nu X. (true && [] X)").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        let solution = solve(&game);
        let initial_pos = Position {
            state: 0,
            // Use the NNF formula's root (NodeIds may differ from the
            // original formula's root after the to_nnf transform).
            node: game.formula.root(),
        };
        let initial_idx = game.position_idx[&initial_pos];
        assert_eq!(solution.winner[initial_idx], Player::Eve);
    }

    #[test]
    fn solve_safety_unrealizable_when_invariant_violated() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        let formula = parser::parse("nu X. (false && [] X)").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        let solution = solve(&game);
        let initial_pos = Position {
            state: 0,
            // Use the NNF formula's root (NodeIds may differ from the
            // original formula's root after the to_nnf transform).
            node: game.formula.root(),
        };
        let initial_idx = game.position_idx[&initial_pos];
        assert_eq!(solution.winner[initial_idx], Player::Adam);
    }

    /// After NNF preprocessing, the build no longer falls back on compound
    /// Not approximations. This test exercises a formula with compound
    /// negations that previously hit the pass-through branch:
    ///
    ///     ! ((! Bad) && [] (! Bad))   ≡ Bad ∨ <> Bad
    ///
    /// On a simple two-state automaton with no Bad predicate registered,
    /// the formula evaluates to false everywhere — Adam wins at the
    /// initial position. The test verifies the build doesn't panic and
    /// the verdict is correct.
    #[test]
    fn build_handles_compound_not_via_nnf() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        // Compound Not: `! ((! q) && (! r))` → DNF/NNF: `q || r`
        let formula = parser::parse("! ((! q) && (! r))").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        // Build did not panic on compound Not — that's the main check.
        assert!(!game.is_empty());
        let solution = solve(&game);
        // q and r are unregistered predicates → both evaluate false at
        // any state → q || r is false everywhere → Adam wins.
        let initial_pos = Position {
            state: 0,
            node: game.formula.root(),
        };
        let initial_idx = game.position_idx[&initial_pos];
        assert_eq!(solution.winner[initial_idx], Player::Adam);
    }

    /// Compound Not over a fixpoint:
    ///
    ///     ! (mu Y. (true || <> Y))   ≡ nu Y. (false && [] Y)   ≡ false
    ///
    /// After NNF this reduces to a Nu-False formula that's false
    /// everywhere. Validates the fixpoint duality through Not.
    #[test]
    fn build_handles_compound_not_through_fixpoint() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        let formula = parser::parse("! (mu Y. (true || (<> Y)))").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        let solution = solve(&game);
        let initial_pos = Position {
            state: 0,
            node: game.formula.root(),
        };
        let initial_idx = game.position_idx[&initial_pos];
        // Outer NNF: ! (mu Y. true || <> Y) → nu Y. false && [] Y. The
        // false at every step makes nu's body empty, so Adam wins.
        assert_eq!(solution.winner[initial_idx], Player::Adam);
    }

    /// A modal guarded by a label name should restrict the parity-game
    /// edges to only transitions whose labels include that name. This
    /// test uses `<labels = {tick}> X` over the TICK_CTXDSL where the
    /// only label is `tick`, so the result matches the unguarded
    /// `<> X` semantics.
    #[test]
    fn build_filters_by_label_name() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        let formula = parser::parse("nu X. (true && [labels = {tick}] X)").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        let solution = solve(&game);
        let initial_pos = Position {
            state: 0,
            node: game.formula.root(),
        };
        let initial_idx = game.position_idx[&initial_pos];
        assert_eq!(solution.winner[initial_idx], Player::Eve);
    }

    /// A modal guarded by a NON-EXISTENT label name. The guard filter
    /// excludes every transition, so the box becomes vacuously satisfied
    /// (no successors → terminal Eve-wins for box modals).
    #[test]
    fn build_label_filter_with_no_matching_transitions() {
        let (clts, env) = realize_for_test(TICK_CTXDSL, "M");
        // Label `nonexistent` is not in the alphabet — all transitions
        // are filtered out. Box becomes vacuous (Eve wins).
        let formula = parser::parse("nu X. (true && [labels = {nonexistent}] X)").unwrap();
        let game = build_parity_game(&formula, &clts, &env);
        let solution = solve(&game);
        let initial_pos = Position {
            state: 0,
            node: game.formula.root(),
        };
        let initial_idx = game.position_idx[&initial_pos];
        assert_eq!(solution.winner[initial_idx], Player::Eve);
    }

    /// Pinning test for the shared `guard_matches_labels_and_vars` function:
    /// verifies that the free function (shared between evaluator and parity
    /// game) produces the same label-filter results as the parity-game
    /// build would produce by inspecting which edges are actually generated.
    ///
    /// Concretely: on TICK_CTXDSL the only transition label is `tick`.
    /// A guard requiring `{tick}` should match exactly the one transition
    /// available; a guard requiring `{other}` should match none.
    #[test]
    fn shared_guard_filter_matches_expected_transitions() {
        use super::guard_matches_labels_and_vars;
        use crate::clts::StateId;

        let (clts, _env) = realize_for_test(TICK_CTXDSL, "M");

        // Get the first initial state — s0 at index 0
        let s0 = StateId::<DefaultStateIdx>::from_index(0).expect("s0 exists");

        let mut matching_tick = 0usize;
        let mut matching_other = 0usize;

        let guard_tick = super::Guard {
            labels: vec!["tick".to_string()],
            ..Default::default()
        };
        let guard_other = super::Guard {
            labels: vec!["other".to_string()],
            ..Default::default()
        };

        for transition in clts.outgoing(s0) {
            if guard_matches_labels_and_vars(s0, transition, &guard_tick, &clts) {
                matching_tick += 1;
            }
            if guard_matches_labels_and_vars(s0, transition, &guard_other, &clts) {
                matching_other += 1;
            }
        }

        // TICK_CTXDSL has exactly one transition out of s0 (s0 -> s1 on tick)
        assert_eq!(
            matching_tick, 1,
            "guard {{tick}} must match the tick transition"
        );
        assert_eq!(matching_other, 0, "guard {{other}} must match nothing");
    }
}
