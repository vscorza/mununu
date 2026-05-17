//! `verify.toml` schema — the user-facing entry point to the general
//! verification framework.
//!
//! A `verify.toml` declares:
//! - `[project]` — name + optional description.
//! - `[[sources]]` — N typed sources (id + adapter + files + options).
//! - `[alphabet]` — how labels across sources synchronise.
//! - `[composition]` — composition semantics + members + name.
//! - `[[properties]]` — properties to verify.
//!
//! Example:
//!
//! ```toml
//! [project]
//! name = "uart_codesign"
//!
//! [[sources]]
//! id = "fw"
//! adapter = "c-codesign"
//! files = ["firmware/firmware.c"]
//! [sources.options]
//! include_paths = ["firmware/include"]
//! defines = ["F_CPU=64000000"]
//! cmsis_stubs = true
//! register_map = "register_map.json"
//! synthesize_automaton = true
//!
//! [[sources]]
//! id = "periph"
//! adapter = "ctxdsl"
//! files = ["spec/uart_protocol.ctxdsl"]
//!
//! [alphabet]
//! strategy = "direct"
//!
//! [composition]
//! semantics = "asynchronous"
//! members = ["fw", "periph"]
//! name = "System"
//!
//! [[properties]]
//! name = "no_double_start"
//! template = "label_blocked_in_state"
//! args = { STATE = "fw.Transmitting", LABEL = "wr_ctrl_tx_start" }
//! over = "System"
//! ```
//!
//! ## Validation
//!
//! [`VerifyConfig::validate`] returns a `Vec<ConfigIssue>` describing
//! every structural problem found, so the orchestrator can surface a
//! single report instead of failing the pipeline mid-step. The
//! orchestrator refuses to proceed when validation returns a
//! non-empty vector.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Parsed `verify.toml` document.
///
/// Does not derive `Eq` because `SourceSection.options` holds
/// `toml::Value`s, which carry floating-point literals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyConfig {
    /// `[project]` block.
    pub project: ProjectSection,
    /// `[[sources]]` array — one entry per source model.
    #[serde(default)]
    pub sources: Vec<SourceSection>,
    /// `[alphabet]` block — alphabet-binding strategy. Optional;
    /// defaults to `{ strategy = "direct" }` when omitted.
    #[serde(default)]
    pub alphabet: AlphabetSection,
    /// `[composition]` block.
    pub composition: CompositionSection,
    /// `[[properties]]` array.
    #[serde(default)]
    pub properties: Vec<PropertySection>,
}

/// `[project]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSection {
    /// Project name. Used in the report header and as a default
    /// for `[composition].name` (`<project.name>System`).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One entry in the `[[sources]]` array.
///
/// Each source has a unique `id` (referenced by `[composition].members`
/// and by property `over` targets), an `adapter` name dispatched
/// through the existing `FormatAdapter` registry, a list of `files`
/// to feed into the adapter, and an `options` map containing
/// adapter-specific configuration.
///
/// Does not derive `Eq` because `options` holds `toml::Value`s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSection {
    /// Stable identifier for this source. Must be a valid CTXDSL
    /// identifier (alphanumeric + underscore, starting with a letter
    /// or underscore). Referenced from `[composition].members` and
    /// from property `over` targets.
    pub id: String,
    /// Adapter name. Dispatched through the registry assembled in
    /// A2.4. Recognised values today (subject to A2 evolution):
    /// `"c-codesign"` (firmware C via `codesign::c_extract_llvm`),
    /// `"sv-rtl"` (custom-SV / yosys frontend), `"ctxdsl"` (raw
    /// hand-authored automaton), `"xstate"`, `"crewai"` (CrewAI
    /// agentic JSON), `"langgraph"` (LangGraph StateGraph JSON),
    /// `"microcode"` (restricted JSON microcode form — plan Part 5.5),
    /// `"extraction"`, `"tlsf"`, `"aiger"`, `"btor2"`, `"promela"`.
    pub adapter: String,
    /// Source files to feed the adapter. Order is significant for
    /// adapters that accept multiple files; for single-file adapters
    /// the orchestrator will iterate and merge outputs (see A2.4).
    pub files: Vec<PathBuf>,
    /// Adapter-specific options as a free-form TOML table. The
    /// orchestrator passes this to the adapter dispatcher which is
    /// responsible for mapping individual keys onto each adapter's
    /// strongly-typed options struct (e.g. `LlvmExtractOptions`,
    /// `AdapterOptions`, …). Schema validation of inner keys happens
    /// inside the adapter; this layer only catches well-formedness.
    #[serde(default)]
    pub options: BTreeMap<String, toml::Value>,
}

