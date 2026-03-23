//! Formula inversion for counterstrategy generation.
//!
//! Applies De Morgan's laws and fixpoint duality to produce the logical
//! negation of a μ-calculus formula. The inverted formula computes the
//! **environment's winning region** — the set of states where the environment
//! can force the original property to be violated.
//!
//! # Duality rules
//!
//! | Original               | Inverted                  |
//! |------------------------|---------------------------|
//! | `A && B`               | `¬A \|\| ¬B`             |
//! | `A \|\| B`             | `¬A && ¬B`               |
//! | `¬A`                   | `A`                       |
//! | `true`                 | `false`                   |
//! | `false`                | `true`                    |
//! | `P` (predicate)        | `¬P`                      |
//! | `mu X. Φ(X)`           | `nu X. ¬Φ(¬X)`           |
//! | `nu X. Φ(X)`           | `mu X. ¬Φ(¬X)`           |
//! | `[] Φ`                 | `<> ¬Φ`                   |
//! | `<> Φ`                 | `[] ¬Φ`                   |
//! | `[ctrl=controllable] Φ`| `<ctrl=environment> ¬Φ`   |
//! | `<ctrl=environment> Φ` | `[ctrl=controllable] ¬Φ`  |

use super::{Control, Formula, FormulaBuilder, FormulaVarId, Guard, ModalKind, Node, NodeId};
use std::collections::HashMap;

/// Produces the logical negation of `formula` by applying De Morgan's laws,
/// fixpoint duality, and modal operator flipping throughout the AST.
pub fn invert(formula: &Formula) -> Formula {
    let mut builder = FormulaBuilder::default();
    let mut var_map: HashMap<FormulaVarId, FormulaVarId> = HashMap::new();
    let root = invert_node(formula, formula.root(), &mut builder, &mut var_map);
    builder.into_formula(root)
}

