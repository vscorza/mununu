//! μ-Calculus infrastructure: intermediate representation, guards, and parser.
//!
//! This module provides the typed representation that other components (parsers,
//! evaluators, optimisers) can share when working with μ-calculus formulas that
//! reference CLTS labels, controllability metadata, and variable valuations.

use crate::clts::{Clts, IdStorage, StateId, Transition};

pub mod evaluator;
pub mod invert;
mod memo;
pub mod nnf;
pub mod parity_game;
pub mod parity_game_3v;
pub mod parser;
pub mod simplify;
pub mod trit;
pub mod truth_domain;

pub use evaluator::{
    ApproximantView, Environment, EvalResult, EvaluationError, EvaluationOptions,
    FixpointConvergenceCallback, FixpointPolarity, PriorApproximant, Signature, WitnessMap,
    evaluate, evaluate_tri, evaluate_tri_with_options, evaluate_with_options,
    evaluate_with_options_and_automaton, evaluate_with_witnesses,
};
pub use parity_game_3v::{
    FailureSubgame, GameEvaluation, Position3v, clts_has_hyper_must_transitions,
    clts_has_non_sharp_transitions, evaluate_3v_game, evaluate_3v_game_with_options,
};
pub use simplify::simplify;
pub use trit::{Trit, TritSet};
pub use truth_domain::{BoolDomain, KleeneDomain, TruthDomain};

/// Classification of a μ-calculus formula by its fixpoint structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyClass {
    /// Pure greatest-fixpoint (nu-only): invariance / safety properties.
    Safety,
    /// Pure least-fixpoint (mu-only): reachability / guarantee properties.
    Reachability,
    /// Mixed fixpoints with alternation depth ≥ 2: liveness / fairness / GR(1).
    Liveness,
    /// Has fixpoints but no alternation (depth 1, mixed mu/nu at same level).
    Mixed,
    /// No fixpoints at all (propositional).
    Propositional,
}

/// Convenience methods for inspecting and traversing [`Node`] values.
pub trait NodeOps {
    /// Returns `true` if this node is a fixpoint (`mu` or `nu`).
    fn is_fixpoint(&self) -> bool;
}

/// Identifier referencing a node inside a [`Formula`] arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub(crate) usize);

impl NodeId {
    /// Returns the zero-based index for this node within the arena.
    pub fn index(self) -> usize {
        self.0
    }
}

/// Identifier assigned to a bound fixpoint variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FormulaVarId(pub(crate) usize);

impl FormulaVarId {
    /// Returns the zero-based index of the fixpoint variable inside the formula.
    pub fn index(self) -> usize {
        self.0
    }
}

/// Complete μ-calculus formula captured as an arena of nodes plus fixpoint metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formula {
    root: NodeId,
    nodes: Vec<Node>,
    vars: Vec<FormulaVar>,
}

impl Formula {
    /// Creates a new formula from its constituent arena.
    pub(crate) fn new(root: NodeId, nodes: Vec<Node>, vars: Vec<FormulaVar>) -> Self {
        Self { root, nodes, vars }
    }

    /// Returns the root node identifier.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// Provides read-only access to all owned nodes.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Returns the node associated with `id`.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    /// Provides read-only access to the fixpoint variable table.
    pub fn vars(&self) -> &[FormulaVar] {
        &self.vars
    }

    /// Returns metadata for the requested fixpoint variable.
    pub fn var(&self, id: FormulaVarId) -> &FormulaVar {
        &self.vars[id.0]
    }

    /// Returns fixpoint variables in nesting order (outermost first) with their
    /// polarity. This defines the signature structure for strategy extraction.
    ///
    /// Each entry is `(FormulaVarId, is_mu)` where `is_mu = true` for least
    /// fixpoints and `false` for greatest fixpoints. The ordering determines
    /// significance in the lexicographic signature comparison: earlier entries
    /// (outer fixpoints) are more significant.
    pub fn fixpoint_nesting_order(&self) -> Vec<(FormulaVarId, bool)> {
        let mut result = Vec::new();
        self.collect_fixpoint_order(self.root, &mut result);
        result
    }

    /// Returns the FormulaVarIds of all mu-fixpoints in nesting order
    /// (outermost first). These are the "obligations" a memory-aware
    /// controller rotates through to ensure each mu-fixpoint receives fair
    /// progress under alternation. Used by `ControllerMode::ProductGame`.
    pub fn mu_obligations(&self) -> Vec<FormulaVarId> {
        self.fixpoint_nesting_order()
            .into_iter()
            .filter_map(|(v, is_mu)| if is_mu { Some(v) } else { None })
            .collect()
    }

