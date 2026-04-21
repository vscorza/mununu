//! SystemVerilog annotation sidecar (`.mununu.json`).
//!
//! Declares which signals to preserve, how to abstract them, which
//! properties to verify, and (optionally) SMT-discovered significant
//! values. The sidecar file lives next to the `.sv` source and is the
//! single source of truth for the Kripke verification pipeline.
//!
//! # Formats
//!
//! Two sidecar formats are supported:
//!
//! - **Single-module** (`mununu_sv_annotation_v1`): one `"module"` field,
//!   one `.sv` source, one automaton output.
//! - **Multi-module** (`mununu_sv_multi_v1`): a `"modules"` array with
//!   connections, composition directives, and cross-module properties.
//!   Builds one Kripke automaton per module and composes them.
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

    /// Whether this signal is combinational (computed from `assign` / `always_comb`).
    /// Combinational signals are included in the state space but their value is
    /// computed from the combinational logic each cycle, not from sequential assignments.
    #[serde(default, skip_serializing_if = "is_false")]
    pub combinational: bool,

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

    /// Override the label prefix used in transitions. When set, transitions
    /// for this input use `label_name` instead of `name` in the generated
    /// labels. Used by multi-module connections to create shared labels
    /// between driving and receiving modules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_name: Option<String>,
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

// ---------------------------------------------------------------------------
// Multi-module sidecar format (mununu_sv_multi_v1)
// ---------------------------------------------------------------------------

/// Top-level annotation for a multi-module sidecar.
///
/// References multiple `.sv` source files, declares inter-module connections,
/// and specifies composition mode and cross-module properties.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiModuleSvAnnotation {
    /// Schema version identifier (should be `"mununu_sv_multi_v1"`).
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Per-module configurations.
    pub modules: Vec<ModuleEntry>,

    /// Inter-module port connections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<ConnectionSpec>,

    /// Composition configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<CompositionConfig>,

    /// Cross-module properties (target the composed automaton).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertyAnnotation>,

    /// Cross-connection discovered values (keyed by `"module.port"`).
    /// Populated by `mununu sv discover`; never hand-written.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub discovered_values: HashMap<String, DiscoveredValues>,
}

/// Configuration for a single module within a multi-module sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    /// Module name (must match the SV module declaration).
    pub name: String,

    /// Path to the `.sv` source file (relative to the sidecar).
    pub source: String,

    /// Clock domain name (for validation; all modules in synchronous
    /// composition must share the same domain or omit this field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_domain: Option<String>,

    /// Register/internal signal annotations (same format as single-module).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalAnnotation>,

    /// Input port annotations (only non-connected inputs; connected
    /// inputs get their domain from the connection spec).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputAnnotation>,

    /// Signals explicitly marked as controllable within this module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,

    /// Module parameter overrides.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, i64>,

    /// Module-local discovered values (for non-connected signals).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub discovered_values: HashMap<String, DiscoveredValues>,
}

/// A connection between two module ports.
///
/// Declares that `from` (an output port of one module) drives `to`
/// (an input port of another module). The shared abstraction is declared
/// once and applied uniformly to both sides.
///
/// # Controllability
///
/// The driving module (source of `from`) owns the signal. In the composed
/// automaton, transitions on this connection are controllable by the driving
/// module. The receiving module's transitions synchronize on the shared label
/// without independently asserting controllability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionSpec {
    /// Source port: `"module_name.port_name"` (must be an output port).
    pub from: String,

    /// Destination port: `"module_name.port_name"` (must be an input port).
    pub to: String,

    /// Shared abstraction for the connected signal.
    pub abstraction: SignalAbstraction,

    /// Upper bound (for `bounded_counter` abstraction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<i64>,

    /// Enum variants (for `enum` abstraction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,

    /// Value map (for `enum` abstraction with numeric mapping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<ValueMapEntry>>,

    /// Human-readable note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Composition configuration for multi-module verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionConfig {
    /// Composition mode: `"synchronous"` (default) or `"asynchronous"`.
    #[serde(default = "default_synchronous")]
    pub mode: String,

    /// Name for the composed automaton in CTXDSL.
    pub name: String,
}

impl ConnectionSpec {
    /// Parse the `from` field into (module_name, port_name).
    pub fn parse_from(&self) -> Option<(&str, &str)> {
        self.from.split_once('.')
    }

    /// Parse the `to` field into (module_name, port_name).
    pub fn parse_to(&self) -> Option<(&str, &str)> {
        self.to.split_once('.')
    }
}

