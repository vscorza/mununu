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
    CompositionInfo, PropertyFormulaSource, PropertyVerdict, SourceSummary, TraceStep,
    TraceTermination, TraceWitness, VerifyError, VerifyReport,
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
    // table once — it's identical for every SV (`sv-rtl` / `sv-yosys`)
    // source under this binding. `None` when binding is Direct or
    // Renamings.
    let register_map_sv_renamings: Option<BTreeMap<String, String>> = match &binding {
        AlphabetBinding::RegisterMap { map, .. } => {
            Some(derive_sv_renamings_from_register_map(map))
        }
        _ => None,
    };

    // R4W-2 (R.4 clustered-COI wiring) — harvest each property's COI
    // seed atoms *before* adapter dispatch. The BTOR2 bit-blaster owns
    // the per-module dep graph, so the seeds must reach it through
    // `AdapterOptions::property_seeds` for the joint-vs-clustered cone
    // comparison to be computable. Properties are resolved again (for
    // real) in step 5; this early pass is best-effort telemetry and
    // never aborts the run.
    let property_seeds = harvest_property_seeds(config);

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
        let additional_files = read_additional_files(base_dir, source)?;

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
            let (raw_ctxdsl, partition_summary) = dispatch_adapter(
                &source.adapter,
                &instance_id,
                &path,
                &content,
                &additional_files,
                &source.options,
                base_dir,
                &property_seeds,
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
            if let Some(rm_renamings) = register_map_sv_renamings.as_ref()
                && is_sv_adapter(&source.adapter)
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
                partition_summary,
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
    // SV-side renaming table (identical for every `sv-rtl` / `sv-yosys`
    // source); `None` for Direct / Renamings bindings.
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
        let additional_files = read_additional_files(base_dir, source)?;
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
            // Inspection never resolves properties (step 5 is a no-op),
            // so there are no per-property COI seeds to harvest — pass
            // an empty slice. The bit-blaster then skips the clustered-
            // COI comparison (legacy intrinsic-seed-only behaviour).
            let (raw_ctxdsl, partition_summary) = dispatch_adapter(
                &source.adapter,
                &instance_id,
                &path,
                &content,
                &additional_files,
                &source.options,
                base_dir,
                &[],
            )?;
            let mut rewritten = match per_source_renamings.get(&source.id) {
                Some(renamings) if !renamings.is_empty() => {
                    apply_renamings_to_ctxdsl(&raw_ctxdsl, renamings)
                }
                _ => raw_ctxdsl,
            };
            if let Some(rm_renamings) = register_map_sv_renamings.as_ref()
                && is_sv_adapter(&source.adapter)
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
                partition_summary,
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
/// - `"sv-yosys"` — the **sole** SystemVerilog route (the native
///   `sv-rtl` parser path was removed in S.2b). KMTS SV via
///   [`crate::adapter::yosys::translate_sv`]
///   (sv2v→Yosys→BTOR2→bit-blast, carrying the MIG-1/MIG-2 soundness
///   fixes). The SV adapter has its own per-source sidecar conventions
///   (`.mununu.json` next to the `.sv` file). Requires `yosys` on PATH.
///   Multi-module composition opts in via `multi_module = true`
///   (+ optional `top`).
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
/// Phase A.3 step 3.6 — adapter dispatch returns both the CTXDSL text
/// **and** the partition summary the adapter populated on its
/// `AdapterOutput`. The orchestrator threads the summary onto the
/// source's `SourceSummary` so the `VerifyReport` surfaces COI
/// telemetry per source.
// R4W-2 added `property_seeds` as an 8th argument; the dispatch context
// is a flat list of independent inputs (adapter name, ids, paths,
// options) rather than a cohesive struct, so an allow is the right call
// here — matching the precedent on the other multi-input dispatch /
// realize helpers in this crate.
#[allow(clippy::too_many_arguments)]
fn dispatch_adapter(
    adapter: &str,
    source_id: &str,
    file_path: &Path,
    content: &str,
    additional_files: &[(PathBuf, String)],
    options: &std::collections::BTreeMap<String, toml::Value>,
    base_dir: &Path,
    // R4W-2 — manifest per-property COI seeds for the clustered-COI
    // telemetry. Consumed only by the `sv-yosys` (BTOR2) route, which
    // owns the dep graph; other adapters ignore it. Empty on the
    // inspection path (no property resolution there).
    property_seeds: &[(String, Vec<String>)],
) -> Result<(String, Option<crate::adapter::partition::PartitionSummary>), VerifyError> {
    let to_pair = |out: crate::adapter::AdapterOutput| (out.ctxdsl, out.partition_summary);
    let err_for = |adapter: &str, source_id: &str, err: crate::adapter::AdapterError| {
        VerifyError::AdapterTranslationFailed {
            source_id: source_id.to_string(),
            adapter: adapter.to_string(),
            message: err.to_string(),
        }
    };

    match adapter {
        "ctxdsl" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            Ok((content.to_string(), None))
        }
        "xstate" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            let opts = AdapterOptions::default();
            XStateAdapter::translate(content, &opts)
                .map(to_pair)
                .map_err(|err| err_for(adapter, source_id, err))
        }
        // The sole SystemVerilog route (S.2b removed the native `sv-rtl`
        // parser path). Runs the sv2v→Yosys-per-module→BTOR2→bit-blast
        // chain (`yosys::translate_sv`), carrying the MIG-1 (Ignored /
        // auto-COI) + MIG-2 (OOB-sink) soundness fixes. Single-module by
        // default; multi-module composition opts in via the source option
        // `multi_module = true` (+ optional `top`), driven from the top
        // netlist. Requires `yosys` on PATH; absence surfaces as an
        // `AdapterTranslationFailed` (locate_yosys error), not silently.
        "sv-yosys" => dispatch_sv_yosys(
            source_id,
            content,
            additional_files,
            options,
            property_seeds,
        ),
        "crewai" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            let opts = AdapterOptions::default();
            crate::adapter::crewai::CrewaiAdapter::translate(content, &opts)
                .map(to_pair)
                .map_err(|err| err_for(adapter, source_id, err))
        }
        "langgraph" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            let opts = AdapterOptions::default();
            crate::adapter::langgraph::LangGraphAdapter::translate(content, &opts)
                .map(to_pair)
                .map_err(|err| err_for(adapter, source_id, err))
        }
        "microcode" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            let opts = AdapterOptions::default();
            crate::adapter::microcode::MicrocodeAdapter::translate(content, &opts)
                .map(to_pair)
                .map_err(|err| err_for(adapter, source_id, err))
        }
        "extraction" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            let mode = options
                .get("mode")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let opts = AdapterOptions {
                mode,
                ..AdapterOptions::default()
            };
            crate::adapter::extraction::ExtractionAdapter::translate(content, &opts)
                .map(to_pair)
                .map_err(|err| err_for(adapter, source_id, err))
        }
        "c-codesign" => {
            warn_unused_additional_files(adapter, source_id, additional_files);
            dispatch_c_codesign(source_id, file_path, options, base_dir)
                .map(|ctxdsl| (ctxdsl, None))
        }
        other => Err(VerifyError::UnknownAdapter {
            source_id: source_id.to_string(),
            adapter: other.to_string(),
        }),
    }
}

