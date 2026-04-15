//! Expression and guard evaluation over abstract states.

use super::expression::{Expr, GuardExpr, GuardResult};
use super::operations::ValueOperations;
use super::state::AbstractState;
use super::value::AbstractValue;
use crate::guard::ComparisonOp;
use std::collections::HashMap;

/// Errors that can occur during evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationError {
    UnknownVariable(String),
    TypeMismatch { expected: String, actual: String },
    DivisionByZero,
    ArithmeticOverflow,
}

impl std::fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownVariable(name) => write!(f, "unknown variable: {}", name),
            Self::TypeMismatch { expected, actual } => {
                write!(f, "type mismatch: expected {}, got {}", expected, actual)
            }
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::ArithmeticOverflow => write!(f, "arithmetic overflow"),
        }
    }
}

impl std::error::Error for EvaluationError {}

/// Helper for evaluating expressions and guards over a single abstract state.
///
/// This struct centralises common evaluation wiring (state + predicate
/// environment) so that call sites like unrolling and μ-calculus integration
/// don't have to thread both pieces separately.
pub struct ExpressionEvaluator<'a> {
    state: &'a AbstractState,
    predicates: &'a HashMap<String, bool>,
}

impl<'a> ExpressionEvaluator<'a> {
    /// Creates a new evaluator bound to a given abstract state and predicate environment.
    pub fn new(state: &'a AbstractState, predicates: &'a HashMap<String, bool>) -> Self {
        Self { state, predicates }
    }

    /// Evaluates an expression using the underlying [`evaluate_expr`] implementation.
    #[inline]
    pub fn evaluate(&self, expr: &Expr) -> Result<AbstractValue, EvaluationError> {
        evaluate_expr(expr, self.state)
    }

    /// Evaluates a guard using the underlying [`evaluate_guard`] implementation.
    #[inline]
    pub fn evaluate_guard(&self, guard: &GuardExpr) -> Result<GuardResult, EvaluationError> {
        evaluate_guard(guard, self.state, self.predicates)
    }
}

/// Evaluates an expression over an abstract state.
///
/// This function evaluates arithmetic and logical expressions over abstract states,
/// where variables may have abstract values (intervals, sets, etc.).
///
/// # Examples
///
/// ## Evaluating variable expressions
/// ```
/// use mununu_core::abstraction::*;
/// use mununu_core::abstraction::expression::Expr;
///
/// let mut state = AbstractState::new("Test");
/// state.set_variable("x", AbstractValue::int_constant(5));
/// state.set_variable("y", AbstractValue::int_constant(10));
///
/// // Evaluate x + y
/// let expr = Expr::Add(
///     Box::new(Expr::var("x")),
///     Box::new(Expr::var("y")),
/// );
/// let result = evaluate_expr(&expr, &state).unwrap();
/// assert_eq!(result, AbstractValue::int_constant(15));
/// ```
///
/// ## Evaluating expressions with intervals
/// ```
/// use mununu_core::abstraction::*;
/// use mununu_core::abstraction::expression::Expr;
///
/// let mut state = AbstractState::new("Test");
/// state.set_variable("x", AbstractValue::int_interval(0, 10));
///
/// // Evaluate x + 5
/// let expr = Expr::Add(
///     Box::new(Expr::var("x")),
///     Box::new(Expr::constant(5)),
/// );
/// let result = evaluate_expr(&expr, &state).unwrap();
/// assert_eq!(result, AbstractValue::int_interval(5, 15));
/// ```
pub fn evaluate_expr(expr: &Expr, state: &AbstractState) -> Result<AbstractValue, EvaluationError> {
    match expr {
        Expr::Var(name) => state
            .get_variable(name)
            .cloned()
            .ok_or_else(|| EvaluationError::UnknownVariable(name.clone())),
        Expr::Const(val) => Ok(AbstractValue::int_constant(*val)),
        Expr::Bool(val) => Ok(AbstractValue::bool_constant(*val)),
        Expr::Add(left, right) => {
            let left_val = evaluate_expr(left, state)?;
            let right_val = evaluate_expr(right, state)?;
            left_val.add_checked(right_val)
        }
        Expr::Sub(left, right) => {
            let left_val = evaluate_expr(left, state)?;
            let right_val = evaluate_expr(right, state)?;
            left_val.sub_checked(right_val)
        }
        Expr::Mul(left, right) => {
            let left_val = evaluate_expr(left, state)?;
            let right_val = evaluate_expr(right, state)?;
            left_val.mul_checked(right_val)
        }
        Expr::Div(left, right) => {
            let left_val = evaluate_expr(left, state)?;
            let right_val = evaluate_expr(right, state)?;
            left_val.div_checked(right_val)
        }
    }
}

