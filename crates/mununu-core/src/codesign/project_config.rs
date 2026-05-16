//! Project-config schema for `mununu codesign verify-project`.
//!
//! A `mununu.codesign.toml` file is the user-facing entry point to the
//! integrated codesign verification flow: it names the project, points
//! at the register-map source (SVD file or sidecar JSON), enumerates
//! the firmware C sources + include paths + defines + CMSIS-stubs
//! preference, enumerates the RTL SV sources + top-module + frontend
//! choice, and declares the properties to verify (either by template
//! reference or by inline mu-calculus formula).
//!
//! The schema is intentionally close to the CLI flags the existing
//! `mununu codesign extract-c`, SV adapter, and `mununu codesign couple`
//! / `verify` subcommands accept. The orchestrator
//! ([`crate::codesign::verify_project`], landing in the same PR slice as
//! this file) parses the TOML, runs the seven-step pipeline (parse →
//! load register-map → extract-c → SV adapter → reconcile → compose →
//! evaluate), and produces a structured `VerifyProjectReport`.
//!
//! ## Schema sketch
//!
//! ```toml
//! [project]
//! name = "nrf52_twim_i2c"
//! peripheral = "TWIM0"
//!
//! [register_map]
//! # Exactly one of `svd` or `json` is required.
//! svd = "upstream/nrf52840.svd"
//! peripheral_name = "TWIM0"
//!
//! [firmware]
//! sources = ["firmware/main.c", "firmware/twim.c"]
//! include_paths = ["firmware/include"]
//! defines = ["F_CPU=64000000"]
//! cmsis_stubs = true
//! cmsis_header_vendor_prefix = "NRF_"
//!
//! [rtl]
//! sources = ["rtl/twim.sv", "rtl/twim_fsm.sv"]
//! top_module = "TWIM0"
//! frontend = "custom-sv"      # or "yosys"
//!
//! [[properties]]
//! name = "init_reachable"
//! template = "reachable"      # OR: formula = "mu X. (Init || <> X)"
//! over = "TWIM0System"
//! ```
//!
//! ## Soundness
//!
//! The orchestrator enforces:
//!
//! - Exactly one of `register_map.svd` or `register_map.json` (XOR).
//! - Exactly one of each property's `template` or `formula` (XOR).
//! - `firmware.sources` and `rtl.sources` non-empty.
//! - `rtl.frontend` is one of the supported values
//!   (today: `"custom-sv"`, `"yosys"`).
//!
//! These constraints are checked by [`ProjectConfig::validate`] and the
//! orchestrator refuses to run on a config that fails validation. The
//! point is to surface configuration errors as a single structured
//! report rather than letting the pipeline fail seven steps later with
//! a less-informative downstream error.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Parsed `mununu.codesign.toml` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// `[project]` block — names + peripheral being verified.
    pub project: ProjectSection,
    /// `[register_map]` block — source of the register-map sidecar.
    pub register_map: RegisterMapSection,
    /// `[firmware]` block — C sources + compile options.
    pub firmware: FirmwareSection,
    /// `[rtl]` block — RTL sources + frontend choice.
    pub rtl: RtlSection,
    /// `[[properties]]` array — what to verify.
    #[serde(default)]
    pub properties: Vec<PropertySection>,
}

/// `[project]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSection {
    /// Project name. Appears in the orchestrator's report and is the
    /// default for any per-property `over` field that's left blank.
    pub name: String,
    /// Peripheral being verified. Must match the `peripheral` field of
    /// the resolved [`crate::codesign::register_map::RegisterMap`].
    pub peripheral: String,
    /// Optional description for the report header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `[register_map]` block. Exactly one of `svd` or `json` is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterMapSection {
    /// Path to a CMSIS-SVD XML file. Mutually exclusive with `json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svd: Option<PathBuf>,
    /// Path to a register-map JSON sidecar (the same format
    /// [`crate::codesign::register_map::RegisterMap`] consumes).
    /// Mutually exclusive with `svd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<PathBuf>,
    /// Peripheral name to select when the source covers multiple
    /// peripherals (typical for SVD files). Required when the source
    /// is an SVD; ignored when the source is a single-peripheral JSON
    /// sidecar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peripheral_name: Option<String>,
}

