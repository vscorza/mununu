//! Negation Normal Form (NNF) transform for mu-calculus formulas.
//!
//! Push every `Not` node down to the atom level (predicates or constants).
//! After NNF transformation, `Not` only appears immediately above
//! `Predicate(name)`, `True`, or `False` — never above compound nodes.
//!
//! Used as a preprocessing pass for the parity-game module, which needs
//! to reason structurally about negation positions in the formula. Without
//! NNF, compound negations like `¬(p ∧ q)` would have to be approximated.
//!
//! # Duality rules applied
//!
//! | Source                 | NNF form                  |
//! |------------------------|---------------------------|
//! | `¬true`                | `false`                   |
//! | `¬false`               | `true`                    |
//! | `¬¬φ`                  | `φ`                       |
//! | `¬(A ∧ B)`             | `¬A ∨ ¬B`                 |
//! | `¬(A ∨ B)`             | `¬A ∧ ¬B`                 |
//! | `¬[α]Φ`                | `<α>¬Φ`                   |
//! | `¬<α>Φ`                | `[α]¬Φ`                   |
//! | `¬μX. Φ(X)`            | `νX. ¬Φ(X)`               |
//! | `¬νX. Φ(X)`            | `μX. ¬Φ(X)`               |
//!
//! Modal control flags also dualize: `Controllable ↔ Environment`,
//! `All ↔ All`. This matches the existing `invert` module's logic but
//! exposes it as a standalone "transform without changing meaning"
//! operation, rather than "produce the negation".
//!
//! # Variable handling
//!
//! Mu/Nu binders rebind their variable; references inside the body keep
//! pointing at the new binder. The dual fixpoint inherits the same set
//! of variable references — the changed starting point (mu starts empty,
//! nu starts full) handles the polarity flip semantically. This matches
//! the convention used in `invert.rs`.

use std::collections::HashMap;

use super::{Control, Formula, FormulaBuilder, FormulaVarId, Guard, ModalKind, Node, NodeId};

/// Transform `formula` into negation normal form. The result is logically
/// equivalent to the input, with every `Not` pushed down to atomic
/// positions (`Predicate`, `True`, or `False`).
pub fn to_nnf(formula: &Formula) -> Formula {
    let mut builder = FormulaBuilder::default();
    let mut var_map: HashMap<FormulaVarId, FormulaVarId> = HashMap::new();
    let root = nnf_node(formula, formula.root(), false, &mut builder, &mut var_map);
    builder.into_formula(root)
}