/// Emit a `tracing::warn!` line per dropped additional file when a
/// single-file adapter is given more than one input. The verify
/// pipeline accepts the over-specified `files = [...]` list but only
/// the primary file reaches the adapter; this notice tells the user
/// the extras were dropped so they don't silently misinterpret the
/// model.
///
/// Today every adapter except `sv-yosys` is single-file; the framework
/// only honours multi-file on the `sv-yosys` multi-module path
/// (`multi_module = true`), where the additional files are the
/// submodule sources the top instantiates.
fn warn_unused_additional_files(
    adapter: &str,
    source_id: &str,
    additional_files: &[(PathBuf, String)],
) {
    if additional_files.is_empty() {
        return;
    }
    let names: Vec<String> = additional_files
        .iter()
        .map(|(p, _)| p.display().to_string())
        .collect();
    tracing::warn!(
        adapter,
        source_id,
        dropped = names.join(", ").as_str(),
        "verify: source's `files = [...]` listed {} additional file(s); adapter `{}` only consumes one — extras dropped: {}",
        additional_files.len(),
        adapter,
        names.join(", "),
    );
}

/// The SV peripheral adapter (`sv-yosys`) that consumes the
/// register-map SV-side renaming table. The KMTS route emits
/// `<signal>_<value>` labels (`adapter::btor2::bit_blast`), exactly the
/// shape the renaming derivation in
/// [`crate::verify::register_map_rewriter`] targets, so the firmware↔RTL
/// rendezvous reconciliation applies to it. (The native `sv-rtl` route
/// that previously also matched here was removed in S.2b.)
fn is_sv_adapter(adapter: &str) -> bool {
    adapter == "sv-yosys"
}

