//! AST types for extraction spec JSON.
//!
//! These types are deserialized from extraction spec files (`.espec.json`)
//! produced by the extraction pipeline. The spec captures:
//! - Source code location and commit hash
//! - State fields with line-anchored evidence
//! - Methods with guards, effects, and controllability
//! - Bug documentation with attack chains
//! - Declarative model configuration (automata, transitions, compositions, properties)

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a JSON value as `Option<String>`, accepting either a string or
/// a number. Numbers are coerced to their string representation.
///
/// This lets spec authors write `"issue": 6533` (the natural form for a
/// GitHub issue or PR number) instead of being forced to quote it as
/// `"issue": "6533"`. See GAP-001.
fn deserialize_int_or_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(serde_json::Value::Number(n)) => Ok(Some(n.to_string())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string or number, got {other}"
        ))),
    }
}

/// Top-level extraction spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionSpec {
    /// Schema version identifier.
    #[serde(rename = "$schema")]
    pub schema: Option<String>,

    /// Source code location.
    pub source: SourceRef,

    /// State fields extracted from source.
    #[serde(default)]
    pub state_fields: Vec<StateField>,

    /// Methods extracted from source.
    #[serde(default)]
    pub methods: Vec<Method>,

    /// Documented bugs with attack chains.
    #[serde(default)]
    pub bugs: Vec<Bug>,

    /// Declarative model configuration for CTXDSL generation.
    pub model_config: ModelConfig,
}

/// Reference to source code location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub repo: Option<String>,
    pub commit: Option<String>,
    pub file: Option<String>,
    pub class: Option<String>,
    pub cve: Option<String>,
    pub ghsa: Option<String>,
    /// GitHub issue number. Accepts either a string (`"issue": "6533"`) or an
    /// integer (`"issue": 6533`); both forms are equivalent.
    #[serde(default, deserialize_with = "deserialize_int_or_string")]
    pub issue: Option<String>,
    /// Pull-request number that introduced the fix. Accepts string or integer
    /// form, like `issue`.
    #[serde(default, deserialize_with = "deserialize_int_or_string")]
    pub fix_pr: Option<String>,
    /// Commit hash that introduced the fix.
    #[serde(default)]
    pub fix_commit: Option<String>,
}

/// A state field extracted from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateField {
    pub id: String,
    pub field: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub line: Option<u32>,
    pub pattern: Option<String>,
    pub initial: Option<serde_json::Value>,
    pub note: Option<String>,
    pub abstraction: Option<serde_json::Value>,
}

/// A method extracted from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub id: String,
    pub name: Option<String>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub controllable: Option<bool>,
    #[serde(default)]
    pub guards: Vec<serde_json::Value>,
    #[serde(default)]
    pub effects: Vec<serde_json::Value>,
    pub note: Option<String>,
}

/// A documented bug with attack chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bug {
    pub id: String,
    pub description: Option<String>,
    pub severity: Option<String>,
    pub missing_guard: Option<serde_json::Value>,
    #[serde(default)]
    pub attack_chain: Vec<AttackStep>,
}

/// A step in an attack chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttackStep {
    pub step: Option<u32>,
    pub action: Option<String>,
    pub line: Option<u32>,
    pub effect: Option<String>,
}

// ---------------------------------------------------------------------------
// Declarative model configuration
// ---------------------------------------------------------------------------

/// Declarative model configuration for CTXDSL generation.
///
/// Contains everything needed to deterministically generate CTXDSL:
/// automata with states and transitions, compositions, properties, and controllers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Context name for the CTXDSL document.
    pub context_name: String,

    /// Controllable labels (used for alphabet, may be empty).
    #[serde(default)]
    pub controllable_labels: Vec<String>,

    /// Uncontrollable labels (used for alphabet).
    #[serde(default)]
    pub uncontrollable_labels: Vec<String>,

    /// Declarative automaton definitions.
    #[serde(default)]
    pub automata: Vec<AutomatonDef>,

    /// Composition directive (optional).
    pub composition: Option<CompositionDef>,

    /// Property definitions with formulas and expected verdicts.
    #[serde(default)]
    pub properties: Vec<PropertyDef>,

    /// Controller synthesis targets.
    #[serde(default)]
    pub controllers: Vec<ControllerDef>,
}

/// A declarative automaton definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomatonDef {
    /// Automaton identifier.
    pub id: String,

    /// States of this automaton.
    #[serde(default)]
    pub states: Vec<StateDef>,

    /// Labels declared as controllable within this automaton.
    #[serde(default)]
    pub controllable_labels: Vec<String>,

    /// Transitions.
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,

    /// Optional fields reference (for traceability, not used in generation).
    #[serde(default)]
    pub fields: Vec<String>,

    /// Optional note.
    pub note: Option<String>,

    /// Optional role description.
    pub role: Option<String>,
}

