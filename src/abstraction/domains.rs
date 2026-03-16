//! Abstract domain definitions for state variable abstraction.

use std::fmt;
use std::ops::Not;

/// Boolean abstract domain.
///
/// Values: `{true, false, unknown}`
/// Lattice: `unknown ⊑ true`, `unknown ⊑ false`, `true` and `false` are incomparable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoolDomain {
    True,
    False,
    Unknown,
}

impl BoolDomain {
    /// Creates a boolean domain value from a concrete boolean.
    pub fn from_bool(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }

    /// Returns the concrete boolean value if known, None if unknown.
    pub fn to_bool(self) -> Option<bool> {
        match self {
            Self::True => Some(true),
            Self::False => Some(false),
            Self::Unknown => None,
        }
    }

    /// Logical AND operation.
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, Self::True) => Self::True,
            (Self::False, _) | (_, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }

    /// Logical OR operation.
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, Self::False) => Self::False,
            (Self::True, _) | (_, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    /// Exclusive OR operation.
    pub fn xor(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, Self::True) | (Self::False, Self::False) => Self::False,
            (Self::True, Self::False) | (Self::False, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    /// Implication operation (a → b).
    pub fn implies(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, Self::True) => Self::True,
            (Self::True, Self::False) => Self::False,
            (Self::False, _) => Self::True,
            _ => Self::Unknown,
        }
    }
}

impl Not for BoolDomain {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

impl fmt::Display for BoolDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::True => write!(f, "true"),
            Self::False => write!(f, "false"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Integer abstract domain using intervals.
///
/// Representation: `[lower, upper]` where `lower, upper ∈ Z ∪ {-∞, +∞}`
/// Lattice: `[a, b] ⊑ [c, d]` if `c ≤ a` and `b ≤ d`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntDomain {
    lower: Option<i64>, // None represents -∞
    upper: Option<i64>, // None represents +∞
}

impl IntDomain {
    /// Creates an interval domain `[lower, upper]`.
    pub fn interval(lower: Option<i64>, upper: Option<i64>) -> Self {
        // Validate: lower <= upper
        if let (Some(l), Some(u)) = (lower, upper) {
            assert!(l <= u, "interval lower bound must be <= upper bound");
        }
        Self { lower, upper }
    }

    /// Creates a constant interval `[value, value]`.
    pub fn constant(value: i64) -> Self {
        Self {
            lower: Some(value),
            upper: Some(value),
        }
    }

    /// Creates an unbounded interval `[-∞, +∞]`.
    pub fn unbounded() -> Self {
        Self {
            lower: None,
            upper: None,
        }
    }

    /// Returns the lower bound (None for -∞).
    pub fn lower(&self) -> Option<i64> {
        self.lower
    }

    /// Returns the upper bound (None for +∞).
    pub fn upper(&self) -> Option<i64> {
        self.upper
    }

    /// Checks if this is a constant (singleton interval).
    pub fn is_constant(&self) -> bool {
        self.lower == self.upper && self.lower.is_some()
    }

    /// Returns the constant value if this is a constant, None otherwise.
    pub fn to_constant(self) -> Option<i64> {
        if self.is_constant() { self.lower } else { None }
    }

    /// Checks if this interval is unbounded.
    pub fn is_unbounded(&self) -> bool {
        self.lower.is_none() && self.upper.is_none()
    }

    /// Checks if this interval contains a value.
    pub fn contains(&self, value: i64) -> bool {
        let lower_ok = self.lower.is_none_or(|l| value >= l);
        let upper_ok = self.upper.is_none_or(|u| value <= u);
        lower_ok && upper_ok
    }

    /// Checks if this interval is a subset of another.
    pub fn is_subset_of(&self, other: &Self) -> bool {
        let lower_ok = match (self.lower, other.lower) {
            (None, _) => true,        // -∞ is subset of anything
            (Some(_l), None) => true, // Any finite is subset of -∞
            (Some(l), Some(r)) => l >= r,
        };
        let upper_ok = match (self.upper, other.upper) {
            (None, _) => true,        // +∞ is subset of anything
            (Some(_u), None) => true, // Any finite is subset of +∞
            (Some(u), Some(r)) => u <= r,
        };
        lower_ok && upper_ok
    }

