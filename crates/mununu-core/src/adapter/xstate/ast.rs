//! AST types for XState v5 JSON machine definitions.
//!
//! These types are deserialized directly from XState JSON using serde.
//! The structure mirrors the XState v5 format with Mununu-specific
//! annotations for controllability and property specification.

use serde::Deserialize;
use std::collections::HashMap;

/// Top-level XState machine definition.
#[derive(Debug, Clone, Deserialize)]
pub struct XStateMachine {
    /// Machine identifier.
    pub id: Option<String>,

    /// Initial state name.
    pub initial: Option<String>,

    /// Context variables (typed state data).
    #[serde(default)]
    pub context: HashMap<String, ContextValue>,

    /// State definitions (recursive for hierarchy).
    #[serde(default)]
    pub states: HashMap<String, XStateNode>,

    /// Mununu-specific annotations for synthesis.
    #[serde(rename = "__mununu", default)]
    pub mununu: Option<MununuAnnotations>,
}

/// A state node in the XState machine (recursive for compound states).
#[derive(Debug, Clone, Deserialize)]
pub struct XStateNode {
    /// State type: `"parallel"`, `"final"`, `"history"`, or absent (simple/compound).
    #[serde(rename = "type")]
    pub type_: Option<String>,

    /// Initial child state (for compound states).
    pub initial: Option<String>,

    /// Event-keyed transitions.
    #[serde(default)]
    pub on: HashMap<String, TransitionConfig>,

    /// Child states (makes this a compound or parallel state).
    #[serde(default)]
    pub states: Option<HashMap<String, XStateNode>>,

    /// Entry actions.
    #[serde(default)]
    pub entry: Vec<ActionConfig>,

    /// Exit actions.
    #[serde(default)]
    pub exit: Vec<ActionConfig>,
}

/// Transition configuration — XState supports multiple formats.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TransitionConfig {
    /// Simple: `"TIMER": "yellow"`
    Simple(String),

    /// Single guarded: `"TIMER": { "target": "yellow", "guard": "isReady" }`
    Guarded(GuardedTransition),

    /// Array of guarded: `"TIMER": [{ "target": "a", "guard": "g1" }, { "target": "b" }]`
    Array(Vec<GuardedTransition>),
}

/// A single guarded transition.
#[derive(Debug, Clone, Deserialize)]
pub struct GuardedTransition {
    /// Target state name.
    pub target: Option<String>,

    /// Guard condition name.
    pub guard: Option<String>,

    /// Actions to execute on this transition.
    #[serde(default)]
    pub actions: Vec<ActionConfig>,
}

/// Action configuration — simplified for synthesis.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ActionConfig {
    /// Named action reference: `"incrementCount"`
    Named(String),

    /// Structured action: `{ "type": "assign", "params": { "counter": 5 } }`
    Structured(StructuredAction),
}

/// A structured action with type and parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct StructuredAction {
    /// Action type (e.g., `"assign"`).
    #[serde(rename = "type")]
    pub type_: String,

    /// Action parameters.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// Context variable value — used for initial context.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ContextValue {
    Bool(bool),
    Number(f64),
    String(String),
}

/// Mununu-specific annotations embedded in the XState JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct MununuAnnotations {
    /// Events classified as controllable (system-initiated).
    #[serde(default)]
    pub controllable: Vec<String>,

    /// Events classified as uncontrollable (environment/user).
    #[serde(default)]
    pub uncontrollable: Vec<String>,

    /// Variable bounds for numeric context variables.
    #[serde(default)]
    pub bounds: HashMap<String, (i64, i64)>,

    /// Temporal properties to verify.
    #[serde(default)]
    pub properties: Vec<PropertyAnnotation>,
}

/// A property annotation in the Mununu metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct PropertyAnnotation {
    /// Property name.
    pub name: String,

    /// LTL or mu-calculus formula string.
    /// Optional when `template_ref` is present.
    #[serde(default)]
    pub formula: Option<String>,

    /// Property role: `"guarantee"`, `"assumption"`, `"invariant"`, or `"standalone"`.
    #[serde(default = "default_role")]
    pub role: String,

    /// Reference to a property template from the template catalog.
    /// When present, the template is instantiated to produce a mu-calculus formula.
    /// If both `formula` and `template_ref` are present, `formula` takes precedence.
    #[serde(default)]
    pub template_ref: Option<crate::adapter::templates::TemplateRef>,
}

