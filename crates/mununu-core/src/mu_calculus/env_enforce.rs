//! Environment-enforce rewrite (P2.5-E T1 — env-strategy generalization over μ-calculus).
//!
//! Rewrites a μ-calculus property `φ` into `env_enforce(φ)` — the 1-player CONTROL reading where the
//! ENVIRONMENT owns every transition (it PICKS the successor). Structurally: every `□` (Box, "for ALL
//! successors") becomes `◇` (Diamond, "the env picks A successor"); everything else (`μ`/`ν`, `∧`/`∨`/`¬`,
//! predicates, `◇`) is unchanged. Evaluating `env_enforce(φ)` over the model yields the WINNING REGION —
//! the states from which an environment strategy can make `φ` hold along the chosen trajectory — so
//! `init ⊆ W` ⟺ such a strategy EXISTS.
//!
//! This GENERALIZES the recoverability special case: `env_enforce(AG EF good) = νY.(μX.(good ∨ ◇X)) ∧ ◇Y`
//! — exactly the `env_maintain_region(EF good)` the bespoke `exact_env_strategy` hand-codes (which stays as
//! the differential oracle). Combined with the existing `exact_symbolic_verdict`, existence for ANY
//! μ-calculus property is `exact_symbolic_verdict(env_enforce(φ)) == Holds`.
//!
//! SOUND for the 1-player case (env = sole player): with all inputs one player `⟨ctrl⟩ = ◇ = ∃input`, so
//! the control LABEL is inert and only the `□→◇` structure matters — hence no `Control`-honoring in the
//! evaluator is needed for T1. The 2-player split (`∃ctrl ∀env`, distinguishing controllable from
//! adversarial inputs) is T3 (needs the ctrl/env input-cube partition + `Control`-aware evaluation).
//! `invert.rs` is the DUAL (opposite game / counter-strategy region); this is the same-polarity
//! successor-swap.

use super::{Formula, FormulaBuilder, FormulaVarId, ModalKind, Node, NodeId};
use std::collections::HashMap;

/// Rewrite `formula` into its environment-enforce form: every `□` becomes `◇` (the environment picks the
/// successor); `μ`/`ν`/`∧`/`∨`/`¬`/predicates/`◇` and every modal guard are structurally unchanged.
pub fn env_enforce(formula: &Formula) -> Formula {
    let mut builder = FormulaBuilder::default();
    let mut var_map: HashMap<FormulaVarId, FormulaVarId> = HashMap::new();
    let root = rewrite_node(formula, formula.root(), &mut builder, &mut var_map);
    builder.into_formula(root)
}

fn rewrite_node(
    formula: &Formula,
    node_id: NodeId,
    builder: &mut FormulaBuilder,
    var_map: &mut HashMap<FormulaVarId, FormulaVarId>,
) -> NodeId {
    match formula.node(node_id).clone() {
        Node::True => builder.push_node(Node::True),
        Node::False => builder.push_node(Node::False),
        Node::Predicate(p) => builder.push_node(Node::Predicate(p)),
        Node::Not(inner) => {
            let n = rewrite_node(formula, inner, builder, var_map);
            builder.push_node(Node::Not(n))
        }
        Node::And(a, b) => {
            let na = rewrite_node(formula, a, builder, var_map);
            let nb = rewrite_node(formula, b, builder, var_map);
            builder.push_node(Node::And(na, nb))
        }
        Node::Or(a, b) => {
            let na = rewrite_node(formula, a, builder, var_map);
            let nb = rewrite_node(formula, b, builder, var_map);
            builder.push_node(Node::Or(na, nb))
        }
        // The successor-swap: `□ → ◇` (the env picks a successor); `◇` stays `◇`. Guard preserved (in the
        // 1-player reading the control label is inert; a later 2-player pass would set it).
        Node::Modal {
            kind: _,
            guard,
            target,
        } => {
            let t = rewrite_node(formula, target, builder, var_map);
            builder.push_node(Node::Modal {
                kind: ModalKind::Diamond,
                guard,
                target: t,
            })
        }
        Node::Mu { var, body } => {
            let new_var = builder.push_var(formula.vars()[var.0].name.clone());
            var_map.insert(var, new_var);
            let new_body = rewrite_node(formula, body, builder, var_map);
            builder.push_node(Node::Mu {
                var: new_var,
                body: new_body,
            })
        }
        Node::Nu { var, body } => {
            let new_var = builder.push_var(formula.vars()[var.0].name.clone());
            var_map.insert(var, new_var);
            let new_body = rewrite_node(formula, body, builder, var_map);
            builder.push_node(Node::Nu {
                var: new_var,
                body: new_body,
            })
        }
        Node::Variable(var) => {
            let mapped = var_map.get(&var).copied().unwrap_or(var);
            builder.push_node(Node::Variable(mapped))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mu_calculus::parser;

    use super::super::{ModalKind, Node};

    fn count_modals(f: &Formula) -> (usize, usize) {
        let mut boxes = 0;
        let mut diamonds = 0;
        for n in f.nodes() {
            if let Node::Modal { kind, .. } = n {
                match kind {
                    ModalKind::Box => boxes += 1,
                    ModalKind::Diamond => diamonds += 1,
                }
            }
        }
        (boxes, diamonds)
    }

    #[test]
    fn env_enforce_flips_box_to_diamond_only() {
        // AG EF good = nu Y. ((mu X. (good || <> X)) && [] Y): 1 box (outer AG) + 1 diamond (inner EF).
        // env_enforce → the box becomes a diamond ⇒ 0 boxes, 2 diamonds; the mu/nu structure is intact.
        let f = parser::parse("nu Y. ((mu X. (good || <> X)) && [] Y)").unwrap();
        assert_eq!(count_modals(&f), (1, 1), "original: 1 box, 1 diamond");
        let e = env_enforce(&f);
        assert_eq!(count_modals(&e), (0, 2), "env_enforce: every box → diamond");
    }

    #[test]
    fn env_enforce_safety_invariant() {
        // AG !bad = nu X. ((! bad) && [] X). env_enforce → nu X. ((! bad) && <> X) = "env can stay safe".
        let f = parser::parse("nu X. ((! bad) && [] X)").unwrap();
        assert_eq!(
            count_modals(&env_enforce(&f)),
            (0, 1),
            "safety box → diamond"
        );
    }

    #[test]
    fn env_enforce_idempotent_on_diamond_only() {
        // A formula with no box: env_enforce is structurally a no-op (twice == once, same modal counts).
        let f = parser::parse("mu X. (good || <> X)").unwrap();
        assert_eq!(count_modals(&env_enforce(&f)), (0, 1));
        assert_eq!(count_modals(&env_enforce(&env_enforce(&f))), (0, 1));
    }
}
