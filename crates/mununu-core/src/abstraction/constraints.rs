//! Constraint management utilities for abstract states.
//!
//! This module provides a small helper responsible for storing and
//! evaluating constraints associated with abstract states. It is a
//! thin layer over `Vec<Constraint>` that centralises evaluation
//! behaviour so callers don't have to re-implement it.

use std::collections::HashMap;

use super::constraint::Constraint;
use super::evaluator::{EvaluationError, ExpressionEvaluator};
use super::expression::{GuardExpr, GuardResult};
use super::state::AbstractState;

/// Manages a collection of constraints for an abstract state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstraintManager {
    constraints: Vec<Constraint>,
}

impl ConstraintManager {
    /// Creates an empty constraint manager.
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }

    /// Creates a manager from an existing vector of constraints.
    pub fn from_vec(constraints: Vec<Constraint>) -> Self {
        Self { constraints }
    }

    /// Returns a slice of all stored constraints.
    pub fn as_slice(&self) -> &[Constraint] {
        &self.constraints
    }

    /// Consumes the manager and returns the underlying vector.
    pub fn into_vec(self) -> Vec<Constraint> {
        self.constraints
    }

    /// Adds a new constraint to the collection.
    pub fn add(&mut self, constraint: Constraint) {
        self.constraints.push(constraint);
    }

    /// Evaluates all constraints against the given abstract state.
    ///
    /// Returns `Ok(true)` if all constraints are satisfied or inconclusive
    /// (`GuardResult::Maybe`), `Ok(false)` if any constraint is definitely
    /// violated, and `Err` if a non-`UnknownVariable` evaluation error occurs.
    pub fn evaluate(
        &self,
        state: &AbstractState,
        predicates: &HashMap<String, bool>,
    ) -> Result<bool, EvaluationError> {
        if self.constraints.is_empty() {
            return Ok(true);
        }

        let evaluator = ExpressionEvaluator::new(state, predicates);
        for constraint in &self.constraints {
            // Interpret each constraint as a guard comparison.
            let guard = GuardExpr::comparison(
                constraint.left.clone(),
                constraint.op,
                constraint.right.clone(),
            );

            match evaluator.evaluate_guard(&guard) {
                Ok(GuardResult::AlwaysFalse) => return Ok(false),
                Ok(GuardResult::AlwaysTrue) | Ok(GuardResult::Maybe) => {
                    // Keep checking remaining constraints.
                }
                Err(EvaluationError::UnknownVariable(_)) => {
                    // Conservatively treat unknown variables as Maybe.
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(true)
    }

    /// Extends the current set of constraints with additional ones.
    ///
    /// This is a simple helper used by refinement/analysis steps that
    /// accumulate constraints as they explore the state space.
    pub fn refine(&mut self, new_constraints: &[Constraint]) {
        self.constraints.extend_from_slice(new_constraints);
    }
}
