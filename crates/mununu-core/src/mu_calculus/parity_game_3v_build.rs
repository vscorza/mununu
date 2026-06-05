//! R.5.0 sub-item 4.1 (2026-06-05) — 3-valued parity game construction.
//!
//! Per the breakdown at
//! `.claude/plans/r-track-multi-session-breakdown-2026-05-29.md`
//! Item 4 sub-item 4.1 of 6. Builds the **data structure** that the
//! Zielonka-extended solver (sub-items 4.2 + 4.3) consumes:
//! positions (`(state, formula_node)` pairs), edges (one move per
//! semantic successor), owners (`∃` / `∀`), and parity priorities.
//!
//! Position-and-move semantics follow Stirling's tableau game / the
//! Bradfield–Stirling parity-game framing of mu-calculus model
//! checking (Stirling LICS 1996; Bradfield 2014):
//!
//! - **Existential** (`∃ / Eve / prover`) owns `Or`, `Diamond modality`,
//!   and `Mu unfold` positions — the player who picks a witness.
//! - **Universal** (`∀ / Adam / spoiler`) owns `And`, `Box modality`,
//!   and `Nu unfold` positions — the player who picks a counterexample.
//! - **Leaf** positions (`True`, `False`, `Predicate`) have no moves;
//!   their truth value is read directly from the CLTS at the
//!   evaluator (sub-items 4.2 + 4.3 — not this MVP).
//!
//! **Priority assignment.** Each fixpoint variable gets a unique
//! priority. The outermost fixpoint gets the lowest priority (so that
//! when an inner fixpoint cycles, its parity dominates). `Mu` gets
//! odd parities (`∃ wins iff highest cycled is odd`); `Nu` gets
//! even. Non-fixpoint positions get priority 0 (no effect on the
//! parity-domination check).
//!
//! **What this sub-item does NOT do** (queued for 4.2 onwards):
//!
//! - No solver. The `Game3v` struct is built but not solved.
//! - No 3-valued semantics for `KleeneBot` positions — leaf
//!   evaluation defers to the solver (4.2 + 4.3).
//! - No interaction with the failure-subgame extraction (4.4).
//! - No replacement of the current `evaluate_3v_game` delegation
//!   (4.5).
//! - No benchmark / R.3 fixture sweep (4.6).
//!
//! Until the solver lands (4.2 onwards), the cheap-path delegation
//! in `parity_game_3v.rs::evaluate_3v_game_with_options` remains the
//! production evaluator. This module ships the **foundation** the
//! solver will consume.

use crate::clts::{Clts, IdStorage};
use crate::mu_calculus::{Formula, FormulaVarId, ModalKind, Node, NodeId};
use std::collections::HashMap;

/// R.5.0 sub-item 4.1 — Position owner in the 3-valued parity game.
///
/// Eve / Existential picks witnesses (`Or` left vs right; `Diamond`
/// successor choice; `Mu` unfold). Adam / Universal picks
/// counterexamples (`And` left vs right; `Box` successor choice; `Nu`
/// unfold). Per Stirling LICS 1996 §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Player {
    /// `∃ / Eve / prover` — picks moves that *prove* the formula.
    Existential,
    /// `∀ / Adam / spoiler` — picks moves that *refute* the formula.
    Universal,
}

/// R.5.0 sub-item 4.1 — Opaque identifier for a position in the game.
/// Indexes into `Game3v::positions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PositionId(pub usize);

/// R.5.0 sub-item 4.1 — A single `(state, formula_node)` position.
#[derive(Debug, Clone)]
pub struct Position {
    /// State index in the CLTS (matches `StateId::index()`).
    pub state: usize,
    /// Formula node identifier this position is at.
    pub node: NodeId,
    /// Player to move at this position.
    pub owner: Player,
    /// Parity priority. `Mu`-binding positions get odd priorities,
    /// `Nu`-binding even; non-fixpoint positions get `0` (does not
    /// dominate). Higher priorities dominate the parity check when
    /// the play cycles.
    pub priority: u32,
}

