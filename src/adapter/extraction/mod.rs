//! Extraction spec adapter.
//!
//! Translates extraction spec JSON (`.espec.json`) into CTXDSL via the
//! explicit-automaton encoding path. Extraction specs are produced by the
//! extraction pipeline:
//!
//! ```text
//! Source code → human extraction → JSON spec → this adapter → CTXDSL
//! ```
//!
//! The spec's `model_config` section carries declarative automaton definitions
//! with mode-filtered transitions, enabling both "fixed" and "vulnerable"
//! variants from a single spec file.

pub mod ast;
pub mod ast_extract;
pub mod validate;

use super::ir::*;
use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo, WarningKind,
};
use ast::{AutomatonDef, ExtractionSpec};

/// Extraction spec adapter implementing [`FormatAdapter`].
pub struct ExtractionAdapter;

impl FormatAdapter for ExtractionAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('{') {
            return false;
        }
        // Extraction specs have "$schema": "extraction_spec_v1" and "model_config"
        trimmed.contains("\"extraction_spec_v1\"") && trimmed.contains("\"model_config\"")
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let spec: ExtractionSpec = serde_json::from_str(content).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("Extraction spec JSON parse error: {e}"),
            location: None,
        })?;

        let mode = options.mode.as_deref().unwrap_or("vulnerable");

        if mode != "fixed" && mode != "vulnerable" {
            return Err(AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: format!("Invalid mode '{mode}'. Must be 'fixed' or 'vulnerable'."),
                location: None,
            });
        }

        let mut warnings = Vec::new();
        let ir = to_ir(&spec, mode, options, &mut warnings)?;

        let result = super::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let property_count = ir.properties.len();
        let signal_count = count_labels(&spec);

        // Prepend provenance headers for traceability
        let ctxdsl = build_provenance_header(&spec, mode) + &result.ctxdsl;

        Ok(AdapterOutput {
            ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::Extraction,
                title: Some(spec.model_config.context_name.clone()),
                signal_count,
                state_count: result.state_count,
                property_count,
            },
        })
    }
}

/// Convert an extraction spec to the shared AdapterIR.
fn to_ir(
    spec: &ExtractionSpec,
    mode: &str,
    options: &AdapterOptions,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let config = &spec.model_config;

    let context_name = options
        .context_name
        .as_deref()
        .unwrap_or(&config.context_name);

    // Determine effective context name (append _vulnerable/_fixed suffix if appropriate)
    let effective_name = context_name.to_string();

    // Build automata from declarative definitions
    let automata: Vec<AutomatonSpec> = config
        .automata
        .iter()
        .map(|def| build_automaton(def, mode, warnings))
        .collect::<Result<Vec<_>, _>>()?;

    if automata.is_empty() {
        warnings.push(AdapterWarning {
            kind: WarningKind::ApproximateTranslation,
            message: "No automata defined in model_config; CTXDSL will have no automata"
                .to_string(),
            location: None,
        });
    }

    // Build composition
    let compositions = match &config.composition {
        Some(comp) => {
            let comp_spec = match comp.type_.as_str() {
                "synchronous" => CompositionSpec::Synchronous {
                    name: comp.name.clone(),
                    members: comp.members.clone(),
                },
                _ => CompositionSpec::Asynchronous {
                    name: comp.name.clone(),
                    members: comp.members.clone(),
                },
            };
            vec![comp_spec]
        }
        None => vec![],
    };

    // Build properties
    let properties: Vec<PropertySpec> = config
        .properties
        .iter()
        .filter_map(|def| {
            let formula_str = def.formula_str()?;
            Some(PropertySpec {
                name: def.id.clone(),
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus(formula_str.to_string()),
                role: PropertyRole::Standalone,
                over: def.over.clone(),
            })
        })
        .collect();

    // Build controller(s)
    let controller = config.controllers.first().map(|def| ControllerSpec {
        name: def.name.clone(),
        source_automaton: def.source.clone(),
        formula_name: def.satisfying.clone(),
    });

    // Build description with provenance
    let description = build_description(spec, mode);

    Ok(AdapterIR {
        metadata: Metadata {
            title: effective_name,
            source_format: SourceFormat::Extraction,
            description: Some(description),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata,
        compositions,
        properties,
        controller,
    })
}

/// Build an `AutomatonSpec` from a declarative automaton definition,
/// filtering transitions by mode.
fn build_automaton(
    def: &AutomatonDef,
    mode: &str,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AutomatonSpec, AdapterError> {
    // Build states
    let any_initial = def.states.iter().any(|s| s.is_initial());
    let states: Vec<StateSpec> = def
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // First state is initial by default if none explicitly marked
            let is_initial = if any_initial { s.is_initial() } else { i == 0 };
            StateSpec {
                name: s.name().to_string(),
                is_initial,
            }
        })
        .collect();

    if states.is_empty() {
        return Err(AdapterError {
            kind: AdapterErrorKind::IrConsistencyError,
            message: format!("Automaton '{}' has no states", def.id),
            location: None,
        });
    }

    // Filter transitions by mode
    let transitions: Vec<TransitionSpec> = def
        .transitions
        .iter()
        .filter(|t| t.mode == "both" || t.mode == mode)
        .map(|t| TransitionSpec {
            source: t.from.clone(),
            target: t.to.clone(),
            labels: vec![t.label.clone()],
        })
        .collect();

    if transitions.is_empty() {
        warnings.push(AdapterWarning {
            kind: WarningKind::ApproximateTranslation,
            message: format!(
                "Automaton '{}' has no transitions in mode '{}'",
                def.id, mode
            ),
            location: None,
        });
    }

    Ok(AutomatonSpec {
        name: def.id.clone(),
        states,
        transitions,
        controllable_labels: def.controllable_labels.clone(),
        internal_labels: vec![],
    })
}