/// `[alphabet]` block — how labels across sources synchronise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlphabetSection {
    /// Binding strategy. `"direct"` (default), `"renamings"`, or
    /// `"register_map"`.
    #[serde(default = "default_strategy")]
    pub strategy: String,
    /// For `strategy = "renamings"`: list of `{ from, to }` entries.
    /// Each `from` is `"<source_id>.<local_label>"`; each `to` is
    /// the canonical name the orchestrator rewrites it to before
    /// composition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamings: Vec<Renaming>,
    /// For `strategy = "register_map"`: path to a register-map JSON
    /// sidecar. The orchestrator derives renamings from the sidecar's
    /// `sv_signal` / `c_accessor` fields via
    /// `codesign::coupling::rendezvous_label_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register_map: Option<PathBuf>,
    /// When `true`, peripheral-only labels (labels on the
    /// peripheral-side source not used by the firmware-side source)
    /// are tolerated instead of treated as a reconcile-gate failure.
    /// Mirrors `mununu codesign reconcile-labels
    /// --allow-peripheral-superset`.
    #[serde(default)]
    pub allow_peripheral_superset: bool,
}

impl Default for AlphabetSection {
    fn default() -> Self {
        Self {
            strategy: default_strategy(),
            renamings: Vec::new(),
            register_map: None,
            allow_peripheral_superset: false,
        }
    }
}

/// Single entry in `[alphabet].renamings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Renaming {
    /// `<source_id>.<local_label>` qualified reference.
    pub from: String,
    /// Canonical label the orchestrator rewrites `from` to.
    pub to: String,
}

/// `[composition]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionSection {
    /// `"synchronous"`, `"asynchronous"`, or `"superset"`. Mirrors
    /// `composition::CompositionSemantics`.
    pub semantics: String,
    /// Source IDs to compose. Each must match a `[[sources]].id`.
    /// Order is preserved for the iterative left-associative
    /// `compose(compose(a, b), c)` walk the realiser performs.
    pub members: Vec<String>,
    /// Composition name. Used as the default `over` target for
    /// properties that don't supply one. Defaults to
    /// `<project.name>System` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One entry in the `[[properties]]` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertySection {
    /// Property name. Must be unique within the config.
    pub name: String,
    /// Template ID. Mutually exclusive with `formula`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Raw mu-calculus formula. Mutually exclusive with `template`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Template arguments. Keys match the template's parameter names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
    /// Automaton or composition to evaluate over. Defaults to
    /// `[composition].name` (or the implicit default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
}