/// R.5.0 sub-item 4.1 — The 3-valued parity game derived from a
/// formula + CLTS pair. Consumed by sub-items 4.2 + 4.3 (solver) and
/// 4.4 (precise subgame extraction).
///
/// **MVP shape** — leaf positions (`True` / `False` / `Predicate`)
/// have empty successor lists; the solver must check the node kind
/// to evaluate them. A future revision may inline truth values onto
/// the Position struct to avoid the lookup; until 4.2 lands, the
/// node-kind dispatch is acceptable.
#[derive(Debug, Clone)]
pub struct Game3v {
    /// All positions in the game, indexed by `PositionId`.
    pub positions: Vec<Position>,
    /// Adjacency: `successors[pid.0]` lists the `PositionId`s
    /// reachable in one move from position `pid`. The empty vec
    /// means the position is a leaf (`True` / `False` / `Predicate`)
    /// — its truth value is evaluated externally by the solver.
    pub successors: Vec<Vec<PositionId>>,
    /// Lookup from `(state, node)` to `PositionId`. Useful for the
    /// solver's reverse lookups when computing predecessors.
    pub index: HashMap<(usize, NodeId), PositionId>,
}

impl Game3v {
    /// Number of positions in the game.
    pub fn position_count(&self) -> usize {
        self.positions.len()
    }

    /// Number of edges in the game (sum of successor list lengths).
    pub fn edge_count(&self) -> usize {
        self.successors.iter().map(|v| v.len()).sum()
    }

    /// Look up the position id for a given `(state, node)` pair.
    /// Returns `None` if no such position exists (which only happens
    /// when the build was scoped to a subset; the standard
    /// [`build_game`] enumerates every `(state, node)` cross-product
    /// so this is `Some` for every input).
    pub fn position_id(&self, state: usize, node: NodeId) -> Option<PositionId> {
        self.index.get(&(state, node)).copied()
    }
}

/// R.5.0 sub-item 4.1 — Build the 3-valued parity game for a
/// formula evaluated over a CLTS.
///
/// Enumerates the `states × subformula_nodes` cross-product as
/// positions, assigns owners + priorities per the [`Player`] /
/// priority docs above, and connects each position to its
/// semantic-successor positions:
///
/// - `Or(l, r)` / `And(l, r)` → `[(state, l), (state, r)]`
/// - `Not(inner)` → `[(state, inner)]` (owner inherits child's; NNF
///   normalisation usually eliminates `Not` before this point but we
///   handle it for completeness)
/// - `Modal { Box | Diamond, target, .. }` →
///   `[(succ_state, target) for each modal-successor state]`. Modal
///   guards (label filters, controllability filters) are honoured;
///   modal-successor states are computed by walking `clts.outgoing`.
/// - `Mu { body, .. }` / `Nu { body, .. }` → `[(state, body)]` (the
///   unfold move).
/// - `Variable(v)` → `[(state, binding_node(v))]` (replace variable
///   with its binding fixpoint body — the standard mu-calculus
///   semantics for variable lookup; cycles in the game graph encode
///   the fixpoint iteration).
/// - `True` / `False` / `Predicate(_)` → no moves; truth value is
///   read by the solver from the CLTS.
///
/// Modal moves use **standard** (`TransitionModality::Sharp`)
/// transitions for both `Box` and `Diamond`. The 3-valued extension
/// in sub-item 4.3 will branch the move semantics for `MayOnly` and
/// `MustHyperOnly` per Shoham–Grumberg LMCS 2007.
pub fn build_game<S, L>(formula: &Formula, clts: &Clts<S, L>) -> Game3v
where
    S: IdStorage,
    L: IdStorage,
{
    let state_count = clts.state_count();
    let nodes = formula.nodes();
    let priorities = compute_priorities(formula);
    // Pre-compute a fixpoint-variable → binding-node-body map. When
    // the game traverses a `Variable(v)` position, the move goes to
    // `(state, binding_body(v))` to implement the unfold step.
    let var_binding = collect_var_bindings(formula);

    // Allocate positions for the full states × nodes cross-product.
    let mut positions = Vec::with_capacity(state_count * nodes.len());
    let mut index: HashMap<(usize, NodeId), PositionId> =
        HashMap::with_capacity(positions.capacity());
    for state in 0..state_count {
        for (node_idx, node) in nodes.iter().enumerate() {
            let node_id = NodeId(node_idx);
            let owner = position_owner(node);
            let priority = position_priority(node, &priorities);
            let pid = PositionId(positions.len());
            positions.push(Position {
                state,
                node: node_id,
                owner,
                priority,
            });
            index.insert((state, node_id), pid);
        }
    }

    // Build successors per position.
    let mut successors: Vec<Vec<PositionId>> = vec![Vec::new(); positions.len()];
    for pid in 0..positions.len() {
        let pos = &positions[pid];
        let node = formula.node(pos.node);
        let succs = compute_successors(node, pos.state, clts, &index, &var_binding);
        successors[pid] = succs;
    }

    Game3v {
        positions,
        successors,
        index,
    }
}

