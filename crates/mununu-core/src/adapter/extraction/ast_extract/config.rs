//! Configuration types for AST-based extraction (`.extract.json`).
//!
//! The config declares WHAT to extract from source code — target classes,
//! state fields, abstraction strategy, controllability — but NOT the
//! automaton topology. The tool derives states and transitions from the AST.

use crate::adapter::domain::AbstractionType;
use serde::Deserialize;
use std::collections::HashMap;

/// Top-level extraction config.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtractionConfig {
    /// Schema version.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,

    /// Domain profile name (e.g., "mcp_server", "protocol_implementation").
    /// Provides non-trivial defaults for controllability, abstraction, composition.
    pub domain: Option<String>,

    /// Source language. If omitted, detected from file extension.
    pub language: Option<String>,

    /// Source file location.
    pub source: SourceConfig,

    /// Extraction targets (one per class/struct to analyze).
    pub targets: Vec<TargetConfig>,

    /// Call summaries for external library methods.
    /// Merged with domain-profile built-in summaries; config overrides take precedence.
    #[serde(default)]
    pub call_summaries: HashMap<String, CallSummary>,

    /// Composition directive for multi-target extraction.
    pub composition: Option<CompositionConfig>,

    /// Properties to verify on the extracted model.
    #[serde(default)]
    pub properties: Vec<PropertyConfig>,

    /// Context name for the generated .espec.json.
    pub context_name: Option<String>,
}

/// Source file location.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    /// Path to the source file (relative to repo root or absolute).
    pub file: String,
    /// Repository identifier (e.g., "owner/repo").
    pub repo: Option<String>,
    /// Pinned commit hash.
    pub commit: Option<String>,
}

/// A target class/struct to extract from.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
    /// Class or struct name to locate in the source file.
    pub class: String,

    /// Automaton identifier in the generated spec.
    /// Defaults to the class name if omitted.
    pub automaton_id: Option<String>,

    /// State fields to include in the model.
    pub state_fields: StateFieldsConfig,

    /// Methods to include/exclude as transitions.
    #[serde(default)]
    pub methods: MethodsConfig,

    /// Controllability overrides (per-method).
    /// Keys are method names, values are "controllable" or "uncontrollable".
    #[serde(default)]
    pub controllability_overrides: HashMap<String, String>,

    /// Custom state name mapping.
    /// Maps field value combinations to human-readable state names.
    /// e.g., `{"started_T_closed_F_initialized_F": "Started"}`
    #[serde(default)]
    pub state_names: HashMap<String, String>,
}

/// State fields configuration — which fields to model and how.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StateFieldsConfig {
    /// Simple: list of field names (abstraction inferred from domain profile).
    Simple(Vec<String>),
    /// Detailed: explicit include list + abstraction overrides.
    Detailed(StateFieldsDetailed),
}

impl StateFieldsConfig {
    /// Get the list of field names to include.
    pub fn field_names(&self) -> &[String] {
        match self {
            StateFieldsConfig::Simple(names) => names,
            StateFieldsConfig::Detailed(d) => &d.include,
        }
    }

    /// Get abstraction override for a field, if any.
    pub fn abstraction_for(&self, field: &str) -> Option<&AbstractionConfig> {
        match self {
            StateFieldsConfig::Simple(_) => None,
            StateFieldsConfig::Detailed(d) => d.abstraction_overrides.get(field),
        }
    }
}

/// Detailed state fields configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct StateFieldsDetailed {
    /// Field names to include in the model.
    pub include: Vec<String>,
    /// Abstraction overrides per field.
    #[serde(default)]
    pub abstraction_overrides: HashMap<String, AbstractionConfig>,
}

/// How to abstract a state field.
#[derive(Debug, Clone, Deserialize)]
pub struct AbstractionConfig {
    /// Abstraction type.
    #[serde(rename = "type")]
    pub type_: AbstractionType,
    /// Upper bound for bounded_counter.
    pub bound: Option<i64>,
    /// Explicit enum variants for enum_values.
    pub variants: Option<Vec<String>>,
}

// AbstractionType is re-exported from crate::adapter::domain.