fn default_strategy() -> String {
    "direct".to_string()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Issues surfaced by [`VerifyConfig::validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConfigIssue {
    /// `[project].name` was empty.
    EmptyProjectName,
    /// `[[sources]]` was empty (no sources to verify).
    NoSources,
    /// `[[sources]]` entry had an empty `id`.
    EmptySourceId { index: usize },
    /// `[[sources]]` entry's `id` is not a valid CTXDSL identifier.
    InvalidSourceId { id: String },
    /// Two `[[sources]]` entries share the same `id`.
    DuplicateSourceId(String),
    /// `[[sources]]` entry had an empty `adapter` field.
    EmptyAdapter { source_id: String },
    /// `[[sources]]` entry had an empty `files` list.
    SourceNoFiles { source_id: String },
    /// `[alphabet].strategy` is not one of `direct` / `renamings` /
    /// `register_map`.
    UnknownAlphabetStrategy(String),
    /// `[alphabet]` strategy is `renamings` but the `renamings` list
    /// is empty.
    RenamingsStrategyWithoutEntries,
    /// `[alphabet]` strategy is `renamings` and a `from` field is
    /// malformed (not `<source_id>.<label>` shape).
    MalformedRenamingFrom { from: String },
    /// `[alphabet]` strategy is `renamings` and a `from` field
    /// references a source ID that doesn't exist.
    RenamingUnknownSource { source_id: String, from: String },
    /// `[alphabet]` strategy is `register_map` but no
    /// `register_map` path is set.
    RegisterMapStrategyWithoutPath,
    /// `[composition].semantics` is not one of `synchronous` /
    /// `asynchronous` / `superset`.
    UnknownCompositionSemantics(String),
    /// `[composition].members` is empty.
    CompositionNoMembers,
    /// `[composition].members` references an unknown source ID.
    CompositionUnknownMember { id: String },
    /// `[[properties]]` entry had both `template` and `formula` set,
    /// or neither.
    PropertyFormulaXorViolation {
        name: String,
        has_template: bool,
        has_formula: bool,
    },
    /// `[[properties]]` entry had an empty `name`.
    EmptyPropertyName,
    /// Two `[[properties]]` entries share the same `name`.
    DuplicatePropertyName(String),
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigIssue::EmptyProjectName => write!(f, "[project].name is empty"),
            ConfigIssue::NoSources => {
                write!(f, "[[sources]] is empty; at least one source is required")
            }
            ConfigIssue::EmptySourceId { index } => {
                write!(f, "[[sources]] entry at index {index} has empty `id`")
            }
            ConfigIssue::InvalidSourceId { id } => write!(
                f,
                "[[sources]] entry `id = \"{id}\"` is not a valid CTXDSL identifier (must match [A-Za-z_][A-Za-z0-9_]*)"
            ),
            ConfigIssue::DuplicateSourceId(id) => {
                write!(f, "[[sources]]: duplicate source id `{id}`")
            }
            ConfigIssue::EmptyAdapter { source_id } => {
                write!(f, "[[sources]] `{source_id}` has empty `adapter`")
            }
            ConfigIssue::SourceNoFiles { source_id } => write!(
                f,
                "[[sources]] `{source_id}` has empty `files` list; at least one path is required"
            ),
            ConfigIssue::UnknownAlphabetStrategy(s) => write!(
                f,
                "[alphabet].strategy = \"{s}\" is not recognised (valid: \"direct\", \"renamings\", \"register_map\")"
            ),
            ConfigIssue::RenamingsStrategyWithoutEntries => write!(
                f,
                "[alphabet].strategy = \"renamings\" but the `renamings` list is empty"
            ),
            ConfigIssue::MalformedRenamingFrom { from } => write!(
                f,
                "[alphabet].renamings: `from = \"{from}\"` is malformed (expected \"<source_id>.<label>\")"
            ),
            ConfigIssue::RenamingUnknownSource { source_id, from } => write!(
                f,
                "[alphabet].renamings: `from = \"{from}\"` references unknown source id `{source_id}`"
            ),
            ConfigIssue::RegisterMapStrategyWithoutPath => write!(
                f,
                "[alphabet].strategy = \"register_map\" but `register_map` path is not set"
            ),
            ConfigIssue::UnknownCompositionSemantics(s) => write!(
                f,
                "[composition].semantics = \"{s}\" is not recognised (valid: \"synchronous\", \"asynchronous\", \"superset\")"
            ),
            ConfigIssue::CompositionNoMembers => write!(
                f,
                "[composition].members is empty; at least one member is required"
            ),
            ConfigIssue::CompositionUnknownMember { id } => {
                write!(f, "[composition].members: unknown source id `{id}`")
            }
            ConfigIssue::PropertyFormulaXorViolation {
                name,
                has_template,
                has_formula,
            } => {
                if *has_template && *has_formula {
                    write!(
                        f,
                        "[[properties]] `{name}`: both `template` and `formula` are set; exactly one is required"
                    )
                } else {
                    write!(
                        f,
                        "[[properties]] `{name}`: neither `template` nor `formula` is set; exactly one is required"
                    )
                }
            }
            ConfigIssue::EmptyPropertyName => write!(f, "[[properties]] entry has empty `name`"),
            ConfigIssue::DuplicatePropertyName(n) => {
                write!(f, "[[properties]]: duplicate property name `{n}`")
            }
        }
    }
}

const VALID_STRATEGIES: &[&str] = &["direct", "renamings", "register_map"];
const VALID_SEMANTICS: &[&str] = &["synchronous", "asynchronous", "superset"];