fn nnf_node(
    formula: &Formula,
    node_id: NodeId,
    negate: bool,
    builder: &mut FormulaBuilder,
    var_map: &mut HashMap<FormulaVarId, FormulaVarId>,
) -> NodeId {
    match formula.node(node_id).clone() {
        Node::True => {
            if negate {
                builder.push_node(Node::False)
            } else {
                builder.push_node(Node::True)
            }
        }
        Node::False => {
            if negate {
                builder.push_node(Node::True)
            } else {
                builder.push_node(Node::False)
            }
        }
        Node::Predicate(p) => {
            if negate {
                let inner = builder.push_node(Node::Predicate(p));
                builder.push_node(Node::Not(inner))
            } else {
                builder.push_node(Node::Predicate(p))
            }
        }
        // ¬¬φ → φ — flip polarity and recurse without emitting a Not node.
        Node::Not(inner) => nnf_node(formula, inner, !negate, builder, var_map),
        Node::And(a, b) => {
            let na = nnf_node(formula, a, negate, builder, var_map);
            let nb = nnf_node(formula, b, negate, builder, var_map);
            if negate {
                // ¬(A ∧ B) → ¬A ∨ ¬B
                builder.push_node(Node::Or(na, nb))
            } else {
                builder.push_node(Node::And(na, nb))
            }
        }
        Node::Or(a, b) => {
            let na = nnf_node(formula, a, negate, builder, var_map);
            let nb = nnf_node(formula, b, negate, builder, var_map);
            if negate {
                // ¬(A ∨ B) → ¬A ∧ ¬B
                builder.push_node(Node::And(na, nb))
            } else {
                builder.push_node(Node::Or(na, nb))
            }
        }
        Node::Modal {
            kind,
            guard,
            target,
        } => {
            let new_target = nnf_node(formula, target, negate, builder, var_map);
            let (new_kind, new_control) = if negate {
                let k = match kind {
                    ModalKind::Box => ModalKind::Diamond,
                    ModalKind::Diamond => ModalKind::Box,
                };
                let c = match guard.control {
                    Control::All => Control::All,
                    Control::Controllable => Control::Environment,
                    Control::Environment => Control::Controllable,
                };
                (k, c)
            } else {
                (kind, guard.control)
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
        Node::Mu { var, body } => {
            let new_var = builder.push_var(formula.vars()[var.0].name.clone());
            var_map.insert(var, new_var);
            let new_body = nnf_node(formula, body, negate, builder, var_map);
            if negate {
                // ¬(μX. Φ(X)) → νX. ¬Φ(X)
                builder.push_node(Node::Nu {
                    var: new_var,
                    body: new_body,
                })
            } else {
                builder.push_node(Node::Mu {
                    var: new_var,
                    body: new_body,
                })
            }
        }
        Node::Nu { var, body } => {
            let new_var = builder.push_var(formula.vars()[var.0].name.clone());
            var_map.insert(var, new_var);
            let new_body = nnf_node(formula, body, negate, builder, var_map);
            if negate {
                // ¬(νX. Φ(X)) → μX. ¬Φ(X)
                builder.push_node(Node::Mu {
                    var: new_var,
                    body: new_body,
                })
            } else {
                builder.push_node(Node::Nu {
                    var: new_var,
                    body: new_body,
                })
            }
        }
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

    /// Walk the formula and confirm `Not` only appears immediately above
    /// atomic positions (Predicate / True / False).
    fn nnf_invariant(formula: &Formula) -> bool {
        for (idx, node) in formula.nodes().iter().enumerate() {
            if let Node::Not(inner_id) = node {
                let inner = formula.node(*inner_id);
                if !matches!(inner, Node::Predicate(_) | Node::True | Node::False) {
                    eprintln!(
                        "NNF violated: Not at index {idx} wraps non-atomic {:?}",
                        inner
                    );
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn nnf_idempotent_on_atom() {
        let f = parser::parse("p").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        assert!(matches!(nnf.node(nnf.root()), Node::Predicate(_)));
    }

    #[test]
    fn nnf_pushes_not_through_and() {
        let f = parser::parse("! (p && q)").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        // ¬(p ∧ q) → ¬p ∨ ¬q — root must be Or
        assert!(matches!(nnf.node(nnf.root()), Node::Or(_, _)));
    }

    #[test]
    fn nnf_pushes_not_through_or() {
        let f = parser::parse("! (p || q)").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        assert!(matches!(nnf.node(nnf.root()), Node::And(_, _)));
    }

    #[test]
    fn nnf_pushes_not_through_box() {
        let f = parser::parse("! ([] p)").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        // ¬[]p → <>¬p — root must be Diamond
        match nnf.node(nnf.root()) {
            Node::Modal { kind, .. } => assert!(matches!(kind, ModalKind::Diamond)),
            other => panic!("expected Diamond at root, got {other:?}"),
        }
    }

    #[test]
    fn nnf_pushes_not_through_diamond() {
        let f = parser::parse("! (<> p)").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        match nnf.node(nnf.root()) {
            Node::Modal { kind, .. } => assert!(matches!(kind, ModalKind::Box)),
            other => panic!("expected Box at root, got {other:?}"),
        }
    }

    #[test]
    fn nnf_dualizes_fixpoint_through_not() {
        // ¬(μX. p ∨ <>X) → νX. ¬p ∧ []X
        let f = parser::parse("! (mu X. (p || (<> X)))").unwrap();
        // sanity check the parse — root must be Not over a Mu
        match f.node(f.root()) {
            Node::Not(inner) => {
                assert!(matches!(f.node(*inner), Node::Mu { .. }));
            }
            other => panic!("expected ¬μ at root, got {other:?}"),
        }
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        match nnf.node(nnf.root()) {
            Node::Nu { .. } => {}
            other => panic!("expected Nu at root after NNF, got {other:?}"),
        }
    }

    #[test]
    fn nnf_double_negation_eliminated() {
        // ¬¬p → p
        let f = parser::parse("! (! p)").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        assert!(matches!(nnf.node(nnf.root()), Node::Predicate(_)));
    }

    #[test]
    fn nnf_passes_through_already_nnf_formula() {
        // nu X. ((!Bad) && [] X) is already in NNF
        let f = parser::parse("nu X. ((!Bad) && [] X)").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        assert!(matches!(nnf.node(nnf.root()), Node::Nu { .. }));
    }

    #[test]
    fn nnf_dualizes_modal_control_flag() {
        // ¬[(ctrl=Controllable)] p → <(ctrl=Environment)> ¬p
        let f = parser::parse("! [(ctrl=Controllable)] p").unwrap();
        let nnf = to_nnf(&f);
        assert!(nnf_invariant(&nnf));
        match nnf.node(nnf.root()) {
            Node::Modal { kind, guard, .. } => {
                assert!(matches!(kind, ModalKind::Diamond));
                assert!(matches!(guard.control, Control::Environment));
            }
            other => panic!("expected Diamond Environment modal, got {other:?}"),
        }
    }
}
