//! Shared domain types for state abstraction across adapters.
//!
//! These types define how state fields are abstracted into finite domains
//! for explicit-state enumeration. Used by the extraction adapter (for
//! source-code-anchored specs) and the SystemVerilog adapter (for
//! register-based Kripke construction).

use serde::Deserialize;
use std::collections::BTreeMap;

/// Default upper bound for `BoundedCounter` fields when no explicit bound is set.
///
/// Chosen as a heuristic: small enough to enumerate quickly (4 states: 0,1,2,3),
/// large enough to capture common patterns like empty/non-empty/full and off-by-one.
/// Users should always prefer setting an explicit bound in extraction specs or
/// sidecar annotations; this default exists only as a fallback.
pub const DEFAULT_COUNTER_BOUND: i64 = 3;

/// Available abstraction strategies for state fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbstractionType {
    /// Exact boolean (2 states: true/false).
    Boolean,
    /// Presence/absence (2 states: None/Some).
    Presence,
    /// Bounded counter (0..bound states).
    BoundedCounter,
    /// Explicit enum variants.
    EnumValues,
    /// Ignored — field not included in state space.
    Ignored,
}

/// Abstract value in a field's domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AbstractValue {
    /// Boolean: true or false.
    Bool(bool),
    /// Presence: present or absent.
    Present(bool),
    /// Bounded counter: 0..bound.
    Counter(i64),
    /// Enum variant by name.
    Variant(String),
}

impl AbstractValue {
    pub fn display_short(&self) -> String {
        match self {
            AbstractValue::Bool(true) => "T".to_string(),
            AbstractValue::Bool(false) => "F".to_string(),
            AbstractValue::Present(true) => "Some".to_string(),
            AbstractValue::Present(false) => "None".to_string(),
            AbstractValue::Counter(n) => n.to_string(),
            AbstractValue::Variant(v) => v.clone(),
        }
    }
}

/// Definition of a field's abstract domain.
#[derive(Debug, Clone)]
pub struct FieldDomain {
    /// Field name (as it appears in source).
    pub name: String,
    /// Abstraction type.
    pub abstraction: AbstractionType,
    /// Upper bound for bounded counter.
    pub bound: Option<i64>,
    /// Explicit variants for enum.
    pub variants: Option<Vec<String>>,
    /// Initial value.
    pub initial: AbstractValue,
}

impl FieldDomain {
    /// Enumerate all values in this domain.
    pub fn values(&self) -> Vec<AbstractValue> {
        match self.abstraction {
            AbstractionType::Boolean => vec![AbstractValue::Bool(false), AbstractValue::Bool(true)],
            AbstractionType::Presence => {
                vec![AbstractValue::Present(false), AbstractValue::Present(true)]
            }
            AbstractionType::BoundedCounter => {
                let bound = self.bound.unwrap_or(DEFAULT_COUNTER_BOUND);
                (0..=bound).map(AbstractValue::Counter).collect()
            }
            AbstractionType::EnumValues => self
                .variants
                .as_ref()
                .map(|vs| {
                    vs.iter()
                        .map(|v| AbstractValue::Variant(v.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            AbstractionType::Ignored => vec![],
        }
    }

    /// Number of abstract values.
    pub fn cardinality(&self) -> usize {
        self.values().len()
    }
}

/// A concrete abstract state — assignment of values to all fields.
pub type AbstractState = BTreeMap<String, AbstractValue>;
