//! Cross-domain property template system.
//!
//! Templates define parameterized mu-calculus formula patterns that can be
//! instantiated with concrete arguments across any domain (RTL, agentic,
//! software extraction, synthesis). Templates resolve to
//! `PropertyFormula::MuCalculus(String)` — the emitter and evaluator see no
//! difference from a hand-written formula.
//!
//! # Architecture
//!
//! ```text
//! builtin_templates.json ──include_str!──► TemplateCatalog
//!                                              │
//!                     TemplateRef { id, args } ─┤
//!                                              ▼
//!                                   TemplateRegistry::instantiate()
//!                                              │
//!                                              ▼
//!                                   PropertyFormula::MuCalculus(String)
//! ```
//!
//! Templates can be referenced from:
//! - `.espec.json` properties (`template_ref` field)
//! - `.mununu.json` SV sidecar properties (`template_ref` field)
//! - XState `__mununu` property annotations (`template_ref` field)
//! - CLI flags (`--template ID --template-arg KEY=VALUE`)
//! - API requests (`template_ref` in verify/synthesize payloads)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use super::ir::{PropertyKind, PropertyRole};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Domain tag for filtering templates by context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateDomain {
    /// Register-transfer level hardware (SystemVerilog)
    Rtl,
    /// Agentic AI protocols (MCP, CrewAI, LangGraph, A2A)
    Agentic,
    /// Software extraction (TypeScript, Python, Rust servers)
    Software,
    /// Synthesis benchmarks (TLSF, AIGER)
    Synthesis,
    /// Applies to all domains.
    Universal,
}

/// Type constraint for a template parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ParamType {
    /// A state predicate name (must exist as a state in the model).
    Predicate,
    /// A state name in the target automaton.
    State,
    /// A positive integer bound.
    Integer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    /// A transition label/event name.
    Label,
    /// Free-form mu-calculus sub-expression.
    Expression,
}

/// Definition of a template parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateParam {
    /// Parameter name, used as placeholder in the formula pattern (e.g., `"TARGET"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Type constraint for validation.
    pub param_type: ParamType,
    /// Default value (used when the parameter is not provided).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Whether this parameter is required.
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

/// A property template definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyTemplate {
    /// Unique template identifier (e.g., `"no_deadlock"`, `"reachable"`).
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Description of what this template checks.
    pub description: String,
    /// Property kind: `"safety"`, `"liveness"`, `"fairness"`.
    pub kind: String,
    /// Property role: `"standalone"`, `"guarantee"`, `"assumption"`, `"invariant"`.
    #[serde(default = "default_standalone")]
    pub role: String,
    /// Domains this template applies to.
    pub domains: Vec<TemplateDomain>,
    /// Parameters that must be bound at instantiation time.
    #[serde(default)]
    pub params: Vec<TemplateParam>,
    /// Mu-calculus formula pattern with `${PARAM}` placeholders.
    pub formula_pattern: String,
    /// Domain-specific hints for parameter binding (domain → param → hint text).
    #[serde(default)]
    pub domain_hints: HashMap<String, HashMap<String, String>>,
    /// Tags for search/filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_standalone() -> String {
    "standalone".to_string()
}

/// A concrete template reference (template ID + bound arguments).
///
/// Used in `.espec.json`, `.mununu.json`, XState `__mununu`, CLI, and API
/// requests as an alternative to a raw formula string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRef {
    /// Template ID to instantiate.
    pub template: String,
    /// Argument bindings: param_name → value.
    #[serde(default)]
    pub args: HashMap<String, String>,
}

/// The full template catalog (deserialized from JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateCatalog {
    pub version: String,
    pub templates: Vec<PropertyTemplate>,
}

/// Result of template instantiation — ready to become a `PropertySpec`.
#[derive(Debug, Clone)]
pub struct InstantiatedProperty {
    /// Generated property name (template ID + args for uniqueness).
    pub name: String,
    /// Property kind derived from the template.
    pub kind: PropertyKind,
    /// Concrete mu-calculus formula (no placeholders).
    pub formula: String,
    /// Property role derived from the template.
    pub role: PropertyRole,
    /// Traceability: which template produced this.
    pub source_template: String,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors during template instantiation.
#[derive(Debug, Clone)]
pub enum TemplateError {
    /// Template ID not found in the registry.
    UnknownTemplate(String),
    /// A required parameter was not provided.
    MissingParam { template: String, param: String },
    /// A parameter value doesn't match its type constraint.
    InvalidParamType {
        param: String,
        expected: String,
        got: String,
    },
    /// The catalog JSON failed to parse.
    CatalogParseError(String),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateError::UnknownTemplate(id) => write!(f, "unknown template: {id}"),
            TemplateError::MissingParam { template, param } => {
                write!(f, "template '{template}' requires parameter '{param}'")
            }
            TemplateError::InvalidParamType {
                param,
                expected,
                got,
            } => write!(f, "parameter '{param}': expected {expected}, got '{got}'"),
            TemplateError::CatalogParseError(msg) => write!(f, "catalog parse error: {msg}"),
        }
    }
}

