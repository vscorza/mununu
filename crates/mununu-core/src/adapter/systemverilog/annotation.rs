//! SystemVerilog annotation sidecar (`.mununu.json`).
//!
//! Declares which signals to preserve, how to abstract them, which
//! properties to verify, and (optionally) SMT-discovered significant
//! values. The sidecar file lives next to the `.sv` source and is the
//! single source of truth for the Kripke verification pipeline.
//!
//! # Pipeline
//!
//! ```text
//! Parse SV → Load .mununu.json → SMT discovery → Build abstraction → Kripke → Verify
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Top-level annotation loaded from `<module>.mununu.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvAnnotation {
    /// Schema version identifier.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Module name (must match the SV module declaration).
    pub module: String,

    /// Path to the `.sv` source file (relative to the sidecar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Register/internal signal annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalAnnotation>,

    /// Input port annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputAnnotation>,

    /// Signals explicitly marked as controllable (output ports or overrides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,

    /// Properties to verify.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertyAnnotation>,

    /// SMT-discovered significant values per signal.
    /// Populated by `mununu sv discover`; user may edit names.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub discovered_values: HashMap<String, DiscoveredValues>,

    /// Module parameter overrides (e.g., `{"DEPTH": 4}`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, i64>,
}

/// Annotation for a register or internal signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalAnnotation {
    /// Signal name (must match an SV declaration).
    pub name: String,

    /// Whether to include this signal in the state space.
    /// Default: `true` if listed, but can be explicitly set to `false`.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub preserve: bool,

    /// Abstraction strategy.
    #[serde(default)]
    pub abstraction: SignalAbstraction,

    /// Upper bound for `bounded_counter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<i64>,

    /// Explicit enum variants (for `abstraction: "enum"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,

    /// Value map entries (for `abstraction: "enum"` with numeric mapping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<ValueMapEntry>>,

    /// Human-readable note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Annotation for an input port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputAnnotation {
    /// Port name.
    pub name: String,

    /// Whether to include this input as a label dimension.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub preserve: bool,

    /// Abstraction strategy (default: boolean for 1-bit).
    #[serde(default)]
    pub abstraction: SignalAbstraction,

    /// Upper bound for `bounded_counter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<i64>,

    /// Explicit enum variants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,

    /// Value map entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<ValueMapEntry>>,
}

/// Abstraction strategy for a signal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalAbstraction {
    /// 1-bit: true/false.
    Boolean,
    /// N-bit kept as individual bits (≤4 bits only).
    BitBlast,
    /// 0..bound counter with saturation.
    BoundedCounter,
    /// Named enum variants (optionally with value mapping).
    Enum,
    /// Let SMT discover significant values from guard analysis.
    #[default]
    Discover,
    /// Exclude from state space.
    Ignored,
}

/// A named value in a value map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueMapEntry {
    pub name: String,
    pub value: i64,
}

/// A property to verify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyAnnotation {
    /// Property identifier.
    pub id: String,

    /// Mu-calculus formula body.
    pub formula: String,

    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Property role: "guarantee" (default), "assumption", or "standalone".
    #[serde(default = "default_guarantee", skip_serializing_if = "is_guarantee")]
    pub role: String,
}

/// SMT-discovered values for a signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredValues {
    /// Discovered significant values with provenance.
    pub values: Vec<DiscoveredValue>,

    /// Name for the catch-all variant (values not in the map).
    #[serde(default = "default_other")]
    pub catch_all: String,
}

/// A single discovered value with provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredValue {
    /// The concrete numeric value.
    pub value: i64,

    /// User-editable variant name.
    pub name: String,

    /// How this value was discovered (e.g., "case label at line 38",
    /// "SMT: guard (cmd * 2 == 6) at line 45").
    #[serde(default)]
    pub from: Option<String>,
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn default_guarantee() -> String {
    "guarantee".to_string()
}

fn is_guarantee(v: &str) -> bool {
    v == "guarantee"
}

