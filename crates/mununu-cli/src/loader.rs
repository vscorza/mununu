//! Loader layer — adapter dispatch, document loading, and realization.
//!
//! This module owns the path from a raw file on disk to a [`RealizedContext`]
//! ready for formula evaluation or synthesis.  Every subcommand handler in
//! `main.rs` reaches into this module via `crate::loader::*`.

use mununu_core::context_dsl::{
    ContextDoc, RealizedContext, parse as parse_context_doc, realize_context,
};
use std::fs;
use std::fs::File;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};

/// Side-channel state-valuation map produced by adapters that enumerate a
/// cross product of state variables (SV Kripke, extraction).  The caller must
/// inject this into the parsed `ContextDoc` so the realizer can resolve
/// field-based predicates like `boot_fsm_ps_BOOT_IDLE` over composite state
/// names like `boot_fsm_ns_BOOT_IDLE_boot_fsm_ps_BOOT_IDLE`.
pub(crate) type StateValuationsMap = std::collections::HashMap<
    String,
    std::collections::HashMap<String, std::collections::BTreeMap<String, String>>,
>;

/// Read and parse a CTXDSL context file from disk.
pub(crate) fn parse_context_file(path: &Path) -> Result<ContextDoc, String> {
    let source = fs::read_to_string(path)
        .map_err(|err| format!("failed to read '{}': {err}", path.display()))?;
    parse_context_doc(&source).map_err(|err| format!("failed to parse '{}': {err}", path.display()))
}

/// Log adapter warnings and translation summary to stderr, then return the
/// CTXDSL text and the structured state valuations.  Shared by every adapter
/// arm in [`load_with_adapter_mode`]. The state valuations are a side-channel
/// from cross-product enumeration (SV Kripke, extraction); the caller must
/// inject them into the parsed `ContextDoc` so the realizer can resolve
/// field-based predicates like `boot_fsm_ps_BOOT_IDLE` over composite state
/// names like `boot_fsm_ns_BOOT_IDLE_boot_fsm_ps_BOOT_IDLE`.
fn log_adapter_output(output: mununu_core::adapter::AdapterOutput) -> (String, StateValuationsMap) {
    log_adapter_output_with_dir(output, None)
}

/// Like [`log_adapter_output`] but, when `sidecar_dir` is `Some`, writes
/// every `AdapterSidecar` the adapter attached (e.g. the yosys frontend's
/// auto-emitted `BlackBoxInterface.json` + `GapMarkerReport.json` files,
/// per Document B task B3) into that directory. Designed so callers that
/// know where the source file lives can park the sidecars right next to
/// it without the CLI inventing an output structure.
fn log_adapter_output_with_dir(
    output: mununu_core::adapter::AdapterOutput,
    sidecar_dir: Option<&std::path::Path>,
) -> (String, StateValuationsMap) {
    for w in &output.warnings {
        eprintln!("adapter warning: {}", w.message);
    }
    eprintln!(
        "Translated {}: {} signals, {} states, {} properties",
        output.source_info.format,
        output.source_info.signal_count,
        output.source_info.state_count,
        output.source_info.property_count,
    );
    if let Some(dir) = sidecar_dir {
        for sidecar in &output.sidecars {
            let target = dir.join(&sidecar.filename);
            match std::fs::write(&target, &sidecar.content) {
                Ok(()) => eprintln!("auto-emitted sidecar: {}", target.display()),
                Err(e) => eprintln!("warning: failed to write sidecar {}: {e}", target.display()),
            }
        }
    } else if !output.sidecars.is_empty() {
        eprintln!(
            "adapter produced {} sidecar(s); no source directory known so they were not written. \
             Call this via a flow that knows the source path (e.g. `context eval <file> --adapter yosys`) to enable auto-write.",
            output.sidecars.len(),
        );
    }
    (output.ctxdsl, output.state_valuations)
}

/// Read a source file, optionally translating it from an external format first.
/// Returns `(ContextDoc, Option<ctxdsl_text>)` — the CTXDSL text is `Some` when
/// an adapter was used (useful for `--print-ctxdsl`).
pub(crate) fn load_with_adapter_mode(
    path: &Path,
    adapter: Option<&str>,
    mode: Option<&str>,
) -> Result<(ContextDoc, Option<String>), String> {
    load_with_adapter_mode_extra(path, adapter, mode, &[], None)
}