fn invert_node(
    formula: &Formula,
    node_id: NodeId,
    builder: &mut FormulaBuilder,
    var_map: &mut HashMap<FormulaVarId, FormulaVarId>,
) -> NodeId {
    match formula.node(node_id).clone() {
        Node::True => builder.push_node(Node::False),
        Node::False => builder.push_node(Node::True),

        Node::Predicate(p) => {
            let inner = builder.push_node(Node::Predicate(p));
            builder.push_node(Node::Not(inner))
        }

        // Double negation: ¬(¬A) = A
        Node::Not(inner) => invert_node(formula, inner, builder, var_map),

        // De Morgan: ¬(A ∧ B) = ¬A ∨ ¬B
        Node::And(a, b) => {
            let na = invert_node(formula, a, builder, var_map);
            let nb = invert_node(formula, b, builder, var_map);
            builder.push_node(Node::Or(na, nb))
        }

        // De Morgan: ¬(A ∨ B) = ¬A ∧ ¬B
        Node::Or(a, b) => {
            let na = invert_node(formula, a, builder, var_map);
            let nb = invert_node(formula, b, builder, var_map);
            builder.push_node(Node::And(na, nb))
        }

        // Modal duality: ¬(□Φ) = ◇¬Φ, ¬(◇Φ) = □¬Φ
        // With game duality: Controllable ↔ Environment
        Node::Modal {
            kind,
            guard,
            target,
        } => {
            let new_target = invert_node(formula, target, builder, var_map);
            let new_kind = match kind {
                ModalKind::Box => ModalKind::Diamond,
                ModalKind::Diamond => ModalKind::Box,
            };
            let new_control = match guard.control {
                Control::All => Control::All,
                Control::Controllable => Control::Environment,
                Control::Environment => Control::Controllable,
            };
            let new_guard = Guard {
                control: new_control,
                ..guard
            };
            builder.push_node(Node::Modal {
                kind: new_kind,
                guard: new_guard,
                target: new_target,
            })
        }

        // Fixpoint duality: ¬(μX. Φ(X)) = νX. ¬Φ(¬X)
        Node::Mu { var, body } => {
            let new_var = builder.push_var(formula.vars()[var.0].name.clone());
            var_map.insert(var, new_var);
            let new_body = invert_node(formula, body, builder, var_map);
            builder.push_node(Node::Nu {
                var: new_var,
                body: new_body,
            })
        }

        // Fixpoint duality: ¬(νX. Φ(X)) = μX. ¬Φ(¬X)
        Node::Nu { var, body } => {
            let new_var = builder.push_var(formula.vars()[var.0].name.clone());
            var_map.insert(var, new_var);
            let new_body = invert_node(formula, body, builder, var_map);
            builder.push_node(Node::Mu {
                var: new_var,
                body: new_body,
            })
        }

        // Variable reference inside fixpoint body: keep as-is.
        // The fixpoint duality works by flipping mu↔nu (which changes the
        // starting point: empty vs full) and applying De Morgan to the body.
        // Variable references stay positive because the dual fixpoint's
        // iteration semantics already account for the negation via the
        // changed starting point and convergence direction.
        Node::Variable(var) => {
            let mapped_var = var_map.get(&var).copied().unwrap_or(var);
            builder.push_node(Node::Variable(mapped_var))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mu_calculus::parser;

    #[test]
    fn invert_safety_invariant() {
        // nu X. ([] X) → mu X. (<> (! X))
        let formula = parser::parse("nu X. ([] X)").unwrap();
        let inverted = invert(&formula);

        // Root should be Mu
        match inverted.node(inverted.root()) {
            Node::Mu { .. } => {}
            other => panic!("expected Mu, got {:?}", other),
        }
    }

    #[test]
    fn invert_reachability() {
        // mu X. (Full || <> X) → nu X. (! Full && [] (! X))
        let formula = parser::parse("mu X. (Full || <> X)").unwrap();
        let inverted = invert(&formula);

        // Root should be Nu
        match inverted.node(inverted.root()) {
            Node::Nu { .. } => {}
            other => panic!("expected Nu, got {:?}", other),
        }
    }

    #[test]
    fn invert_double_negation() {
        // ! (! p) → p
        let formula = parser::parse("! (! p)").unwrap();
        let inverted = invert(&formula);

        // Inverted of ¬(¬p) should be ¬p (since invert(¬(¬p)) = invert(p) = ¬p)
        // Wait: invert applies ¬ to the whole formula.
        // ¬(¬(¬p)) = ¬p... Actually invert(¬(¬p)):
        // invert(Not(Not(p))) = invert(p) = Not(Predicate("p"))
        match inverted.node(inverted.root()) {
            Node::Not(_) => {}
            other => panic!("expected Not(Predicate), got {:?}", other),
        }
    }

    #[test]
    fn invert_controllable_box() {
        // [ (ctrl = controllable) ] p → < (ctrl = environment) > (! p)
        let formula = parser::parse("[ (ctrl = controllable) ] p").unwrap();
        let inverted = invert(&formula);

        match inverted.node(inverted.root()) {
            Node::Modal { kind, guard, .. } => {
                assert_eq!(*kind, ModalKind::Diamond);
                assert_eq!(guard.control, Control::Environment);
            }
            other => panic!("expected Diamond with Environment, got {:?}", other),
        }
    }

    #[test]
    fn invert_game_reachability() {
        // mu X. (Full || [ (ctrl = controllable) ] X)
        // → nu X. (! Full && < (ctrl = environment) > (! X))
        let formula = parser::parse("mu X. (Full || [ (ctrl = controllable) ] X)").unwrap();
        let inverted = invert(&formula);

        // Root: Nu
        match inverted.node(inverted.root()) {
            Node::Nu { body, .. } => {
                // Body: And(Not(Full), Diamond(Environment, Not(X)))
                match inverted.node(*body) {
                    Node::And(_, _) => {}
                    other => panic!("expected And, got {:?}", other),
                }
            }
            other => panic!("expected Nu, got {:?}", other),
        }
    }
}
