//! Verify orchestrator (A2.4) — the public entry point that wires
//! [`crate::verify::config`], [`crate::verify::binding`], adapter
//! dispatch, [`crate::verify::assemble`], parsing, realization, and
//! μ-calculus evaluation into the [`VerifyReport`].
//!
//! ## Pipeline
//!
//! ```text
//! VerifyConfig + base_dir
//!     │
//!     ▼  validate()  — fail-fast on any ConfigIssue
//!     │
//!     ▼  AlphabetBinding::from_config()
//!     │
//!     ▼  for each [[sources]]:
//!     │     read files → dispatch adapter → AdapterOutput.ctxdsl
//!     │     apply per-source renamings  → SourceCtxdsl
//!     │
//!     ▼  assemble_unified_ctxdsl()      → assembled CTXDSL document
//!     │
//!     ▼  context_dsl::parse()           → main ContextDoc + props ContextDoc
//!     ▼  realize_context(main, &[props])→ RealizedContext
//!     │
//!     ▼  for each [[properties]]:
//!     │     resolve template (if any)   → concrete formula
//!     │     mu_calculus::evaluate_with_options → PropertyVerdict
//!     │
//!     ▼  VerifyReport
//! ```
//!
//! ## Adapter dispatch (today)
//!
//! The dispatch table currently understands two adapter names:
//!
//! - **`"ctxdsl"`** — pass-through. The source's file content is
//!   already CTXDSL; no translation needed. Useful for hand-authored
//!   protocol specs and for testing the orchestrator without dragging
//!   in real adapters.
//!
//! - **`"xstate"`** — uses [`crate::adapter::xstate::XStateAdapter`].
//!
//! Other adapters (`c-codesign`, `sv-rtl`, `tlsf`, `aiger`, `btor2`,
//! `promela`, `extraction`) are deferred to A2.5+ — they each require
//! adapter-option plumbing that maps the config's free-form
//! `[[sources]].options` table onto each adapter's strongly-typed
//! option struct, which is out of scope for the first orchestrator
//! slice. Calls with an unrecognised adapter return
//! [`VerifyError::UnknownAdapter`].

use std::path::Path;