/// MIG-4 (S-track migration) — dispatch the `sv-yosys` KMTS route.
/// Runs the SV → (sv2v) → Yosys → BTOR2 → bit-blast chain via
/// [`crate::adapter::yosys::translate_sv`], which produces CTXDSL the
/// verify assembler ingests exactly like the native `sv-rtl` path's
/// output. The bit-blast lift carries the MIG-1 (Ignored / auto-COI)
/// and MIG-2 (OOB-sink over-approximation) soundness fixes, so this
/// route is a sound SV abstraction. `additional_files` are threaded as
/// Yosys additional sources (by basename) so multi-file designs
/// elaborate. Requires `yosys` on PATH.
///
/// R-MM-5b — multi-module composition route. Opt in with the source
/// option `multi_module = true` (optional `top = "<module>"`; the top is
/// auto-detected from the netlist when omitted). The driver lifts each
/// submodule to a KMTS, renames each instance's ports to the connected
/// nets, and synchronously fold-composes the instances
/// ([`crate::adapter::yosys::multi_module::compose_sv_multi_module`]); the
/// composed product is serialised back to CTXDSL
/// ([`crate::adapter::yosys::multi_module::clts_to_ctxdsl`], automaton
/// `Circuit` to match the single-module path's name so a property's `over`
/// binds identically) and re-enters the standard parse→realise→evaluate
/// pipeline. Composed valuations are all-numeric (R-MM-5b-i) so property
/// atoms like `<inst>__<sig> == k` bind to actual values. Properties come
/// from the verify manifest's `[[properties]]` (over the composed
/// automaton), so no property block is injected into the emitted CTXDSL.
fn dispatch_sv_yosys(
    source_id: &str,
    content: &str,
    additional_files: &[(PathBuf, String)],
    options: &std::collections::BTreeMap<String, toml::Value>,
    property_seeds: &[(String, Vec<String>)],
) -> Result<(String, Option<crate::adapter::partition::PartitionSummary>), VerifyError> {
    // R4W-2 — carry the manifest's per-property COI seeds into the
    // bit-blaster so it can compute the joint-vs-clustered cone
    // comparison over its dep graph (surfaced on
    // `PartitionSummary::cluster_coi`). Empty seeds preserve the legacy
    // intrinsic-seed-only behaviour.
    let opts = AdapterOptions {
        property_seeds: property_seeds.to_vec(),
        ..AdapterOptions::default()
    };
    let additional_sources: Vec<(String, String)> = additional_files
        .iter()
        .filter_map(|(path, body)| {
            path.file_name()
                .and_then(|s| s.to_str())
                .map(|name| (name.to_string(), body.clone()))
        })
        .collect();

    let is_multi_module = options
        .get("multi_module")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_multi_module {
        let top = options
            .get("top")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let yopts = crate::adapter::yosys::YosysOptions {
            top,
            per_module_btor: true,
            additional_sources,
            ..Default::default()
        };
        let comp =
            crate::adapter::yosys::multi_module::compose_sv_multi_module(content, &opts, &yopts)
                .map_err(|err| VerifyError::AdapterTranslationFailed {
                    source_id: source_id.to_string(),
                    adapter: "sv-yosys".to_string(),
                    message: err.to_string(),
                })?;
        let ctxdsl = crate::adapter::yosys::multi_module::clts_to_ctxdsl(
            &comp.composed,
            "Circuit",
            source_id,
        )
        .map_err(|err| VerifyError::AdapterTranslationFailed {
            source_id: source_id.to_string(),
            adapter: "sv-yosys".to_string(),
            message: err.to_string(),
        })?;
        return Ok((ctxdsl, None));
    }

    let yopts = crate::adapter::yosys::YosysOptions {
        additional_sources,
        ..Default::default()
    };
    crate::adapter::yosys::translate_sv(content, &opts, &yopts)
        .map(|out| (out.ctxdsl, out.partition_summary))
        .map_err(|err| VerifyError::AdapterTranslationFailed {
            source_id: source_id.to_string(),
            adapter: "sv-yosys".to_string(),
            message: err.to_string(),
        })
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

/// R4W-2 (R.4 clustered-COI wiring) — resolve + parse each manifest
/// property and collect its COI seed atoms as
/// `(property_name, seed_atom_names)`.
///
/// The seeds feed [`crate::adapter::partition::coi::cluster_coi_report`]
/// in the BTOR2 bit-blaster (via [`AdapterOptions::property_seeds`]),
/// which owns the per-module dep graph the cones are walked over.
///
/// Best-effort telemetry: a property that fails to resolve (bad
/// template) or fails to parse (malformed formula) contributes no
/// seeds — its real error surfaces in step 5 / the eval phase, so this
/// pass never aborts the verify run. Returns an empty vec when the
/// manifest declares no properties.
fn harvest_property_seeds(config: &VerifyConfig) -> Vec<(String, Vec<String>)> {
    let registry = TemplateRegistry::builtin();
    config
        .properties
        .iter()
        .filter_map(|p| {
            let (formula_text, _src) = resolve_property_formula(p, &registry).ok()?;
            let formula = crate::mu_calculus::parser::parse(&formula_text).ok()?;
            let atoms = crate::adapter::partition::coi::property_seed_atoms(&formula);
            Some((p.name.clone(), atoms.into_iter().collect()))
        })
        .collect()
}

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

    let counterexample = if !satisfied {
        build_counterexample_witness(clts, &result, TRACE_WITNESS_STEP_CAP)
    } else {
        None
    };

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
        counterexample,
    })
}