    /// Addition: `[a.lower + b.lower, a.upper + b.upper]`
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: Self) -> Self {
        let lower = match (self.lower, other.lower) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            _ => None, // -∞
        };
        let upper = match (self.upper, other.upper) {
            (Some(a), Some(b)) => {
                let sum = a.checked_add(b);
                if sum.is_some() && sum.unwrap() < i64::MAX {
                    sum
                } else {
                    None // +∞
                }
            }
            _ => None, // +∞
        };
        Self { lower, upper }
    }

    /// Subtraction: `[a.lower - b.upper, a.upper - b.lower]`
    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: Self) -> Self {
        let lower = match (self.lower, other.upper) {
            (Some(a), Some(b)) => Some(a.saturating_sub(b)),
            (Some(_), None) => None, // -∞ - finite = -∞
            _ => None,               // -∞
        };
        let upper = match (self.upper, other.lower) {
            (Some(a), Some(b)) => {
                let diff = a.checked_sub(b);
                if diff.is_some() && diff.unwrap() > i64::MIN {
                    diff
                } else {
                    None // +∞
                }
            }
            _ => None, // +∞
        };
        Self { lower, upper }
    }

    /// Multiplication: more complex (sign analysis)
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, other: Self) -> Self {
        // Handle zero
        if self.contains(0) || other.contains(0) {
            if self.is_constant() && self.to_constant() == Some(0) {
                return Self::constant(0);
            }
            if other.is_constant() && other.to_constant() == Some(0) {
                return Self::constant(0);
            }
        }

        // Handle constants
        if let Some(c) = self.to_constant() {
            return other.scale(c);
        }
        if let Some(c) = other.to_constant() {
            return self.scale(c);
        }

        // For intervals, compute corners
        let corners = [
            (self.lower, other.lower),
            (self.lower, other.upper),
            (self.upper, other.lower),
            (self.upper, other.upper),
        ];

        let mut min_val: Option<i64> = None;
        let mut max_val: Option<i64> = None;

        for (a, b) in corners.iter() {
            if let (Some(a_val), Some(b_val)) = (a, b) {
                if let Some(product) = a_val.checked_mul(*b_val) {
                    min_val = min_val.map(|m| m.min(product)).or(Some(product));
                    max_val = max_val.map(|m| m.max(product)).or(Some(product));
                } else {
                    // Overflow - unbounded
                    return Self::unbounded();
                }
            } else {
                // Unbounded - result is unbounded
                return Self::unbounded();
            }
        }

        Self {
            lower: min_val,
            upper: max_val,
        }
    }

    /// Scales an interval by a constant: `[c * lower, c * upper]` (or reversed if c < 0)
    fn scale(self, c: i64) -> Self {
        if c == 0 {
            return Self::constant(0);
        }

        let (lower, upper) = if c > 0 {
            (
                self.lower.and_then(|l| l.checked_mul(c)),
                self.upper.and_then(|u| u.checked_mul(c)),
            )
        } else {
            // Negative: reverse bounds
            (
                self.upper.and_then(|u| u.checked_mul(c)),
                self.lower.and_then(|l| l.checked_mul(c)),
            )
        };

        Self { lower, upper }
    }

    /// Division: requires division-by-zero handling
    #[allow(clippy::should_implement_trait)]
    pub fn div(self, other: Self) -> Result<Self, DivisionError> {
        // Check for division by zero
        if other.contains(0) {
            return Err(DivisionError::DivisionByZero);
        }

        // Handle constant divisor
        if let Some(c) = other.to_constant() {
            return Ok(self.scale(1 / c)); // Scale by 1/c
        }

        // For intervals, compute corners
        let corners = [
            (self.lower, other.lower),
            (self.lower, other.upper),
            (self.upper, other.lower),
            (self.upper, other.upper),
        ];

        let mut min_val: Option<i64> = None;
        let mut max_val: Option<i64> = None;

        for (a, b) in corners.iter() {
            if let (Some(a_val), Some(b_val)) = (a, b) {
                if *b_val != 0 {
                    if let Some(quotient) = a_val.checked_div(*b_val) {
                        min_val = min_val.map(|m| m.min(quotient)).or(Some(quotient));
                        max_val = max_val.map(|m| m.max(quotient)).or(Some(quotient));
                    } else {
                        // Overflow - unbounded
                        return Ok(Self::unbounded());
                    }
                }
            } else {
                // Unbounded - result is unbounded
                return Ok(Self::unbounded());
            }
        }

        Ok(Self {
            lower: min_val,
            upper: max_val,
        })
    }

    /// Widens the interval if it exceeds the threshold.
    pub fn widen(self, threshold: i64) -> Self {
        if let (Some(l), Some(u)) = (self.lower, self.upper)
            && u.saturating_sub(l) > threshold
        {
            // Widen to [lower, +∞]
            return Self {
                lower: self.lower,
                upper: None,
            };
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivisionError {
    DivisionByZero,
}

impl fmt::Display for IntDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.lower, self.upper) {
            (Some(l), Some(u)) if l == u => write!(f, "{}", l),
            (Some(l), Some(u)) => write!(f, "[{}, {}]", l, u),
            (Some(l), None) => write!(f, "[{}, +∞]", l),
            (None, Some(u)) => write!(f, "[-∞, {}]", u),
            (None, None) => write!(f, "[-∞, +∞]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bool_domain_operations() {
        assert_eq!(!BoolDomain::True, BoolDomain::False);
        assert_eq!(!BoolDomain::False, BoolDomain::True);
        assert_eq!(!BoolDomain::Unknown, BoolDomain::Unknown);

        assert_eq!(BoolDomain::True.and(BoolDomain::True), BoolDomain::True);
        assert_eq!(BoolDomain::True.and(BoolDomain::False), BoolDomain::False);
        assert_eq!(
            BoolDomain::True.and(BoolDomain::Unknown),
            BoolDomain::Unknown
        );

        assert_eq!(BoolDomain::False.or(BoolDomain::True), BoolDomain::True);
        assert_eq!(BoolDomain::False.or(BoolDomain::False), BoolDomain::False);
        assert_eq!(
            BoolDomain::False.or(BoolDomain::Unknown),
            BoolDomain::Unknown
        );

        assert_eq!(BoolDomain::True.xor(BoolDomain::False), BoolDomain::True);
        assert_eq!(BoolDomain::True.xor(BoolDomain::True), BoolDomain::False);
        assert_eq!(
            BoolDomain::True.xor(BoolDomain::Unknown),
            BoolDomain::Unknown
        );

        assert_eq!(BoolDomain::True.implies(BoolDomain::True), BoolDomain::True);
        assert_eq!(
            BoolDomain::True.implies(BoolDomain::False),
            BoolDomain::False
        );
        assert_eq!(
            BoolDomain::False.implies(BoolDomain::True),
            BoolDomain::True
        );
    }

    #[test]
    fn test_int_domain_constant() {
        let c = IntDomain::constant(5);
        assert_eq!(c.to_constant(), Some(5));
        assert!(c.is_constant());
        assert!(c.contains(5));
        assert!(!c.contains(4));
    }

    #[test]
    fn test_int_domain_add() {
        let a = IntDomain::interval(Some(0), Some(5));
        let b = IntDomain::interval(Some(10), Some(20));
        let sum = a.add(b);
        assert_eq!(sum.lower(), Some(10));
        assert_eq!(sum.upper(), Some(25));
    }

    #[test]
    fn test_int_domain_sub() {
        let a = IntDomain::interval(Some(10), Some(20));
        let b = IntDomain::interval(Some(0), Some(5));
        let diff = a.sub(b);
        assert_eq!(diff.lower(), Some(5));
        assert_eq!(diff.upper(), Some(20));
    }

    #[test]
    fn test_int_domain_mul() {
        let a = IntDomain::interval(Some(0), Some(5));
        let b = IntDomain::interval(Some(10), Some(20));
        let product = a.mul(b);
        assert_eq!(product.lower(), Some(0));
        assert_eq!(product.upper(), Some(100));
    }

    #[test]
    fn test_int_domain_div() {
        let a = IntDomain::interval(Some(0), Some(10));
        let b = IntDomain::interval(Some(2), Some(5));
        let quotient = a.div(b).unwrap();
        assert_eq!(quotient.lower(), Some(0));
        assert_eq!(quotient.upper(), Some(5));
    }

    #[test]
    fn test_int_domain_div_by_zero() {
        let a = IntDomain::interval(Some(10), Some(20));
        let b = IntDomain::interval(Some(-2), Some(2)); // Contains zero
        assert_eq!(a.div(b), Err(DivisionError::DivisionByZero));
    }

    #[test]
    fn test_int_domain_widen() {
        let interval = IntDomain::interval(Some(0), Some(1001));
        let widened = interval.widen(1000);
        assert_eq!(widened.lower(), Some(0));
        assert_eq!(widened.upper(), None); // +∞
    }
}