/// A state definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StateDef {
    /// Simple: just a name string.
    Simple(String),
    /// Structured: name with optional initial flag.
    Structured(StateDefStructured),
}

/// Structured state definition with optional initial flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDefStructured {
    pub name: String,
    #[serde(default)]
    pub initial: bool,
}

impl StateDef {
    pub fn name(&self) -> &str {
        match self {
            StateDef::Simple(s) => s,
            StateDef::Structured(s) => &s.name,
        }
    }

    pub fn is_initial(&self) -> bool {
        match self {
            StateDef::Simple(_) => false,
            StateDef::Structured(s) => s.initial,
        }
    }
}

/// A transition definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionDef {
    /// Source state.
    pub from: String,
    /// Target state.
    pub to: String,
    /// Label for this transition.
    pub label: String,
    /// Mode filter: arbitrary string tag; the default `"both"` means the
    /// transition is always included regardless of the `--mode` CLI value.
    /// Any other tag (e.g., `"fixed"`, `"vulnerable"`, `"as_audited"`,
    /// `"with_provider_cache"`) means the transition is included only when
    /// the CLI `--mode` matches. The CLI accepts the universal defaults
    /// `"fixed"` / `"vulnerable"` / `"both"` plus any tag that appears on at
    /// least one transition in the spec.
    #[serde(default = "default_mode_both")]
    pub mode: String,
    /// Optional traceability back to extraction spec method/guard.
    pub derived_from: Option<serde_json::Value>,
    /// Optional comment.
    pub comment: Option<String>,
}

fn default_mode_both() -> String {
    "both".to_string()
}

/// Composition directive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionDef {
    /// Composition type: "synchronous" or "asynchronous".
    #[serde(rename = "type")]
    pub type_: String,
    /// Composition name.
    pub name: String,
    /// Member automaton identifiers.
    pub members: Vec<String>,
}

/// A property definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDef {
    /// Property identifier.
    pub id: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Mu-calculus formula body.
    pub formula: Option<String>,
    /// Formula template (alternative name, for backward compat with existing specs).
    pub formula_template: Option<String>,
    /// Reference to a property template from the template catalog.
    /// When present, the template is instantiated with the given args to produce
    /// a mu-calculus formula. If both `formula` and `template_ref` are present,
    /// `formula` takes precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_ref: Option<crate::adapter::templates::TemplateRef>,
    /// "over" target automaton or composition name.
    pub over: Option<String>,
    /// Expected verdict in "fixed" mode.
    pub holds_in_fixed: Option<bool>,
    /// Expected verdict in "vulnerable" mode.
    pub holds_in_vulnerable: Option<bool>,
}

impl PropertyDef {
    /// Get the formula string, preferring `formula` over `formula_template`.
    ///
    /// Does not resolve `template_ref` — callers should use
    /// [`resolve_formula`] for template-aware resolution.
    pub fn formula_str(&self) -> Option<&str> {
        self.formula.as_deref().or(self.formula_template.as_deref())
    }
}