fn default_role() -> String {
    "standalone".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_machine() {
        let json = r#"{
            "id": "trafficLight",
            "initial": "green",
            "states": {
                "green": { "on": { "TIMER": "yellow" } },
                "yellow": { "on": { "TIMER": "red" } },
                "red": { "on": { "TIMER": "green" } }
            }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        assert_eq!(machine.id.as_deref(), Some("trafficLight"));
        assert_eq!(machine.initial.as_deref(), Some("green"));
        assert_eq!(machine.states.len(), 3);
    }

    #[test]
    fn parse_guarded_transitions() {
        let json = r#"{
            "id": "test",
            "initial": "a",
            "states": {
                "a": {
                    "on": {
                        "GO": { "target": "b", "guard": "isReady" },
                        "MULTI": [
                            { "target": "b", "guard": "g1" },
                            { "target": "c" }
                        ]
                    }
                },
                "b": {},
                "c": {}
            }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        let a = &machine.states["a"];
        assert!(matches!(a.on["GO"], TransitionConfig::Guarded(_)));
        assert!(matches!(a.on["MULTI"], TransitionConfig::Array(_)));
    }

    #[test]
    fn parse_mununu_annotations() {
        let json = r#"{
            "id": "test",
            "initial": "s0",
            "states": { "s0": {} },
            "__mununu": {
                "controllable": ["TIMER"],
                "uncontrollable": ["USER_INPUT"],
                "bounds": { "counter": [0, 10] },
                "properties": [
                    { "name": "safe", "formula": "nu X. ([] X)", "role": "guarantee" }
                ]
            }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        let ann = machine.mununu.unwrap();
        assert_eq!(ann.controllable, vec!["TIMER"]);
        assert_eq!(ann.uncontrollable, vec!["USER_INPUT"]);
        assert_eq!(ann.bounds["counter"], (0, 10));
        assert_eq!(ann.properties.len(), 1);
        assert_eq!(ann.properties[0].name, "safe");
    }

    #[test]
    fn parse_parallel_state() {
        let json = r#"{
            "id": "test",
            "initial": "main",
            "states": {
                "main": {
                    "type": "parallel",
                    "states": {
                        "regionA": {
                            "initial": "off",
                            "states": {
                                "off": { "on": { "TOGGLE": "on" } },
                                "on": { "on": { "TOGGLE": "off" } }
                            }
                        },
                        "regionB": {
                            "initial": "idle",
                            "states": {
                                "idle": { "on": { "START": "active" } },
                                "active": {}
                            }
                        }
                    }
                }
            }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        let main = &machine.states["main"];
        assert_eq!(main.type_.as_deref(), Some("parallel"));
        let children = main.states.as_ref().unwrap();
        assert_eq!(children.len(), 2);
        assert!(children.contains_key("regionA"));
        assert!(children.contains_key("regionB"));
    }

    #[test]
    fn parse_context_values() {
        let json = r#"{
            "id": "test",
            "initial": "s0",
            "context": {
                "count": 0,
                "enabled": true,
                "mode": "normal"
            },
            "states": { "s0": {} }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        assert_eq!(machine.context.len(), 3);
        assert!(matches!(machine.context["count"], ContextValue::Number(n) if n == 0.0));
        assert!(matches!(
            machine.context["enabled"],
            ContextValue::Bool(true)
        ));
        assert!(matches!(machine.context["mode"], ContextValue::String(ref s) if s == "normal"));
    }

    #[test]
    fn parse_compound_state() {
        let json = r#"{
            "id": "test",
            "initial": "auth",
            "states": {
                "auth": {
                    "initial": "idle",
                    "states": {
                        "idle": { "on": { "LOGIN": "pending" } },
                        "pending": { "on": { "SUCCESS": "done" } },
                        "done": {}
                    }
                }
            }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        let auth = &machine.states["auth"];
        assert_eq!(auth.initial.as_deref(), Some("idle"));
        assert_eq!(auth.states.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn parse_actions() {
        let json = r#"{
            "id": "test",
            "initial": "s0",
            "states": {
                "s0": {
                    "entry": ["log"],
                    "on": {
                        "GO": {
                            "target": "s1",
                            "actions": [
                                "notify",
                                { "type": "assign", "params": { "count": 5 } }
                            ]
                        }
                    }
                },
                "s1": {}
            }
        }"#;
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        let s0 = &machine.states["s0"];
        assert_eq!(s0.entry.len(), 1);
        assert!(matches!(&s0.entry[0], ActionConfig::Named(n) if n == "log"));
    }
}