impl VerifyConfig {
    /// Parse a TOML document.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Run all structural checks. Empty vector ⇔ config is well-formed.
    pub fn validate(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();

        if self.project.name.is_empty() {
            issues.push(ConfigIssue::EmptyProjectName);
        }

        // Sources
        if self.sources.is_empty() {
            issues.push(ConfigIssue::NoSources);
        }
        let mut seen_source_ids: HashSet<String> = HashSet::new();
        for (index, source) in self.sources.iter().enumerate() {
            if source.id.is_empty() {
                issues.push(ConfigIssue::EmptySourceId { index });
            } else if !is_valid_identifier(&source.id) {
                issues.push(ConfigIssue::InvalidSourceId {
                    id: source.id.clone(),
                });
            }
            if !seen_source_ids.insert(source.id.clone()) {
                issues.push(ConfigIssue::DuplicateSourceId(source.id.clone()));
            }
            if source.adapter.is_empty() {
                issues.push(ConfigIssue::EmptyAdapter {
                    source_id: source.id.clone(),
                });
            }
            if source.files.is_empty() {
                issues.push(ConfigIssue::SourceNoFiles {
                    source_id: source.id.clone(),
                });
            }
        }

        // Alphabet
        if !VALID_STRATEGIES.contains(&self.alphabet.strategy.as_str()) {
            issues.push(ConfigIssue::UnknownAlphabetStrategy(
                self.alphabet.strategy.clone(),
            ));
        }
        match self.alphabet.strategy.as_str() {
            "renamings" => {
                if self.alphabet.renamings.is_empty() {
                    issues.push(ConfigIssue::RenamingsStrategyWithoutEntries);
                }
                for r in &self.alphabet.renamings {
                    match parse_renaming_from(&r.from) {
                        Some((source_id, _label)) => {
                            if !seen_source_ids.contains(source_id) {
                                issues.push(ConfigIssue::RenamingUnknownSource {
                                    source_id: source_id.to_string(),
                                    from: r.from.clone(),
                                });
                            }
                        }
                        None => {
                            issues.push(ConfigIssue::MalformedRenamingFrom {
                                from: r.from.clone(),
                            });
                        }
                    }
                }
            }
            "register_map" if self.alphabet.register_map.is_none() => {
                issues.push(ConfigIssue::RegisterMapStrategyWithoutPath);
            }
            _ => {} // "direct" / "register_map" with path set / unknown — no extra checks here
        }

        // Composition
        if !VALID_SEMANTICS.contains(&self.composition.semantics.as_str()) {
            issues.push(ConfigIssue::UnknownCompositionSemantics(
                self.composition.semantics.clone(),
            ));
        }
        if self.composition.members.is_empty() {
            issues.push(ConfigIssue::CompositionNoMembers);
        }
        for m in &self.composition.members {
            // Wildcard expansion: `<src>.*` resolves to every automaton
            // emitted by `<src>`. The validator only enforces source
            // existence; the assembler does the expansion.
            let bare = m.strip_suffix(".*").unwrap_or(m.as_str());
            if !seen_source_ids.contains(bare) {
                issues.push(ConfigIssue::CompositionUnknownMember { id: m.clone() });
            }
        }

        // Properties
        let mut seen_property_names: HashSet<String> = HashSet::new();
        for p in &self.properties {
            if p.name.is_empty() {
                issues.push(ConfigIssue::EmptyPropertyName);
            }
            let has_template = p.template.is_some();
            let has_formula = p.formula.is_some();
            if has_template == has_formula {
                issues.push(ConfigIssue::PropertyFormulaXorViolation {
                    name: p.name.clone(),
                    has_template,
                    has_formula,
                });
            }
            if !p.name.is_empty() && !seen_property_names.insert(p.name.clone()) {
                issues.push(ConfigIssue::DuplicatePropertyName(p.name.clone()));
            }
        }

        issues
    }

    /// Resolve a property's `over` target, falling back to the
    /// composition name (or its implicit default).
    pub fn resolve_over(&self, p: &PropertySection) -> String {
        if let Some(o) = p.over.as_ref() {
            return o.clone();
        }
        self.composition_name()
    }

    /// Return the composition name (explicit if set, otherwise the
    /// project-derived default `<project.name>System`).
    pub fn composition_name(&self) -> String {
        self.composition
            .name
            .clone()
            .unwrap_or_else(|| format!("{}System", self.project.name))
    }
}

