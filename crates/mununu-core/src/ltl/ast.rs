//! LTL (Linear Temporal Logic) AST representation.
//!
//! This module defines the abstract syntax tree for LTL formulas, including
//! all temporal operators (G, F, X, U, W, R) and propositional operators.

/// LTL formula represented as an abstract syntax tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LtlFormula {
    // Atomic formulas
    /// Truth constant: `true`
    True,
    /// Falsity constant: `false`
    False,
    /// State predicate: `p`
    Predicate(String),

    // Propositional operators
    /// Negation: `!φ`
    Not(Box<LtlFormula>),
    /// Conjunction: `φ && ψ`
    And(Box<LtlFormula>, Box<LtlFormula>),
    /// Disjunction: `φ || ψ`
    Or(Box<LtlFormula>, Box<LtlFormula>),
    /// Implication: `φ -> ψ`
    Implies(Box<LtlFormula>, Box<LtlFormula>),

    // Temporal operators (basic)
    /// Next: `X φ` - In the next step, φ holds
    Next(Box<LtlFormula>),
    /// Always: `G φ` - Globally, φ always holds
    Always(Box<LtlFormula>),
    /// Eventually: `F φ` - Eventually φ (finally)
    Eventually(Box<LtlFormula>),
    /// Until: `φ U ψ` - φ holds until ψ happens (and ψ eventually happens)
    Until {
        left: Box<LtlFormula>,
        right: Box<LtlFormula>,
    },
    /// Weak until: `φ W ψ` - φ holds until ψ happens, or φ always holds
    WeakUntil {
        left: Box<LtlFormula>,
        right: Box<LtlFormula>,
    },
    /// Release: `φ R ψ` - ψ holds until φ releases it
    Release {
        left: Box<LtlFormula>,
        right: Box<LtlFormula>,
    },

    // Derived patterns (for convenience)
    /// Recurrence: `GF φ` - Infinitely often φ (always eventually)
    Recurrence(Box<LtlFormula>),
    /// Stabilization: `FG φ` - Eventually forever φ (stabilization)
    Stabilization(Box<LtlFormula>),
    /// Response: `G(φ -> F(ψ))` - Every request is eventually granted (responsiveness)
    Response {
        trigger: Box<LtlFormula>,
        response: Box<LtlFormula>,
    },
}

#[cfg(test)]
mod tests {
    use super::LtlFormula;

    #[test]
    fn test_ltl_ast_creation() {
        // Test atomic formulas
        let true_formula = LtlFormula::True;
        let false_formula = LtlFormula::False;
        let predicate = LtlFormula::Predicate("safe".to_string());

        assert!(matches!(true_formula, LtlFormula::True));
        assert!(matches!(false_formula, LtlFormula::False));
        assert!(matches!(predicate, LtlFormula::Predicate(ref s) if s == "safe"));

        // Test propositional operators
        let not_formula = LtlFormula::Not(Box::new(LtlFormula::Predicate("deadlock".to_string())));
        let and_formula = LtlFormula::And(
            Box::new(LtlFormula::Predicate("safe".to_string())),
            Box::new(LtlFormula::Predicate("bounded".to_string())),
        );
        let or_formula = LtlFormula::Or(
            Box::new(LtlFormula::Predicate("error".to_string())),
            Box::new(LtlFormula::Predicate("warning".to_string())),
        );
        let implies_formula = LtlFormula::Implies(
            Box::new(LtlFormula::Predicate("request".to_string())),
            Box::new(LtlFormula::Predicate("grant".to_string())),
        );

        assert!(matches!(not_formula, LtlFormula::Not(_)));
        assert!(matches!(and_formula, LtlFormula::And(_, _)));
        assert!(matches!(or_formula, LtlFormula::Or(_, _)));
        assert!(matches!(implies_formula, LtlFormula::Implies(_, _)));

        // Test temporal operators
        let next_formula = LtlFormula::Next(Box::new(LtlFormula::Predicate("alarm".to_string())));
        let always_formula =
            LtlFormula::Always(Box::new(LtlFormula::Predicate("safe".to_string())));
        let eventually_formula =
            LtlFormula::Eventually(Box::new(LtlFormula::Predicate("completed".to_string())));
        let until_formula = LtlFormula::Until {
            left: Box::new(LtlFormula::Predicate("request".to_string())),
            right: Box::new(LtlFormula::Predicate("grant".to_string())),
        };
        let weak_until_formula = LtlFormula::WeakUntil {
            left: Box::new(LtlFormula::Predicate("request".to_string())),
            right: Box::new(LtlFormula::Predicate("grant".to_string())),
        };
        let release_formula = LtlFormula::Release {
            left: Box::new(LtlFormula::Predicate("request".to_string())),
            right: Box::new(LtlFormula::Predicate("grant".to_string())),
        };

        assert!(matches!(next_formula, LtlFormula::Next(_)));
        assert!(matches!(always_formula, LtlFormula::Always(_)));
        assert!(matches!(eventually_formula, LtlFormula::Eventually(_)));
        assert!(matches!(until_formula, LtlFormula::Until { .. }));
        assert!(matches!(weak_until_formula, LtlFormula::WeakUntil { .. }));
        assert!(matches!(release_formula, LtlFormula::Release { .. }));

        // Test derived patterns
        let recurrence_formula =
            LtlFormula::Recurrence(Box::new(LtlFormula::Predicate("heartbeat".to_string())));
        let stabilization_formula =
            LtlFormula::Stabilization(Box::new(LtlFormula::Predicate("idle".to_string())));
        let response_formula = LtlFormula::Response {
            trigger: Box::new(LtlFormula::Predicate("request".to_string())),
            response: Box::new(LtlFormula::Predicate("grant".to_string())),
        };

        assert!(matches!(recurrence_formula, LtlFormula::Recurrence(_)));
        assert!(matches!(
            stabilization_formula,
            LtlFormula::Stabilization(_)
        ));
        assert!(matches!(response_formula, LtlFormula::Response { .. }));
    }

    #[test]
    fn test_ltl_ast_debug() {
        let formula = LtlFormula::Always(Box::new(LtlFormula::Predicate("safe".to_string())));
        let debug_str = format!("{:?}", formula);
        assert!(debug_str.contains("Always"));
        assert!(debug_str.contains("Predicate"));
        assert!(debug_str.contains("safe"));
    }

    #[test]
    fn test_ltl_ast_clone() {
        let formula = LtlFormula::And(
            Box::new(LtlFormula::Always(Box::new(LtlFormula::Predicate(
                "safe".to_string(),
            )))),
            Box::new(LtlFormula::Eventually(Box::new(LtlFormula::Predicate(
                "completed".to_string(),
            )))),
        );
        let cloned = formula.clone();
        assert_eq!(formula, cloned);
    }

    #[test]
    fn test_ltl_ast_equality() {
        let formula1 = LtlFormula::Predicate("safe".to_string());
        let formula2 = LtlFormula::Predicate("safe".to_string());
        let formula3 = LtlFormula::Predicate("unsafe".to_string());

        assert_eq!(formula1, formula2);
        assert_ne!(formula1, formula3);
    }
}