/// Evaluates a guard expression over an abstract state.
///
/// This function evaluates guard expressions (comparisons, logical operators)
/// over abstract states, returning a three-valued result:
/// - `AlwaysTrue`: The guard is definitely satisfied
/// - `AlwaysFalse`: The guard is definitely not satisfied
/// - `Maybe`: The guard may or may not be satisfied (requires refinement)
///
/// # Examples
///
/// ## Evaluating concrete comparisons
/// ```
/// use mununu_core::abstraction::*;
/// use mununu_core::abstraction::expression::{Expr, GuardExpr};
/// use mununu_core::guard::ComparisonOp;
/// use std::collections::HashMap;
///
/// let mut state = AbstractState::new("Test");
/// state.set_variable("x", AbstractValue::int_constant(5));
///
/// // Evaluate x > 3
/// let guard = GuardExpr::comparison(
///     Expr::var("x"),
///     ComparisonOp::Gt,
///     Expr::constant(3),
/// );
/// let result = evaluate_guard(&guard, &state, &HashMap::new()).unwrap();
/// assert_eq!(result, GuardResult::AlwaysTrue);
/// ```
///
/// ## Evaluating interval comparisons (Maybe result)
/// ```
/// use mununu_core::abstraction::*;
/// use mununu_core::abstraction::expression::{Expr, GuardExpr};
/// use mununu_core::guard::ComparisonOp;
/// use std::collections::HashMap;
///
/// let mut state = AbstractState::new("Test");
/// state.set_variable("x", AbstractValue::int_interval(0, 10));
///
/// // Evaluate x > 5 (may be true or false)
/// let guard = GuardExpr::comparison(
///     Expr::var("x"),
///     ComparisonOp::Gt,
///     Expr::constant(5),
/// );
/// let result = evaluate_guard(&guard, &state, &HashMap::new()).unwrap();
/// assert_eq!(result, GuardResult::Maybe);
/// ```
pub fn evaluate_guard(
    guard: &GuardExpr,
    state: &AbstractState,
    predicates: &HashMap<String, bool>,
) -> Result<GuardResult, EvaluationError> {
    match guard {
        GuardExpr::True => Ok(GuardResult::AlwaysTrue),
        GuardExpr::False => Ok(GuardResult::AlwaysFalse),
        GuardExpr::Comparison { left, op, right } => {
            let left_val = match evaluate_expr(left, state) {
                Ok(val) => val,
                Err(EvaluationError::UnknownVariable(_)) => return Ok(GuardResult::Maybe),
                Err(e) => return Err(e),
            };
            let right_val = match evaluate_expr(right, state) {
                Ok(val) => val,
                Err(EvaluationError::UnknownVariable(_)) => return Ok(GuardResult::Maybe),
                Err(e) => return Err(e),
            };
            evaluate_comparison(left_val, *op, right_val)
        }
        GuardExpr::And(left, right) => {
            let left_res = evaluate_guard(left, state, predicates)?;
            let right_res = evaluate_guard(right, state, predicates)?;
            Ok(left_res.and(right_res))
        }
        GuardExpr::Or(left, right) => {
            let left_res = evaluate_guard(left, state, predicates)?;
            let right_res = evaluate_guard(right, state, predicates)?;
            Ok(left_res.or(right_res))
        }
        GuardExpr::Not(inner) => {
            let inner_res = evaluate_guard(inner, state, predicates)?;
            Ok(!inner_res)
        }
        GuardExpr::Predicate(name) => {
            // Look up predicate in environment
            if let Some(&value) = predicates.get(name) {
                Ok(if value {
                    GuardResult::AlwaysTrue
                } else {
                    GuardResult::AlwaysFalse
                })
            } else {
                // Unknown predicate - conservative: assume maybe
                Ok(GuardResult::Maybe)
            }
        }
    }
}