/// Method inclusion/exclusion configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MethodsConfig {
    /// Methods to include (if empty, all public methods are included).
    #[serde(default)]
    pub include: Vec<String>,
    /// Methods to exclude (applied after include).
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// How an external library call affects model state.
#[derive(Debug, Clone, Deserialize)]
pub struct CallSummary {
    /// Effect on state: "increment_counter", "decrement_counter", "reset_to_zero",
    /// "set_true", "set_false", "set_present", "set_absent", "read_only", "none".
    pub effect: String,
    /// Which state field is affected ("receiver" = the object the method is called on).
    pub on_field: Option<String>,
    /// Guard condition implied by this call (e.g., "counter_gt_zero" for Map.get).
    pub guard: Option<String>,
    /// Human-readable explanation.
    pub note: Option<String>,
}

/// Composition directive.
#[derive(Debug, Clone, Deserialize)]
pub struct CompositionConfig {
    /// "synchronous" or "asynchronous".
    #[serde(rename = "type")]
    pub type_: String,
    /// Composition name.
    pub name: String,
}

/// Property to verify.
#[derive(Debug, Clone, Deserialize)]
pub struct PropertyConfig {
    /// Property identifier.
    pub id: String,
    /// Mu-calculus formula body.
    pub formula: String,
    /// Target automaton or composition.
    pub over: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let json = r#"{
            "source": {"file": "src/server.ts"},
            "targets": [{
                "class": "Server",
                "state_fields": ["_started", "_closed"],
                "methods": {"include": ["start", "close"]}
            }]
        }"#;
        let config: ExtractionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.targets.len(), 1);
        assert_eq!(config.targets[0].class, "Server");
        assert_eq!(
            config.targets[0].state_fields.field_names(),
            &["_started", "_closed"]
        );
    }

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "$schema": "extraction_config_v1",
            "domain": "mcp_server",
            "language": "typescript",
            "source": {
                "file": "packages/server/src/server/streamableHttp.ts",
                "repo": "modelcontextprotocol/typescript-sdk",
                "commit": "9ed62fe..."
            },
            "targets": [{
                "class": "WebStandardStreamableHTTPServerTransport",
                "automaton_id": "TransportLifecycle",
                "state_fields": {
                    "include": ["_started", "_closed", "_initialized"],
                    "abstraction_overrides": {
                        "_streamMapping": {"type": "bounded_counter", "bound": 3}
                    }
                },
                "methods": {
                    "include": ["start", "close", "handlePostRequest", "send"],
                    "exclude": ["writeSSEEvent"]
                },
                "controllability_overrides": {
                    "send": "controllable"
                }
            }],
            "call_summaries": {
                "Map.prototype.clear": {"effect": "reset_to_zero", "on_field": "receiver"}
            },
            "composition": {
                "type": "asynchronous",
                "name": "transport_system"
            },
            "properties": [{
                "id": "no_requests_after_close",
                "formula": "nu X. ((!Closed || ([ev_handlePostRequest] false)) && ([] X))",
                "over": "TransportLifecycle"
            }]
        }"#;
        let config: ExtractionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.domain.as_deref(), Some("mcp_server"));
        assert_eq!(
            config.targets[0].state_fields.field_names(),
            &["_started", "_closed", "_initialized"]
        );
        assert!(
            config.targets[0]
                .state_fields
                .abstraction_for("_streamMapping")
                .is_some()
        );
        assert_eq!(config.properties.len(), 1);
        assert_eq!(config.call_summaries.len(), 1);
    }

    #[test]
    fn parse_simple_state_fields() {
        let json = r#"{
            "source": {"file": "test.ts"},
            "targets": [{
                "class": "Test",
                "state_fields": ["a", "b", "c"]
            }]
        }"#;
        let config: ExtractionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.targets[0].state_fields.field_names(),
            &["a", "b", "c"]
        );
        assert!(
            config.targets[0]
                .state_fields
                .abstraction_for("a")
                .is_none()
        );
    }

    #[test]
    fn parse_controllability_overrides() {
        let json = r#"{
            "source": {"file": "test.ts"},
            "targets": [{
                "class": "Server",
                "state_fields": ["_active"],
                "controllability_overrides": {
                    "start": "controllable",
                    "handleRequest": "uncontrollable"
                }
            }]
        }"#;
        let config: ExtractionConfig = serde_json::from_str(json).unwrap();
        let overrides = &config.targets[0].controllability_overrides;
        assert_eq!(overrides.get("start").unwrap(), "controllable");
        assert_eq!(overrides.get("handleRequest").unwrap(), "uncontrollable");
    }
}