/// Parse `"<source_id>.<label>"`. Returns `None` if there's no dot or
/// either side is empty.
fn parse_renaming_from(s: &str) -> Option<(&str, &str)> {
    let (sid, label) = s.split_once('.')?;
    if sid.is_empty() || label.is_empty() {
        return None;
    }
    Some((sid, label))
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_DIRECT: &str = r#"
[project]
name = "two_xstate_machines"

[[sources]]
id = "a"
adapter = "xstate"
files = ["a.xstate.json"]

[[sources]]
id = "b"
adapter = "xstate"
files = ["b.xstate.json"]

[alphabet]
strategy = "direct"

[composition]
semantics = "synchronous"
members = ["a", "b"]
name = "Pair"

[[properties]]
name = "no_deadlock"
template = "no_deadlock"
"#;

    const VALID_RENAMINGS: &str = r#"
[project]
name = "renamed_pair"

[[sources]]
id = "left"
adapter = "ctxdsl"
files = ["left.ctxdsl"]

[[sources]]
id = "right"
adapter = "ctxdsl"
files = ["right.ctxdsl"]

[[alphabet.renamings]]
from = "left.go"
to = "shared.go"

[[alphabet.renamings]]
from = "right.start"
to = "shared.go"

[alphabet]
strategy = "renamings"

[composition]
semantics = "asynchronous"
members = ["left", "right"]

[[properties]]
name = "reachable"
formula = "mu X. (Init || <> X)"
over = "Pair"
"#;

    const VALID_REGISTER_MAP: &str = r#"
[project]
name = "uart_codesign"

[[sources]]
id = "fw"
adapter = "c-codesign"
files = ["firmware.c"]

[[sources]]
id = "periph"
adapter = "sv-rtl"
files = ["uart.sv"]

[alphabet]
strategy = "register_map"
register_map = "register_map.json"
allow_peripheral_superset = true

[composition]
semantics = "asynchronous"
members = ["fw", "periph"]
name = "UARTSystem"

[[properties]]
name = "init_reachable"
template = "reachable"
over = "UARTSystem"
"#;

    #[test]
    fn parses_direct_strategy_config() {
        let cfg = VerifyConfig::from_toml(VALID_DIRECT).expect("valid TOML");
        assert_eq!(cfg.project.name, "two_xstate_machines");
        assert_eq!(cfg.sources.len(), 2);
        assert_eq!(cfg.sources[0].id, "a");
        assert_eq!(cfg.alphabet.strategy, "direct");
        assert_eq!(cfg.composition.semantics, "synchronous");
        assert_eq!(cfg.composition_name(), "Pair");
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn parses_renamings_strategy_config() {
        let cfg = VerifyConfig::from_toml(VALID_RENAMINGS).expect("valid TOML");
        assert_eq!(cfg.alphabet.strategy, "renamings");
        assert_eq!(cfg.alphabet.renamings.len(), 2);
        assert_eq!(cfg.alphabet.renamings[0].from, "left.go");
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn parses_register_map_strategy_config() {
        let cfg = VerifyConfig::from_toml(VALID_REGISTER_MAP).expect("valid TOML");
        assert_eq!(cfg.alphabet.strategy, "register_map");
        assert!(cfg.alphabet.allow_peripheral_superset);
        assert!(cfg.alphabet.register_map.is_some());
        assert_eq!(cfg.composition.semantics, "asynchronous");
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn alphabet_section_defaults_to_direct_when_omitted() {
        let toml_src = r#"
[project]
name = "minimal"

[[sources]]
id = "only"
adapter = "ctxdsl"
files = ["only.ctxdsl"]

[composition]
semantics = "synchronous"
members = ["only"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        assert_eq!(cfg.alphabet.strategy, "direct");
        assert!(cfg.alphabet.renamings.is_empty());
        assert!(cfg.alphabet.register_map.is_none());
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn rejects_empty_project_name() {
        let toml_src = r#"
[project]
name = ""

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::EmptyProjectName))
        );
    }

    #[test]
    fn rejects_empty_sources_list() {
        let toml_src = r#"
[project]
name = "x"

[composition]
semantics = "synchronous"
members = []
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(|i| matches!(i, ConfigIssue::NoSources)));
    }

    #[test]
    fn rejects_invalid_source_id() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "1bad"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["1bad"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::InvalidSourceId { id } if id == "1bad"))
        );
    }

    #[test]
    fn rejects_duplicate_source_id() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "dup"