fn default_other() -> String {
    "OTHER".to_string()
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Look for `<stem>.mununu.json` next to the given `.sv` file.
pub fn find_sidecar(sv_path: &Path) -> Option<std::path::PathBuf> {
    let stem = sv_path.file_stem()?.to_str()?;
    let dir = sv_path.parent()?;
    let sidecar = dir.join(format!("{stem}.mununu.json"));
    if sidecar.exists() {
        Some(sidecar)
    } else {
        None
    }
}

/// Load and parse a `.mununu.json` sidecar file.
pub fn load_annotation(path: &Path) -> Result<SvAnnotation, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("failed to parse '{}': {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Merging with inline @mununu comments
// ---------------------------------------------------------------------------

use super::ast::{DomainAnnotationKind, Module, MununuPropertyKind};
use crate::adapter::domain::{AbstractValue, AbstractionType, FieldDomain};

/// Merged configuration from sidecar + inline comments.
/// Sidecar entries take precedence over inline comments on conflicts.
pub struct MergedConfig {
    /// Signal domains keyed by name (registers + input ports).
    pub signal_domains: HashMap<String, SignalConfig>,
    /// Input port domains keyed by name.
    pub input_domains: HashMap<String, SignalConfig>,
    /// Controllable signal names.
    pub controllable: Vec<String>,
    /// Properties to verify.
    pub properties: Vec<PropertyAnnotation>,
    /// Parameter overrides.
    pub parameters: HashMap<String, i64>,
    /// Whether kripke mode is forced.
    pub force_kripke: bool,
    /// Whether this config came from a sidecar (explicit opt-in)
    /// or from inline annotations (additive — unlisted signals are auto-detected).
    pub from_sidecar: bool,
}

/// Resolved configuration for a single signal.
pub struct SignalConfig {
    pub preserve: bool,
    pub domain: FieldDomain,
    pub value_map: Vec<(String, i64)>,
}

/// Build a merged config from an optional sidecar and the parsed module.
pub fn merge_config(annotation: Option<&SvAnnotation>, module: &Module) -> MergedConfig {
    if let Some(ann) = annotation {
        merge_from_sidecar(ann, module)
    } else {
        merge_from_inline(module)
    }
}

fn merge_from_sidecar(ann: &SvAnnotation, _module: &Module) -> MergedConfig {
    let mut signal_domains = HashMap::new();
    let mut input_domains = HashMap::new();

    // Process signal annotations
    for sig in &ann.signals {
        let (domain, value_map) = resolve_signal_domain(sig, ann);
        signal_domains.insert(
            sig.name.clone(),
            SignalConfig {
                preserve: sig.preserve,
                domain,
                value_map,
            },
        );
    }

    // Process input annotations
    for inp in &ann.inputs {
        let (domain, value_map) = resolve_input_domain(inp, ann);
        input_domains.insert(
            inp.name.clone(),
            SignalConfig {
                preserve: inp.preserve,
                domain,
                value_map,
            },
        );
    }

    // Properties from sidecar
    let properties = ann.properties.clone();

    MergedConfig {
        signal_domains,
        input_domains,
        controllable: ann.controllable.clone(),
        properties,
        parameters: ann.parameters.clone(),
        force_kripke: true, // sidecar always implies kripke mode
        from_sidecar: true,
    }
}

fn resolve_signal_domain(
    sig: &SignalAnnotation,
    ann: &SvAnnotation,
) -> (FieldDomain, Vec<(String, i64)>) {
    if !sig.preserve {
        return (
            FieldDomain {
                name: sig.name.clone(),
                abstraction: AbstractionType::Ignored,
                bound: None,
                variants: None,
                initial: AbstractValue::Counter(0),
            },
            vec![],
        );
    }

    match &sig.abstraction {
        SignalAbstraction::Boolean => (
            FieldDomain {
                name: sig.name.clone(),
                abstraction: AbstractionType::Boolean,
                bound: None,
                variants: None,
                initial: AbstractValue::Bool(false),
            },
            vec![],
        ),

        SignalAbstraction::BoundedCounter => {
            let bound = sig.bound.unwrap_or(3);
            (
                FieldDomain {
                    name: sig.name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some(bound),
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            )
        }

        SignalAbstraction::Enum => {
            let mut variants = sig.variants.clone().unwrap_or_default();
            let mut value_map = Vec::new();

            if let Some(vm) = &sig.value_map {
                for entry in vm {
                    value_map.push((entry.name.clone(), entry.value));
                    if !variants.contains(&entry.name) {
                        variants.push(entry.name.clone());
                    }
                }
            }

            // Add catch-all if not present
            if variants.is_empty() {
                variants.push("OTHER".to_string());
            }

            (
                FieldDomain {
                    name: sig.name.clone(),
                    abstraction: AbstractionType::EnumValues,
                    bound: None,
                    variants: Some(variants),
                    initial: AbstractValue::Variant(
                        sig.variants
                            .as_ref()
                            .and_then(|v| v.first().cloned())
                            .unwrap_or_else(|| "OTHER".to_string()),
                    ),
                },
                value_map,
            )
        }

        SignalAbstraction::Discover => {
            // Check if discovered_values exist for this signal
            if let Some(discovered) = ann.discovered_values.get(&sig.name) {
                let mut variants: Vec<String> =
                    discovered.values.iter().map(|v| v.name.clone()).collect();
                variants.push(discovered.catch_all.clone());

                let value_map: Vec<(String, i64)> = discovered
                    .values
                    .iter()
                    .map(|v| (v.name.clone(), v.value))
                    .collect();

                (
                    FieldDomain {
                        name: sig.name.clone(),
                        abstraction: AbstractionType::EnumValues,
                        bound: None,
                        variants: Some(variants.clone()),
                        initial: AbstractValue::Variant(
                            variants.first().cloned().unwrap_or_default(),
                        ),
                    },
                    value_map,
                )
            } else {
                // No discovered values yet — mark as Ignored with a warning
                // (the user should run `mununu sv discover` first)
                (
                    FieldDomain {
                        name: sig.name.clone(),
                        abstraction: AbstractionType::Ignored,
                        bound: None,
                        variants: None,
                        initial: AbstractValue::Counter(0),
                    },
                    vec![],
                )
            }
        }

        SignalAbstraction::BitBlast => {
            // Treat as bounded_counter 0..(2^width - 1)
            // Width comes from the SV declaration, not available here — use bound
            let bound = sig.bound.unwrap_or(15); // default 4-bit
            (
                FieldDomain {
                    name: sig.name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some(bound),
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            )
        }

        SignalAbstraction::Ignored => (
            FieldDomain {
                name: sig.name.clone(),
                abstraction: AbstractionType::Ignored,
                bound: None,
                variants: None,
                initial: AbstractValue::Counter(0),
            },
            vec![],
        ),
    }
}

fn resolve_input_domain(
    inp: &InputAnnotation,
    ann: &SvAnnotation,
) -> (FieldDomain, Vec<(String, i64)>) {
    if !inp.preserve {
        return (
            FieldDomain {
                name: inp.name.clone(),
                abstraction: AbstractionType::Ignored,
                bound: None,
                variants: None,
                initial: AbstractValue::Counter(0),
            },
            vec![],
        );
    }

    // Reuse signal resolution via a temporary SignalAnnotation
    let sig = SignalAnnotation {
        name: inp.name.clone(),
        preserve: inp.preserve,
        abstraction: inp.abstraction.clone(),
        bound: inp.bound,
        variants: inp.variants.clone(),
        value_map: inp.value_map.clone(),
        note: None,
    };
    resolve_signal_domain(&sig, ann)
}

/// Build a MergedConfig from inline `@mununu` comments only (no sidecar).
fn merge_from_inline(module: &Module) -> MergedConfig {
    let mut signal_domains = HashMap::new();

    // Domain annotations from inline comments
    for ann in &module.domain_annotations {
        let (domain, value_map) = match &ann.domain_kind {
            DomainAnnotationKind::Boolean => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::Boolean,
                    bound: None,
                    variants: None,
                    initial: AbstractValue::Bool(false),
                },
                vec![],
            ),
            DomainAnnotationKind::BoundedCounter { lower, upper } => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some(*upper),
                    variants: None,
                    initial: AbstractValue::Counter(*lower),
                },
                vec![],
            ),
            DomainAnnotationKind::Enum {
                variants,
                value_map,
            } => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::EnumValues,
                    bound: None,
                    variants: Some(variants.clone()),
                    initial: AbstractValue::Variant(variants.first().cloned().unwrap_or_default()),
                },
                value_map.clone(),
            ),
            DomainAnnotationKind::Ignored => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::Ignored,
                    bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            ),
        };
        signal_domains.insert(
            ann.register_name.clone(),
            SignalConfig {
                preserve: domain.abstraction != AbstractionType::Ignored,
                domain,
                value_map,
            },
        );
    }

    // Properties from inline comments
    let properties: Vec<PropertyAnnotation> = module
        .mununu_properties
        .iter()
        .map(|p| PropertyAnnotation {
            id: p.name.clone(),
            formula: p.formula.clone(),
            description: None,
            role: match p.kind {
                MununuPropertyKind::Assume => "assumption".to_string(),
                _ => "guarantee".to_string(),
            },
        })
        .collect();

    // Parameters from module
    let parameters: HashMap<String, i64> = module
        .parameters
        .iter()
        .map(|p| (p.name.clone(), p.default_value))
        .collect();

    MergedConfig {
        signal_domains,
        input_domains: HashMap::new(),
        controllable: module.controllable_signals.clone(),
        properties,
        parameters,
        force_kripke: module.force_kripke,
        from_sidecar: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_sidecar() {
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "fifo",
            "signals": [
                {"name": "state", "abstraction": "enum", "variants": ["IDLE", "WRITING", "READING"]},
                {"name": "fill", "abstraction": "bounded_counter", "bound": 4}
            ],
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)"}
            ]
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.module, "fifo");
        assert_eq!(ann.signals.len(), 2);
        assert_eq!(ann.signals[0].abstraction, SignalAbstraction::Enum);
        assert_eq!(ann.signals[1].bound, Some(4));
        assert_eq!(ann.properties.len(), 1);
    }

    #[test]
    fn parse_discover_with_discovered_values() {
        let json = r#"{
            "module": "alu",
            "signals": [
                {"name": "cmd", "abstraction": "discover"}
            ],
            "discovered_values": {
                "cmd": {
                    "values": [
                        {"value": 0, "name": "NOP", "from": "case label at line 38"},
                        {"value": 1, "name": "LOAD", "from": "case label at line 39"}
                    ],
                    "catch_all": "OTHER"
                }
            },
            "properties": []
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        let discovered = ann.discovered_values.get("cmd").unwrap();
        assert_eq!(discovered.values.len(), 2);
        assert_eq!(discovered.values[0].name, "NOP");
        assert_eq!(discovered.values[0].value, 0);
        assert_eq!(discovered.catch_all, "OTHER");
    }

    #[test]
    fn parse_full_sidecar() {
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "alu",
            "source": "alu.sv",
            "signals": [
                {"name": "acc", "abstraction": "bounded_counter", "bound": 7},
                {"name": "cmd", "abstraction": "discover", "note": "opcode register"},
                {"name": "data_buf", "preserve": false}
            ],
            "inputs": [
                {"name": "start", "abstraction": "boolean"},
                {"name": "operand", "abstraction": "bounded_counter", "bound": 3},
                {"name": "data_in", "preserve": false}
            ],
            "controllable": ["acc"],
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)", "description": "No deadlock"},
                {"id": "reset_clears", "formula": "nu X. ([] X)", "role": "guarantee"}
            ],
            "parameters": {"WIDTH": 8},
            "discovered_values": {}
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.module, "alu");
        assert_eq!(ann.signals.len(), 3);
        assert!(!ann.signals[2].preserve); // data_buf excluded
        assert_eq!(ann.inputs.len(), 3);
        assert!(!ann.inputs[2].preserve); // data_in excluded
        assert_eq!(ann.controllable, vec!["acc"]);
        assert_eq!(ann.parameters.get("WIDTH"), Some(&8));
    }

    #[test]
    fn resolve_discover_with_values() {
        let json = r#"{
            "module": "test",
            "signals": [
                {"name": "cmd", "abstraction": "discover"}
            ],
            "discovered_values": {
                "cmd": {
                    "values": [
                        {"value": 0, "name": "NOP"},
                        {"value": 3, "name": "START"}
                    ],
                    "catch_all": "OTHER"
                }
            },
            "properties": []
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        let (domain, value_map) = resolve_signal_domain(&ann.signals[0], &ann);

        assert_eq!(domain.abstraction, AbstractionType::EnumValues);
        assert_eq!(
            domain.variants.as_ref().unwrap(),
            &["NOP", "START", "OTHER"]
        );
        assert_eq!(
            value_map,
            vec![("NOP".to_string(), 0), ("START".to_string(), 3)]
        );
    }

    #[test]
    fn resolve_discover_without_values() {
        let json = r#"{
            "module": "test",
            "signals": [
                {"name": "cmd", "abstraction": "discover"}
            ],
            "properties": []
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        let (domain, _) = resolve_signal_domain(&ann.signals[0], &ann);

        // No discovered values yet → Ignored (user must run discover first)
        assert_eq!(domain.abstraction, AbstractionType::Ignored);
    }
}