/// Evaluates a comparison between two abstract values.
fn evaluate_comparison(
    left: AbstractValue,
    op: ComparisonOp,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    // Check if both are integers
    if !left.is_integer() || !right.is_integer() {
        // Try boolean comparison
        if left.is_boolean() && right.is_boolean() {
            return evaluate_boolean_comparison(left, op, right);
        }
        return Err(EvaluationError::TypeMismatch {
            expected: "int or bool".to_string(),
            actual: format!("{:?} and {:?}", left, right),
        });
    }

    match op {
        ComparisonOp::Eq => evaluate_int_eq(left, right),
        ComparisonOp::Ne => evaluate_int_eq(left, right).map(|r| !r),
        ComparisonOp::Lt => evaluate_int_lt(left, right),
        ComparisonOp::Le => evaluate_int_le(left, right),
        ComparisonOp::Gt => evaluate_int_gt(left, right),
        ComparisonOp::Ge => evaluate_int_ge(left, right),
    }
}

/// Evaluates boolean comparison (equality only for booleans).
fn evaluate_boolean_comparison(
    left: AbstractValue,
    op: ComparisonOp,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    match op {
        ComparisonOp::Eq | ComparisonOp::Ne => {
            let eq_result = evaluate_boolean_eq(left, right);
            if matches!(op, ComparisonOp::Ne) {
                Ok(!eq_result)
            } else {
                Ok(eq_result)
            }
        }
        _ => Err(EvaluationError::TypeMismatch {
            expected: "comparison operator for booleans (only == and != are supported)".to_string(),
            actual: format!("{:?}", op),
        }),
    }
}

/// Evaluates boolean equality.
fn evaluate_boolean_eq(left: AbstractValue, right: AbstractValue) -> GuardResult {
    match (left, right) {
        // Both constants
        (AbstractValue::BoolConstant(a), AbstractValue::BoolConstant(b)) => {
            if a == b {
                GuardResult::AlwaysTrue
            } else {
                GuardResult::AlwaysFalse
            }
        }
        // Constant and set
        (AbstractValue::BoolConstant(a), AbstractValue::BoolSet(b_set)) => {
            if b_set.len() == 1 && b_set.contains(&a) {
                GuardResult::AlwaysTrue
            } else if !b_set.contains(&a) {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            }
        }
        (AbstractValue::BoolSet(a_set), AbstractValue::BoolConstant(b)) => evaluate_boolean_eq(
            AbstractValue::BoolConstant(b),
            AbstractValue::BoolSet(a_set),
        ),
        // Both sets
        (AbstractValue::BoolSet(a_set), AbstractValue::BoolSet(b_set)) => {
            let intersection: Vec<_> = a_set.intersection(&b_set).collect();
            if intersection.is_empty() {
                GuardResult::AlwaysFalse
            } else if a_set == b_set && a_set.len() == 1 {
                GuardResult::AlwaysTrue
            } else {
                GuardResult::Maybe
            }
        }
        _ => GuardResult::Maybe,
    }
}