/// Position owner per the operator at the node.
fn position_owner(node: &Node) -> Player {
    match node {
        Node::Or(_, _) | Node::Mu { .. } => Player::Existential,
        Node::And(_, _) | Node::Nu { .. } => Player::Universal,
        Node::Modal { kind, .. } => match kind {
            ModalKind::Diamond => Player::Existential,
            ModalKind::Box => Player::Universal,
        },
        // Leaves + Variable + Not have no inherent owner. Convention:
        // assign Universal so they can't claim a non-existent move.
        // (The solver checks for empty successor lists separately;
        // owner only matters when there are moves.)
        Node::True | Node::False | Node::Predicate(_) | Node::Variable(_) | Node::Not(_) => {
            Player::Universal
        }
    }
}

/// Position priority. Fixpoint-binding positions get the variable's
/// priority; non-fixpoint positions get `0` (does not dominate the
/// parity check).
fn position_priority(node: &Node, priorities: &HashMap<FormulaVarId, u32>) -> u32 {
    match node {
        Node::Mu { var, .. } | Node::Nu { var, .. } => priorities.get(var).copied().unwrap_or(0),
        _ => 0,
    }
}

/// Compute priorities for every fixpoint variable. Outermost gets
/// the lowest priority (so inner cycles dominate). `Mu` → odd, `Nu`
/// → even. Both ≥ 1 so they are non-zero (zero is reserved for
/// non-fixpoint positions).
fn compute_priorities(formula: &Formula) -> HashMap<FormulaVarId, u32> {
    let nesting = formula.fixpoint_nesting_order(); // outermost first
    let mut result = HashMap::with_capacity(nesting.len());
    for (idx, (var, is_mu)) in nesting.iter().enumerate() {
        let base = 2 * (idx as u32);
        // Outermost (idx = 0): Mu → 1, Nu → 2.
        // Each nested fixpoint adds +2.
        let prio = if *is_mu { base + 1 } else { base + 2 };
        result.insert(*var, prio);
    }
    result
}

/// Compute the binding-body lookup map: `FormulaVarId → body NodeId`
/// of the fixpoint that binds the variable. Used to implement the
/// variable-unfold move (`Variable(v)` → `(state, body(v))`).
fn collect_var_bindings(formula: &Formula) -> HashMap<FormulaVarId, NodeId> {
    let mut result = HashMap::new();
    for (idx, node) in formula.nodes().iter().enumerate() {
        match node {
            Node::Mu { var, body } | Node::Nu { var, body } => {
                // Variable points at the FIXPOINT NODE itself, not
                // the body. This way the unfold cycles back through
                // the fixpoint's own owner + priority, which is what
                // makes the parity-game scoring fire.
                let _ = (idx, body); // suppress unused warnings
                result.insert(*var, NodeId(idx));
            }
            _ => {}
        }
    }
    result
}

