//! Constraint representation for abstract states.

use super::expression::Expr;
use crate::guard::ComparisonOp;
use std::fmt;

/// Constraint on variables in an abstract state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub left: Expr,
    pub op: ComparisonOp,
    pub right: Expr,
}

/// Kind of constraint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstraintKind {
    /// Variable compared to constant: `var op constant`
    VarConst {
        var: String,
        op: ComparisonOp,
        constant: i64,
    },
    /// Variable compared to variable: `left op right`
    VarVar {
        left: String,
        op: ComparisonOp,
        right: String,
    },
    /// Complex arithmetic expression: `expr op expr`
    Arithmetic { expr: Expr },
}

impl Constraint {
    /// Creates a variable-to-constant constraint.
    pub fn var_const(var: String, op: ComparisonOp, constant: i64) -> Self {
        let var_clone = var.clone();
        Self {
            kind: ConstraintKind::VarConst { var, op, constant },
            left: Expr::Var(var_clone),
            op,
            right: Expr::Const(constant),
        }
    }

    /// Creates a variable-to-variable constraint.
    pub fn var_var(left: String, op: ComparisonOp, right: String) -> Self {
        let left_clone = left.clone();
        let right_clone = right.clone();
        Self {
            kind: ConstraintKind::VarVar { left, op, right },
            left: Expr::Var(left_clone),
            op,
            right: Expr::Var(right_clone),
        }
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.left, self.op, self.right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constraint_var_const() {
        let constraint = Constraint::var_const("x".to_string(), ComparisonOp::Gt, 5);
        assert!(matches!(constraint.kind, ConstraintKind::VarConst { .. }));
    }

    #[test]
    fn test_constraint_var_var() {
        let constraint = Constraint::var_var("x".to_string(), ComparisonOp::Lt, "y".to_string());
        assert!(matches!(constraint.kind, ConstraintKind::VarVar { .. }));
    }
}
