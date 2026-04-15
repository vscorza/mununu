//! Abstract state representation.

use super::constraint::Constraint;
use super::constraints::ConstraintManager;
use super::value::AbstractValue;
use std::collections::HashMap;
use std::fmt;

/// Abstract state combining location and variable values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbstractState {
    /// Original state name (e.g., "Executing")
    pub location: String,
    /// Variable values in this abstract state
    pub variables: HashMap<String, AbstractValue>,
    /// Global constraints on variables
    pub constraints: ConstraintManager,
}

impl AbstractState {
    /// Creates a new abstract state with a location.
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            variables: HashMap::new(),
            constraints: ConstraintManager::new(),
        }
    }

    /// Sets a variable value.
    pub fn set_variable(&mut self, name: impl Into<String>, value: AbstractValue) {
        self.variables.insert(name.into(), value);
    }

    /// Gets a variable value.
    pub fn get_variable(&self, name: &str) -> Option<&AbstractValue> {
        self.variables.get(name)
    }

    /// Adds a constraint.
    pub fn add_constraint(&mut self, constraint: Constraint) {
        self.constraints.add(constraint);
    }

    /// Generates a state name for this abstract state.
    pub fn state_name(&self) -> String {
        let mut parts = vec![self.location.clone()];

        // Add variable values (for small domains)
        let mut var_parts: Vec<_> = self.variables.iter().collect();
        var_parts.sort_by_key(|(name, _)| *name);

        for (var, value) in var_parts {
            match value {
                AbstractValue::BoolConstant(b) => {
                    parts.push(format!("{}_{}", var, b));
                }
                AbstractValue::BoolSet(set) => {
                    if set.len() == 1 {
                        parts.push(format!("{}_{}", var, set.iter().next().unwrap()));
                    } else {
                        parts.push(format!("{}_unknown", var));
                    }
                }
                AbstractValue::IntConstant(n) => {
                    parts.push(format!("{}_{}", var, n));
                }
                AbstractValue::IntInterval(min, max) => {
                    if min == max {
                        parts.push(format!("{}_{}", var, min));
                    } else {
                        parts.push(format!("{}_[{},{}]", var, min, max));
                    }
                }
                AbstractValue::IntSet(set) => {
                    if set.len() == 1 {
                        parts.push(format!("{}_{}", var, set.iter().next().unwrap()));
                    } else {
                        parts.push(format!("{}_set", var));
                    }
                }
                AbstractValue::SymbolConstant(s) => {
                    parts.push(format!("{}_{}", var, s));
                }
                AbstractValue::SymbolSet(set) => {
                    if set.len() == 1 {
                        parts.push(format!("{}_{}", var, set.iter().next().unwrap()));
                    } else {
                        parts.push(format!("{}_set", var));
                    }
                }
                AbstractValue::IntTop | AbstractValue::SymbolTop => {
                    parts.push(format!("{}_top", var));
                }
                AbstractValue::PositiveInfinity => {
                    parts.push(format!("{}_inf", var));
                }
                AbstractValue::NegativeInfinity => {
                    parts.push(format!("{}_-inf", var));
                }
                AbstractValue::Undefined => {
                    parts.push(format!("{}_undefined", var));
                }
            }
        }

        parts.join("_")
    }
}

impl fmt::Display for AbstractState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.state_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abstract_state_creation() {
        let mut state = AbstractState::new("Executing");
        state.set_variable("x", AbstractValue::int_constant(5));
        state.set_variable("y", AbstractValue::bool_constant(true));

        assert_eq!(
            state.get_variable("x"),
            Some(&AbstractValue::int_constant(5))
        );
        assert_eq!(
            state.get_variable("y"),
            Some(&AbstractValue::bool_constant(true))
        );
    }

    #[test]
    fn test_abstract_state_name() {
        let mut state = AbstractState::new("Executing");
        state.set_variable("x", AbstractValue::int_constant(5));
        state.set_variable("y", AbstractValue::bool_constant(true));

        let name = state.state_name();
        assert!(name.contains("Executing"));
        assert!(name.contains("x_5"));
        assert!(name.contains("y_true"));
    }
}