/// Evaluates integer equality comparison.
fn evaluate_int_eq(
    left: AbstractValue,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    match (left, right) {
        // Both constants
        (AbstractValue::IntConstant(a), AbstractValue::IntConstant(b)) => Ok(if a == b {
            GuardResult::AlwaysTrue
        } else {
            GuardResult::AlwaysFalse
        }),
        // Constant and interval
        (AbstractValue::IntConstant(a), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if b_min <= a && a <= b_max {
                if b_min == a && a == b_max {
                    GuardResult::AlwaysTrue // Singleton interval equals constant
                } else {
                    GuardResult::Maybe
                }
            } else {
                GuardResult::AlwaysFalse
            })
        }
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntConstant(b)) => {
            evaluate_int_eq(
                AbstractValue::IntConstant(b),
                AbstractValue::IntInterval(a_min, a_max),
            )
        }
        // Both intervals
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntInterval(b_min, b_max)) => {
            // If intervals don't overlap, definitely false
            if a_max < b_min || a_min > b_max {
                Ok(GuardResult::AlwaysFalse)
            } else if a_min == b_min && a_max == b_max && a_min == a_max {
                // Same singleton interval
                Ok(GuardResult::AlwaysTrue)
            } else {
                Ok(GuardResult::Maybe)
            }
        }
        // Constant and set
        (AbstractValue::IntConstant(a), AbstractValue::IntSet(b_set)) => {
            Ok(if b_set.len() == 1 && b_set.contains(&a) {
                GuardResult::AlwaysTrue
            } else if !b_set.contains(&a) {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        (AbstractValue::IntSet(a_set), AbstractValue::IntConstant(b)) => {
            evaluate_int_eq(AbstractValue::IntConstant(b), AbstractValue::IntSet(a_set))
        }
        // Interval and set
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntSet(b_set)) => {
            let in_range: Vec<_> = b_set
                .iter()
                .filter(|x| a_min <= **x && **x <= a_max)
                .collect();
            Ok(if in_range.is_empty() {
                GuardResult::AlwaysFalse
            } else if in_range.len() == 1 && a_min == a_max && b_set.contains(&a_min) {
                GuardResult::AlwaysTrue
            } else {
                GuardResult::Maybe
            })
        }
        (AbstractValue::IntSet(a_set), AbstractValue::IntInterval(b_min, b_max)) => {
            evaluate_int_eq(
                AbstractValue::IntInterval(b_min, b_max),
                AbstractValue::IntSet(a_set),
            )
        }
        // Both sets
        (AbstractValue::IntSet(a_set), AbstractValue::IntSet(b_set)) => {
            let intersection: Vec<_> = a_set.intersection(&b_set).collect();
            Ok(if intersection.is_empty() {
                GuardResult::AlwaysFalse
            } else if a_set == b_set && a_set.len() == 1 {
                GuardResult::AlwaysTrue
            } else {
                GuardResult::Maybe
            })
        }
        // Top values
        (AbstractValue::IntTop, _) | (_, AbstractValue::IntTop) => Ok(GuardResult::Maybe),
        (AbstractValue::PositiveInfinity, AbstractValue::PositiveInfinity) => {
            Ok(GuardResult::AlwaysTrue)
        }
        (AbstractValue::NegativeInfinity, AbstractValue::NegativeInfinity) => {
            Ok(GuardResult::AlwaysTrue)
        }
        (AbstractValue::PositiveInfinity, _) | (_, AbstractValue::PositiveInfinity) => {
            Ok(GuardResult::AlwaysFalse)
        }
        (AbstractValue::NegativeInfinity, _) | (_, AbstractValue::NegativeInfinity) => {
            Ok(GuardResult::AlwaysFalse)
        }
        // Undefined
        (AbstractValue::Undefined, _) | (_, AbstractValue::Undefined) => Ok(GuardResult::Maybe),
        _ => Ok(GuardResult::Maybe),
    }
}