use crate::adapter::templates::{TemplateRef, TemplateRegistry};
use crate::adapter::xstate::XStateAdapter;
use crate::adapter::{AdapterOptions, FormatAdapter};
use crate::clts::IdStorage;
use crate::mu_calculus::{EvaluationOptions, evaluate_with_options};
use crate::verify::assemble::{
    AutomatonDiscovery, CompositionSpec, ResolvedProperty, SourceCtxdsl, assemble_unified_ctxdsl,
    extract_context_body,
};
use crate::verify::binding::{AlphabetBinding, apply_renamings_to_ctxdsl};
use crate::verify::config::{PropertySection, VerifyConfig};
use crate::verify::report::{
    CompositionInfo, PropertyFormulaSource, PropertyVerdict, SourceSummary, VerifyError,
    VerifyReport,
};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the verify pipeline against `config` and produce a
/// [`VerifyReport`].
///
/// `base_dir` is the directory relative paths in the config resolve
/// against (typically the parent directory of the `verify.toml`).
pub fn verify_project(config: &VerifyConfig, base_dir: &Path) -> Result<VerifyReport, VerifyError> {
    // 1. Validate config.
    let issues = config.validate();
    if !issues.is_empty() {
        return Err(VerifyError::ConfigValidationFailed(issues));
    }

    // 2. Build alphabet binding.
    let binding =
        AlphabetBinding::from_config(config, base_dir).map_err(VerifyError::AlphabetBinding)?;
    let per_source_renamings = binding.per_source_renamings();

    // 3. For each source: read files, dispatch adapter, apply renamings.
    let mut source_ctxdsls: Vec<SourceCtxdsl> = Vec::with_capacity(config.sources.len());
    let mut source_summaries: Vec<SourceSummary> = Vec::with_capacity(config.sources.len());
    for source in &config.sources {
        // Read the first source file and pass it to the adapter.
        // Multi-file sources are a follow-up — most adapters today
        // take one file. We document the limitation.
        let primary_file = source
            .files
            .first()
            .expect("validator rejected empty files");
        let path = resolve_path(base_dir, primary_file);
        let content =
            std::fs::read_to_string(&path).map_err(|source_err| VerifyError::SourceReadFailed {
                path: path.clone(),
                source: source_err,
            })?;

        let raw_ctxdsl = dispatch_adapter(&source.adapter, &source.id, &content)?;

        // Apply per-source renamings from the binding. For Direct
        // strategy the map is absent / empty so this is a no-op.
        let rewritten = match per_source_renamings.get(&source.id) {
            Some(renamings) if !renamings.is_empty() => {
                apply_renamings_to_ctxdsl(&raw_ctxdsl, renamings)
            }
            _ => raw_ctxdsl,
        };

        source_ctxdsls.push(SourceCtxdsl {
            source_id: source.id.clone(),
            ctxdsl: rewritten,
        });
        source_summaries.push(SourceSummary {
            id: source.id.clone(),
            adapter: source.adapter.clone(),
            automaton: None, // filled after assembly resolves the member
        });
    }

    // 4. Build CompositionSpec (resolve composition_name default).
    let composition = CompositionSpec {
        semantics: config.composition.semantics.clone(),
        members: config.composition.members.clone(),
        name: config.composition_name(),
    };

    // 5. Resolve properties — turn each `template` into a concrete
    // formula via the builtin registry; pass `formula` through as-is.
    let template_registry = TemplateRegistry::builtin();
    let mut resolved_properties: Vec<ResolvedProperty> =
        Vec::with_capacity(config.properties.len());
    let mut property_formula_sources: Vec<PropertyFormulaSource> = Vec::new();
    for p in &config.properties {
        let (formula_text, source) = resolve_property_formula(p, &template_registry)?;
        let over = config.resolve_over(p);
        resolved_properties.push(ResolvedProperty {
            name: p.name.clone(),
            formula: formula_text,
            over,
        });
        property_formula_sources.push(source);
    }

    // 6. Assemble the unified CTXDSL document.
    let assembled = assemble_unified_ctxdsl(
        &config.project.name,
        &source_ctxdsls,
        &composition,
        &resolved_properties,
        &AutomatonDiscovery::FirstAutomaton,
    )
    .map_err(VerifyError::Assemble)?;

    // Backfill SourceSummary.automaton now that we know the
    // composition's resolved members (FirstAutomaton strategy reads
    // them from each source's CTXDSL — re-derive here for the
    // report).
    let resolved_members =
        derive_resolved_member_names(&source_ctxdsls, &config.composition.members);
    for s in &mut source_summaries {
        s.automaton = resolved_members.get(&s.id).cloned();
    }
    let composition_info = CompositionInfo {
        semantics: composition.semantics.clone(),
        name: composition.name.clone(),
        members: composition
            .members
            .iter()
            .filter_map(|id| resolved_members.get(id).cloned())
            .collect(),
    };

    // 7. Parse + realize.
    let (main_text, props_text) = split_main_and_props(&assembled);
    let main_doc = crate::context_dsl::parse(&main_text).map_err(|e| {
        VerifyError::AssembledCtxdslParseFailed {
            message: format!("{e:?}"),
            snippet: snippet_around_error(&main_text, &format!("{e:?}")),
        }
    })?;
    let sidecar_docs = if let Some(props) = props_text.as_deref() {
        let sidecar = crate::context_dsl::parse(props).map_err(|e| {
            VerifyError::AssembledCtxdslParseFailed {
                message: format!("{e:?}"),
                snippet: snippet_around_error(props, &format!("{e:?}")),
            }
        })?;
        vec![sidecar]
    } else {
        Vec::new()
    };
    let realized = crate::context_dsl::realize_context(&main_doc, &sidecar_docs).map_err(|e| {
        VerifyError::RealizeFailed {
            message: format!("{e:?}"),
        }
    })?;

    // 8. Evaluate each property.
    let mut property_verdicts: Vec<PropertyVerdict> = Vec::with_capacity(resolved_properties.len());
    for (idx, p) in resolved_properties.iter().enumerate() {
        let verdict = evaluate_one_property(
            &realized,
            &p.name,
            &p.over,
            property_formula_sources[idx].clone(),
            &p.formula,
        )?;
        property_verdicts.push(verdict);
    }

    Ok(VerifyReport {
        project: config.project.name.clone(),
        sources: source_summaries,
        composition: composition_info,
        property_verdicts,
    })
}

