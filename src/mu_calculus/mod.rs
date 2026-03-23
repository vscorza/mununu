//! μ-Calculus infrastructure: intermediate representation, guards, and parser.
//!
//! This module provides the typed representation that other components (parsers,
//! evaluators, optimisers) can share when working with μ-calculus formulas that
//! reference CLTS labels, controllability metadata, and variable valuations.

pub mod evaluator;
pub mod invert;
mod memo;
pub mod parser;
pub mod simplify;

pub use evaluator::{
    Environment, EvalResult, EvaluationError, EvaluationOptions, evaluate, evaluate_with_options,
    evaluate_with_options_and_automaton,
};
pub use simplify::simplify;

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

#[cfg(test)]
mod tests {
    use super::parser;

    #[test]
    fn parses_basic_formula() {
        let parsed = parser::parse("true").expect("formula parses");
        let root = parsed.root();
        assert!(matches!(parsed.node(root), super::Node::True));
    }
}