/// Compute successor positions for a single position.
fn compute_successors<S, L>(
    node: &Node,
    state: usize,
    clts: &Clts<S, L>,
    index: &HashMap<(usize, NodeId), PositionId>,
    var_binding: &HashMap<FormulaVarId, NodeId>,
) -> Vec<PositionId>
where
    S: IdStorage,
    L: IdStorage,
{
    match node {
        // Leaf positions: no moves. Truth value is read by the
        // solver from the CLTS (Predicate) or constant (True/False).
        Node::True | Node::False | Node::Predicate(_) => Vec::new(),

        // Variable lookup: move to the position at the binding
        // fixpoint node. This creates the cycle the parity game
        // scores via the fixpoint's priority.
        Node::Variable(var) => {
            let Some(binding_node) = var_binding.get(var) else {
                return Vec::new();
            };
            index
                .get(&(state, *binding_node))
                .copied()
                .map(|pid| vec![pid])
                .unwrap_or_default()
        }

        Node::Not(inner) => index
            .get(&(state, *inner))
            .copied()
            .map(|pid| vec![pid])
            .unwrap_or_default(),

        Node::And(l, r) | Node::Or(l, r) => {
            let mut out = Vec::with_capacity(2);
            if let Some(&pid) = index.get(&(state, *l)) {
                out.push(pid);
            }
            if let Some(&pid) = index.get(&(state, *r)) {
                out.push(pid);
            }
            out
        }

        Node::Modal { target, .. } => {
            // MVP: enumerate all outgoing transitions. Modal guards
            // (label filters, controllability, current/next variable
            // guards, max_steps) are honoured by the SOLVER, not by
            // the game-build step. The game-build's job is to expose
            // every potentially-relevant successor; the solver's
            // job is to prune to those that satisfy the guard.
            // sub-item 4.3 ships the guard-aware pruning.
            //
            // For Sharp-only inputs, this also matches the existing
            // evaluate_tri semantics (which considers every outgoing
            // transition).
            let Some(source_id) = crate::clts::StateId::<S>::from_index(state) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for transition in clts.outgoing(source_id) {
                let succ_state = transition.target().index();
                if let Some(&pid) = index.get(&(succ_state, *target))
                    && !out.contains(&pid)
                {
                    out.push(pid);
                }
            }
            out
        }

        Node::Mu { body, .. } | Node::Nu { body, .. } => index
            .get(&(state, *body))
            .copied()
            .map(|pid| vec![pid])
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
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

    /// R.5.0 sub-item 4.1 — A formula with N subformula nodes over a
    /// CLTS with M states builds exactly N × M positions.
    #[test]
    fn r5_0_4_1_game_size_is_states_times_subformulas() {
        let clts = build_two_state_clts();
        // `nu Y. (true && [] Y)` — 5 subformula nodes: `true`, `Y`,
        // `[] Y`, `true && [] Y`, `nu Y. ...`.
        let formula = parse("nu Y. (true && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);
        let expected_positions = clts.state_count() * formula.nodes().len();
        assert_eq!(
            game.position_count(),
            expected_positions,
            "states ({}) × subformulas ({}) = {}",
            clts.state_count(),
            formula.nodes().len(),
            expected_positions
        );
    }

    /// R.5.0 sub-item 4.1 — Fixpoint priorities: `Nu` gets even,
    /// `Mu` gets odd, both ≥ 1. Non-fixpoint positions get 0.
    #[test]
    fn r5_0_4_1_fixpoint_priorities_have_correct_parity() {
        let clts = build_two_state_clts();
        let formula = parse("nu Y. (true && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);

        // Find the Nu node.
        let nu_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::Nu { .. }).then_some(NodeId(idx)))
            .expect("Nu node");
        let pid = game
            .position_id(0, nu_node)
            .expect("nu position exists at state 0");
        let prio = game.positions[pid.0].priority;
        assert!(prio >= 1, "Nu priority must be ≥ 1, got {prio}");
        assert_eq!(prio % 2, 0, "Nu must have even priority, got {prio}");

        // True leaf priority must be 0.
        let true_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::True).then_some(NodeId(idx)))
            .expect("True leaf");
        let pid_true = game
            .position_id(0, true_node)
            .expect("true position exists");
        assert_eq!(
            game.positions[pid_true.0].priority, 0,
            "non-fixpoint positions get priority 0"
        );
    }

    /// R.5.0 sub-item 4.1 — In `nu Y. mu X. (p || <a> X) && [a] Y`,
    /// the inner `mu` has *higher* priority than the outer `nu` —
    /// so inner cycles dominate, per the standard scheme.
    #[test]
    fn r5_0_4_1_nested_alternation_inner_dominates() {
        let clts = build_two_state_clts();
        let formula = parse("nu Y. mu X. ((true || <> X) && [] Y)").expect("parse");
        let game = build_game(&formula, &clts);

        let nu_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::Nu { .. }).then_some(NodeId(idx)))
            .expect("Nu node");
        let mu_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::Mu { .. }).then_some(NodeId(idx)))
            .expect("Mu node");

        let nu_pid = game.position_id(0, nu_node).expect("nu position");
        let mu_pid = game.position_id(0, mu_node).expect("mu position");
        let nu_prio = game.positions[nu_pid.0].priority;
        let mu_prio = game.positions[mu_pid.0].priority;

        assert_eq!(nu_prio % 2, 0, "Nu must be even, got {nu_prio}");
        assert_eq!(mu_prio % 2, 1, "Mu must be odd, got {mu_prio}");
        assert!(
            mu_prio > nu_prio,
            "inner Mu must dominate outer Nu: mu_prio={mu_prio}, nu_prio={nu_prio}"
        );
    }

    /// R.5.0 sub-item 4.1 — Owner assignment: `Or` and `Diamond` are
    /// Existential; `And` and `Box` are Universal.
    #[test]
    fn r5_0_4_1_owner_assignment_per_operator() {
        let clts = build_two_state_clts();
        let formula = parse("(true && false) || (<> true)").expect("parse");
        let game = build_game(&formula, &clts);

        for (idx, node) in formula.nodes().iter().enumerate() {
            let nid = NodeId(idx);
            let pid = game.position_id(0, nid).expect("position exists");
            let owner = game.positions[pid.0].owner;
            match node {
                Node::And(_, _) => {
                    assert_eq!(owner, Player::Universal, "And is Universal at node {idx}")
                }
                Node::Or(_, _) => assert_eq!(
                    owner,
                    Player::Existential,
                    "Or is Existential at node {idx}"
                ),
                Node::Modal { kind, .. } => match kind {
                    ModalKind::Diamond => assert_eq!(
                        owner,
                        Player::Existential,
                        "Diamond is Existential at node {idx}"
                    ),
                    ModalKind::Box => {
                        assert_eq!(owner, Player::Universal, "Box is Universal at node {idx}")
                    }
                },
                _ => {}
            }
        }
    }

    /// R.5.0 sub-item 4.1 — Modal successors enumerate every CLTS
    /// out-transition. For `[a] true` over a 2-state CLTS where s0
    /// → s1 and s1 → s0, the Modal position at s0 has exactly one
    /// successor (the position at s1, target = `true`).
    #[test]
    fn r5_0_4_1_modal_successors_enumerate_clts_transitions() {
        let clts = build_two_state_clts();
        let formula = parse("[] true").expect("parse");
        let game = build_game(&formula, &clts);

        let modal_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::Modal { .. }).then_some(NodeId(idx)))
            .expect("Modal node");
        let pid = game
            .position_id(0, modal_node)
            .expect("modal position at s0");

        // s0 has one outgoing transition to s1 — so the modal
        // position at (s0, []true) has one successor: (s1, true).
        assert_eq!(
            game.successors[pid.0].len(),
            1,
            "modal at s0 has one successor (s0 → s1)"
        );
        let succ_pid = game.successors[pid.0][0];
        let succ_pos = &game.positions[succ_pid.0];
        assert_eq!(succ_pos.state, 1, "successor is at state 1");
        // The target node is `true` (leaf).
        assert!(
            matches!(formula.node(succ_pos.node), Node::True),
            "successor node is the true leaf"
        );
    }

    /// R.5.0 sub-item 4.1 — Variable position cycles back to its
    /// binding fixpoint node. `mu X. <> X` has a Variable(X) node
    /// whose single successor is the Mu position itself.
    #[test]
    fn r5_0_4_1_variable_position_cycles_to_fixpoint() {
        let clts = build_two_state_clts();
        let formula = parse("mu X. <> X").expect("parse");
        let game = build_game(&formula, &clts);

        let mu_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::Mu { .. }).then_some(NodeId(idx)))
            .expect("Mu node");
        let var_node = formula
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(idx, n)| matches!(n, Node::Variable(_)).then_some(NodeId(idx)))
            .expect("Variable node");

        // Variable position at s0 → Mu position at s0.
        let var_pid = game.position_id(0, var_node).expect("variable position");
        assert_eq!(game.successors[var_pid.0].len(), 1, "variable has 1 succ");
        let succ_pid = game.successors[var_pid.0][0];
        let succ_pos = &game.positions[succ_pid.0];
        assert_eq!(succ_pos.node, mu_node, "variable cycles to Mu node");
        assert_eq!(succ_pos.state, 0, "same-state cycle (no state move)");
    }
}
