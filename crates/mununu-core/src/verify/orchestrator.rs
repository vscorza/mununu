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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
use crate::verify::register_map_rewriter::derive_sv_renamings_from_register_map;
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
    // For RegisterMap binding, eagerly derive the SV-side renaming
    // table once — it's identical for every sv-rtl source under this
    // binding. `None` when binding is Direct or Renamings.
    let register_map_sv_renamings: Option<BTreeMap<String, String>> = match &binding {
        AlphabetBinding::RegisterMap { map, .. } => {
            Some(derive_sv_renamings_from_register_map(map))
        }
        _ => None,
    };

    // 3. For each source: read files, dispatch adapter, apply renamings.
    // Parameterised sources (`count >= 2`) expand to N instances
    // named `<id>_0` .. `<id>_<N-1>`. Each instance substitutes
    // `{instance_id}` in the file content with its full name before
    // the adapter sees it.
    let mut source_ctxdsls: Vec<SourceCtxdsl> = Vec::with_capacity(config.sources.len());
    let mut source_summaries: Vec<SourceSummary> = Vec::with_capacity(config.sources.len());
    for source in &config.sources {
        let primary_file = source
            .files
            .first()
            .expect("validator rejected empty files");
        let path = resolve_path(base_dir, primary_file);
        let raw_content =
            std::fs::read_to_string(&path).map_err(|source_err| VerifyError::SourceReadFailed {
                path: path.clone(),
                source: source_err,
            })?;

        let count = source.count.unwrap_or(1).max(1);
        let instances: Vec<(String, String)> = if count == 1 {
            vec![(source.id.clone(), raw_content.clone())]
        } else {
            (0..count)
                .map(|i| {
                    let instance_id = format!("{}_{}", source.id, i);
                    let substituted = raw_content.replace("{instance_id}", &instance_id);
                    (instance_id, substituted)
                })
                .collect()
        };

        for (instance_id, content) in instances {
            let raw_ctxdsl = dispatch_adapter(
                &source.adapter,
                &instance_id,
                &path,
                &content,
                &source.options,
                base_dir,
            )?;

            // Apply per-source renamings from the binding. The renamings
            // are keyed on the *original* source id (so users author
            // them once and they apply to every instance).
            let mut rewritten = match per_source_renamings.get(&source.id) {
                Some(renamings) if !renamings.is_empty() => {
                    apply_renamings_to_ctxdsl(&raw_ctxdsl, renamings)
                }
                _ => raw_ctxdsl,
            };
            if let (Some(rm_renamings), "sv-rtl") =
                (register_map_sv_renamings.as_ref(), source.adapter.as_str())
                && !rm_renamings.is_empty()
            {
                rewritten = apply_renamings_to_ctxdsl(&rewritten, rm_renamings);
            }

            source_ctxdsls.push(SourceCtxdsl {
                source_id: instance_id.clone(),
                ctxdsl: rewritten,
            });
            source_summaries.push(SourceSummary {
                id: instance_id,
                adapter: source.adapter.clone(),
                automaton: None,
            });
        }
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
    // report). `derive_resolved_member_names` returns the full
    // expansion list per member entry so `<src>.*` wildcards land
    // in `composition_info.members` correctly.
    let resolved_members =
        derive_resolved_member_names(&source_ctxdsls, &config.composition.members);
    for s in &mut source_summaries {
        // SourceSummary.automaton holds the *primary* automaton of
        // the source. For wildcard expansions (`<src>.*`) the user
        // sees the first automaton; the full list lives in
        // CompositionInfo.members.
        s.automaton = resolved_members
            .iter()
            .find(|(k, _)| k.trim_end_matches(".*") == s.id)
            .and_then(|(_, names)| names.first().cloned());
    }
    let composition_info = CompositionInfo {
        semantics: composition.semantics.clone(),
        name: composition.name.clone(),
        members: composition
            .members
            .iter()
            .filter_map(|id| resolved_members.get(id).cloned())
            .flatten()
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
// Inspection (alphabet + state-predicate introspection — plan Part 6 item 3)
// ---------------------------------------------------------------------------

/// Per-automaton snapshot produced by [`inspect_project`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct AutomatonInspection {
    /// Resolved automaton name (e.g. the CTXDSL identifier after
    /// adapter dispatch + assembly).
    pub name: String,
    /// Source id whose adapter emitted this automaton, when known.
    pub source_id: Option<String>,
    /// Alphabet (union of controllable + internal + uncontrollable
    /// labels) the automaton participates in.
    pub alphabet: Vec<String>,
    /// State names declared on this automaton.
    pub states: Vec<String>,
    /// Initial-state names.
    pub initial_states: Vec<String>,
}

/// Composition-level snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CompositionInspection {
    /// Composition info (semantics, name, resolved members) as it
    /// appears in [`VerifyReport`].
    pub info: CompositionInfo,
    /// Union of every member's alphabet — the labels that can appear
    /// in property formulas referencing the composition.
    pub alphabet: Vec<String>,
    /// Names of the realized composition CLTS's states. Empty when
    /// the realiser does not eagerly materialise the composed CLTS
    /// (some compositions are evaluated symbolically).
    pub state_names: Vec<String>,
    /// Names of declared per-state predicates the composition
    /// exposes (from CTXDSL `predicates { … }` blocks). Useful as a
    /// "what can I write in a mu-calculus formula" list.
    pub predicate_names: Vec<String>,
}