// ---------------------------------------------------------------------------
// Adapter dispatch
// ---------------------------------------------------------------------------

/// Translate a single source file via the adapter named in the config.
///
/// Today's dispatch only knows `"ctxdsl"` (pass-through) and
/// `"xstate"`. Other adapters return [`VerifyError::UnknownAdapter`];
/// support is added incrementally in later A2 slices as the
/// adapter-options plumbing matures.
fn dispatch_adapter(adapter: &str, source_id: &str, content: &str) -> Result<String, VerifyError> {
    match adapter {
        "ctxdsl" => Ok(content.to_string()),
        "xstate" => {
            let options = AdapterOptions::default();
            match XStateAdapter::translate(content, &options) {
                Ok(out) => Ok(out.ctxdsl),
                Err(err) => Err(VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                }),
            }
        }
        other => Err(VerifyError::UnknownAdapter {
            source_id: source_id.to_string(),
            adapter: other.to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Property template resolution
// ---------------------------------------------------------------------------

fn resolve_property_formula(
    p: &PropertySection,
    registry: &TemplateRegistry,
) -> Result<(String, PropertyFormulaSource), VerifyError> {
    if let Some(template_id) = &p.template {
        let tref = TemplateRef {
            template: template_id.clone(),
            args: p.args.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        let inst =
            registry
                .instantiate(&tref)
                .map_err(|e| VerifyError::TemplateInstantiationFailed {
                    property: p.name.clone(),
                    message: e.to_string(),
                })?;
        Ok((
            inst.formula,
            PropertyFormulaSource::Template {
                id: template_id.clone(),
                args: p.args.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            },
        ))
    } else if let Some(formula) = &p.formula {
        Ok((formula.clone(), PropertyFormulaSource::Inline))
    } else {
        // The validator rejects this case; defensive arm.
        Err(VerifyError::TemplateInstantiationFailed {
            property: p.name.clone(),
            message: "neither `template` nor `formula` was supplied".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Property evaluation
// ---------------------------------------------------------------------------

fn evaluate_one_property(
    realized: &crate::context_dsl::RealizedContext,
    name: &str,
    over: &str,
    formula_source: PropertyFormulaSource,
    formula_text: &str,
) -> Result<PropertyVerdict, VerifyError> {
    // The property name in the assembled context isn't necessarily
    // unique relative to predicates/controllers — but our assembler
    // is the only writer, so the lookup is stable.
    let formula = realized
        .formulas
        .get(name)
        .ok_or_else(|| VerifyError::EvaluationFailed {
            property: name.to_string(),
            message: format!(
                "formula `{name}` not present in realized context (this is a bug in the assembler)"
            ),
        })?;
    let clts = realized.context.clts(over).ok_or_else(|| {
        let known: Vec<String> = realized.context.clts_names().into_iter().collect();
        VerifyError::UnknownAutomaton {
            property: name.to_string(),
            over: over.to_string(),
            known,
        }
    })?;
    let env = realized.environment_for(over);
    let options = EvaluationOptions::default();
    let result = evaluate_with_options(&formula.formula, clts, &env, &options).map_err(|e| {
        VerifyError::EvaluationFailed {
            property: name.to_string(),
            message: e.to_string(),
        }
    })?;

    let total_states = clts.state_count();
    let satisfying_states = (0..total_states)
        .filter(|i| result.get(*i).map(|b| *b).unwrap_or(false))
        .count();
    let initial_states: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|sid| clts.state_name(*sid).map(str::to_string))
        .collect();
    let initial_satisfying: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|sid| {
            if result.get(sid.index()).map(|b| *b).unwrap_or(false) {
                clts.state_name(*sid).map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    let satisfied = !initial_states.is_empty() && initial_satisfying.len() == initial_states.len();

    Ok(PropertyVerdict {
        name: name.to_string(),
        formula_source,
        formula: formula_text.to_string(),
        over: over.to_string(),
        satisfied,
        total_states,
        satisfying_states,
        initial_states,
        initial_satisfying,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_path(base_dir: &Path, p: &Path) -> std::path::PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// Split the assembler's two-context output into `(main, props)`.
/// The assembler emits the main context first followed by an optional
/// `\ncontext <X>Props { ... }`. Parsing each separately keeps error
/// messages localised.
fn split_main_and_props(assembled: &str) -> (String, Option<String>) {
    if let Some(idx) = assembled.find("\ncontext ") {
        // Make sure the second `context` keyword is preceded by the
        // newline-then-keyword pattern the assembler emits.
        let main = assembled[..idx].to_string();
        let props = assembled[idx + 1..].to_string();
        (main, Some(props))
    } else {
        (assembled.to_string(), None)
    }
}

/// Derive each composition member's resolved automaton name by
/// running the same `FirstAutomaton` scan the assembler uses. Returns
/// a `source_id → automaton_name` map.
fn derive_resolved_member_names(
    sources: &[SourceCtxdsl],
    member_ids: &[String],
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for mid in member_ids {
        if let Some(src) = sources.iter().find(|s| &s.source_id == mid)
            && let Some(body) = extract_context_body(&src.ctxdsl)
            && let Some(name) = scan_first_automaton(body)
        {
            out.insert(mid.clone(), name.to_string());
        }
    }
    out
}

fn scan_first_automaton(body: &str) -> Option<&str> {
    let kw = body.find("automaton")?;
    let after = &body[kw + "automaton".len()..];
    let trimmed = after.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(trimmed.len());
    if end == 0 {
        return None;
    }
    Some(&trimmed[..end])
}

fn snippet_around_error(text: &str, _err: &str) -> String {
    let lines: Vec<&str> = text.lines().take(20).collect();
    lines.join("\n")
}

// Silence unused-import lint for IdStorage: it's brought into scope
// so that the closure-style generic bounds in
// `evaluate_one_property` don't need to repeat the constraint.
#[allow(dead_code)]
fn _id_storage_marker<S: IdStorage, L: IdStorage>(_: &crate::clts::Clts<S, L>) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_ctxdsl_source(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    const SIMPLE_LIGHT_CTXDSL: &str = r#"
context Light {
    alphabet { label tick_light; }
    automata {
        automaton Light {
            states { state green initial; state yellow; state red; }
            transitions {
                transition green -> yellow on label tick_light;
                transition yellow -> red on label tick_light;
                transition red -> green on label tick_light;
            }
        }
    }
}
"#;

    const SIMPLE_GATE_CTXDSL: &str = r#"
context Gate {
    alphabet { label tick_gate; }
    automata {
        automaton Gate {
            states { state closed initial; state open; }
            transitions {
                transition closed -> open on label tick_gate;
                transition open -> closed on label tick_gate;
            }
        }
    }
}
"#;

    fn build_two_source_config(dir: &Path) -> VerifyConfig {
        let _ = write_ctxdsl_source(dir, "light.ctxdsl", SIMPLE_LIGHT_CTXDSL);
        let _ = write_ctxdsl_source(dir, "gate.ctxdsl", SIMPLE_GATE_CTXDSL);
        let toml_src = r#"
[project]
name = "Demo"

[[sources]]
id = "light"
adapter = "ctxdsl"
files = ["light.ctxdsl"]

[[sources]]
id = "gate"
adapter = "ctxdsl"
files = ["gate.ctxdsl"]

[alphabet]
strategy = "direct"

[composition]
semantics = "asynchronous"
members = ["light", "gate"]
name = "System"

[[properties]]
name = "no_deadlock"
template = "no_deadlock"
over = "System"

[[properties]]
name = "reach_self"
formula = "true"
over = "System"
"#;
        VerifyConfig::from_toml(toml_src).unwrap()
    }

    #[test]
    fn end_to_end_two_ctxdsl_sources_synchronous() {
        let temp = tempdir().unwrap();
        let config = build_two_source_config(temp.path());
        let report = verify_project(&config, temp.path()).expect("verify pipeline succeeded");

        assert_eq!(report.project, "Demo");
        assert_eq!(report.sources.len(), 2);
        assert_eq!(report.sources[0].id, "light");
        assert_eq!(report.sources[0].adapter, "ctxdsl");
        assert_eq!(report.sources[0].automaton.as_deref(), Some("Light"));
        assert_eq!(report.sources[1].automaton.as_deref(), Some("Gate"));
        assert_eq!(report.composition.name, "System");
        assert_eq!(report.composition.semantics, "asynchronous");
        assert_eq!(
            report.composition.members,
            vec!["Light".to_string(), "Gate".to_string()]
        );
        assert_eq!(report.property_verdicts.len(), 2);
        // `true` is satisfied everywhere; verdict.satisfied should be true.
        let reach = report
            .property_verdicts
            .iter()
            .find(|v| v.name == "reach_self")
            .unwrap();
        assert!(reach.satisfied);
        assert!(reach.total_states > 0);
        match &reach.formula_source {
            PropertyFormulaSource::Inline => {}
            other => panic!("expected Inline, got {other:?}"),
        }
        // Template-sourced verdict carries the template id.
        let no_dl = report
            .property_verdicts
            .iter()
            .find(|v| v.name == "no_deadlock")
            .unwrap();
        match &no_dl.formula_source {
            PropertyFormulaSource::Template { id, .. } => assert_eq!(id, "no_deadlock"),
            other => panic!("expected Template, got {other:?}"),
        }
    }

    #[test]
    fn config_validation_failures_short_circuit() {
        // Build a config whose `[composition].members` references an unknown source id.
        let temp = tempdir().unwrap();
        let _ = write_ctxdsl_source(temp.path(), "light.ctxdsl", SIMPLE_LIGHT_CTXDSL);
        let toml_src = r#"
[project]
name = "Bad"

[[sources]]
id = "light"
adapter = "ctxdsl"
files = ["light.ctxdsl"]

[composition]
semantics = "synchronous"
members = ["light", "ghost"]
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let err = verify_project(&config, temp.path()).unwrap_err();
        match err {
            VerifyError::ConfigValidationFailed(issues) => {
                assert!(issues.iter().any(|i| matches!(
                    i,
                    crate::verify::config::ConfigIssue::CompositionUnknownMember { id } if id == "ghost"
                )));
            }
            other => panic!("expected ConfigValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn unknown_adapter_returns_dispatch_error() {
        let temp = tempdir().unwrap();
        let _ = write_ctxdsl_source(temp.path(), "x.txt", "// not really anything");
        let toml_src = r#"
[project]
name = "X"

[[sources]]
id = "x"
adapter = "made-up-adapter"
files = ["x.txt"]

[composition]
semantics = "synchronous"
members = ["x"]

[[properties]]
name = "p"
formula = "true"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let err = verify_project(&config, temp.path()).unwrap_err();
        assert!(matches!(
            err,
            VerifyError::UnknownAdapter { adapter, .. } if adapter == "made-up-adapter"
        ));
    }

    #[test]
    fn source_read_failure_propagates() {
        let temp = tempdir().unwrap();
        // Reference a file that doesn't exist on disk.
        let toml_src = r#"
[project]
name = "X"

[[sources]]
id = "x"
adapter = "ctxdsl"
files = ["missing.ctxdsl"]

[composition]
semantics = "synchronous"
members = ["x"]
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let err = verify_project(&config, temp.path()).unwrap_err();
        assert!(matches!(err, VerifyError::SourceReadFailed { .. }));
    }

    #[test]
    fn template_arg_collision_is_caught() {
        // Template ID exists but a required arg is missing.
        let temp = tempdir().unwrap();
        let _ = write_ctxdsl_source(temp.path(), "light.ctxdsl", SIMPLE_LIGHT_CTXDSL);
        let toml_src = r#"
[project]
name = "X"

[[sources]]
id = "light"
adapter = "ctxdsl"
files = ["light.ctxdsl"]

[composition]
semantics = "synchronous"
members = ["light"]
name = "Only"

[[properties]]
name = "p"
template = "reachable"
# TARGET arg deliberately omitted
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let err = verify_project(&config, temp.path()).unwrap_err();
        assert!(matches!(
            err,
            VerifyError::TemplateInstantiationFailed { .. }
        ));
    }
}
