//! State variable abstraction for CLTS verification.
//!
//! This module implements abstract domains and state unrolling to support
//! guards and effects while preserving μ-calculus monotonicity.

pub mod constraint;
pub mod constraints;
pub mod domains;
pub mod evaluator;
pub mod expression;
pub mod heuristics;
pub mod integration;
pub mod operations;
pub mod refinement;
pub mod state;
pub mod unrolling;
pub mod value;

pub use constraint::{Constraint, ConstraintKind};
pub use constraints::ConstraintManager;
pub use domains::{BoolDomain, IntDomain};
pub use evaluator::{EvaluationError, evaluate_expr, evaluate_guard};
pub use expression::{Expr, GuardExpr, GuardResult};
pub use heuristics::{
    HeuristicConfig, StateSpaceStats, VariableContext, apply_widening_if_needed,
    select_abstract_type,
};
pub use integration::{JsonToUnrollingConverter, JsonUnrollingOptions};
pub use operations::ValueOperations;
pub use state::AbstractState;
pub use unrolling::{
    BuildingContext, ConflictError, Effect, OriginalState, OriginalTransition, UnrolledClts,
    UnrolledTransition, UnrollingError, UnrollingOptions, VariableDecl, check_conflicts,
    compatible, unroll_states,
};
pub use value::AbstractValue;