/// Controller synthesis target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerDef {
    /// Controller name.
    pub name: String,
    /// Source automaton or composition.
    pub source: String,
    /// Formula to satisfy.
    pub satisfying: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_spec() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": { "repo": "test/repo", "commit": "abc123" },
            "model_config": {
                "context_name": "test_model",
                "automata": [
                    {
                        "id": "Main",
                        "states": [
                            {"name": "S0", "initial": true},
                            {"name": "S1"}
                        ],
                        "transitions": [
                            {"from": "S0", "to": "S1", "label": "ev_go"}
                        ]
                    }
                ]
            }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.model_config.context_name, "test_model");
        assert_eq!(spec.model_config.automata.len(), 1);
        assert_eq!(spec.model_config.automata[0].states.len(), 2);
        assert_eq!(spec.model_config.automata[0].transitions.len(), 1);
    }

    #[test]
    fn parse_mode_filtered_transitions() {
        let json = r#"{
            "source": {},
            "model_config": {
                "context_name": "test",
                "automata": [{
                    "id": "A",
                    "states": [{"name": "Open", "initial": true}, {"name": "Closed"}],
                    "transitions": [
                        {"from": "Open", "to": "Closed", "label": "ev_close"},
                        {"from": "Closed", "to": "Closed", "label": "ev_request", "mode": "vulnerable"},
                        {"from": "Closed", "to": "Closed", "label": "noop", "mode": "both"}
                    ]
                }]
            }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        let transitions = &spec.model_config.automata[0].transitions;
        assert_eq!(transitions.len(), 3);
        assert_eq!(transitions[0].mode, "both");
        assert_eq!(transitions[1].mode, "vulnerable");
        assert_eq!(transitions[2].mode, "both");
    }

    #[test]
    fn parse_simple_state_names() {
        let json = r#"{
            "source": {},
            "model_config": {
                "context_name": "test",
                "automata": [{
                    "id": "A",
                    "states": ["S0", "S1", "S2"],
                    "transitions": []
                }]
            }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        let states = &spec.model_config.automata[0].states;
        assert_eq!(states.len(), 3);
        assert_eq!(states[0].name(), "S0");
        assert!(!states[0].is_initial());
    }

    #[test]
    fn parse_composition() {
        let json = r#"{
            "source": {},
            "model_config": {
                "context_name": "test",
                "automata": [],
                "composition": {
                    "type": "asynchronous",
                    "name": "system",
                    "members": ["A", "B"]
                }
            }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        let comp = spec.model_config.composition.unwrap();
        assert_eq!(comp.type_, "asynchronous");
        assert_eq!(comp.members, vec!["A", "B"]);
    }

    #[test]
    fn parse_properties() {
        let json = r#"{
            "source": {},
            "model_config": {
                "context_name": "test",
                "properties": [
                    {
                        "id": "safety",
                        "formula": "nu X. ((!Bad) && ([] X))",
                        "over": "Main",
                        "holds_in_fixed": true,
                        "holds_in_vulnerable": false
                    }
                ]
            }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        let props = &spec.model_config.properties;
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].formula_str(), Some("nu X. ((!Bad) && ([] X))"));
        assert_eq!(props[0].over.as_deref(), Some("Main"));
    }

    #[test]
    fn source_ref_accepts_integer_issue() {
        // GAP-001: `"issue": 6533` (integer) must parse and be coerced to "6533".
        let json = r#"{
            "source": { "issue": 6533 },
            "model_config": { "context_name": "t" }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.source.issue, Some("6533".to_string()));
    }

    #[test]
    fn source_ref_accepts_string_issue() {
        // GAP-001: `"issue": "6533"` (string) must continue to parse.
        let json = r#"{
            "source": { "issue": "6533" },
            "model_config": { "context_name": "t" }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.source.issue, Some("6533".to_string()));
    }

    #[test]
    fn source_ref_accepts_fix_pr_and_fix_commit() {
        // GAP-001: `fix_pr` (integer) and `fix_commit` (string) must populate.
        let json = r#"{
            "source": {
                "fix_pr": 3126,
                "fix_commit": "a37c4d6f4928a3e1d91f2061fc6af142b17e0408"
            },
            "model_config": { "context_name": "t" }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.source.fix_pr, Some("3126".to_string()));
        assert_eq!(
            spec.source.fix_commit,
            Some("a37c4d6f4928a3e1d91f2061fc6af142b17e0408".to_string())
        );
    }

    #[test]
    fn source_ref_fix_fields_default_to_none() {
        // GAP-001: when `fix_pr` / `fix_commit` are absent, both stay `None`.
        let json = r#"{
            "source": { "repo": "foo/bar" },
            "model_config": { "context_name": "t" }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.source.fix_pr, None);
        assert_eq!(spec.source.fix_commit, None);
    }

    #[test]
    fn parse_full_mcp_style_spec() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {
                "repo": "modelcontextprotocol/typescript-sdk",
                "commit": "9ed62fe7d8be",
                "file": "packages/server/src/server/streamableHttp.ts",
                "class": "WebStandardStreamableHTTPServerTransport"
            },
            "state_fields": [
                {"id": "started", "field": "_started", "type": "boolean", "line": 227, "initial": false}
            ],
            "methods": [
                {"id": "start", "controllable": true, "guards": [], "effects": []}
            ],
            "bugs": [
                {"id": "bug1", "description": "test bug", "attack_chain": [
                    {"step": 1, "action": "do something", "line": 100}
                ]}
            ],
            "model_config": {
                "context_name": "mcp_test",
                "controllable_labels": ["ev_start"],
                "uncontrollable_labels": ["ev_request"],
                "automata": [
                    {
                        "id": "Lifecycle",
                        "states": [{"name": "Idle", "initial": true}, {"name": "Active"}],
                        "controllable_labels": ["ev_start"],
                        "transitions": [
                            {"from": "Idle", "to": "Active", "label": "ev_start"},
                            {"from": "Active", "to": "Active", "label": "ev_request"}
                        ]
                    }
                ],
                "properties": [
                    {
                        "id": "safety",
                        "formula": "nu X. ([] X)",
                        "over": "Lifecycle",
                        "holds_in_fixed": true,
                        "holds_in_vulnerable": true
                    }
                ]
            }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(
            spec.source.repo.as_deref(),
            Some("modelcontextprotocol/typescript-sdk")
        );
        assert_eq!(spec.state_fields.len(), 1);
        assert_eq!(spec.methods.len(), 1);
        assert_eq!(spec.bugs.len(), 1);
        assert_eq!(spec.model_config.automata.len(), 1);
        assert_eq!(spec.model_config.properties.len(), 1);
    }
}