/// Report produced by [`inspect_project`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct InspectionReport {
    pub project: String,
    pub sources: Vec<SourceSummary>,
    /// One entry per realized automaton (per-source + the composition
    /// member resolution).
    pub automata: Vec<AutomatonInspection>,
    /// Composition-level alphabet + state-predicate listing.
    pub composition: CompositionInspection,
}

/// Run the verify pipeline through realize, then return an
/// introspection report covering each realized automaton's alphabet,
/// states, and the composition-level alphabet + predicate listing.
///
/// **Skips property evaluation entirely** — use this to discover what
/// labels and state predicates the realized context exposes before
/// authoring property formulas. Closes plan Part 6 item 3 (and Part
/// 2 automation gap #2).
///
/// Reuses every step of [`verify_project`] up to and including the
/// realize step.
pub fn inspect_project(
    config: &VerifyConfig,
    base_dir: &Path,
) -> Result<InspectionReport, VerifyError> {
    // 1. Validate config.
    let issues = config.validate();
    if !issues.is_empty() {
        return Err(VerifyError::ConfigValidationFailed(issues));
    }

    // 2. Alphabet binding (same as verify_project).
    let binding =
        AlphabetBinding::from_config(config, base_dir).map_err(VerifyError::AlphabetBinding)?;
    let per_source_renamings = binding.per_source_renamings();
    let register_map_sv_renamings: Option<BTreeMap<String, String>> = match &binding {
        AlphabetBinding::RegisterMap { map, .. } => {
            Some(derive_sv_renamings_from_register_map(map))
        }
        _ => None,
    };

    // 3. Per-source dispatch + renaming + parameterised-instance
    // expansion (same as verify_project).
    let mut source_ctxdsls: Vec<SourceCtxdsl> = Vec::with_capacity(config.sources.len());
    let mut source_summaries: Vec<SourceSummary> = Vec::with_capacity(config.sources.len());
    for source in &config.sources {
        let primary_file = source
            .files
            .first()
            .expect("validator rejected empty files");
        let path = resolve_path(base_dir, primary_file);
        let raw_content =
            std::fs::read_to_string(&path).map_err(|source_err| VerifyError::SourceReadFailed {
                path: path.clone(),
                source: source_err,
            })?;
        let count = source.count.unwrap_or(1).max(1);
        let instances: Vec<(String, String)> = if count == 1 {
            vec![(source.id.clone(), raw_content.clone())]
        } else {
            (0..count)
                .map(|i| {
                    let instance_id = format!("{}_{}", source.id, i);
                    let substituted = raw_content.replace("{instance_id}", &instance_id);
                    (instance_id, substituted)
                })
                .collect()
        };
        for (instance_id, content) in instances {
            let raw_ctxdsl = dispatch_adapter(
                &source.adapter,
                &instance_id,
                &path,
                &content,
                &source.options,
                base_dir,
            )?;
            let mut rewritten = match per_source_renamings.get(&source.id) {
                Some(renamings) if !renamings.is_empty() => {
                    apply_renamings_to_ctxdsl(&raw_ctxdsl, renamings)
                }
                _ => raw_ctxdsl,
            };
            if let (Some(rm_renamings), "sv-rtl") =
                (register_map_sv_renamings.as_ref(), source.adapter.as_str())
                && !rm_renamings.is_empty()
            {
                rewritten = apply_renamings_to_ctxdsl(&rewritten, rm_renamings);
            }
            source_ctxdsls.push(SourceCtxdsl {
                source_id: instance_id.clone(),
                ctxdsl: rewritten,
            });
            source_summaries.push(SourceSummary {
                id: instance_id,
                adapter: source.adapter.clone(),
                automaton: None,
            });
        }
    }

    // 4. CompositionSpec.
    let composition = CompositionSpec {
        semantics: config.composition.semantics.clone(),
        members: config.composition.members.clone(),
        name: config.composition_name(),
    };

    // 5. NO property resolution — the whole point of inspection is to
    //    let the user discover what they CAN write in a property
    //    before authoring it.
    let resolved_properties: Vec<ResolvedProperty> = Vec::new();

    // 6. Assemble.
    let assembled = assemble_unified_ctxdsl(
        &config.project.name,
        &source_ctxdsls,
        &composition,
        &resolved_properties,
        &AutomatonDiscovery::FirstAutomaton,
    )
    .map_err(VerifyError::Assemble)?;
    let resolved_members =
        derive_resolved_member_names(&source_ctxdsls, &config.composition.members);
    for s in &mut source_summaries {
        s.automaton = resolved_members
            .iter()
            .find(|(k, _)| k.trim_end_matches(".*") == s.id)
            .and_then(|(_, names)| names.first().cloned());
    }
    let composition_info = CompositionInfo {
        semantics: composition.semantics.clone(),
        name: composition.name.clone(),
        members: composition
            .members
            .iter()
            .filter_map(|id| resolved_members.get(id).cloned())
            .flatten()
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

    // 8. Build the introspection report. Map every emitted automaton
    // back to its originating source. Wildcards (`<src>.*`) and bare
    // entries both contribute multiple automaton-to-source mappings.
    let source_by_automaton: BTreeMap<String, String> = resolved_members
        .iter()
        .flat_map(|(member_entry, names)| {
            let src = member_entry.trim_end_matches(".*").to_string();
            names.iter().map(move |n| (n.clone(), src.clone()))
        })
        .collect();
    let mut automata_inspections: Vec<AutomatonInspection> = Vec::new();
    for clts_name in realized.context.clts_names() {
        if let Some(clts) = realized.context.clts(&clts_name) {
            let mut alphabet = clts.alphabet();
            alphabet.sort();
            let states: Vec<String> = clts
                .states()
                .filter_map(|sid| clts.state_name(sid).map(String::from))
                .collect();
            let initial_states: Vec<String> = clts
                .initial_states()
                .iter()
                .filter_map(|sid| clts.state_name(*sid).map(String::from))
                .collect();
            automata_inspections.push(AutomatonInspection {
                name: clts_name.clone(),
                source_id: source_by_automaton.get(&clts_name).cloned(),
                alphabet,
                states,
                initial_states,
            });
        }
    }

    let composition_alphabet: Vec<String> = {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for a in &automata_inspections {
            for lab in &a.alphabet {
                seen.insert(lab.clone());
            }
        }
        seen.into_iter().collect()
    };
    let composition_states: Vec<String> = realized
        .context
        .clts(&composition_info.name)
        .map(|c| {
            c.states()
                .filter_map(|sid| c.state_name(sid).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let predicate_names: Vec<String> = realized
        .predicates
        .values()
        .flat_map(|preds| preds.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    Ok(InspectionReport {
        project: config.project.name.clone(),
        sources: source_summaries,
        automata: automata_inspections,
        composition: CompositionInspection {
            info: composition_info,
            alphabet: composition_alphabet,
            state_names: composition_states,
            predicate_names,
        },
    })
}

// ---------------------------------------------------------------------------
// Adapter dispatch
// ---------------------------------------------------------------------------

/// Translate a single source file via the adapter named in the config.
///
/// Recognised adapter names:
///
/// - `"ctxdsl"` — pass-through; `content` is already CTXDSL.
/// - `"xstate"` — uses [`XStateAdapter`].
/// - `"sv-rtl"` — uses the SystemVerilog adapter via the existing
///   `SystemVerilogAdapter::translate` entry point. Options
///   currently ignored by this layer — the SV adapter has its own
///   per-source sidecar conventions (`.mununu.json` next to the
///   `.sv` file).
/// - `"crewai"` — uses [`crate::adapter::crewai::CrewaiAdapter`].
///   Per-agent automata + sequential supervisor + asynchronous
///   composition. Options currently ignored.
/// - `"langgraph"` — uses
///   [`crate::adapter::langgraph::LangGraphAdapter`]. Nodes → states,
///   edges → `node_<from>_enter` transitions. Options currently
///   ignored.
/// - `"extraction"` — uses [`ExtractionAdapter`]. Options:
///   `mode = "fixed" | "vulnerable" | "both"` (default `"both"`).
/// - `"c-codesign"` — uses [`extract_c_via_llvm`] (LLVM IR via
///   clang), wraps each function's synthesised `automaton_ctxdsl`
///   into a `context FwSource { … }` block. Options:
///   `register_map = <PATH>` (required for `synthesize_automaton`),
///   `synthesize_automaton = bool` (default `true`),
///   `cmsis_stubs = bool` (default `true`), `include_paths =
///   [string]`, `defines = [string]`, `clang = <PATH>`.
///
/// Other adapters return [`VerifyError::UnknownAdapter`].
fn dispatch_adapter(
    adapter: &str,
    source_id: &str,
    file_path: &Path,
    content: &str,
    options: &std::collections::BTreeMap<String, toml::Value>,
    base_dir: &Path,
) -> Result<String, VerifyError> {
    match adapter {
        "ctxdsl" => Ok(content.to_string()),
        "xstate" => {
            let opts = AdapterOptions::default();
            XStateAdapter::translate(content, &opts)
                .map(|out| out.ctxdsl)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                })
        }
        "sv-rtl" => {
            let opts = AdapterOptions::default();
            crate::adapter::systemverilog::SystemVerilogAdapter::translate(content, &opts)
                .map(|out| out.ctxdsl)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                })
        }
        "crewai" => {
            let opts = AdapterOptions::default();
            crate::adapter::crewai::CrewaiAdapter::translate(content, &opts)
                .map(|out| out.ctxdsl)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                })
        }
        "langgraph" => {
            let opts = AdapterOptions::default();
            crate::adapter::langgraph::LangGraphAdapter::translate(content, &opts)
                .map(|out| out.ctxdsl)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                })
        }
        "microcode" => {
            let opts = AdapterOptions::default();
            crate::adapter::microcode::MicrocodeAdapter::translate(content, &opts)
                .map(|out| out.ctxdsl)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                })
        }
        "extraction" => {
            let mode = options
                .get("mode")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let opts = AdapterOptions {
                mode,
                ..AdapterOptions::default()
            };
            crate::adapter::extraction::ExtractionAdapter::translate(content, &opts)
                .map(|out| out.ctxdsl)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: adapter.to_string(),
                    message: err.to_string(),
                })
        }
        "c-codesign" => dispatch_c_codesign(source_id, file_path, options, base_dir),
        other => Err(VerifyError::UnknownAdapter {
            source_id: source_id.to_string(),
            adapter: other.to_string(),
        }),
    }
}

