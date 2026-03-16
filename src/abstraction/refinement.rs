//! State refinement for handling Maybe guard results.

use super::constraint::Constraint;
use super::expression::{Expr, GuardExpr};
use super::state::AbstractState;
use super::value::AbstractValue;
use crate::guard::ComparisonOp;

/// Refines a state by splitting it based on a guard that evaluates to Maybe.
///
/// When a guard evaluates to `Maybe`, we need to split the state into two:
/// - One where the guard is true
/// - One where the guard is false
pub fn refine_state_with_guard(state: &AbstractState, guard: &GuardExpr) -> Vec<AbstractState> {
    match guard {
        GuardExpr::True => vec![state.clone()], // Keep state
        GuardExpr::False => vec![],             // Remove state
        GuardExpr::Comparison { left, op, right } => refine_comparison(state, left, *op, right),
        GuardExpr::And(left, right) => {
            // Refine with left, then refine each result with right
            let left_refined = refine_state_with_guard(state, left);
            let mut result = Vec::new();
            for refined in left_refined {
                result.extend(refine_state_with_guard(&refined, right));
            }
            result
        }
        GuardExpr::Or(left, right) => {
            // For OR, we need to consider all combinations
            // This is more complex - for now, return both refinements
            let mut result = Vec::new();
            result.extend(refine_state_with_guard(state, left));
            result.extend(refine_state_with_guard(state, right));
            result
        }
        GuardExpr::Not(inner) => {
            // Negate the inner guard and refine
            let negated = negate_guard(inner);
            refine_state_with_guard(state, &negated)
        }
        GuardExpr::Predicate(_) => {
            // Can't refine predicates - return original state
            vec![state.clone()]
        }
    }
}

/// Refines a state based on a comparison guard.
fn refine_comparison(
    state: &AbstractState,
    left: &Expr,
    op: ComparisonOp,
    right: &Expr,
) -> Vec<AbstractState> {
    // For now, simple refinement: split based on variable intervals
    // This is a placeholder - full implementation would:
    // 1. Evaluate left and right expressions to get intervals
    // 2. Split intervals based on the comparison
    // 3. Create new states with refined intervals and path constraints

    // Try to extract variable from left expression
    if let Expr::Var(var_name) = left
        && let Some(&AbstractValue::IntInterval(min, max)) = state.get_variable(var_name)
        && let Expr::Const(constant) = right
    {
        return refine_interval_comparison(state, var_name, min, max, op, *constant);
    }

    // If we can't refine, return original state
    vec![state.clone()]
}

fn add_branch_constraint(
    state: &mut AbstractState,
    var_name: &str,
    op: ComparisonOp,
    constant: i64,
) {
    state.add_constraint(Constraint::var_const(var_name.to_string(), op, constant));
}

