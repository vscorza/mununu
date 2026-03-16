//! Expression and guard expression evaluation.

use crate::guard::ComparisonOp;
use std::fmt;
use std::ops::Not;

/// Expression in guards and effects.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    Var(String),
    Const(i64),
    Bool(bool),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Creates a variable expression.
    pub fn var(name: impl Into<String>) -> Self {
        Self::Var(name.into())
    }

    /// Creates a constant expression.
    pub fn constant(value: i64) -> Self {
        Self::Const(value)
    }

    /// Creates a boolean expression.
    pub fn bool(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Var(name) => write!(f, "{}", name),
            Self::Const(val) => write!(f, "{}", val),
            Self::Bool(val) => write!(f, "{}", val),
            Self::Add(left, right) => write!(f, "({} + {})", left, right),
            Self::Sub(left, right) => write!(f, "({} - {})", left, right),
            Self::Mul(left, right) => write!(f, "({} * {})", left, right),
            Self::Div(left, right) => write!(f, "({} / {})", left, right),
        }
    }
}

/// Guard expression for transition guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardExpr {
    True,
    False,
    Comparison {
        left: Expr,
        op: ComparisonOp,
        right: Expr,
    },
    And(Box<GuardExpr>, Box<GuardExpr>),
    Or(Box<GuardExpr>, Box<GuardExpr>),
    Not(Box<GuardExpr>),
    Predicate(String),
}

impl GuardExpr {
    /// Creates a true guard.
    pub fn true_guard() -> Self {
        Self::True
    }

    /// Creates a false guard.
    pub fn false_guard() -> Self {
        Self::False
    }

    /// Creates a comparison guard.
    pub fn comparison(left: Expr, op: ComparisonOp, right: Expr) -> Self {
        Self::Comparison { left, op, right }
    }
}

/// Result of evaluating a guard on an abstract state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardResult {
    /// Guard is always satisfied in this abstract state
    AlwaysTrue,
    /// Guard is never satisfied in this abstract state
    AlwaysFalse,
    /// Guard may or may not be satisfied (needs refinement)
    Maybe,
}

impl GuardResult {
    /// Logical AND of two guard results.
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::AlwaysTrue, Self::AlwaysTrue) => Self::AlwaysTrue,
            (Self::AlwaysFalse, _) | (_, Self::AlwaysFalse) => Self::AlwaysFalse,
            _ => Self::Maybe,
        }
    }

    /// Logical OR of two guard results.
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::AlwaysFalse, Self::AlwaysFalse) => Self::AlwaysFalse,
            (Self::AlwaysTrue, _) | (_, Self::AlwaysTrue) => Self::AlwaysTrue,
            _ => Self::Maybe,
        }
    }
}

impl Not for GuardResult {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Self::AlwaysTrue => Self::AlwaysFalse,
            Self::AlwaysFalse => Self::AlwaysTrue,
            Self::Maybe => Self::Maybe,
        }
    }
}

impl fmt::Display for GuardResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlwaysTrue => write!(f, "always_true"),
            Self::AlwaysFalse => write!(f, "always_false"),
            Self::Maybe => write!(f, "maybe"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expr_display() {
        let expr = Expr::Add(Box::new(Expr::var("x")), Box::new(Expr::constant(5)));
        assert_eq!(expr.to_string(), "(x + 5)");
    }

    #[test]
    fn test_guard_result_and() {
        assert_eq!(
            GuardResult::AlwaysTrue.and(GuardResult::AlwaysTrue),
            GuardResult::AlwaysTrue
        );
        assert_eq!(
            GuardResult::AlwaysTrue.and(GuardResult::AlwaysFalse),
            GuardResult::AlwaysFalse
        );
        assert_eq!(
            GuardResult::AlwaysTrue.and(GuardResult::Maybe),
            GuardResult::Maybe
        );
    }
}