    fn collect_fixpoint_order(&self, id: NodeId, out: &mut Vec<(FormulaVarId, bool)>) {
        match self.node(id) {
            Node::Mu { var, body } => {
                out.push((*var, true));
                self.collect_fixpoint_order(*body, out);
            }
            Node::Nu { var, body } => {
                out.push((*var, false));
                self.collect_fixpoint_order(*body, out);
            }
            Node::And(l, r) | Node::Or(l, r) => {
                self.collect_fixpoint_order(*l, out);
                self.collect_fixpoint_order(*r, out);
            }
            Node::Not(inner) => self.collect_fixpoint_order(*inner, out),
            Node::Modal { target, .. } => self.collect_fixpoint_order(*target, out),
            Node::True | Node::False | Node::Predicate(_) | Node::Variable(_) => {}
        }
    }

    /// Computes the alternation depth of the formula.
    ///
    /// Alternation depth counts the maximum nesting of mu inside nu (or vice
    /// versa). Depth 0 = no fixpoints, depth 1 = single-type fixpoints (safety
    /// or reachability), depth 2+ = requires memory for correct strategy
    /// extraction (liveness, GR(1), parity).
    pub fn alternation_depth(&self) -> usize {
        self.ad_node(self.root, &mut vec![None; self.nodes.len()])
    }

    /// Recursive helper for alternation depth computation.
    /// `cache[node_index]` caches computed depths.
    fn ad_node(&self, id: NodeId, cache: &mut Vec<Option<usize>>) -> usize {
        if let Some(d) = cache[id.0] {
            return d;
        }
        let depth = match &self.nodes[id.0] {
            Node::True | Node::False | Node::Predicate(_) | Node::Variable(_) => 0,
            Node::Not(inner) => self.ad_node(*inner, cache),
            Node::And(l, r) | Node::Or(l, r) => {
                self.ad_node(*l, cache).max(self.ad_node(*r, cache))
            }
            Node::Modal { target, .. } => self.ad_node(*target, cache),
            Node::Mu { body, .. } => {
                let body_depth = self.ad_node(*body, cache);
                // Check if the body contains a nu — that's an alternation
                if self.contains_opposite_fixpoint(*body, true) {
                    body_depth.max(1) + 1
                } else {
                    body_depth.max(1)
                }
            }
            Node::Nu { body, .. } => {
                let body_depth = self.ad_node(*body, cache);
                // Check if the body contains a mu — that's an alternation
                if self.contains_opposite_fixpoint(*body, false) {
                    body_depth.max(1) + 1
                } else {
                    body_depth.max(1)
                }
            }
        };
        cache[id.0] = Some(depth);
        depth
    }

    /// Returns true if the subtree rooted at `id` contains a fixpoint of the
    /// opposite kind. `looking_for_nu = true` means we're inside a mu and
    /// looking for a nu (and vice versa).
    fn contains_opposite_fixpoint(&self, id: NodeId, looking_for_nu: bool) -> bool {
        match &self.nodes[id.0] {
            Node::True | Node::False | Node::Predicate(_) | Node::Variable(_) => false,
            Node::Not(inner) => self.contains_opposite_fixpoint(*inner, looking_for_nu),
            Node::And(l, r) | Node::Or(l, r) => {
                self.contains_opposite_fixpoint(*l, looking_for_nu)
                    || self.contains_opposite_fixpoint(*r, looking_for_nu)
            }
            Node::Modal { target, .. } => self.contains_opposite_fixpoint(*target, looking_for_nu),
            Node::Nu { .. } if looking_for_nu => true,
            Node::Mu { .. } if !looking_for_nu => true,
            Node::Mu { body, .. } | Node::Nu { body, .. } => {
                self.contains_opposite_fixpoint(*body, looking_for_nu)
            }
        }
    }

    /// Classifies this formula into a [`PropertyClass`] based on its fixpoint
    /// structure.
    pub fn property_class(&self) -> PropertyClass {
        let (has_mu, has_nu) = self.fixpoint_kinds(self.root);
        let ad = self.alternation_depth();

        match (has_mu, has_nu, ad) {
            (false, false, _) => PropertyClass::Propositional,
            (false, true, _) => PropertyClass::Safety,
            (true, false, _) => PropertyClass::Reachability,
            (true, true, d) if d >= 2 => PropertyClass::Liveness,
            _ => PropertyClass::Mixed,
        }
    }