/// Refines a state by splitting an interval based on a comparison with a constant.
fn refine_interval_comparison(
    state: &AbstractState,
    var_name: &str,
    min: i64,
    max: i64,
    op: ComparisonOp,
    constant: i64,
) -> Vec<AbstractState> {
    let (lower, upper) = (Some(min), Some(max));

    match op {
        ComparisonOp::Gt => {
            // x > c: split into [lower, c] (guard false) and [c+1, upper] (guard true)
            let mut result = Vec::new();
            if let Some(l) = lower
                && l <= constant
            {
                let mut state1 = state.clone();
                state1.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(l, constant),
                );
                add_branch_constraint(&mut state1, var_name, ComparisonOp::Le, constant);
                result.push(state1);
            }
            if let Some(u) = upper
                && u > constant
            {
                let mut state2 = state.clone();
                state2.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(constant + 1, u),
                );
                add_branch_constraint(&mut state2, var_name, ComparisonOp::Gt, constant);
                result.push(state2);
            }
            if result.is_empty() {
                vec![state.clone()]
            } else {
                result
            }
        }
        ComparisonOp::Ge => {
            // x >= c: split into [lower, c-1] (guard false) and [c, upper] (guard true)
            let mut result = Vec::new();
            if let Some(l) = lower
                && l < constant
            {
                let mut state1 = state.clone();
                state1.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(l, constant - 1),
                );
                add_branch_constraint(&mut state1, var_name, ComparisonOp::Lt, constant);
                result.push(state1);
            }
            if let Some(u) = upper
                && u >= constant
            {
                let mut state2 = state.clone();
                state2.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(constant, u),
                );
                add_branch_constraint(&mut state2, var_name, ComparisonOp::Ge, constant);
                result.push(state2);
            }
            if result.is_empty() {
                vec![state.clone()]
            } else {
                result
            }
        }
        ComparisonOp::Lt => {
            // x < c: split into [lower, c-1] (guard true) and [c, upper] (guard false)
            let mut result = Vec::new();
            if let Some(l) = lower
                && l < constant
            {
                let mut state1 = state.clone();
                state1.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(l, constant - 1),
                );
                add_branch_constraint(&mut state1, var_name, ComparisonOp::Lt, constant);
                result.push(state1);
            }
            if let Some(u) = upper
                && u >= constant
            {
                let mut state2 = state.clone();
                state2.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(constant, u),
                );
                add_branch_constraint(&mut state2, var_name, ComparisonOp::Ge, constant);
                result.push(state2);
            }
            if result.is_empty() {
                vec![state.clone()]
            } else {
                result
            }
        }
        ComparisonOp::Le => {
            // x <= c: split into [lower, c] (guard true) and [c+1, upper] (guard false)
            let mut result = Vec::new();
            if let Some(l) = lower
                && l <= constant
            {
                let mut state1 = state.clone();
                state1.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(l, constant),
                );
                add_branch_constraint(&mut state1, var_name, ComparisonOp::Le, constant);
                result.push(state1);
            }
            if let Some(u) = upper
                && u > constant
            {
                let mut state2 = state.clone();
                state2.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(constant + 1, u),
                );
                add_branch_constraint(&mut state2, var_name, ComparisonOp::Gt, constant);
                result.push(state2);
            }
            if result.is_empty() {
                vec![state.clone()]
            } else {
                result
            }
        }
        ComparisonOp::Eq => {
            // x == c: split into [lower, c-1] (guard false), [c, c] (guard true), [c+1, upper] (guard false)
            let mut result = Vec::new();
            if let Some(l) = lower
                && l < constant
            {
                let mut state1 = state.clone();
                state1.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(l, constant - 1),
                );
                add_branch_constraint(&mut state1, var_name, ComparisonOp::Lt, constant);
                result.push(state1);
            }
            // Add the constant state
            let mut state2 = state.clone();
            state2.set_variable(var_name.to_string(), AbstractValue::int_constant(constant));
            add_branch_constraint(&mut state2, var_name, ComparisonOp::Eq, constant);
            result.push(state2);
            if let Some(u) = upper
                && u > constant
            {
                let mut state3 = state.clone();
                state3.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(constant + 1, u),
                );
                add_branch_constraint(&mut state3, var_name, ComparisonOp::Gt, constant);
                result.push(state3);
            }
            result
        }
        ComparisonOp::Ne => {
            // x != c: split into [lower, c-1] and [c+1, upper], both satisfying `x != c`
            let mut result = Vec::new();
            if let Some(l) = lower
                && l < constant
            {
                let mut state1 = state.clone();
                state1.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(l, constant - 1),
                );
                add_branch_constraint(&mut state1, var_name, ComparisonOp::Ne, constant);
                result.push(state1);
            }
            if let Some(u) = upper
                && u > constant
            {
                let mut state2 = state.clone();
                state2.set_variable(
                    var_name.to_string(),
                    AbstractValue::int_interval(constant + 1, u),
                );
                add_branch_constraint(&mut state2, var_name, ComparisonOp::Ne, constant);
                result.push(state2);
            }
            if result.is_empty() {
                vec![state.clone()]
            } else {
                result
            }
        }
    }
}

/// Negates a guard expression.
fn negate_guard(guard: &GuardExpr) -> GuardExpr {
    match guard {
        GuardExpr::True => GuardExpr::False,
        GuardExpr::False => GuardExpr::True,
        GuardExpr::Not(inner) => *inner.clone(),
        GuardExpr::Comparison { left, op, right } => {
            let negated_op = negate_comparison_op(*op);
            GuardExpr::comparison(left.clone(), negated_op, right.clone())
        }
        GuardExpr::And(left, right) => {
            GuardExpr::Or(Box::new(negate_guard(left)), Box::new(negate_guard(right)))
        }
        GuardExpr::Or(left, right) => {
            GuardExpr::And(Box::new(negate_guard(left)), Box::new(negate_guard(right)))
        }
        GuardExpr::Predicate(name) => GuardExpr::Not(Box::new(GuardExpr::Predicate(name.clone()))),
    }
}

/// Negates a comparison operator.
fn negate_comparison_op(op: ComparisonOp) -> ComparisonOp {
    match op {
        ComparisonOp::Gt => ComparisonOp::Le,
        ComparisonOp::Ge => ComparisonOp::Lt,
        ComparisonOp::Lt => ComparisonOp::Ge,
        ComparisonOp::Le => ComparisonOp::Gt,
        ComparisonOp::Eq => ComparisonOp::Ne,
        ComparisonOp::Ne => ComparisonOp::Eq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::ComparisonOp;

    #[test]
    fn test_refine_interval_comparison_gt() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_interval(0, 10));

        let refined = refine_interval_comparison(&state, "x", 0, 10, ComparisonOp::Gt, 5);
        assert_eq!(refined.len(), 2);
        // Should have [0, 5] and [6, 10]
    }

    #[test]
    fn test_refine_interval_comparison_eq() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_interval(0, 10));

        let refined = refine_interval_comparison(&state, "x", 0, 10, ComparisonOp::Eq, 5);
        assert_eq!(refined.len(), 3);
        // Should have [0, 4], [5, 5], [6, 10]
    }
}