/// Optional source-language preprocessor for SV-shaped adapters.
/// Currently only `sv2v` is recognised (returns `Some("sv2v")` →
/// caller sets `YosysOptions::use_sv2v`). Anything else returns a
/// user-visible error so a typo doesn't silently disable the
/// requested preprocessor.
pub(crate) fn validate_preprocessor(name: Option<&str>) -> Result<Option<&str>, String> {
    match name {
        None => Ok(None),
        Some("sv2v") => Ok(Some("sv2v")),
        Some(other) => Err(format!("unknown preprocessor '{other}'. Supported: sv2v")),
    }
}

/// Like [`load_with_adapter_mode`] but accepts a slice of additional
/// source files and an optional preprocessor name. For the `sv-yosys`
/// adapter the additional sources are forwarded to
/// `YosysOptions::additional_sources` so sv2v / Yosys can resolve
/// cross-file packages, interfaces, and `\`include`-style directives;
/// the preprocessor name (`Some("sv2v")`) toggles
/// `YosysOptions::use_sv2v`. Ignored by all other adapters.
pub(crate) fn load_with_adapter_mode_extra(
    path: &Path,
    adapter: Option<&str>,
    mode: Option<&str>,
    additional_sv_paths: &[PathBuf],
    preprocessor: Option<&str>,
) -> Result<(ContextDoc, Option<String>), String> {
    use mununu_core::adapter::{AdapterOptions, FormatAdapter};

    let source = fs::read_to_string(path)
        .map_err(|err| format!("failed to read '{}': {err}", path.display()))?;

    let options_default = AdapterOptions::default();
    let options_with_mode = AdapterOptions {
        mode: mode.map(|s| s.to_string()),
        ..Default::default()
    };

    // BTOR2 / sv-yosys auto-load `.mununu.json` next to the source so
    // the bit-blaster's `FieldDomain` abstraction kicks in. SV adapter
    // already does this via filesystem convention; this mirrors it.
    let load_btor_sidecar = |opts: &AdapterOptions| -> AdapterOptions {
        let mut o = opts.clone();
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let candidate = path.with_file_name(format!("{stem}.mununu.json"));
            if candidate.exists()
                && let Ok(content) = fs::read_to_string(&candidate)
            {
                eprintln!("Loaded sidecar: {}", candidate.display());
                o.sidecar_json = Some(content);
            }
        }
        o
    };

    let (ctxdsl_source, state_valuations) = match adapter {
        Some("tlsf") => log_adapter_output(
            mununu_core::adapter::tlsf::TlsfAdapter::translate(&source, &options_default)
                .map_err(|e| format!("TLSF adapter error: {e}"))?,
        ),
        Some("aiger") => log_adapter_output(
            mununu_core::adapter::aiger::AigerAdapter::translate(&source, &options_default)
                .map_err(|e| format!("AIGER adapter error: {e}"))?,
        ),
        Some("btor2") | Some("btor") => log_adapter_output(
            mununu_core::adapter::btor2::Btor2Adapter::translate(
                &source,
                &load_btor_sidecar(&options_default),
            )
            .map_err(|e| format!("BTOR2 adapter error: {e}"))?,
        ),
        Some("sv-yosys") | Some("yosys") => {
            // Yosys-driven SV elaboration → BTOR2 → CLTS.
            // Per Phase 1 of the RTL roadmap (S1: Yosys-as-front-end).
            // Sidecars emitted for `(* blackbox *)` modules (Document B
            // task B3) land in the source file's parent directory so the
            // user finds them next to the `.sv` they just ran.
            //
            // Multi-file: `additional_sv_paths` (passed in from the CLI
            // sidecar flag) become extra `.sv` sources for Yosys / sv2v
            // to resolve cross-file packages and imports. Each entry is
            // read off disk and shipped as a (filename, content) pair.
            let mut additional: Vec<(String, String)> =
                Vec::with_capacity(additional_sv_paths.len());
            for extra in additional_sv_paths {
                let content = fs::read_to_string(extra).map_err(|err| {
                    format!(
                        "failed to read additional SV source '{}': {err}",
                        extra.display()
                    )
                })?;
                let name = extra
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| {
                        format!(
                            "additional SV source '{}' has no usable filename",
                            extra.display()
                        )
                    })?
                    .to_string();
                additional.push((name, content));
            }
            let use_sv2v = matches!(preprocessor, Some("sv2v"));
            // `MUNUNU_YOSYS_SETUNDEF_ANYSEQ=1` opt-in for CWE-1245-class
            // bug-preserving extraction. See the SOUNDNESS comment in
            // `adapter::yosys::build_script`. CLI-level flag is
            // deferred — env-var lets the Caliptra PoF fixture's
            // `validate.sh` opt in without touching every adapter
            // call-site.
            let setundef_anyseq = std::env::var("MUNUNU_YOSYS_SETUNDEF_ANYSEQ")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            // R-Y1 (§Phase 8) — `MUNUNU_YOSYS_SETUNDEF_ANYCONST=1`
            // opt-in for the intermediate init policy: one nondeterministic
            // constant input per undef bit (no per-cycle state cells).
            // Strictly between -zero (masks bugs) and -anyseq (state
            // explosion). Precedence: ANYSEQ wins if both are set.
            let setundef_anyconst = std::env::var("MUNUNU_YOSYS_SETUNDEF_ANYCONST")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            // R-Y2 (§Phase 8 §8.1) — load the SV sidecar if it exists
            // and extract per-signal init-policy overrides. The
            // overrides are surgical (per-signal anyconst on declared
            // signals only, keeping every other undef at zero), which
            // is the load-bearing Caliptra unblock per §Phase 8 §8.2.
            // Silently skips if no sidecar exists or the schema does
            // not parse — non-blocking for fixtures that pre-date R-Y2.
            let init_policy_overrides: mununu_core::adapter::yosys::InitPolicyOverrides = {
                let mut overrides = Vec::new();
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let candidate = path.with_file_name(format!("{stem}.mununu.json"));
                    if candidate.exists()
                        && let Ok(content) = fs::read_to_string(&candidate)
                        && let Ok(ann) = serde_json::from_str::<
                            mununu_core::adapter::systemverilog::annotation::SvAnnotation,
                        >(&content)
                    {
                        overrides = ann.init_policy_overrides();
                        if !overrides.is_empty() {
                            eprintln!(
                                "R-Y2: applying per-signal init-policy overrides from sidecar: {}",
                                overrides
                                    .iter()
                                    .map(|(n, p)| format!("{n}={p:?}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                    }
                }
                overrides
            };
            let yopts = mununu_core::adapter::yosys::YosysOptions {
                primary_source_path: Some(path.to_string_lossy().into_owned()),
                additional_sources: additional,
                use_sv2v,
                setundef_anyseq,
                setundef_anyconst,
                init_policy_overrides,
                ..Default::default()
            };
            log_adapter_output_with_dir(
                mununu_core::adapter::yosys::translate_sv(
                    &source,
                    &load_btor_sidecar(&options_default),
                    &yopts,
                )
                .map_err(|e| format!("Yosys SV adapter error: {e}"))?,
                path.parent(),
            )
        }
        Some("promela") => log_adapter_output(
            mununu_core::adapter::promela::PromelaAdapter::translate(&source, &options_default)
                .map_err(|e| format!("Promela adapter error: {e}"))?,
        ),
        Some("systemverilog") | Some("sv") => log_adapter_output(
            mununu_core::adapter::systemverilog::SystemVerilogAdapter::translate_with_path(
                &source,
                &options_default,
                path,
            )
            .map_err(|e| format!("SystemVerilog adapter error: {e}"))?,
        ),
        Some("sv-multi") => {
            // Custom-SV multi-module path. Input is the sidecar JSON;
            // source files are looked up relative to the sidecar's
            // directory. Auto-emits black-box sidecars per Document B
            // task B3 custom-SV half.
            use std::collections::HashMap;
            let sidecar_dir = path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            // Pre-load every source file the sidecar references so the
            // multi-module path's get_source closure resolves them
            // off disk.
            let ann: mununu_core::adapter::systemverilog::annotation::MultiModuleSvAnnotation =
                serde_json::from_str(&source)
                    .map_err(|e| format!("sv-multi: failed to parse sidecar JSON: {e}"))?;
            let mut sources: HashMap<String, String> = HashMap::new();
            let mut all_refs: Vec<String> = ann
                .modules
                .iter()
                .map(|m| m.source.clone())
                .chain(ann.blackbox_modules.iter().map(|m| m.source.clone()))
                .collect();
            all_refs.sort();
            all_refs.dedup();
            for src_path in &all_refs {
                let full = sidecar_dir.join(src_path);
                let content = std::fs::read_to_string(&full)
                    .map_err(|e| format!("sv-multi: failed to read '{}': {e}", full.display()))?;
                sources.insert(src_path.clone(), content);
            }
            log_adapter_output_with_dir(
                mununu_core::adapter::systemverilog::SystemVerilogAdapter::translate_multi_module_content(
                    &source,
                    &sources,
                    &options_default,
                )
                .map_err(|e| format!("sv-multi adapter error: {e}"))?,
                Some(&sidecar_dir),
            )
        }
        Some("xstate") => log_adapter_output(
            mununu_core::adapter::xstate::XStateAdapter::translate(&source, &options_default)
                .map_err(|e| format!("XState adapter error: {e}"))?,
        ),
        Some("extraction") => log_adapter_output(
            mununu_core::adapter::extraction::ExtractionAdapter::translate(
                &source,
                &options_with_mode,
            )
            .map_err(|e| format!("Extraction adapter error: {e}"))?,
        ),
        Some("auto") => log_adapter_output(
            mununu_core::adapter::auto_translate(&source, &options_with_mode)
                .map_err(|e| format!("adapter error: {e}"))?,
        ),
        Some(fmt) => {
            return Err(format!(
                "unknown adapter format '{fmt}'. Supported: tlsf, aiger, btor2, promela, xstate, systemverilog, sv-yosys, extraction, auto"
            ));
        }
        None => {
            // Auto-detect by file extension if no adapter specified
            if let Some(fmt) = mununu_core::adapter::detect_format_by_extension(path) {
                eprintln!("Auto-detected format '{}' from extension", fmt);
                return load_with_adapter_mode_extra(
                    path,
                    Some(fmt),
                    mode,
                    additional_sv_paths,
                    preprocessor,
                );
            }
            (source, std::collections::HashMap::new())
        }
    };

    let was_adapter = adapter.is_some();
    let mut doc = parse_context_doc(&ctxdsl_source)
        .map_err(|err| format!("failed to parse '{}': {err}", path.display()))?;
    // Inject the side-channel state valuations from the adapter so the
    // realizer can resolve field-based predicates over composite state names.
    doc.state_valuations = state_valuations;
    Ok((
        doc,
        if was_adapter {
            Some(ctxdsl_source)
        } else {
            None
        },
    ))
}

pub(crate) fn load_context_documents(
    context_path: &Path,
    sidecar_paths: &[PathBuf],
    adapter: Option<&str>,
) -> Result<(ContextDoc, Vec<ContextDoc>, Option<String>), String> {
    load_context_documents_mode(context_path, sidecar_paths, adapter, None, None)
}

pub(crate) fn load_context_documents_mode(
    context_path: &Path,
    sidecar_paths: &[PathBuf],
    adapter: Option<&str>,
    mode: Option<&str>,
    preprocessor: Option<&str>,
) -> Result<(ContextDoc, Vec<ContextDoc>, Option<String>), String> {
    // Sidecar argument routing.
    //
    // - `.sv` / `.svh`: additional sources for the sv-yosys adapter
    //   (multi-file SV elaboration).
    // - `.mununu.json` (the abstraction sidecar): a no-op at the CLI
    //   level. The adapter auto-loads the file from the primary
    //   source's directory via path adjacency (see
    //   [`load_btor_sidecar`]); the user-passed path is informational
    //   and must not be parsed as a CTXDSL document.
    // - everything else: a CTXDSL sidecar document.
    let mut sv_sources: Vec<PathBuf> = Vec::new();
    let mut ctxdsl_sidecars: Vec<PathBuf> = Vec::new();
    let mut ignored_mununu_json: Vec<PathBuf> = Vec::new();
    for p in sidecar_paths.iter().cloned() {
        let ext = p.extension().and_then(|e| e.to_str());
        if matches!(ext, Some("sv") | Some("svh")) {
            sv_sources.push(p);
        } else if p
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".mununu.json"))
        {
            ignored_mununu_json.push(p);
        } else {
            ctxdsl_sidecars.push(p);
        }
    }
    for p in &ignored_mununu_json {
        eprintln!(
            "note: --sidecar {} is auto-loaded by the adapter via path adjacency; \
             the explicit flag is informational only",
            p.display()
        );
    }
    let (context_doc, ctxdsl_text) =
        load_with_adapter_mode_extra(context_path, adapter, mode, &sv_sources, preprocessor)?;
    let mut sidecar_docs = Vec::with_capacity(ctxdsl_sidecars.len());
    for path in &ctxdsl_sidecars {
        sidecar_docs.push(parse_context_file(path)?);
    }
    Ok((context_doc, sidecar_docs, ctxdsl_text))
}