    /// Returns (has_mu, has_nu) for the subtree rooted at `id`.
    fn fixpoint_kinds(&self, id: NodeId) -> (bool, bool) {
        match &self.nodes[id.0] {
            Node::True | Node::False | Node::Predicate(_) | Node::Variable(_) => (false, false),
            Node::Not(inner) => self.fixpoint_kinds(*inner),
            Node::And(l, r) | Node::Or(l, r) => {
                let (lm, ln) = self.fixpoint_kinds(*l);
                let (rm, rn) = self.fixpoint_kinds(*r);
                (lm || rm, ln || rn)
            }
            Node::Modal { target, .. } => self.fixpoint_kinds(*target),
            Node::Mu { body, .. } => {
                let (_, bn) = self.fixpoint_kinds(*body);
                (true, bn)
            }
            Node::Nu { body, .. } => {
                let (bm, _) = self.fixpoint_kinds(*body);
                (bm, true)
            }
        }
    }
}

/// Arena entry describing a single fixpoint variable (name only for now).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaVar {
    pub name: String,
}

/// Typed μ-calculus node inside a [`Formula`] arena.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    True,
    False,
    Predicate(String),
    Variable(FormulaVarId),
    Not(NodeId),
    And(NodeId, NodeId),
    Or(NodeId, NodeId),
    Modal {
        kind: ModalKind,
        guard: Guard,
        target: NodeId,
    },
    Mu {
        var: FormulaVarId,
        body: NodeId,
    },
    Nu {
        var: FormulaVarId,
        body: NodeId,
    },
}

impl NodeOps for Node {
    #[inline]
    fn is_fixpoint(&self) -> bool {
        matches!(self, Node::Mu { .. } | Node::Nu { .. })
    }
}

/// Distinguishes box vs. diamond modalities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    Box,
    Diamond,
}

/// Controllability guard applied to modalities.
///
/// - `All`: no controllability distinction (all transitions treated equally)
/// - `Controllable`: system perspective — all uncontrollable must satisfy, some controllable must
/// - `Environment`: environment perspective (dual of Controllable) — some uncontrollable satisfies OR all controllable satisfy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Control {
    #[default]
    All,
    Controllable,
    Environment,
}

/// Variable guard describing required/forbidden symbols.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VariableGuard {
    pub required: Vec<String>,
    pub forbidden: Vec<String>,
}

/// Aggregated guard information for label/variable/controllability constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub labels: Vec<String>,
    pub current: VariableGuard,
    pub next: VariableGuard,
    pub control: Control,
    pub max_steps: Option<u32>,
}

impl Default for Guard {
    fn default() -> Self {
        Self {
            labels: Vec::new(),
            current: VariableGuard::default(),
            next: VariableGuard::default(),
            control: Control::All,
            max_steps: None,
        }
    }
}

/// Builder for constructing μ-calculus formulas programmatically.
#[derive(Debug, Default)]
pub struct FormulaBuilder {
    nodes: Vec<Node>,
    vars: Vec<FormulaVar>,
}

impl FormulaBuilder {
    /// Adds a node to the formula and returns its identifier.
    pub fn push_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        id
    }

    /// Adds a fixpoint variable to the formula and returns its identifier.
    pub fn push_var(&mut self, name: String) -> FormulaVarId {
        let id = FormulaVarId(self.vars.len());
        self.vars.push(FormulaVar { name });
        id
    }

    /// Returns the node associated with the given identifier.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    /// Consumes the builder and creates a formula with the given root node.
    pub fn into_formula(self, root: NodeId) -> Formula {
        Formula::new(root, self.nodes, self.vars)
    }
}

// ---------------------------------------------------------------------------
// Shared guard filter — used by both the evaluator and the parity game
// ---------------------------------------------------------------------------