adapter = "x"
files = ["a"]

[[sources]]
id = "dup"
adapter = "y"
files = ["b"]

[composition]
semantics = "synchronous"
members = ["dup"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::DuplicateSourceId(s) if s == "dup"))
        );
    }

    #[test]
    fn rejects_source_with_no_files() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = []

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::SourceNoFiles { source_id } if source_id == "a"))
        );
    }

    #[test]
    fn rejects_unknown_alphabet_strategy() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[alphabet]
strategy = "labels-by-vibes"

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(
            |i| matches!(i, ConfigIssue::UnknownAlphabetStrategy(s) if s == "labels-by-vibes")
        ));
    }

    #[test]
    fn rejects_renamings_strategy_without_entries() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[alphabet]
strategy = "renamings"

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::RenamingsStrategyWithoutEntries))
        );
    }

    #[test]
    fn rejects_renaming_with_malformed_from() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[[alphabet.renamings]]
from = "no_dot_here"
to = "canonical"

[alphabet]
strategy = "renamings"

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(
            |i| matches!(i, ConfigIssue::MalformedRenamingFrom { from } if from == "no_dot_here")
        ));
    }

    #[test]
    fn rejects_renaming_with_unknown_source() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[[alphabet.renamings]]
from = "ghost.label"
to = "canonical"

[alphabet]
strategy = "renamings"

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::RenamingUnknownSource { source_id, .. } if source_id == "ghost"))
        );
    }

    #[test]
    fn rejects_register_map_strategy_without_path() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[alphabet]
strategy = "register_map"

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::RegisterMapStrategyWithoutPath))
        );
    }

    #[test]
    fn rejects_unknown_composition_semantics() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "lockstep"
members = ["a"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues.iter().any(
                |i| matches!(i, ConfigIssue::UnknownCompositionSemantics(s) if s == "lockstep")
            )
        );
    }

    #[test]
    fn rejects_composition_with_unknown_member() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["a", "ghost"]
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues.iter().any(
                |i| matches!(i, ConfigIssue::CompositionUnknownMember { id } if id == "ghost")
            )
        );
    }

    #[test]
    fn rejects_property_with_both_template_and_formula() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["a"]

[[properties]]
name = "p"
template = "reachable"
formula = "true"
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(|i| matches!(
            i,
            ConfigIssue::PropertyFormulaXorViolation {
                has_template: true,
                has_formula: true,
                ..
            }
        )));
    }

    #[test]
    fn rejects_property_with_neither_template_nor_formula() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["a"]

[[properties]]
name = "p"
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(|i| matches!(
            i,
            ConfigIssue::PropertyFormulaXorViolation {
                has_template: false,
                has_formula: false,
                ..
            }
        )));
    }

    #[test]
    fn rejects_duplicate_property_names() {
        let toml_src = r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["a"]

[[properties]]
name = "p"
formula = "true"

[[properties]]
name = "p"
template = "reachable"
"#;
        let cfg = VerifyConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::DuplicatePropertyName(n) if n == "p"))
        );
    }

    #[test]
    fn resolve_over_falls_back_to_composition_name_then_project_default() {
        let cfg = VerifyConfig::from_toml(VALID_DIRECT).unwrap();
        // Explicit `over` wins.
        let mut p = PropertySection {
            name: "x".to_string(),
            template: Some("no_deadlock".to_string()),
            formula: None,
            args: BTreeMap::new(),
            over: Some("Custom".to_string()),
        };
        assert_eq!(cfg.resolve_over(&p), "Custom");
        // Missing `over` falls back to composition.name (set in VALID_DIRECT).
        p.over = None;
        assert_eq!(cfg.resolve_over(&p), "Pair");

        // When composition.name is omitted, fall back to `<project>System`.
        let toml_src = r#"
[project]
name = "demo"

[[sources]]
id = "a"
adapter = "x"
files = ["a"]

[composition]
semantics = "synchronous"
members = ["a"]
"#;
        let cfg2 = VerifyConfig::from_toml(toml_src).unwrap();
        assert_eq!(cfg2.composition_name(), "demoSystem");
    }
}