pub(crate) fn realize_documents(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
) -> Result<RealizedContext, String> {
    realize_context(context_doc, sidecar_docs).map_err(|err| {
        format!(
            "failed to realise context '{}': {err}",
            context_doc.name.name
        )
    })
}

/// Prints the internal structure of a context to stdout or a file.
pub(crate) fn print_context_structure(
    context: &mununu_core::context::Context,
    output_path: Option<PathBuf>,
) -> Result<(), String> {
    let path_ref = output_path.as_ref();
    let mut writer: Box<dyn IoWrite> =
        if let Some(path) = path_ref {
            Box::new(File::create(path).map_err(|err| {
                format!("failed to create output file '{}': {err}", path.display())
            })?)
        } else {
            Box::new(io::stdout())
        };

    context
        .print_structure(&mut writer)
        .map_err(|err| format!("failed to write context structure: {err}"))?;

    if let Some(path) = path_ref {
        println!("Context structure written to {}", path.display());
    }

    Ok(())
}

/// Print the intermediate CTXDSL text (after adapter translation) to stdout or a file.
pub(crate) fn print_ctxdsl_output(
    ctxdsl: &str,
    output_path: Option<&PathBuf>,
) -> Result<(), String> {
    let mut writer: Box<dyn IoWrite> =
        if let Some(path) = output_path {
            Box::new(File::create(path).map_err(|err| {
                format!("failed to create output file '{}': {err}", path.display())
            })?)
        } else {
            Box::new(io::stdout())
        };

    writer
        .write_all(ctxdsl.as_bytes())
        .map_err(|err| format!("failed to write CTXDSL: {err}"))?;

    if let Some(path) = output_path {
        eprintln!("CTXDSL written to {}", path.display());
    }

    Ok(())
}