/// Maximum number of steps recorded by [`build_counterexample_witness`].
/// 20 steps is enough to surface the typical violation in shipped
/// fixtures (the chaotic codesign trace from `Idle` to `Sending` is
/// ~6 steps in the worst case) without producing a wall of state
/// names in the verify report.
const TRACE_WITNESS_STEP_CAP: usize = 20;

/// Construct a forward-walk witness from a violating initial state.
///
/// Picks the first initial state that does not satisfy `result`,
/// then walks outgoing transitions for up to `max_steps`, preferring
/// successors that also violate the property. Falls back to any
/// unvisited successor when no violating successor is available.
///
/// Returns `None` when:
/// - every initial state satisfies the property (caller should not
///   invoke this), or
/// - the composition has no initial states.
fn build_counterexample_witness<S, L>(
    clts: &crate::clts::Clts<S, L>,
    satisfaction: &bitvec::vec::BitVec<usize, bitvec::order::Lsb0>,
    max_steps: usize,
) -> Option<TraceWitness>
where
    S: IdStorage,
    L: IdStorage,
{
    use std::collections::HashSet;

    let violating_initial = clts
        .initial_states()
        .iter()
        .copied()
        .find(|sid| !satisfaction.get(sid.index()).map(|b| *b).unwrap_or(false))?;

    let initial_state = clts.state_name(violating_initial)?.to_string();

    let mut steps: Vec<TraceStep> = Vec::new();
    let mut visited: HashSet<usize> = HashSet::new();
    visited.insert(violating_initial.index());
    let mut current = violating_initial;
    let mut termination = TraceTermination::Sink;
    let mut path_states_by_index: Vec<usize> = vec![violating_initial.index()];

    for _ in 0..max_steps {
        let outgoing = clts.outgoing(current);
        if outgoing.is_empty() {
            termination = TraceTermination::Sink;
            break;
        }

        // Prefer an unvisited violating successor.
        let mut pick = outgoing.iter().find(|t| {
            let idx = t.target().index();
            !visited.contains(&idx) && !satisfaction.get(idx).map(|b| *b).unwrap_or(true)
        });
        // Fallback to any unvisited successor.
        if pick.is_none() {
            pick = outgoing
                .iter()
                .find(|t| !visited.contains(&t.target().index()));
        }

        let Some(transition) = pick else {
            // Every successor visited — close the cycle on the first
            // outgoing edge.
            let first = &outgoing[0];
            let succ_idx = first.target().index();
            let succ_name = clts
                .state_name(first.target())
                .map(str::to_string)
                .unwrap_or_else(|| format!("state_{succ_idx}"));
            let label = format_transition_label(clts, first);
            steps.push(TraceStep {
                label,
                successor_state: succ_name,
            });
            let return_to_step = path_states_by_index
                .iter()
                .position(|&i| i == succ_idx)
                .unwrap_or(0);
            termination = TraceTermination::Cycle { return_to_step };
            break;
        };

        let succ = transition.target();
        let succ_idx = succ.index();
        let succ_name = clts
            .state_name(succ)
            .map(str::to_string)
            .unwrap_or_else(|| format!("state_{succ_idx}"));
        let label = format_transition_label(clts, transition);
        steps.push(TraceStep {
            label,
            successor_state: succ_name,
        });
        visited.insert(succ_idx);
        path_states_by_index.push(succ_idx);
        current = succ;
    }

    if steps.len() == max_steps && !matches!(termination, TraceTermination::Cycle { .. }) {
        termination = TraceTermination::LengthLimit;
    }

    Some(TraceWitness {
        initial_state,
        steps,
        termination,
    })
}

