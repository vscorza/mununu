//! Common operations over abstract values.
//!
//! This module provides a small trait that wraps the core arithmetic on
//! [`AbstractValue`], so callers such as the evaluator and unrolling logic
//! can share the same checked behaviour (including type checking and
//! error reporting).

use super::evaluator::EvaluationError;
use super::value::AbstractValue;

/// High-level arithmetic operations for [`AbstractValue`].
pub trait ValueOperations {
    /// Returns a normalized representation of this value.
    fn normalize_value(self) -> AbstractValue;

    /// Adds two abstract values, returning a checked result.
    fn add_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError>;

    /// Subtracts two abstract values, returning a checked result.
    fn sub_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError>;

    /// Multiplies two abstract values, returning a checked result.
    fn mul_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError>;

    /// Divides two abstract values, returning a checked result.
    fn div_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError>;
}

impl ValueOperations for AbstractValue {
    #[inline]
    fn normalize_value(self) -> AbstractValue {
        self.normalize()
    }

    fn add_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError> {
        if !self.is_integer() || !other.is_integer() {
            return Err(EvaluationError::TypeMismatch {
                expected: "int".to_string(),
                actual: format!("{:?} and {:?}", self, other),
            });
        }
        Ok(self.add(&other))
    }

    fn sub_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError> {
        if !self.is_integer() || !other.is_integer() {
            return Err(EvaluationError::TypeMismatch {
                expected: "int".to_string(),
                actual: format!("{:?} and {:?}", self, other),
            });
        }
        Ok(self.sub(&other))
    }

    fn mul_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError> {
        if !self.is_integer() || !other.is_integer() {
            return Err(EvaluationError::TypeMismatch {
                expected: "int".to_string(),
                actual: format!("{:?} and {:?}", self, other),
            });
        }
        Ok(self.mul(&other))
    }

    fn div_checked(self, other: AbstractValue) -> Result<AbstractValue, EvaluationError> {
        if !self.is_integer() || !other.is_integer() {
            return Err(EvaluationError::TypeMismatch {
                expected: "int".to_string(),
                actual: format!("{:?} and {:?}", self, other),
            });
        }
        self.div(&other)
            .map_err(|_| EvaluationError::DivisionByZero)
    }
}