/// Shared preamble result for `context eval` and `context synthesize`.
///
/// Both subcommands load documents, optionally print intermediate CTXDSL,
/// resolve a formula name, and realize the context before diverging.
/// This struct carries the outputs of that common setup so neither caller
/// duplicates the logic.
pub(crate) struct PreparedEvalContext {
    pub(crate) realized: RealizedContext,
    pub(crate) formula_name: String,
}

/// Input parameters for [`prepare_eval_context`].
///
/// Groups the fields that are identical across `ContextEvalArgs` and
/// `ContextSynthesizeArgs` so the helper stays under clippy's argument-count
/// limit without introducing a builder or unrelated abstraction.
pub(crate) struct EvalContextParams<'a> {
    pub(crate) context: &'a Path,
    pub(crate) sidecars: &'a [PathBuf],
    pub(crate) adapter: Option<&'a str>,
    pub(crate) mode: Option<&'a str>,
    pub(crate) preprocessor: Option<&'a str>,
    pub(crate) print_ctxdsl_path: Option<&'a Option<PathBuf>>,
    /// Non-empty only for `context eval`; pass `&[]` for `context synthesize`.
    pub(crate) stubs: &'a [PathBuf],
    pub(crate) formula: &'a Option<String>,
    pub(crate) template: &'a Option<String>,
    pub(crate) template_args: &'a [String],
    pub(crate) automaton: &'a str,
}