/// Render one multi-label transition's label payload as a
/// comma-joined string (`label_a,label_b`). Falls back to `"?"` when
/// the label store cannot resolve the ID, which only happens if the
/// CLTS is malformed.
fn format_transition_label<S, L>(
    clts: &crate::clts::Clts<S, L>,
    transition: &crate::clts::Transition<S, L>,
) -> String
where
    S: IdStorage,
    L: IdStorage,
{
    let mut parts: Vec<String> = Vec::new();
    for lid in transition.labels() {
        if let Some(payload) = clts.label_payload(*lid) {
            for sym in payload {
                parts.push(sym.clone());
            }
        }
    }
    if parts.is_empty() {
        "?".to_string()
    } else {
        parts.join(",")
    }
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

/// Read every entry in `source.files[1..]` and return `(path,
/// content)` pairs ready to hand to [`dispatch_adapter`]. The primary
/// file at `files[0]` is read separately by the orchestrator. Errors
/// (missing / unreadable files) propagate as `SourceReadFailed`.
fn read_additional_files(
    base_dir: &Path,
    source: &crate::verify::config::SourceSection,
) -> Result<Vec<(PathBuf, String)>, VerifyError> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    for f in source.files.iter().skip(1) {
        let path = resolve_path(base_dir, f);
        let content =
            std::fs::read_to_string(&path).map_err(|err| VerifyError::SourceReadFailed {
                path: path.clone(),
                source: err,
            })?;
        out.push((path, content));
    }
    Ok(out)
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
    fn additional_files_on_single_file_adapter_warn_but_succeed() {
        // The xstate adapter only consumes one file. If the user
        // over-specifies `files = ["a.xstate.json", "extra.xstate.json"]`,
        // the orchestrator reads both, dispatches only the primary,
        // and the warn helper logs the dropped extras. The verdict
        // should still match the primary-file-only result.
        let temp = tempdir().unwrap();
        let xstate = r#"{ "id": "primary", "initial": "s0", "states": { "s0": {} } }"#;
        let extra = r#"{ "id": "extra", "initial": "x0", "states": { "x0": {} } }"#;
        let _ = write_ctxdsl_source(temp.path(), "primary.xstate.json", xstate);
        let _ = write_ctxdsl_source(temp.path(), "extra.xstate.json", extra);
        let toml_src = r#"
[project]
name = "MultiFileWarn"

[[sources]]
id = "x"
adapter = "xstate"
files = ["primary.xstate.json", "extra.xstate.json"]

[composition]
semantics = "asynchronous"
members = ["x"]
name = "S"

[[properties]]
name = "p"
formula = "true"
over = "S"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let report = verify_project(&config, temp.path()).expect("multi-file dispatch succeeds");
        assert_eq!(report.sources.len(), 1);
        // The XState adapter consumed only the primary file — the
        // automaton in the composition came from `primary.xstate.json`.
        assert_eq!(report.sources[0].id, "x");
        assert!(report.property_verdicts[0].satisfied);
    }

    #[test]
    fn missing_additional_file_surfaces_as_source_read_failure() {
        // If the additional file doesn't exist on disk, the
        // orchestrator surfaces the same `SourceReadFailed` error as
        // a missing primary file — fail-fast, not silent.
        let temp = tempdir().unwrap();
        let _ = write_ctxdsl_source(temp.path(), "light.ctxdsl", SIMPLE_LIGHT_CTXDSL);
        let toml_src = r#"
[project]
name = "MissingExtra"

[[sources]]
id = "light"
adapter = "ctxdsl"
files = ["light.ctxdsl", "missing.ctxdsl"]

[composition]
semantics = "asynchronous"
members = ["light"]
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let err = verify_project(&config, temp.path()).unwrap_err();
        match err {
            VerifyError::SourceReadFailed { path, .. } => {
                assert!(path.ends_with("missing.ctxdsl"));
            }
            other => panic!("expected SourceReadFailed, got {other:?}"),
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

    /// MIG-4 (S-track migration) — the `sv-yosys` KMTS route is a
    /// recognized adapter. CI-safe: the route calls `yosys`, but if
    /// yosys is absent the dispatch returns `AdapterTranslationFailed`
    /// (a locate_yosys error), NOT `UnknownAdapter` — so this holds
    /// regardless of whether yosys is installed. (When yosys IS present
    /// the route runs the full SV→BTOR2→bit-blast chain.)
    #[test]
    fn sv_yosys_adapter_route_is_recognized() {
        let result = dispatch_adapter(
            "sv-yosys",
            "src0",
            std::path::Path::new("work.sv"),
            "module m(input logic clk_i); endmodule",
            &[],
            &std::collections::BTreeMap::new(),
            std::path::Path::new("."),
            &[],
        );
        // Ok (yosys present) OR AdapterTranslationFailed (yosys absent)
        // both prove the route is wired; only UnknownAdapter fails.
        assert!(
            !matches!(result, Err(VerifyError::UnknownAdapter { .. })),
            "sv-yosys must be a recognized adapter route, not UnknownAdapter"
        );
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

    /// The register-map SV-side rewriter gate ([`is_sv_adapter`]) admits
    /// the KMTS route (`sv-yosys`) — the sole SystemVerilog route after
    /// the S.2b native-parser excision. The rewriter targets the
    /// `<signal>_<value>` labels the KMTS bit-blaster emits, so the
    /// firmware↔RTL rendezvous reconciliation applies. (The native
    /// `sv-rtl` route that previously also matched here was removed.)
    /// The renaming derivation itself is unit-tested in
    /// `crate::verify::register_map_rewriter`; an end-to-end
    /// SV+register_map rewrite is a yosys-gated integration-test
    /// follow-up.
    #[test]
    fn is_sv_adapter_matches_only_the_kmts_route() {
        assert!(is_sv_adapter("sv-yosys"));
        assert!(!is_sv_adapter("sv-rtl"));
        assert!(!is_sv_adapter("ctxdsl"));
        assert!(!is_sv_adapter("c-codesign"));
        assert!(!is_sv_adapter("xstate"));
    }

    #[test]
    fn violated_property_emits_counterexample_witness() {
        // Pin a property to `false` so every initial state violates;
        // expect the orchestrator to attach a TraceWitness rooted at
        // a violating initial state.
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
members = ["light"]
name = "Sys"

[[properties]]
name = "impossible"
formula = "false"
over = "Sys"
"#;
        let config = VerifyConfig::from_toml(toml_src).unwrap();
        let report = verify_project(&config, temp.path()).expect("pipeline runs");
        assert_eq!(report.property_verdicts.len(), 1);
        let v = &report.property_verdicts[0];
        assert!(!v.satisfied, "expected VIOLATED");
        let witness = v
            .counterexample
            .as_ref()
            .expect("violated verdicts carry a counterexample");
        assert!(
            !witness.initial_state.is_empty(),
            "initial_state should name a violating initial"
        );
        // The walk should produce at least one step or terminate at a sink.
        match &witness.termination {
            TraceTermination::Sink
            | TraceTermination::Cycle { .. }
            | TraceTermination::LengthLimit => {}
        }
    }

    #[test]
    fn satisfied_property_does_not_emit_counterexample() {
        let temp = tempdir().unwrap();
        let config = build_two_source_config(temp.path());
        let report = verify_project(&config, temp.path()).expect("pipeline runs");
        for v in &report.property_verdicts {
            assert!(v.satisfied);
            assert!(
                v.counterexample.is_none(),
                "satisfied verdicts should not carry a counterexample"
            );
        }
    }
}