fn default_synchronous() -> String {
    "synchronous".to_string()
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

fn is_false(v: &bool) -> bool {
    !*v
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

/// Load and parse a `.mununu.json` sidecar file (single-module format).
pub fn load_annotation(path: &Path) -> Result<SvAnnotation, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("failed to parse '{}': {e}", path.display()))
}

/// Detect whether a `.mununu.json` file uses the multi-module format.
///
/// Returns `true` if the JSON contains a top-level `"modules"` array key.
pub fn is_multi_module(path: &Path) -> Result<bool, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse '{}': {e}", path.display()))?;
    Ok(value.get("modules").is_some_and(|v| v.is_array()))
}

/// Load and parse a multi-module `.mununu.json` sidecar file.
pub fn load_multi_annotation(path: &Path) -> Result<MultiModuleSvAnnotation, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let ann: MultiModuleSvAnnotation = serde_json::from_str(&content).map_err(|e| {
        format!(
            "failed to parse multi-module sidecar '{}': {e}",
            path.display()
        )
    })?;
    validate_multi_annotation(&ann)?;
    Ok(ann)
}

/// Validate a multi-module annotation for structural correctness.
fn validate_multi_annotation(ann: &MultiModuleSvAnnotation) -> Result<(), String> {
    if ann.modules.is_empty() {
        return Err("multi-module sidecar must have at least one module".to_string());
    }

    // Check for duplicate module names
    let mut seen_modules = std::collections::HashSet::new();
    for entry in &ann.modules {
        if !seen_modules.insert(&entry.name) {
            return Err(format!("duplicate module name '{}'", entry.name));
        }
    }

    // Validate connections reference existing modules
    for conn in &ann.connections {
        let (from_mod, _from_port) = conn.parse_from().ok_or_else(|| {
            format!(
                "invalid connection 'from' format: '{}' (expected module.port)",
                conn.from
            )
        })?;
        let (to_mod, _to_port) = conn.parse_to().ok_or_else(|| {
            format!(
                "invalid connection 'to' format: '{}' (expected module.port)",
                conn.to
            )
        })?;

        if !seen_modules.contains(&from_mod.to_string()) {
            return Err(format!(
                "connection references unknown module '{}' in from: '{}'",
                from_mod, conn.from
            ));
        }
        if !seen_modules.contains(&to_mod.to_string()) {
            return Err(format!(
                "connection references unknown module '{}' in to: '{}'",
                to_mod, conn.to
            ));
        }

        if from_mod == to_mod {
            return Err(format!(
                "connection cannot be within the same module: '{}' → '{}'",
                conn.from, conn.to
            ));
        }
    }

    // Validate clock domains if composition is synchronous
    if let Some(ref comp) = ann.composition
        && comp.mode == "synchronous"
    {
        let domains: Vec<&str> = ann
            .modules
            .iter()
            .filter_map(|m| m.clock_domain.as_deref())
            .collect();
        if domains.len() > 1 {
            let first = domains[0];
            for d in &domains[1..] {
                if *d != first {
                    return Err(format!(
                        "synchronous composition requires same clock domain, \
                         but found '{}' and '{}'. Use \"mode\": \"asynchronous\" \
                         or align clock domains.",
                        first, d
                    ));
                }
            }
        }
    }

    Ok(())
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
    pub combinational: bool,
    /// Override the label prefix used in transitions for this input.
    /// When `Some`, the Kripke builder uses this name instead of the
    /// signal's own name for generating transition labels. This enables
    /// shared labels across multi-module connections.
    pub label_name: Option<String>,
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
                combinational: sig.combinational,
                label_name: None,
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
                combinational: false,
                label_name: inp.label_name.clone(),
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
        combinational: false,
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
                combinational: false,
                label_name: None,
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

    // ---------------------------------------------------------------
    // Multi-module sidecar format tests
    // ---------------------------------------------------------------

    #[test]
    fn parse_multi_module_minimal() {
        let json = r#"{
            "$schema": "mununu_sv_multi_v1",
            "modules": [
                {
                    "name": "arbiter",
                    "source": "arbiter.sv",
                    "signals": [
                        {"name": "state", "abstraction": "enum", "variants": ["IDLE", "GRANT"]}
                    ]
                },
                {
                    "name": "datapath",
                    "source": "datapath.sv",
                    "signals": [
                        {"name": "busy", "abstraction": "boolean"}
                    ]
                }
            ],
            "connections": [
                {"from": "arbiter.grant", "to": "datapath.start", "abstraction": "boolean"}
            ],
            "composition": {"mode": "synchronous", "name": "system"},
            "properties": [
                {"id": "no_deadlock", "formula": "nu X. ([] X)"}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.modules.len(), 2);
        assert_eq!(ann.modules[0].name, "arbiter");
        assert_eq!(ann.modules[1].name, "datapath");
        assert_eq!(ann.connections.len(), 1);
        assert_eq!(ann.connections[0].parse_from(), Some(("arbiter", "grant")));
        assert_eq!(ann.connections[0].parse_to(), Some(("datapath", "start")));
        assert_eq!(ann.composition.as_ref().unwrap().mode, "synchronous");
        assert_eq!(ann.properties.len(), 1);
    }

    #[test]
    fn parse_multi_module_with_discovery() {
        let json = r#"{
            "modules": [
                {"name": "ctrl", "source": "ctrl.sv", "signals": []},
                {"name": "exec", "source": "exec.sv", "signals": []}
            ],
            "connections": [
                {
                    "from": "ctrl.cmd_out",
                    "to": "exec.cmd_in",
                    "abstraction": "discover"
                }
            ],
            "composition": {"name": "system"},
            "discovered_values": {
                "ctrl.cmd_out": {
                    "values": [
                        {"value": 0, "name": "NOP", "from": "SMT: guard at line 15"},
                        {"value": 3, "name": "EXEC", "from": "SMT: guard at line 22"}
                    ],
                    "catch_all": "OTHER"
                }
            }
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.connections[0].abstraction, SignalAbstraction::Discover);
        let disc = ann.discovered_values.get("ctrl.cmd_out").unwrap();
        assert_eq!(disc.values.len(), 2);
        assert_eq!(disc.values[1].name, "EXEC");
        assert_eq!(disc.catch_all, "OTHER");
    }

    #[test]
    fn validate_multi_module_duplicate_name() {
        let json = r#"{
            "modules": [
                {"name": "foo", "source": "a.sv", "signals": []},
                {"name": "foo", "source": "b.sv", "signals": []}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("duplicate module name"));
    }

    #[test]
    fn validate_multi_module_unknown_module_in_connection() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "signals": []}
            ],
            "connections": [
                {"from": "a.out", "to": "b.inp", "abstraction": "boolean"}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("unknown module 'b'"));
    }

    #[test]
    fn validate_multi_module_same_module_connection() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "signals": []}
            ],
            "connections": [
                {"from": "a.out", "to": "a.inp", "abstraction": "boolean"}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("cannot be within the same module"));
    }

    #[test]
    fn validate_multi_module_clock_domain_mismatch() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "clock_domain": "clk_fast", "signals": []},
                {"name": "b", "source": "b.sv", "clock_domain": "clk_slow", "signals": []}
            ],
            "composition": {"mode": "synchronous", "name": "system"}
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("synchronous composition requires same clock domain"));
    }

    #[test]
    fn validate_multi_module_async_different_clocks_ok() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "clock_domain": "clk_fast", "signals": []},
                {"name": "b", "source": "b.sv", "clock_domain": "clk_slow", "signals": []}
            ],
            "composition": {"mode": "asynchronous", "name": "system"}
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        assert!(validate_multi_annotation(&ann).is_ok());
    }

    #[test]
    fn connection_spec_parse_helpers() {
        let conn = ConnectionSpec {
            from: "arbiter.grant_a".to_string(),
            to: "datapath.start".to_string(),
            abstraction: SignalAbstraction::Boolean,
            bound: None,
            variants: None,
            value_map: None,
            note: None,
        };
        assert_eq!(conn.parse_from(), Some(("arbiter", "grant_a")));
        assert_eq!(conn.parse_to(), Some(("datapath", "start")));
    }

    #[test]
    fn multi_module_with_enum_connection() {
        let json = r#"{
            "modules": [
                {"name": "ctrl", "source": "ctrl.sv", "signals": []},
                {"name": "mem", "source": "mem.sv", "signals": []}
            ],
            "connections": [
                {
                    "from": "ctrl.cmd",
                    "to": "mem.cmd",
                    "abstraction": "enum",
                    "variants": ["READ", "WRITE", "IDLE"],
                    "value_map": [
                        {"name": "READ", "value": 1},
                        {"name": "WRITE", "value": 2},
                        {"name": "IDLE", "value": 0}
                    ],
                    "note": "Memory command bus"
                }
            ],
            "composition": {"name": "system"}
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let conn = &ann.connections[0];
        assert_eq!(conn.abstraction, SignalAbstraction::Enum);
        assert_eq!(conn.variants.as_ref().unwrap().len(), 3);
        assert_eq!(conn.value_map.as_ref().unwrap().len(), 3);
        assert_eq!(conn.note.as_deref(), Some("Memory command bus"));
    }
}