/// Execute the shared preamble for `context eval` and `context synthesize`.
pub(crate) fn prepare_eval_context(
    p: EvalContextParams<'_>,
) -> Result<PreparedEvalContext, String> {
    let (context_doc, mut sidecar_docs, adapter_ctxdsl) =
        load_context_documents_mode(p.context, p.sidecars, p.adapter, p.mode, p.preprocessor)?;

    // Print intermediate CTXDSL if requested
    if let Some(output_path) = p.print_ctxdsl_path {
        if let Some(ctxdsl) = &adapter_ctxdsl {
            print_ctxdsl_output(ctxdsl, output_path.as_ref())?;
        } else {
            eprintln!("No adapter translation — CTXDSL is the input file itself");
        }
    }

    // Load stub files: translate each .espec.json via extraction adapter → CTXDSL → parse as sidecar
    // Only context_eval passes stubs; context_synthesize passes an empty slice.
    for stub_path in p.stubs {
        let (stub_doc, _) = load_with_adapter_mode(stub_path, Some("extraction"), p.mode)
            .map_err(|e| format!("Failed to load stub '{}': {e}", stub_path.display()))?;
        eprintln!(
            "Loaded stub: {} ({} automata)",
            stub_path.display(),
            stub_doc.automata.len()
        );
        sidecar_docs.push(stub_doc);
    }

    // Resolve formula: either --formula NAME or --template ID [--template-arg K=V]
    let formula_name = crate::resolve_formula_name(
        p.formula,
        p.template,
        p.template_args,
        p.automaton,
        &mut sidecar_docs,
    )?;

    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    Ok(PreparedEvalContext {
        realized,
        formula_name,
    })
}
