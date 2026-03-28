use crate::context_dsl::ast::{Automaton, ContextDoc, MuFormula, TransitionDecl};

/// Helper for visiting common Context DSL AST structures.
pub(crate) struct AstTraverser;

impl AstTraverser {
    /// Invokes `visitor` for each μ-calculus formula in `doc`.
    pub fn visit_formulas<F, E>(doc: &ContextDoc, mut visitor: F) -> Result<(), E>
    where
        F: FnMut(&MuFormula) -> Result<(), E>,
    {
        for formula in &doc.mu_formulas {
            visitor(formula)?;
        }
        Ok(())
    }

    /// Invokes `visitor` for each transition in `automaton`.
    pub fn visit_transitions<F, E>(automaton: &Automaton, mut visitor: F) -> Result<(), E>
    where
        F: FnMut(&TransitionDecl) -> Result<(), E>,
    {
        for transition in &automaton.transitions {
            visitor(transition)?;
        }
        Ok(())
    }
}
