//! Integration of state unrolling with JSON model input.

use super::heuristics::HeuristicConfig;
use super::unrolling::{
    Effect, OriginalState, OriginalTransition, UnrollingError, UnrollingOptions, VariableDecl,
};
use serde_json::Value;

/// Converts JSON model structures to unrolling structures.
///
/// This converter works with JSON values to avoid dependency on domain-specific structs.
pub struct JsonToUnrollingConverter;

impl JsonToUnrollingConverter {
    /// Converts states (from JSON) to original states.
    #[allow(clippy::result_large_err)]
    pub fn convert_states_from_json(
        states: &[Value],
    ) -> Result<Vec<OriginalState>, UnrollingError> {
        states
            .iter()
            .map(|s| {
                Ok(OriginalState {
                    name: s["name"]
                        .as_str()
                        .ok_or_else(|| UnrollingError::ParseError {
                            expression: "state.name".to_string(),
                            error: "missing or invalid name".to_string(),
                        })?
                        .to_string(),
                    initial: s.get("initial").and_then(|v| v.as_bool()).unwrap_or(false),
                })
            })
            .collect()
    }

    /// Converts transitions (from JSON) to original transitions.
    #[allow(clippy::result_large_err)]
    pub fn convert_transitions_from_json(
        transitions: &[Value],
    ) -> Result<Vec<OriginalTransition>, UnrollingError> {
        transitions
            .iter()
            .map(|t| {
                Ok(OriginalTransition {
                    from: t["from"]
                        .as_str()
                        .ok_or_else(|| UnrollingError::ParseError {
                            expression: "transition.from".to_string(),
                            error: "missing or invalid from".to_string(),
                        })?
                        .to_string(),
                    to: t["to"]
                        .as_str()
                        .ok_or_else(|| UnrollingError::ParseError {
                            expression: "transition.to".to_string(),
                            error: "missing or invalid to".to_string(),
                        })?
                        .to_string(),
                    label: t["label"]
                        .as_str()
                        .ok_or_else(|| UnrollingError::ParseError {
                            expression: "transition.label".to_string(),
                            error: "missing or invalid label".to_string(),
                        })?
                        .to_string(),
                    guard: t
                        .get("guard")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    effects: t
                        .get("effects")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .map(|e| Effect {
                                    target: e["target"].as_str().unwrap_or("").to_string(),
                                    value_expr: e["value"].as_str().unwrap_or("0").to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    // R.5 Item K sub-item K.2b (2026-06-06) — JSON-
                    // sourced transitions default to Sharp; modality
                    // is not yet plumbed through the JSON schema
                    // (queued for a separate JSON-schema follow-up).
                    modality: crate::context_dsl::ast::TransitionModalitySpec::Sharp,
                    // R.5 Item K sub-item K.1b-unrolled — JSON
                    // schema doesn't carry multi-target hyper-must;
                    // defaults to empty (singleton hyper-must when
                    // modality is MustOnly).
                    additional_targets: Vec::new(),
                })
            })
            .collect()
    }

    /// Converts variables (from JSON) to variable declarations.
    #[allow(clippy::result_large_err)]
    pub fn convert_variables_from_json(
        variables: &[Value],
    ) -> Result<Vec<VariableDecl>, UnrollingError> {
        variables
            .iter()
            .map(|v| {
                let initial = v.get("initial").map(|val| {
                    // Convert JSON value to string
                    match val {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => "0".to_string(), // Default
                    }
                });
                Ok(VariableDecl {
                    name: v["name"]
                        .as_str()
                        .ok_or_else(|| UnrollingError::ParseError {
                            expression: "variable.name".to_string(),
                            error: "missing or invalid name".to_string(),
                        })?
                        .to_string(),
                    ty: v
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("bool")
                        .to_string(),
                    initial,
                })
            })
            .collect()
    }
}

/// Options for unrolling during translation.
#[derive(Debug, Clone, Default)]
pub struct JsonUnrollingOptions {
    /// Enable unrolling (default: false for backward compatibility)
    pub enabled: bool,
    /// Maximum states per location
    pub max_states_per_location: Option<usize>,
    /// Use interval abstraction
    pub use_interval_abstraction: bool,
}

impl From<JsonUnrollingOptions> for UnrollingOptions {
    fn from(opts: JsonUnrollingOptions) -> Self {
        Self {
            max_states_per_location: opts.max_states_per_location,
            use_interval_abstraction: opts.use_interval_abstraction,
            widen_after: None,
            heuristic_config: Some(HeuristicConfig::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_states_from_json() {
        let states = vec![
            json!({"name": "Start", "initial": true}),
            json!({"name": "End", "initial": false}),
        ];

        let original_states = JsonToUnrollingConverter::convert_states_from_json(&states).unwrap();
        assert_eq!(original_states.len(), 2);
        assert_eq!(original_states[0].name, "Start");
        assert!(original_states[0].initial);
    }

    #[test]
    fn test_convert_transitions_from_json() {
        let transitions = vec![json!({
            "from": "Start",
            "to": "End",
            "label": "complete"
        })];

        let original_transitions =
            JsonToUnrollingConverter::convert_transitions_from_json(&transitions).unwrap();
        assert_eq!(original_transitions.len(), 1);
        assert_eq!(original_transitions[0].from, "Start");
    }

    #[test]
    fn test_convert_variables_from_json() {
        let variables = vec![json!({
            "name": "x",
            "type": "i64",
            "initial": 5
        })];

        let vars = JsonToUnrollingConverter::convert_variables_from_json(&variables).unwrap();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].name, "x");
        assert_eq!(vars[0].initial, Some("5".to_string()));
    }
}