/// Build provenance description from spec metadata.
fn build_description(spec: &ExtractionSpec, mode: &str) -> String {
    let mut parts = Vec::new();

    parts.push(format!("Translated from extraction spec (mode: {mode})"));

    if let Some(file) = &spec.source.file {
        if let Some(commit) = &spec.source.commit {
            let short = if commit.len() > 12 {
                &commit[..12]
            } else {
                commit
            };
            parts.push(format!("Source: {file} @ {short}"));
        } else {
            parts.push(format!("Source: {file}"));
        }
    }

    if let Some(cve) = &spec.source.cve {
        parts.push(format!("CVE: {cve}"));
    }

    if let Some(issue) = &spec.source.issue {
        parts.push(format!("Issue: {issue}"));
    }

    parts.join(" | ")
}

/// Build machine-parseable provenance header for the generated CTXDSL.
///
/// These headers enable CI to distinguish generated-from-spec files from
/// hand-written ones, and to trace CTXDSL back to specific source code.
fn build_provenance_header(spec: &ExtractionSpec, mode: &str) -> String {
    let mut lines = Vec::new();
    lines.push("// @generated-from: extraction_spec_v1".to_string());

    if let Some(file) = &spec.source.file {
        lines.push(format!("// @source-file: {file}"));
    }
    if let Some(commit) = &spec.source.commit {
        lines.push(format!("// @commit: {commit}"));
    }
    if let Some(repo) = &spec.source.repo {
        lines.push(format!("// @repo: {repo}"));
    }
    lines.push(format!("// @mode: {mode}"));

    if let Some(cve) = &spec.source.cve {
        lines.push(format!("// @cve: {cve}"));
    }
    if let Some(ghsa) = &spec.source.ghsa {
        lines.push(format!("// @ghsa: {ghsa}"));
    }
    if let Some(issue) = &spec.source.issue {
        lines.push(format!("// @issue: {issue}"));
    }

    // Attack chain summary (if bugs documented)
    for bug in &spec.bugs {
        if let Some(desc) = &bug.description {
            let short = if desc.len() > 100 {
                format!("{}...", &desc[..97])
            } else {
                desc.clone()
            };
            lines.push(format!("// @bug: {} — {}", bug.id, short));
        }
    }

    lines.push("//".to_string());
    lines.push(String::new());
    lines.join("\n")
}