/// `[firmware]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareSection {
    /// C source files to extract. Each is processed by
    /// [`crate::codesign::c_extract_llvm::extract_c_via_llvm`].
    pub sources: Vec<PathBuf>,
    /// Include paths handed to clang as `-I`.
    #[serde(default)]
    pub include_paths: Vec<PathBuf>,
    /// Preprocessor defines, e.g. `"F_CPU=64000000"`.
    #[serde(default)]
    pub defines: Vec<String>,
    /// Enable the bundled `cmsis-stubs/` include path (Phase L8).
    #[serde(default = "default_true")]
    pub cmsis_stubs: bool,
    /// Vendor prefix passed to the SVD→CMSIS-header emitter when one
    /// is needed (e.g. `"NRF_"` produces `NRF_TWIM_Type` /
    /// `NRF_TWIM0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmsis_header_vendor_prefix: Option<String>,
}

/// `[rtl]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtlSection {
    /// SystemVerilog / Verilog source files describing the
    /// peripheral.
    pub sources: Vec<PathBuf>,
    /// Top module name. Used by both frontends to root the elaboration.
    pub top_module: String,
    /// Which SV frontend to use. `"custom-sv"` (the annotation-driven
    /// mununu frontend) is the default; `"yosys"` for the BTOR2-based
    /// path.
    #[serde(default = "default_frontend")]
    pub frontend: String,
}

/// One entry in the `[[properties]]` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertySection {
    /// Property name (must be unique within the config).
    pub name: String,
    /// Template ID (e.g. `"no_deadlock"`, `"reachable"`). Mutually
    /// exclusive with `formula`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Raw mu-calculus formula. Mutually exclusive with `template`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Template arguments, when `template` is set. Keys are the
    /// template's parameter names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
    /// Automaton or composition the formula is evaluated over.
    /// Defaults to `<project.peripheral>System` (the composition
    /// emitted by `compose::compose_codesign_ctxdsl`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_frontend() -> String {
    "custom-sv".to_string()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validation issues surfaced by [`ProjectConfig::validate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConfigIssue {
    /// `[register_map]` had both `svd` and `json` set, or neither.
    RegisterMapSourceXorViolation { has_svd: bool, has_json: bool },
    /// `[register_map].svd` was set without `peripheral_name`.
    SvdMissingPeripheralName,
    /// `[firmware].sources` was empty.
    FirmwareNoSources,
    /// `[rtl].sources` was empty.
    RtlNoSources,
    /// `[rtl].frontend` value isn't recognised.
    UnknownFrontend { value: String },
    /// A `[[properties]]` entry had both `template` and `formula`
    /// set, or neither.
    PropertyFormulaXorViolation {
        name: String,
        has_template: bool,
        has_formula: bool,
    },
    /// Two `[[properties]]` entries share the same `name`.
    DuplicatePropertyName(String),
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigIssue::RegisterMapSourceXorViolation { has_svd, has_json } => {
                if *has_svd && *has_json {
                    write!(
                        f,
                        "[register_map]: both `svd` and `json` are set; exactly one is required"
                    )
                } else {
                    write!(
                        f,
                        "[register_map]: neither `svd` nor `json` is set; exactly one is required"
                    )
                }
            }
            ConfigIssue::SvdMissingPeripheralName => write!(
                f,
                "[register_map]: `svd` source requires `peripheral_name` (SVDs cover multiple peripherals)"
            ),
            ConfigIssue::FirmwareNoSources => {
                write!(
                    f,
                    "[firmware]: `sources` is empty; at least one C file is required"
                )
            }
            ConfigIssue::RtlNoSources => {
                write!(
                    f,
                    "[rtl]: `sources` is empty; at least one SV/V file is required"
                )
            }
            ConfigIssue::UnknownFrontend { value } => write!(
                f,
                "[rtl]: `frontend = \"{value}\"` is not recognised (valid: \"custom-sv\", \"yosys\")"
            ),
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
            ConfigIssue::DuplicatePropertyName(n) => {
                write!(f, "[[properties]]: duplicate property name `{n}`")
            }
        }
    }
}

const VALID_FRONTENDS: &[&str] = &["custom-sv", "yosys"];