/// Returns `true` if `transition` passes the label and state-variable sub-filters
/// of `guard`.
///
/// This is the common "phase 2–4" filter shared by:
/// - [`evaluator::EvalContext::guard_matches`] (which additionally handles
///   controllability at the `eval_modal` dispatch level before calling here).
/// - [`parity_game::transition_matches_guard`] (which checks controllability
///   upfront, then delegates here for the label and variable filters).
///
/// Checks performed (in order):
/// 1. **Label-name filter**: every name in `guard.labels` must appear in at
///    least one of the transition's label bitsets.
/// 2. **Current-state variable filter**: `guard.current.required` names must
///    all be present in the source state's variable set; `guard.current.forbidden`
///    names must all be absent.
/// 3. **Next-state variable filter**: same logic applied to the target state.
///
/// Controllability (`guard.control`) is **not** checked here. Callers are
/// responsible for controllability filtering before calling this function.
pub(crate) fn guard_matches_labels_and_vars<S, L>(
    source: StateId<S>,
    transition: &Transition<S, L>,
    guard: &Guard,
    clts: &Clts<S, L>,
) -> bool
where
    S: IdStorage,
    L: IdStorage,
{
    // 1. Label-name filter: every required label must appear in at least one
    //    of the transition's label bitsets.
    if !guard.labels.is_empty() {
        for required in &guard.labels {
            let found = transition.labels().iter().any(|label_id| {
                clts.label_bitset(*label_id)
                    .is_some_and(|bitset| bitset.test(required.as_str()))
            });
            if !found {
                return false;
            }
        }
    }

    // 2. Current-state variable filter.
    if !guard.current.required.is_empty() || !guard.current.forbidden.is_empty() {
        let state_vars = clts.state_variable_bitset(source);
        if guard
            .current
            .required
            .iter()
            .any(|var| !state_vars.contains(var.as_str()))
        {
            return false;
        }
        if guard
            .current
            .forbidden
            .iter()
            .any(|var| state_vars.contains(var.as_str()))
        {
            return false;
        }
    }

    // 3. Next-state variable filter.
    if !guard.next.required.is_empty() || !guard.next.forbidden.is_empty() {
        let target_vars = clts.state_variable_bitset(transition.target());
        if guard
            .next
            .required
            .iter()
            .any(|var| !target_vars.contains(var.as_str()))
        {
            return false;
        }
        if guard
            .next
            .forbidden
            .iter()
            .any(|var| target_vars.contains(var.as_str()))
        {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::PropertyClass;
    use super::parser;

    #[test]
    fn parses_basic_formula() {
        let parsed = parser::parse("true").expect("formula parses");
        let root = parsed.root();
        assert!(matches!(parsed.node(root), super::Node::True));
    }

    #[test]
    fn alternation_depth_propositional() {
        let f = parser::parse("true && !false").unwrap();
        assert_eq!(f.alternation_depth(), 0);
        assert_eq!(f.property_class(), PropertyClass::Propositional);
    }

    #[test]
    fn alternation_depth_safety() {
        // nu X. (p && [] X) — pure greatest fixpoint, depth 1
        let f = parser::parse("nu X. (p && [] X)").unwrap();
        assert_eq!(f.alternation_depth(), 1);
        assert_eq!(f.property_class(), PropertyClass::Safety);
    }

    #[test]
    fn alternation_depth_reachability() {
        // mu X. (p || <> X) — pure least fixpoint, depth 1
        let f = parser::parse("mu X. (p || <> X)").unwrap();
        assert_eq!(f.alternation_depth(), 1);
        assert_eq!(f.property_class(), PropertyClass::Reachability);
    }

    #[test]
    fn alternation_depth_liveness() {
        // nu X. (mu Y. (p || <> Y)) && [] X — mu inside nu, depth 2
        let f = parser::parse("nu X. ((mu Y. (p || <> Y)) && [] X)").unwrap();
        assert_eq!(f.alternation_depth(), 2);
        assert_eq!(f.property_class(), PropertyClass::Liveness);
    }

    #[test]
    fn alternation_depth_nested_same_kind() {
        // nu X. (nu Y. (p && [] Y) && [] X) — nested nu, no alternation, depth 1
        let f = parser::parse("nu X. ((nu Y. (p && [] Y)) && [] X)").unwrap();
        assert_eq!(f.alternation_depth(), 1);
        assert_eq!(f.property_class(), PropertyClass::Safety);
    }

    #[test]
    fn mu_obligations_returns_all_mu_vars_in_nesting_order() {
        // GR(1)-like: nu X. ((mu Y1. (A || <> Y1)) && (mu Y2. (B || <> Y2)) && [] X)
        let f = parser::parse("nu X. ((mu Y1. (A || <> Y1)) && (mu Y2. (B || <> Y2)) && [] X)")
            .unwrap();
        let obligations = f.mu_obligations();
        assert_eq!(
            obligations.len(),
            2,
            "two mu-fixpoints (Y1, Y2) expected as obligations"
        );
        // Confirm the names are the mu vars (Y1 outer, Y2 inner) — order is
        // outermost-first per fixpoint_nesting_order.
        let names: Vec<&str> = obligations
            .iter()
            .map(|v| f.var(*v).name.as_str())
            .collect();
        assert_eq!(names, vec!["Y1", "Y2"]);
    }

    #[test]
    fn mu_obligations_empty_for_pure_nu_safety() {
        let f = parser::parse("nu X. (p && [] X)").unwrap();
        assert!(f.mu_obligations().is_empty());
    }

    #[test]
    fn mu_obligations_single_for_pure_reachability() {
        let f = parser::parse("mu X. (p || <> X)").unwrap();
        assert_eq!(f.mu_obligations().len(), 1);
    }
}