impl std::error::Error for TemplateError {}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry providing lookup, validation, and instantiation of property templates.
pub struct TemplateRegistry {
    templates: HashMap<String, PropertyTemplate>,
}

impl TemplateRegistry {
    /// Load the built-in template catalog (compiled into the binary).
    pub fn builtin() -> Self {
        let json = include_str!("builtin_templates.json");
        Self::from_json(json).expect("built-in template catalog must be valid JSON")
    }

    /// Load a catalog from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, TemplateError> {
        let catalog: TemplateCatalog = serde_json::from_str(json)
            .map_err(|e| TemplateError::CatalogParseError(e.to_string()))?;
        let mut templates = HashMap::new();
        for t in catalog.templates {
            templates.insert(t.id.clone(), t);
        }
        Ok(TemplateRegistry { templates })
    }

    /// Merge another registry's templates into this one (other overrides on conflict).
    pub fn merge(&mut self, other: TemplateRegistry) {
        self.templates.extend(other.templates);
    }

    /// List all templates applicable to a given domain.
    pub fn for_domain(&self, domain: TemplateDomain) -> Vec<&PropertyTemplate> {
        self.templates
            .values()
            .filter(|t| {
                t.domains.contains(&TemplateDomain::Universal) || t.domains.contains(&domain)
            })
            .collect()
    }

    /// Get a template by ID.
    pub fn get(&self, id: &str) -> Option<&PropertyTemplate> {
        self.templates.get(id)
    }

    /// Return the full catalog (for API serialization).
    pub fn catalog(&self) -> TemplateCatalog {
        TemplateCatalog {
            version: "1.0".to_string(),
            templates: self.templates.values().cloned().collect(),
        }
    }

    /// Instantiate a template with concrete argument bindings.
    ///
    /// Validates required params, applies defaults, performs `${PARAM}`
    /// substitution, and returns a ready-to-use `InstantiatedProperty`.
    pub fn instantiate(&self, tref: &TemplateRef) -> Result<InstantiatedProperty, TemplateError> {
        let template = self
            .templates
            .get(&tref.template)
            .ok_or_else(|| TemplateError::UnknownTemplate(tref.template.clone()))?;

        // Build effective args: user-provided + defaults
        let mut effective_args: HashMap<String, String> = HashMap::new();
        for param in &template.params {
            if let Some(val) = tref.args.get(&param.name) {
                validate_param_value(param, val)?;
                effective_args.insert(param.name.clone(), val.clone());
            } else if let Some(default) = &param.default {
                effective_args.insert(param.name.clone(), default.clone());
            } else if param.required {
                return Err(TemplateError::MissingParam {
                    template: tref.template.clone(),
                    param: param.name.clone(),
                });
            }
        }

        // Substitute ${PARAM} placeholders in the formula pattern
        let mut formula = template.formula_pattern.clone();
        for (name, value) in &effective_args {
            formula = formula.replace(&format!("${{{name}}}"), value);
        }

        // Derive PropertyKind and PropertyRole from template metadata
        let kind = parse_property_kind(&template.kind);
        let role = parse_property_role(&template.role);

        // Generate a descriptive property name
        let name = if effective_args.is_empty() {
            template.id.clone()
        } else {
            let args_suffix: Vec<String> = effective_args
                .iter()
                .map(|(k, v)| format!("{k}_{v}"))
                .collect();
            format!("{}_{}", template.id, args_suffix.join("_"))
        };

        Ok(InstantiatedProperty {
            name,
            kind,
            formula,
            role,
            source_template: template.id.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_param_value(param: &TemplateParam, value: &str) -> Result<(), TemplateError> {
    match &param.param_type {
        ParamType::Predicate | ParamType::State => {
            // Must be a valid identifier: alphanumeric + underscore
            if !value.chars().all(|c| c.is_alphanumeric() || c == '_') || value.is_empty() {
                return Err(TemplateError::InvalidParamType {
                    param: param.name.clone(),
                    expected: "identifier (alphanumeric + underscore)".to_string(),
                    got: value.to_string(),
                });
            }
        }
        ParamType::Integer { min, max } => {
            let n: i64 = value.parse().map_err(|_| TemplateError::InvalidParamType {
                param: param.name.clone(),
                expected: "integer".to_string(),
                got: value.to_string(),
            })?;
            if let Some(lo) = min
                && n < *lo
            {
                return Err(TemplateError::InvalidParamType {
                    param: param.name.clone(),
                    expected: format!("integer >= {lo}"),
                    got: value.to_string(),
                });
            }
            if let Some(hi) = max
                && n > *hi
            {
                return Err(TemplateError::InvalidParamType {
                    param: param.name.clone(),
                    expected: format!("integer <= {hi}"),
                    got: value.to_string(),
                });
            }
        }
        ParamType::Label => {
            // Labels can contain alphanumeric, underscore, and sometimes dots
            if value.is_empty() {
                return Err(TemplateError::InvalidParamType {
                    param: param.name.clone(),
                    expected: "non-empty label".to_string(),
                    got: value.to_string(),
                });
            }
        }
        ParamType::Expression => {
            // Free-form: no validation beyond non-empty
            if value.is_empty() {
                return Err(TemplateError::InvalidParamType {
                    param: param.name.clone(),
                    expected: "non-empty expression".to_string(),
                    got: value.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn parse_property_kind(s: &str) -> PropertyKind {
    match s {
        "safety" => PropertyKind::Safety,
        "liveness" => PropertyKind::Liveness,
        "fairness" => PropertyKind::Fairness,
        _ => PropertyKind::Safety,
    }
}

fn parse_property_role(s: &str) -> PropertyRole {
    match s {
        "assumption" => PropertyRole::Assumption,
        "guarantee" => PropertyRole::Guarantee,
        "invariant" => PropertyRole::Invariant,
        _ => PropertyRole::Standalone,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_builtin_catalog() {
        let reg = TemplateRegistry::builtin();
        assert!(
            reg.templates.len() >= 7,
            "expected at least 7 built-in templates, got {}",
            reg.templates.len()
        );
    }

    #[test]
    fn instantiate_no_deadlock() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "no_deadlock".to_string(),
            args: HashMap::new(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert_eq!(result.name, "no_deadlock");
        assert_eq!(result.kind, PropertyKind::Safety);
        assert!(result.formula.contains("nu X"));
        assert!(result.formula.contains("<> true"));
        assert!(!result.formula.contains("${"));
    }

    #[test]
    fn instantiate_reachable_with_target() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "reachable".to_string(),
            args: [("TARGET".to_string(), "Idle".to_string())]
                .into_iter()
                .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("Idle"));
        assert!(!result.formula.contains("${TARGET}"));
        assert_eq!(result.kind, PropertyKind::Liveness);
    }

    #[test]
    fn instantiate_bounded_with_defaults() {
        let reg = TemplateRegistry::builtin();
        // Only provide OVERFLOW; UNDERFLOW should use default "false"
        let tref = TemplateRef {
            template: "bounded".to_string(),
            args: [("OVERFLOW".to_string(), "fill_5".to_string())]
                .into_iter()
                .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("fill_5"));
        assert!(result.formula.contains("false"));
        assert!(!result.formula.contains("${"));
    }

    #[test]
    fn missing_required_param_errors() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "reachable".to_string(),
            args: HashMap::new(),
        };
        let err = reg.instantiate(&tref).unwrap_err();
        assert!(matches!(err, TemplateError::MissingParam { .. }));
    }

    #[test]
    fn unknown_template_errors() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "nonexistent".to_string(),
            args: HashMap::new(),
        };
        let err = reg.instantiate(&tref).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownTemplate(_)));
    }

    #[test]
    fn invalid_predicate_param_errors() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "reachable".to_string(),
            args: [("TARGET".to_string(), "bad state!".to_string())]
                .into_iter()
                .collect(),
        };
        let err = reg.instantiate(&tref).unwrap_err();
        assert!(matches!(err, TemplateError::InvalidParamType { .. }));
    }

    #[test]
    fn filter_by_domain() {
        let reg = TemplateRegistry::builtin();
        let universal = reg.for_domain(TemplateDomain::Universal);
        let agentic = reg.for_domain(TemplateDomain::Agentic);
        // All universal templates should appear in agentic domain results
        assert!(agentic.len() >= universal.len());
    }

    #[test]
    fn mutual_exclusion_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "mutual_exclusion".to_string(),
            args: [
                ("A".to_string(), "P1_Active".to_string()),
                ("B".to_string(), "P2_Active".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("P1_Active"));
        assert!(result.formula.contains("P2_Active"));
        assert!(result.formula.contains("!"));
    }

    #[test]
    fn response_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "response".to_string(),
            args: [
                ("TRIGGER".to_string(), "req".to_string()),
                ("RESPONSE".to_string(), "ack".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("req"));
        assert!(result.formula.contains("ack"));
        assert_eq!(result.kind, PropertyKind::Liveness);
    }

    #[test]
    fn label_blocked_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "label_blocked_in_state".to_string(),
            args: [
                ("STATE".to_string(), "Empty".to_string()),
                ("LABEL".to_string(), "ev_get".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("Empty"));
        assert!(result.formula.contains("ev_get"));
    }

    #[test]
    fn catalog_round_trips() {
        let reg = TemplateRegistry::builtin();
        let catalog = reg.catalog();
        let json = serde_json::to_string(&catalog).unwrap();
        let reg2 = TemplateRegistry::from_json(&json).unwrap();
        assert_eq!(reg.templates.len(), reg2.templates.len());
    }

    /// Concurrency-template suite (compositional MCP). Confirms all five new
    /// templates are registered, instantiate cleanly, and produce formulas
    /// with the expected param substitutions. The pair `no_clobber` /
    /// `clobber_reachable` is asserted together — they're meant to ship as
    /// a non-vacuous safety+witness pair.
    #[test]
    fn concurrency_templates_registered() {
        let reg = TemplateRegistry::builtin();
        for id in [
            "no_clobber",
            "clobber_reachable",
            "mutual_exclusion_3",
            "bounded_handoff",
            "no_lost_update",
        ] {
            assert!(
                reg.get(id).is_some(),
                "expected concurrency template `{id}` to be registered"
            );
        }
        // Every concurrency template must be in the `agentic` domain
        // listing — that's the headline domain for compositional MCP.
        let agentic = reg.for_domain(TemplateDomain::Agentic);
        let agentic_ids: Vec<&str> = agentic.iter().map(|t| t.id.as_str()).collect();
        for id in [
            "no_clobber",
            "clobber_reachable",
            "bounded_handoff",
            "no_lost_update",
        ] {
            assert!(
                agentic_ids.contains(&id),
                "expected `{id}` to surface for the agentic domain, got {:?}",
                agentic_ids
            );
        }
    }

    #[test]
    fn no_clobber_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "no_clobber".to_string(),
            args: [("RESOURCE_CORRUPT".to_string(), "F_clobbered".to_string())]
                .into_iter()
                .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("F_clobbered"));
        assert!(result.formula.contains("nu X."));
        assert_eq!(result.kind, PropertyKind::Safety);
    }

    #[test]
    fn clobber_reachable_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "clobber_reachable".to_string(),
            args: [("RESOURCE_CORRUPT".to_string(), "F_clobbered".to_string())]
                .into_iter()
                .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("F_clobbered"));
        assert!(result.formula.contains("mu X."));
        assert_eq!(result.kind, PropertyKind::Liveness);
    }

    #[test]
    fn mutual_exclusion_3_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "mutual_exclusion_3".to_string(),
            args: [
                ("A".to_string(), "X1".to_string()),
                ("B".to_string(), "X2".to_string()),
                ("C".to_string(), "X3".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("X1"));
        assert!(result.formula.contains("X2"));
        assert!(result.formula.contains("X3"));
        // Three pairwise-exclusion clauses.
        assert!(result.formula.matches("&&").count() >= 3);
    }

    #[test]
    fn bounded_handoff_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "bounded_handoff".to_string(),
            args: [
                ("HANDOFF_TRIGGERED".to_string(), "Req".to_string()),
                ("HANDOFF_COMPLETE".to_string(), "Ack".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("Req"));
        assert!(result.formula.contains("Ack"));
        assert_eq!(result.kind, PropertyKind::Liveness);
    }

    #[test]
    fn no_lost_update_template() {
        let reg = TemplateRegistry::builtin();
        let tref = TemplateRef {
            template: "no_lost_update".to_string(),
            args: [
                ("WRITE_STARTED".to_string(), "WriteIssued".to_string()),
                ("WRITE_VISIBLE".to_string(), "WritePersisted".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let result = reg.instantiate(&tref).unwrap();
        assert!(result.formula.contains("WriteIssued"));
        assert!(result.formula.contains("WritePersisted"));
        assert_eq!(result.kind, PropertyKind::Safety);
    }
}