/// Evaluates integer less-than comparison.
fn evaluate_int_lt(
    left: AbstractValue,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    match (left, right) {
        // Both constants
        (AbstractValue::IntConstant(a), AbstractValue::IntConstant(b)) => Ok(if a < b {
            GuardResult::AlwaysTrue
        } else {
            GuardResult::AlwaysFalse
        }),
        // Constant and interval
        (AbstractValue::IntConstant(a), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a < b_min {
                GuardResult::AlwaysTrue
            } else if a >= b_max {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntConstant(b)) => {
            // a < b  <=>  !(a >= b)
            evaluate_int_ge(
                AbstractValue::IntInterval(a_min, a_max),
                AbstractValue::IntConstant(b),
            )
            .map(|r| !r)
        }
        // Both intervals
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a_max < b_min {
                GuardResult::AlwaysTrue
            } else if a_min >= b_max {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Handle sets and other types (delegate to interval approximation)
        (left, right) => {
            // For sets and complex types, convert to interval approximation
            let left_interval = abstract_value_to_interval_bounds(&left)?;
            let right_interval = abstract_value_to_interval_bounds(&right)?;
            evaluate_int_lt(left_interval, right_interval)
        }
    }
}

/// Evaluates integer less-than-or-equal comparison.
fn evaluate_int_le(
    left: AbstractValue,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    match (left, right) {
        // Both constants
        (AbstractValue::IntConstant(a), AbstractValue::IntConstant(b)) => Ok(if a <= b {
            GuardResult::AlwaysTrue
        } else {
            GuardResult::AlwaysFalse
        }),
        // Constant and interval
        (AbstractValue::IntConstant(a), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a <= b_min {
                GuardResult::AlwaysTrue
            } else if a > b_max {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntConstant(b)) => {
            Ok(if a_max <= b {
                GuardResult::AlwaysTrue
            } else if a_min > b {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Both intervals
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a_max <= b_min {
                GuardResult::AlwaysTrue
            } else if a_min > b_max {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Handle sets and other types
        (left, right) => {
            let left_interval = abstract_value_to_interval_bounds(&left)?;
            let right_interval = abstract_value_to_interval_bounds(&right)?;
            evaluate_int_le(left_interval, right_interval)
        }
    }
}

/// Evaluates integer greater-than comparison.
fn evaluate_int_gt(
    left: AbstractValue,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    match (left, right) {
        // Both constants
        (AbstractValue::IntConstant(a), AbstractValue::IntConstant(b)) => Ok(if a > b {
            GuardResult::AlwaysTrue
        } else {
            GuardResult::AlwaysFalse
        }),
        // Constant and interval
        (AbstractValue::IntConstant(a), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a > b_max {
                GuardResult::AlwaysTrue
            } else if a <= b_min {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntConstant(b)) => {
            Ok(if a_min > b {
                GuardResult::AlwaysTrue
            } else if a_max <= b {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Both intervals
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a_min > b_max {
                GuardResult::AlwaysTrue
            } else if a_max <= b_min {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Handle sets and other types
        (left, right) => {
            let left_interval = abstract_value_to_interval_bounds(&left)?;
            let right_interval = abstract_value_to_interval_bounds(&right)?;
            evaluate_int_gt(left_interval, right_interval)
        }
    }
}

/// Evaluates integer greater-than-or-equal comparison.
fn evaluate_int_ge(
    left: AbstractValue,
    right: AbstractValue,
) -> Result<GuardResult, EvaluationError> {
    match (left, right) {
        // Both constants
        (AbstractValue::IntConstant(a), AbstractValue::IntConstant(b)) => Ok(if a >= b {
            GuardResult::AlwaysTrue
        } else {
            GuardResult::AlwaysFalse
        }),
        // Constant and interval
        (AbstractValue::IntConstant(a), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a >= b_max {
                GuardResult::AlwaysTrue
            } else if a < b_min {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntConstant(b)) => {
            Ok(if a_min >= b {
                GuardResult::AlwaysTrue
            } else if a_max < b {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Both intervals
        (AbstractValue::IntInterval(a_min, a_max), AbstractValue::IntInterval(b_min, b_max)) => {
            Ok(if a_min >= b_max {
                GuardResult::AlwaysTrue
            } else if a_max < b_min {
                GuardResult::AlwaysFalse
            } else {
                GuardResult::Maybe
            })
        }
        // Handle sets and other types
        (left, right) => {
            let left_interval = abstract_value_to_interval_bounds(&left)?;
            let right_interval = abstract_value_to_interval_bounds(&right)?;
            evaluate_int_ge(left_interval, right_interval)
        }
    }
}

/// Converts an abstract value to interval bounds for comparison purposes.
/// Returns an IntInterval representation of the value.
fn abstract_value_to_interval_bounds(
    val: &AbstractValue,
) -> Result<AbstractValue, EvaluationError> {
    match val {
        AbstractValue::IntConstant(n) => Ok(AbstractValue::IntInterval(*n, *n)),
        AbstractValue::IntInterval(..) => Ok(val.clone()),
        AbstractValue::IntSet(set) => {
            if set.is_empty() {
                Err(EvaluationError::TypeMismatch {
                    expected: "non-empty integer set".to_string(),
                    actual: "empty set".to_string(),
                })
            } else {
                let min_val = *set.iter().min().unwrap();
                let max_val = *set.iter().max().unwrap();
                Ok(AbstractValue::IntInterval(min_val, max_val))
            }
        }
        AbstractValue::IntTop => {
            // Use a very wide interval as approximation
            Ok(AbstractValue::IntInterval(i64::MIN, i64::MAX))
        }
        AbstractValue::PositiveInfinity => Ok(AbstractValue::IntInterval(i64::MAX, i64::MAX)),
        AbstractValue::NegativeInfinity => Ok(AbstractValue::IntInterval(i64::MIN, i64::MIN)),
        _ => Err(EvaluationError::TypeMismatch {
            expected: "integer value".to_string(),
            actual: format!("{:?}", val),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_expr_var() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_constant(5));

        let expr = Expr::var("x");
        let result = evaluate_expr(&expr, &state).unwrap();
        assert_eq!(result.as_int_constant(), Some(5));
    }

    #[test]
    fn test_evaluate_expr_add() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_constant(5));
        state.set_variable("y", AbstractValue::int_constant(10));

        let expr = Expr::Add(Box::new(Expr::var("x")), Box::new(Expr::var("y")));
        let result = evaluate_expr(&expr, &state).unwrap();
        assert_eq!(result.as_int_constant(), Some(15));
    }

    #[test]
    fn test_evaluate_guard_comparison() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_constant(5));

        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Gt, Expr::constant(3));
        let predicates = HashMap::new();
        let result = evaluate_guard(&guard, &state, &predicates).unwrap();
        assert_eq!(result, GuardResult::AlwaysTrue);
    }

    #[test]
    fn test_evaluate_guard_maybe() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_interval(0, 10));

        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Gt, Expr::constant(5));
        let predicates = HashMap::new();
        let result = evaluate_guard(&guard, &state, &predicates).unwrap();
        assert_eq!(result, GuardResult::Maybe);
    }

    // ===== Comprehensive Comparison Tests =====

    #[test]
    fn test_comparison_constant_constant_eq() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_constant(5));

        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(5));
        let predicates = HashMap::new();
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );

        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(3));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );
    }

    #[test]
    fn test_comparison_interval_constant() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_interval(0, 10));

        // x == 5: maybe (5 is in [0, 10] but interval is not singleton)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(5));
        let predicates = HashMap::new();
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::Maybe
        );

        // x == 15: always false (15 is outside [0, 10])
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(15));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );

        // x > 5: maybe (some values in [0, 10] are > 5, some are not)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Gt, Expr::constant(5));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::Maybe
        );

        // x > 15: always false (all values in [0, 10] are <= 15)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Gt, Expr::constant(15));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );

        // x > -5: always true (all values in [0, 10] are > -5)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Gt, Expr::constant(-5));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );
    }

    #[test]
    fn test_comparison_interval_interval() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_interval(0, 10));
        state.set_variable("y", AbstractValue::int_interval(5, 15));

        // x < y: maybe (overlap exists)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Lt, Expr::var("y"));
        let predicates = HashMap::new();
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::Maybe
        );

        state.set_variable("x", AbstractValue::int_interval(0, 5));
        state.set_variable("y", AbstractValue::int_interval(10, 15));
        // x < y: always true (max(x) = 5 < min(y) = 10)
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );

        state.set_variable("x", AbstractValue::int_interval(10, 15));
        state.set_variable("y", AbstractValue::int_interval(0, 5));
        // x < y: always false (min(x) = 10 > max(y) = 5)
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );
    }

    #[test]
    fn test_comparison_set_constant() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_set(vec![1, 3, 5, 7]));

        // x == 5: maybe (5 is in set but set has other elements)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(5));
        let predicates = HashMap::new();
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::Maybe
        );

        // x == 10: always false (10 is not in set)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(10));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );

        state.set_variable("x", AbstractValue::int_set(vec![5]));
        // x == 5: always true (singleton set)
        let guard = GuardExpr::comparison(Expr::var("x"), ComparisonOp::Eq, Expr::constant(5));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );
    }

    #[test]
    fn test_comparison_all_operators() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_constant(5));
        state.set_variable("y", AbstractValue::int_constant(3));

        let predicates = HashMap::new();

        // Test all comparison operators with x=5, y=3
        // So: 5 == 3 is false, 5 != 3 is true, 5 < 3 is false, etc.
        let ops = vec![
            (ComparisonOp::Eq, GuardResult::AlwaysFalse), // 5 == 3 is false
            (ComparisonOp::Ne, GuardResult::AlwaysTrue),  // 5 != 3 is true
            (ComparisonOp::Lt, GuardResult::AlwaysFalse), // 5 < 3 is false
            (ComparisonOp::Le, GuardResult::AlwaysFalse), // 5 <= 3 is false
            (ComparisonOp::Gt, GuardResult::AlwaysTrue),  // 5 > 3 is true
            (ComparisonOp::Ge, GuardResult::AlwaysTrue),  // 5 >= 3 is true
        ];

        for (op, expected) in ops {
            let guard = GuardExpr::comparison(Expr::var("x"), op, Expr::var("y"));
            let result = evaluate_guard(&guard, &state, &predicates).unwrap();
            assert_eq!(result, expected, "Operator {:?} with 5 op 3", op);
        }
    }

    #[test]
    fn test_boolean_comparison() {
        let mut state = AbstractState::new("Test");
        state.set_variable("flag", AbstractValue::bool_constant(true));

        let predicates = HashMap::new();

        // flag == true: always true
        let guard = GuardExpr::comparison(Expr::var("flag"), ComparisonOp::Eq, Expr::bool(true));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );

        // flag == false: always false
        let guard = GuardExpr::comparison(Expr::var("flag"), ComparisonOp::Eq, Expr::bool(false));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );

        state.set_variable("flag", AbstractValue::bool_set(vec![true, false]));
        // flag == true: maybe (set contains both true and false)
        let guard = GuardExpr::comparison(Expr::var("flag"), ComparisonOp::Eq, Expr::bool(true));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::Maybe
        );
    }

    #[test]
    fn test_guard_logical_operations() {
        let mut state = AbstractState::new("Test");
        state.set_variable("x", AbstractValue::int_constant(5));
        state.set_variable("y", AbstractValue::int_constant(3));

        let predicates = HashMap::new();

        // x > 3 && y < 10: AlwaysTrue && AlwaysTrue = AlwaysTrue
        let guard = GuardExpr::And(
            Box::new(GuardExpr::comparison(
                Expr::var("x"),
                ComparisonOp::Gt,
                Expr::constant(3),
            )),
            Box::new(GuardExpr::comparison(
                Expr::var("y"),
                ComparisonOp::Lt,
                Expr::constant(10),
            )),
        );
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );

        // x > 10 && y < 10: AlwaysFalse && AlwaysTrue = AlwaysFalse
        let guard = GuardExpr::And(
            Box::new(GuardExpr::comparison(
                Expr::var("x"),
                ComparisonOp::Gt,
                Expr::constant(10),
            )),
            Box::new(GuardExpr::comparison(
                Expr::var("y"),
                ComparisonOp::Lt,
                Expr::constant(10),
            )),
        );
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysFalse
        );

        // x > 10 || y < 10: AlwaysFalse || AlwaysTrue = AlwaysTrue
        let guard = GuardExpr::Or(
            Box::new(GuardExpr::comparison(
                Expr::var("x"),
                ComparisonOp::Gt,
                Expr::constant(10),
            )),
            Box::new(GuardExpr::comparison(
                Expr::var("y"),
                ComparisonOp::Lt,
                Expr::constant(10),
            )),
        );
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );

        // !(x > 10): !AlwaysFalse = AlwaysTrue
        let guard = GuardExpr::Not(Box::new(GuardExpr::comparison(
            Expr::var("x"),
            ComparisonOp::Gt,
            Expr::constant(10),
        )));
        assert_eq!(
            evaluate_guard(&guard, &state, &predicates).unwrap(),
            GuardResult::AlwaysTrue
        );
    }
}