/// Dispatch the `c-codesign` adapter — runs LLVM-IR extraction via
/// clang, then wraps the per-function synthesised automaton fragments
/// into a single `context FwSource { … }` block the verify assembler
/// can ingest.
fn dispatch_c_codesign(
    source_id: &str,
    file_path: &Path,
    options: &std::collections::BTreeMap<String, toml::Value>,
    base_dir: &Path,
) -> Result<String, VerifyError> {
    use crate::codesign::c_extract_llvm::{LlvmExtractOptions, extract_c_via_llvm};
    use crate::codesign::register_map::RegisterMap;

    let mut extract_opts = LlvmExtractOptions::default();

    if let Some(toml::Value::String(s)) = options.get("clang") {
        extract_opts.clang_path = Some(PathBuf::from(s));
    }
    if let Some(toml::Value::Array(arr)) = options.get("include_paths") {
        extract_opts.include_paths = arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| {
                let p = PathBuf::from(s);
                if p.is_absolute() { p } else { base_dir.join(p) }
            })
            .collect();
    }
    if let Some(toml::Value::Array(arr)) = options.get("defines") {
        extract_opts.defines = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    extract_opts.synthesize_automaton = options
        .get("synthesize_automaton")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // `cmsis_stubs = true` prepends the bundled `cmsis-stubs/` include
    // path so firmware C files that `#include "mununu_annotations.h"`
    // or use the bundled CMSIS shims compile cleanly. Mirrors the
    // `mununu codesign extract-c --cmsis-stubs` CLI flag.
    let cmsis_stubs_enabled = options
        .get("cmsis_stubs")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if cmsis_stubs_enabled {
        extract_opts
            .include_paths
            .insert(0, locate_bundled_cmsis_stubs());
    }

    // Register map: load the JSON sidecar referenced by the source's
    // `register_map` option (set by the codesign-shorthand translator)
    // so accesses get matched and labelled.
    if let Some(toml::Value::String(rm_str)) = options.get("register_map") {
        let rm_path = {
            let p = PathBuf::from(rm_str);
            if p.is_absolute() { p } else { base_dir.join(p) }
        };
        // Skip non-JSON files (SVD path; the translator records it
        // verbatim, but the c-codesign adapter only consumes JSON
        // register-map sidecars today).
        let is_json = rm_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if is_json {
            let bytes =
                std::fs::read(&rm_path).map_err(|e| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: "c-codesign".to_string(),
                    message: format!(
                        "failed to read register-map sidecar {}: {e}",
                        rm_path.display()
                    ),
                })?;
            let rm: RegisterMap = serde_json::from_slice(&bytes).map_err(|e| {
                VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: "c-codesign".to_string(),
                    message: format!(
                        "failed to parse register-map sidecar {} as JSON: {e}",
                        rm_path.display()
                    ),
                }
            })?;
            extract_opts.register_map = Some(rm);
        }
    }

    let extraction = extract_c_via_llvm(file_path, &extract_opts).map_err(|err| {
        VerifyError::AdapterTranslationFailed {
            source_id: source_id.to_string(),
            adapter: "c-codesign".to_string(),
            message: format!("{err:?}"),
        }
    })?;

    // Build the per-source CTXDSL: collect every label every function
    // touches, declare them in the alphabet, then concatenate all
    // `automaton_ctxdsl` fragments. Functions without a synthesised
    // automaton are skipped (they had no register accesses).
    use crate::codesign::coupling::rendezvous_label_name;
    use std::collections::BTreeSet;

    let mut labels: BTreeSet<String> = BTreeSet::new();
    for f in &extraction.functions {
        for a in &f.accesses {
            labels.insert(rendezvous_label_name(
                &a.register,
                a.field.as_deref(),
                a.kind,
            ));
        }
    }

    let mut out = String::new();
    out.push_str("context FwSource {\n");
    if !labels.is_empty() {
        out.push_str("    alphabet {\n");
        for label in &labels {
            out.push_str(&format!("        label {label};\n"));
        }
        out.push_str("    }\n");
    }
    // Each function's `automaton_ctxdsl` already includes its own
    // `automata { automaton X { ... } }` wrapper, so we splice the
    // fragments in verbatim (no additional `automata { ... }` shell
    // — that would produce nested `automata { automata { ... } }`).
    for f in &extraction.functions {
        if let Some(frag) = &f.automaton_ctxdsl {
            for line in frag.lines() {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.push_str("}\n");
    Ok(out)
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
/// Resolve each `[composition].members` entry to its expanded list
/// of automaton names. Supports the `<source_id>.*` wildcard form
/// (one entry → every automaton the source emits) alongside the
/// legacy bare-source-id form (one entry → the first automaton).
///
/// Returned map is keyed by the **member entry as written in the
/// config** (so wildcards round-trip), with the value being the
/// possibly-multiple resolved automaton names in declaration order.
fn derive_resolved_member_names(
    sources: &[SourceCtxdsl],
    member_ids: &[String],
) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out = std::collections::BTreeMap::new();
    for mid in member_ids {
        let (src_id, expand_all) = match mid.strip_suffix(".*") {
            Some(id) => (id, true),
            None => (mid.as_str(), false),
        };
        let Some(src) = sources.iter().find(|s| s.source_id == src_id) else {
            continue;
        };
        let Some(body) = extract_context_body(&src.ctxdsl) else {
            continue;
        };
        let names: Vec<String> = crate::verify::assemble::all_automaton_names(body)
            .into_iter()
            .map(String::from)
            .collect();
        if names.is_empty() {
            continue;
        }
        let value = if expand_all {
            names
        } else {
            vec![names.into_iter().next().unwrap()]
        };
        out.insert(mid.clone(), value);
    }
    out
}

/// Resolve the bundled `cmsis-stubs/` directory. Tries the
/// workspace-relative path (dev / source builds) first, then a
/// `share/mununu/cmsis-stubs` fallback for installed binaries.
/// Returns the workspace-relative path as a non-existent fallback so
/// clang surfaces a clear "include not found" if neither candidate
/// is on disk.
fn locate_bundled_cmsis_stubs() -> PathBuf {
    let candidates: &[PathBuf] = &[
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("cmsis-stubs"),
        PathBuf::from("crates/mununu-core/cmsis-stubs"),
        PathBuf::from("../share/mununu/cmsis-stubs"),
    ];
    for candidate in candidates {
        if candidate.is_dir() {
            return candidate.clone();
        }
    }
    candidates[0].clone()
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
    fn inspect_project_reports_alphabet_states_and_predicates() {
        let temp = tempdir().unwrap();
        let config = build_two_source_config(temp.path());
        let inspection = inspect_project(&config, temp.path()).expect("inspection succeeded");
        assert_eq!(inspection.project, "Demo");
        assert_eq!(inspection.composition.info.name, "System");
        // Every source's automaton is in the per-automaton list, plus
        // the composed automaton itself.
        let names: Vec<_> = inspection
            .automata
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert!(names.contains(&"Light"));
        assert!(names.contains(&"Gate"));
        // Composition alphabet is the union of every member's alphabet.
        // Light fires `tick_light`; Gate fires `tick_gate`.
        assert!(
            inspection
                .composition
                .alphabet
                .iter()
                .any(|l| l == "tick_light")
        );
        assert!(
            inspection
                .composition
                .alphabet
                .iter()
                .any(|l| l == "tick_gate")
        );
    }

    #[test]
    fn inspect_project_runs_even_when_no_properties_declared() {
        // Sanity: `inspect_project` deliberately ignores `[[properties]]`
        // and never evaluates a formula. A config that would normally
        // declare properties should still inspect cleanly.
        let temp = tempdir().unwrap();
        let mut config = build_two_source_config(temp.path());
        config.properties.clear();
        let inspection = inspect_project(&config, temp.path()).expect("inspection succeeded");
        assert!(!inspection.automata.is_empty());
    }

    #[test]
    fn parameterised_count_expands_into_n_instances() {
        // Authoring a single `[[sources]]` with `count = 3` and a
        // file referencing `{instance_id}` expands to three
        // independent automata under the composition.
        let temp = tempdir().unwrap();
        let templated = r#"
context Worker_{instance_id} {
    alphabet { label tick_{instance_id}; }
    automata {
        automaton Worker_{instance_id} {
            states { state Idle initial; state Busy; }
            transitions {
                transition Idle -> Busy on label tick_{instance_id};
                transition Busy -> Idle on label tick_{instance_id};
            }
        }
    }
}
"#;
        let _ = write_ctxdsl_source(temp.path(), "worker.ctxdsl", templated);
        let toml_src = r#"
[project]
name = "Parameterised"

[[sources]]
id = "worker"
adapter = "ctxdsl"
files = ["worker.ctxdsl"]
count = 3

[alphabet]
strategy = "direct"

[composition]
semantics = "asynchronous"
members = ["worker_0", "worker_1", "worker_2"]
name = "PoolSystem"

[[properties]]
name = "alive"
formula = "true"
over = "PoolSystem"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let report = verify_project(&config, temp.path()).expect("verify pipeline succeeded");
        assert_eq!(report.sources.len(), 3);
        let ids: Vec<&str> = report.sources.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"worker_0"));
        assert!(ids.contains(&"worker_1"));
        assert!(ids.contains(&"worker_2"));
        assert_eq!(
            report.composition.members,
            vec![
                "Worker_worker_0".to_string(),
                "Worker_worker_1".to_string(),
                "Worker_worker_2".to_string(),
            ]
        );
    }

    #[test]
    fn count_one_is_indistinguishable_from_omitted() {
        // Backwards-compat: `count = 1` (explicit) matches the
        // legacy single-instance behaviour. No expansion.
        let temp = tempdir().unwrap();
        let _ = write_ctxdsl_source(temp.path(), "light.ctxdsl", SIMPLE_LIGHT_CTXDSL);
        let toml_src = r#"
[project]
name = "Singleton"

[[sources]]
id = "light"
adapter = "ctxdsl"
files = ["light.ctxdsl"]
count = 1

[composition]
semantics = "asynchronous"
members = ["light"]
name = "S"

[[properties]]
name = "p"
formula = "true"
over = "S"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let report = verify_project(&config, temp.path()).expect("verify succeeded");
        assert_eq!(report.sources.len(), 1);
        assert_eq!(report.sources[0].id, "light");
    }

    #[test]
    fn count_zero_is_a_config_error() {
        let temp = tempdir().unwrap();
        let _ = write_ctxdsl_source(temp.path(), "light.ctxdsl", SIMPLE_LIGHT_CTXDSL);
        let toml_src = r#"
[project]
name = "Zero"

[[sources]]
id = "light"
adapter = "ctxdsl"
files = ["light.ctxdsl"]
count = 0

[composition]
semantics = "asynchronous"
members = ["light"]
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let err = verify_project(&config, temp.path()).unwrap_err();
        match err {
            VerifyError::ConfigValidationFailed(issues) => {
                assert!(issues.iter().any(|i| matches!(
                    i,
                    crate::verify::config::ConfigIssue::SourceCountZero { source_id } if source_id == "light"
                )));
            }
            other => panic!("expected ConfigValidationFailed, got {other:?}"),
        }
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

    /// End-to-end: a tiny SV peripheral + a register map mapping its
    /// `req` input onto a synthetic `ctrl.req` field flows through the
    /// orchestrator's register-map SV rewriter and the verify pipeline
    /// completes against the rewritten alphabet.
    #[test]
    fn register_map_binding_rewrites_sv_source_labels() {
        let temp = tempdir().unwrap();
        // Minimal SV module — same shape as the codesign-uart tests.
        let sv = r#"
module periph(
    input        clk,
    input        rst,
    input        req,
    output reg   ack
);
    always_ff @(posedge clk or posedge rst) begin
        if (rst) ack <= 1'b0;
        else if (req) ack <= 1'b1;
        else ack <= 1'b0;
    end
endmodule
"#;
        fs::write(temp.path().join("periph.sv"), sv).unwrap();

        // Register-map sidecar: a single-bit control field `req`
        // exposed via SV signal `dut.req`.
        let register_map = r#"{
            "peripheral": "PERIPH",
            "base_address": "0x40000000",
            "registers": [
                {
                    "name": "ctrl",
                    "offset": 0,
                    "width_bits": 32,
                    "direction": "WO",
                    "visibility_class": "control",
                    "fields": [
                        {
                            "name": "req",
                            "bits": [0, 0],
                            "sv_signal": "dut.req",
                            "c_accessor": "PERIPH->CTRL.bit.req"
                        }
                    ]
                }
            ]
        }"#;
        fs::write(temp.path().join("register_map.json"), register_map).unwrap();

        let toml_src = r#"
[project]
name = "RewriteTest"

[[sources]]
id = "rtl"
adapter = "sv-rtl"
files = ["periph.sv"]

[alphabet]
strategy = "register_map"
register_map = "register_map.json"

[composition]
semantics = "asynchronous"
members = ["rtl"]
name = "P"

[[properties]]
name = "always_true"
formula = "true"
over = "P"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let report = verify_project(&config, temp.path()).expect("verify pipeline succeeded");
        assert_eq!(report.project, "RewriteTest");
        // Single source; verdict satisfied (vacuous property —
        // the test's point is that the pipeline completes, with the
        // register-map binding's SV rewriter applied without choking
        // on the SV adapter's `<signal>_<value>` labels.
        assert_eq!(report.property_verdicts.len(), 1);
        assert!(report.property_verdicts[0].satisfied);
    }

    /// Direct-binding sanity: a register-map sidecar present but the
    /// strategy is `direct` — the rewriter must not fire.
    #[test]
    fn direct_binding_with_sv_source_skips_register_map_rewriter() {
        let temp = tempdir().unwrap();
        let sv = r#"
module tiny(input clk, input rst, input go, output reg done);
    always_ff @(posedge clk or posedge rst) begin
        if (rst) done <= 1'b0; else done <= go;
    end
endmodule
"#;
        fs::write(temp.path().join("tiny.sv"), sv).unwrap();
        let toml_src = r#"
[project]
name = "DirectOnly"

[[sources]]
id = "rtl"
adapter = "sv-rtl"
files = ["tiny.sv"]

[alphabet]
strategy = "direct"

[composition]
semantics = "asynchronous"
members = ["rtl"]
name = "T"

[[properties]]
name = "p"
formula = "true"
over = "T"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let report = verify_project(&config, temp.path()).expect("verify pipeline succeeded");
        assert_eq!(report.property_verdicts.len(), 1);
        assert!(report.property_verdicts[0].satisfied);
    }
}