/// Count distinct labels in the spec.
fn count_labels(spec: &ExtractionSpec) -> usize {
    let mut labels = std::collections::HashSet::new();
    for l in &spec.model_config.controllable_labels {
        labels.insert(l.as_str());
    }
    for l in &spec.model_config.uncontrollable_labels {
        labels.insert(l.as_str());
    }
    for aut in &spec.model_config.automata {
        for t in &aut.transitions {
            labels.insert(&t.label);
        }
    }
    labels.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate_spec(json: &str, mode: &str) -> AdapterOutput {
        let options = AdapterOptions {
            mode: Some(mode.to_string()),
            ..Default::default()
        };
        ExtractionAdapter::translate(json, &options).expect("translation should succeed")
    }

    #[test]
    fn detect_extraction_spec() {
        assert!(ExtractionAdapter::detect(
            r#"{"$schema": "extraction_spec_v1", "source": {}, "model_config": {"context_name": "t"}}"#
        ));
        assert!(!ExtractionAdapter::detect(
            r#"{"id": "test", "initial": "s0", "states": {}}"#
        ));
        assert!(!ExtractionAdapter::detect("INFO { TITLE: \"test\" }"));
    }

    #[test]
    fn translate_simple_spec() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {"repo": "test/repo", "commit": "abc123def456"},
            "model_config": {
                "context_name": "simple_test",
                "automata": [
                    {
                        "id": "Main",
                        "states": [
                            {"name": "Idle", "initial": true},
                            {"name": "Active"}
                        ],
                        "controllable_labels": ["ev_go"],
                        "transitions": [
                            {"from": "Idle", "to": "Active", "label": "ev_go"},
                            {"from": "Active", "to": "Active", "label": "ev_go"},
                            {"from": "Idle", "to": "Idle", "label": "noop"},
                            {"from": "Active", "to": "Active", "label": "noop"}
                        ]
                    }
                ],
                "properties": [
                    {
                        "id": "safety",
                        "formula": "nu X. ([] X)",
                        "over": "Main"
                    }
                ]
            }
        }"#;

        let output = translate_spec(json, "vulnerable");
        assert_eq!(output.source_info.format, SourceFormat::Extraction);
        assert!(!output.ctxdsl.is_empty());
        assert!(output.ctxdsl.contains("context simple_test"));
        assert!(output.ctxdsl.contains("automaton Main"));
        assert!(
            output
                .ctxdsl
                .contains("transition Idle -> Active on label ev_go")
        );

        // Parse and realize to validate
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("Main").expect("Main automaton");
        assert_eq!(clts.state_count(), 2);
    }

    #[test]
    fn mode_filters_transitions() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {},
            "model_config": {
                "context_name": "mode_test",
                "automata": [{
                    "id": "A",
                    "states": [
                        {"name": "Open", "initial": true},
                        {"name": "Closed"}
                    ],
                    "transitions": [
                        {"from": "Open", "to": "Closed", "label": "ev_close"},
                        {"from": "Closed", "to": "Closed", "label": "ev_request", "mode": "vulnerable"},
                        {"from": "Open", "to": "Open", "label": "noop"},
                        {"from": "Closed", "to": "Closed", "label": "noop"}
                    ]
                }]
            }
        }"#;

        // Vulnerable mode: ev_request transition exists in Closed
        let vuln = translate_spec(json, "vulnerable");
        assert!(vuln.ctxdsl.contains("ev_request"));

        // Fixed mode: ev_request transition filtered out
        let fixed = translate_spec(json, "fixed");
        assert!(!fixed.ctxdsl.contains("ev_request"));
    }

    #[test]
    fn composition_emitted() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {},
            "model_config": {
                "context_name": "comp_test",
                "automata": [
                    {
                        "id": "A",
                        "states": [{"name": "S0", "initial": true}],
                        "transitions": [{"from": "S0", "to": "S0", "label": "ev_x"}]
                    },
                    {
                        "id": "B",
                        "states": [{"name": "T0", "initial": true}],
                        "transitions": [{"from": "T0", "to": "T0", "label": "ev_x"}]
                    }
                ],
                "composition": {
                    "type": "asynchronous",
                    "name": "system",
                    "members": ["A", "B"]
                }
            }
        }"#;

        let output = translate_spec(json, "vulnerable");
        assert!(output.ctxdsl.contains("asynchronous system"));
        assert!(output.ctxdsl.contains("members [A, B]"));
    }

    #[test]
    fn properties_with_over() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {},
            "model_config": {
                "context_name": "prop_test",
                "automata": [
                    {
                        "id": "Main",
                        "states": [{"name": "S0", "initial": true}, {"name": "Bad"}],
                        "transitions": [
                            {"from": "S0", "to": "Bad", "label": "ev_fail", "mode": "vulnerable"},
                            {"from": "S0", "to": "S0", "label": "noop"},
                            {"from": "Bad", "to": "Bad", "label": "noop"}
                        ]
                    }
                ],
                "properties": [
                    {
                        "id": "no_bad",
                        "formula": "nu X. ((!Bad) && ([] X))",
                        "over": "Main",
                        "holds_in_fixed": true,
                        "holds_in_vulnerable": false
                    }
                ]
            }
        }"#;

        let output = translate_spec(json, "vulnerable");
        assert!(output.ctxdsl.contains("formula no_bad"));
        assert!(output.ctxdsl.contains("over Main"));
        assert!(output.ctxdsl.contains("nu X. ((!Bad) && ([] X))"));
    }

    #[test]
    fn controller_emitted() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {},
            "model_config": {
                "context_name": "ctrl_test",
                "automata": [{
                    "id": "System",
                    "states": [{"name": "S0", "initial": true}],
                    "controllable_labels": ["ev_act"],
                    "transitions": [{"from": "S0", "to": "S0", "label": "ev_act"}]
                }],
                "properties": [{"id": "safe", "formula": "nu X. ([] X)", "over": "System"}],
                "controllers": [{"name": "enforcer", "source": "System", "satisfying": "safe"}]
            }
        }"#;

        let output = translate_spec(json, "vulnerable");
        assert!(output.ctxdsl.contains("controller enforcer"));
        assert!(output.ctxdsl.contains("source System"));
        assert!(output.ctxdsl.contains("satisfying safe"));
    }

    #[test]
    fn provenance_headers_emitted() {
        let json = r#"{
            "$schema": "extraction_spec_v1",
            "source": {
                "repo": "owner/repo",
                "commit": "abc123def456789",
                "file": "src/server.ts",
                "cve": "CVE-2026-99999",
                "issue": "Transport reuse bug"
            },
            "bugs": [{"id": "bug1", "description": "Missing guard on close"}],
            "model_config": {
                "context_name": "prov_test",
                "automata": [{
                    "id": "A",
                    "states": [{"name": "S0", "initial": true}],
                    "transitions": [{"from": "S0", "to": "S0", "label": "noop"}]
                }]
            }
        }"#;

        let output = translate_spec(json, "vulnerable");
        assert!(
            output
                .ctxdsl
                .contains("// @generated-from: extraction_spec_v1")
        );
        assert!(output.ctxdsl.contains("// @source-file: src/server.ts"));
        assert!(output.ctxdsl.contains("// @commit: abc123def456789"));
        assert!(output.ctxdsl.contains("// @repo: owner/repo"));
        assert!(output.ctxdsl.contains("// @mode: vulnerable"));
        assert!(output.ctxdsl.contains("// @cve: CVE-2026-99999"));
        assert!(output.ctxdsl.contains("// @bug: bug1"));
    }
}