impl ProjectConfig {
    /// Parse a TOML document.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Run all structural checks. Returns the empty vector when the
    /// config is well-formed.
    pub fn validate(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();

        // [register_map] XOR
        let has_svd = self.register_map.svd.is_some();
        let has_json = self.register_map.json.is_some();
        if has_svd == has_json {
            issues.push(ConfigIssue::RegisterMapSourceXorViolation { has_svd, has_json });
        }
        if has_svd && self.register_map.peripheral_name.is_none() {
            issues.push(ConfigIssue::SvdMissingPeripheralName);
        }

        // Firmware / RTL sources non-empty
        if self.firmware.sources.is_empty() {
            issues.push(ConfigIssue::FirmwareNoSources);
        }
        if self.rtl.sources.is_empty() {
            issues.push(ConfigIssue::RtlNoSources);
        }

        // RTL frontend value
        if !VALID_FRONTENDS.contains(&self.rtl.frontend.as_str()) {
            issues.push(ConfigIssue::UnknownFrontend {
                value: self.rtl.frontend.clone(),
            });
        }

        // Property XOR + duplicate-name checks
        let mut seen_names = std::collections::HashSet::new();
        for p in &self.properties {
            let has_template = p.template.is_some();
            let has_formula = p.formula.is_some();
            if has_template == has_formula {
                issues.push(ConfigIssue::PropertyFormulaXorViolation {
                    name: p.name.clone(),
                    has_template,
                    has_formula,
                });
            }
            if !seen_names.insert(p.name.clone()) {
                issues.push(ConfigIssue::DuplicatePropertyName(p.name.clone()));
            }
        }

        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = r#"
[project]
name = "uart_demo"
peripheral = "UART_LITE"

[register_map]
json = "examples/industrial/codesign_uart/register_map.json"

[firmware]
sources = ["firmware/main.c"]
include_paths = ["firmware/include"]
defines = ["F_CPU=64000000"]
cmsis_stubs = true

[rtl]
sources = ["rtl/uart.sv"]
top_module = "uart_lite"
frontend = "custom-sv"

[[properties]]
name = "init_reachable"
template = "reachable"
over = "UART_LITESystem"

  [properties.args]
  TARGET = "Init"

[[properties]]
name = "no_deadlock"
formula = "nu X. (<> true && [] X)"
"#;

    #[test]
    fn parses_full_config_round_trips() {
        let cfg = ProjectConfig::from_toml(VALID_CONFIG).expect("valid TOML");
        assert_eq!(cfg.project.name, "uart_demo");
        assert_eq!(cfg.project.peripheral, "UART_LITE");
        assert!(cfg.register_map.svd.is_none());
        assert_eq!(
            cfg.register_map.json.as_deref().and_then(|p| p.to_str()),
            Some("examples/industrial/codesign_uart/register_map.json")
        );
        assert_eq!(cfg.firmware.sources.len(), 1);
        assert!(cfg.firmware.cmsis_stubs);
        assert_eq!(cfg.rtl.frontend, "custom-sv");
        assert_eq!(cfg.properties.len(), 2);
        assert_eq!(cfg.properties[0].template.as_deref(), Some("reachable"));
        assert_eq!(
            cfg.properties[0].args.get("TARGET").map(String::as_str),
            Some("Init")
        );
        assert_eq!(
            cfg.properties[1].formula.as_deref(),
            Some("nu X. (<> true && [] X)")
        );
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn default_frontend_is_custom_sv() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        assert_eq!(cfg.rtl.frontend, "custom-sv");
        assert!(cfg.firmware.cmsis_stubs); // default_true
    }

    #[test]
    fn rejects_both_svd_and_json() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
svd = "x.svd"
json = "x.json"
peripheral_name = "P"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(|i| matches!(
            i,
            ConfigIssue::RegisterMapSourceXorViolation {
                has_svd: true,
                has_json: true
            }
        )));
    }

    #[test]
    fn rejects_neither_svd_nor_json() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(issues.iter().any(|i| matches!(
            i,
            ConfigIssue::RegisterMapSourceXorViolation {
                has_svd: false,
                has_json: false
            }
        )));
    }

    #[test]
    fn svd_requires_peripheral_name() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
svd = "x.svd"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::SvdMissingPeripheralName))
        );
    }

    #[test]
    fn rejects_unknown_frontend() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"
frontend = "verilator"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues.iter().any(
                |i| matches!(i, ConfigIssue::UnknownFrontend { value } if value == "verilator")
            )
        );
    }

    #[test]
    fn rejects_property_with_both_template_and_formula() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"

[[properties]]
name = "p1"
template = "reachable"
formula = "true"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
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
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"

[[properties]]
name = "p1"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
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
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = ["a.c"]

[rtl]
sources = ["a.sv"]
top_module = "p"

[[properties]]
name = "p1"
formula = "true"

[[properties]]
name = "p1"
template = "reachable"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::DuplicatePropertyName(n) if n == "p1"))
        );
    }

    #[test]
    fn rejects_empty_firmware_sources() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = []

[rtl]
sources = ["a.sv"]
top_module = "p"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::FirmwareNoSources))
        );
    }

    #[test]
    fn rejects_empty_rtl_sources() {
        let toml_src = r#"
[project]
name = "x"
peripheral = "P"

[register_map]
json = "rm.json"

[firmware]
sources = ["a.c"]

[rtl]
sources = []
top_module = "p"
"#;
        let cfg = ProjectConfig::from_toml(toml_src).unwrap();
        let issues = cfg.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ConfigIssue::RtlNoSources))
        );
    }
}
