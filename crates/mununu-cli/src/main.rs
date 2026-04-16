use clap::{Args, Parser, Subcommand, ValueEnum};
use mununu_core::abstraction::unrolling::{
    Effect, OriginalState, OriginalTransition, UnrollingOptions, VariableDecl, unroll_states,
};
use mununu_core::clts::{Clts, DefaultLabelIdx, LabelId};
use mununu_core::context::{
    ControllerDiagnostics, ControllerSynthesis, ControllerSynthesisOptions, DiagnosticsOptions,
};
use mununu_core::context_dsl::ast::{
    BinaryOp, Expr, ExprKind, StateRef, StateSelector, TransitionLabel, UnaryOp,
};
use mununu_core::context_dsl::{
    ContextDoc, RealizedContext, parse as parse_context_doc, realize_context,
};
use mununu_core::mu_calculus::EvaluationOptions;
use serde::Serialize;
use serde_json::{self, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug)]
#[command(
    name = "mununu",
    about = "CLTS Verification Tool CLI",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Inspect or manipulate Context DSL artefacts.
    Context {
        #[command(subcommand)]
        command: Box<ContextCommand>,
    },
    /// Extraction spec tools (validate, check provenance).
    Extraction {
        #[command(subcommand)]
        command: Box<ExtractionCommand>,
    },
    /// Start HTTP API server
    Server {
        /// Server address (default: 127.0.0.1:8080)
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
    },
}

#[derive(Subcommand, Debug)]
enum ExtractionCommand {
    /// Validate an extraction spec's line anchors against the actual source file.
    Validate(ExtractionValidateArgs),
    /// Check provenance headers in CTXDSL files.
    Check(ExtractionCheckArgs),
}

#[derive(Args, Debug)]
struct ExtractionValidateArgs {
    /// Path to extraction spec JSON (.json or .espec.json).
    #[arg(value_name = "SPEC")]
    spec: PathBuf,
    /// Path to the source file referenced by the spec.
    #[arg(value_name = "SOURCE")]
    source: PathBuf,
    /// Output as JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
    /// Number of lines to search around expected position for drifted anchors.
    #[arg(long, default_value = "5")]
    drift_window: usize,
}

#[derive(Args, Debug)]
struct ExtractionCheckArgs {
    /// CTXDSL files to check for provenance headers.
    #[arg(value_name = "FILE", num_args = 1..)]
    files: Vec<PathBuf>,
    /// Require @generated-from header (fail if missing).
    #[arg(long)]
    require_generated: bool,
    /// Require @model-source header (fail if missing).
    #[arg(long)]
    require_model_source: bool,
}

#[derive(Subcommand, Debug)]
enum ContextCommand {
    /// Parse and validate main + sidecar documents, optionally copying them to an output folder.
    Merge(ContextMergeArgs),
    /// Emit a JSON summary of automata, predicates, controllers, and formulas.
    Summarize(ContextSummarizeArgs),
    /// List guard predicates registered for the context (optionally filter by automaton).
    Predicates(ContextPredicatesArgs),
    /// Evaluate a μ-calculus formula over the realised context.
    Eval(ContextEvalArgs),
    /// Synthesise a controller for the given automaton/formula pair.
    Synth(ContextSynthesizeArgs),
    /// Generate a Cytoscape graph visualization of automata.
    Graph(ContextGraphArgs),
}

#[derive(Args, Debug)]
struct ContextMergeArgs {
    /// Context + sidecar files to merge (first entry is treated as the main context).
    #[arg(value_name = "FILE", num_args = 1..)]
    files: Vec<PathBuf>,
    /// Directory where the provided files should be copied.
    #[arg(long = "output", value_name = "DIR")]
    output: Option<PathBuf>,
    /// Overwrite the output directory if it already exists.
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct ContextSummarizeArgs {
    /// Primary context document to load.
    #[arg(value_name = "CONTEXT")]
    context: PathBuf,
    /// Optional sidecar documents to merge.
    #[arg(long = "sidecar", value_name = "FILE")]
    sidecars: Vec<PathBuf>,
    /// Print the internal structure of the context to stdout or a file.
    #[arg(long = "print-structure", value_name = "FILE")]
    print_structure: Option<Option<PathBuf>>,
}

#[derive(Args, Debug)]
struct ContextPredicatesArgs {
    /// Primary context document to load.
    #[arg(value_name = "CONTEXT")]
    context: PathBuf,
    /// Optional sidecar documents to merge.
    #[arg(long = "sidecar", value_name = "FILE")]
    sidecars: Vec<PathBuf>,
    /// Restrict output to a single automaton.
    #[arg(long = "automaton", value_name = "NAME")]
    automaton: Option<String>,
}

#[derive(Args, Debug)]
struct ContextEvalArgs {
    /// Primary context document to load.
    #[arg(value_name = "CONTEXT")]
    context: PathBuf,
    /// Optional sidecar documents to merge.
    #[arg(long = "sidecar", value_name = "FILE")]
    sidecars: Vec<PathBuf>,
    /// Translate from an external format before processing (tlsf, aiger, promela, extraction, auto).
    #[arg(long = "adapter", value_name = "FORMAT")]
    adapter: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    #[arg(long = "mode", value_name = "MODE")]
    mode: Option<String>,
    /// μ-calculus formula to evaluate.
    #[arg(long = "formula", value_name = "NAME")]
    formula: String,
    /// Automaton over which the formula should be evaluated.
    #[arg(long = "automaton", value_name = "NAME")]
    automaton: String,
    /// Disable guard partitions during evaluation.
    #[arg(long = "no-partitions")]
    no_partitions: bool,
    /// Hide labels (comma-separated) — reclassify as internal before evaluation.
    #[arg(long = "hide", value_delimiter = ',')]
    hide: Vec<String>,
    /// Apply bisimulation minimization to the target automaton before evaluation.
    #[arg(long = "minimize")]
    minimize: bool,
    /// Print the internal structure of the context to stdout or a file.
    #[arg(long = "print-structure", value_name = "FILE")]
    print_structure: Option<Option<PathBuf>>,
}

#[derive(Args, Debug)]
struct ContextSynthesizeArgs {
    /// Primary context document to load.
    #[arg(value_name = "CONTEXT")]
    context: PathBuf,
    /// Optional sidecar documents to merge.
    #[arg(long = "sidecar", value_name = "FILE")]
    sidecars: Vec<PathBuf>,
    /// Translate from an external format before processing (tlsf, aiger, promela, extraction, auto).
    #[arg(long = "adapter", value_name = "FORMAT")]
    adapter: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    #[arg(long = "mode", value_name = "MODE")]
    mode: Option<String>,
    /// μ-calculus formula to synthesise.
    #[arg(long = "formula", value_name = "NAME")]
    formula: String,
    /// Automaton over which the controller should be synthesised.
    #[arg(long = "automaton", value_name = "NAME")]
    automaton: String,
    /// Disable guard partitions during evaluation.
    #[arg(long = "no-partitions")]
    no_partitions: bool,
    /// Run a structural minimisation pass on the synthesised controller.
    #[arg(long)]
    minimize: bool,
    /// Extract a positional strategy: keep only one controllable transition per state.
    #[arg(long = "extract-strategy")]
    extract_strategy: bool,
    /// Emit counterexample/counterstrategy diagnostics when unrealizable.
    #[arg(long)]
    counterexample: bool,
    /// Capture deadlock traces in diagnostics.
    #[arg(long = "deadlock-traces")]
    deadlock_traces: bool,
    /// Cap the number of counterstrategy traces collected.
    #[arg(long = "max-counter-traces", value_name = "N")]
    max_counter_traces: Option<usize>,
    /// Skip proof obligation emission for violating initial states.
    #[arg(long = "no-proof-obligations")]
    no_proof_obligations: bool,
    /// Path where a JSON summary of the synthesis result should be written.
    #[arg(long = "dump-json", value_name = "FILE")]
    dump_json: Option<PathBuf>,
    /// Path where a controller-only DSL snapshot should be written.
    #[arg(long = "emit-dsl", value_name = "FILE")]
    emit_dsl: Option<PathBuf>,
    /// Path where diagnostics should be exported as a DSL sidecar.
    #[arg(long = "dump-diagnostics", value_name = "FILE")]
    dump_diagnostics: Option<PathBuf>,
    /// Print the internal structure of the context to stdout or a file.
    #[arg(long = "print-structure", value_name = "FILE")]
    print_structure: Option<Option<PathBuf>>,
    /// Output format for the synthesized controller: ctxdsl (default), xstate, systemverilog.
    #[arg(long = "output-format", value_name = "FORMAT")]
    output_format: Option<String>,
    /// Path where the native-format controller should be written (requires --output-format).
    #[arg(long = "emit-native", value_name = "FILE")]
    emit_native: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ContextGraphArgs {
    /// Primary context document to load.
    #[arg(value_name = "CONTEXT")]
    context: PathBuf,
    /// Optional sidecar documents to merge.
    #[arg(long = "sidecar", value_name = "FILE")]
    sidecars: Vec<PathBuf>,
    /// Type of graph to generate: `dsl` (ctxdsl inferred automata), `unrolled` (internal after unrolling), or `both`.
    #[arg(long, value_enum, default_value = "dsl")]
    r#type: GraphOutputType,
    /// Output file path for the HTML visualization.
    #[arg(long = "output", value_name = "FILE")]
    output: PathBuf,
    /// Restrict output to a single automaton.
    #[arg(long = "automaton", value_name = "NAME")]
    automaton: Option<String>,
    /// Generate a counterstrategy graph for a failed formula (requires --formula and --automaton).
    #[arg(long)]
    counterstrategy: bool,
    /// Formula to evaluate for counterstrategy generation.
    #[arg(long = "formula", value_name = "NAME")]
    formula: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum GraphOutputType {
    /// Show only the ctxdsl inferred automata graph.
    Dsl,
    /// Show only the internal representation after abstraction/unrolling.
    Unrolled,
    /// Show both graphs.
    Both,
}

fn main() {
    init_tracing();
    let cli = Cli::parse();
    if let Err(err) = dispatch(cli.command) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Create logs directory if it doesn't exist
    let logs_dir = PathBuf::from("logs");
    let log_file = logs_dir.join("mununu.log");

    // Try to set up file logging
    let file_result = fs::create_dir_all(&logs_dir)
        .and_then(|_| File::options().create(true).append(true).open(&log_file));

    match file_result {
        Ok(file) => {
            // Set up dual logging: both stdout and file
            let file_layer = fmt::layer()
                .with_writer(file)
                .with_ansi(false) // Disable ANSI colors in file
                .with_target(false)
                .compact();

            let stderr_layer = fmt::layer()
                .with_writer(std::io::stderr)
                .with_target(false)
                .compact();

            // Initialize with both layers and apply filter at the registry level
            let _ = Registry::default()
                .with(filter)
                .with(file_layer)
                .with(stderr_layer)
                .try_init();

            tracing::info!(
                log_file = %log_file.display(),
                "Logging initialized. Logs are written to both stdout and file."
            );
        }
        Err(e) => {
            eprintln!(
                "Warning: Failed to set up file logging ({:?}): {}. Logs will only go to stdout.",
                log_file, e
            );
            // Fall back to stdout-only logging
            let builder = fmt().with_env_filter(filter).with_target(false).compact();
            let _ = builder.try_init();
        }
    }
}

fn dispatch(command: Commands) -> Result<(), String> {
    match command {
        Commands::Context { command } => handle_context(*command),
        Commands::Extraction { command } => handle_extraction(*command),
        Commands::Server { addr } => {
            use std::net::SocketAddr;
            let addr: SocketAddr = addr
                .parse()
                .map_err(|e| format!("Invalid address: {}", e))?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("Failed to create runtime: {}", e))?;
            rt.block_on(mununu_core::api::start_server(addr))
                .map_err(|e| format!("Server error: {}", e))?;
            Ok(())
        }
    }
}

fn handle_context(command: ContextCommand) -> Result<(), String> {
    match command {
        ContextCommand::Merge(args) => context_merge(args),
        ContextCommand::Summarize(args) => context_summarize(args),
        ContextCommand::Predicates(args) => context_predicates(args),
        ContextCommand::Eval(args) => context_eval(args),
        ContextCommand::Synth(args) => context_synthesize(args),
        ContextCommand::Graph(args) => context_graph(args),
    }
}

fn handle_extraction(command: ExtractionCommand) -> Result<(), String> {
    match command {
        ExtractionCommand::Validate(args) => extraction_validate(args),
        ExtractionCommand::Check(args) => extraction_check(args),
    }
}

fn extraction_validate(args: ExtractionValidateArgs) -> Result<(), String> {
    let spec_content = fs::read_to_string(&args.spec)
        .map_err(|e| format!("Failed to read spec '{}': {e}", args.spec.display()))?;

    let report = mununu_core::adapter::extraction::validate::validate_spec(
        &spec_content,
        &args.source,
        args.drift_window,
    )?;

    if args.json {
        // JSON output
        let anchors: Vec<serde_json::Value> = report
            .anchors
            .iter()
            .map(|a| {
                use mununu_core::adapter::extraction::validate::AnchorResult;
                match a {
                    AnchorResult::Exact {
                        spec_id,
                        section,
                        line,
                    } => serde_json::json!({
                        "status": "exact", "spec_id": spec_id, "section": section, "line": line
                    }),
                    AnchorResult::Drifted {
                        spec_id,
                        section,
                        expected_line,
                        found_line,
                        drift,
                    } => serde_json::json!({
                        "status": "drifted", "spec_id": spec_id, "section": section,
                        "expected_line": expected_line, "found_line": found_line, "drift": drift
                    }),
                    AnchorResult::Mismatch {
                        spec_id,
                        section,
                        expected_line,
                        expected_pattern,
                        actual_at_line,
                    } => serde_json::json!({
                        "status": "mismatch", "spec_id": spec_id, "section": section,
                        "expected_line": expected_line, "expected_pattern": expected_pattern,
                        "actual_at_line": actual_at_line
                    }),
                    AnchorResult::Error {
                        spec_id,
                        section,
                        message,
                    } => serde_json::json!({
                        "status": "error", "spec_id": spec_id, "section": section,
                        "message": message
                    }),
                }
            })
            .collect();

        let output = serde_json::json!({
            "summary": {
                "total": report.summary.total,
                "exact": report.summary.exact,
                "drifted": report.summary.drifted,
                "mismatch": report.summary.mismatch,
                "error": report.summary.error,
                "uncovered_accesses": report.summary.uncovered_accesses,
            },
            "commit_match": report.commit_match,
            "anchors": anchors,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        // Human-readable output
        if let Some(matches) = report.commit_match {
            println!("Commit: {}", if matches { "MATCH" } else { "MISMATCH" });
            println!();
        }

        println!(
            "Anchor checks: {} total, {} exact, {} drifted, {} MISMATCH, {} ERROR",
            report.summary.total,
            report.summary.exact,
            report.summary.drifted,
            report.summary.mismatch,
            report.summary.error,
        );
        println!();

        for a in &report.anchors {
            use mununu_core::adapter::extraction::validate::AnchorResult;
            match a {
                AnchorResult::Exact { spec_id, .. } => {
                    println!("  OK    {spec_id}");
                }
                AnchorResult::Drifted {
                    spec_id,
                    expected_line,
                    found_line,
                    drift,
                    ..
                } => {
                    println!(
                        "  DRIFT {spec_id} (expected line {expected_line}, found at {found_line}, drift={drift:+})"
                    );
                }
                AnchorResult::Mismatch {
                    spec_id,
                    expected_line,
                    expected_pattern,
                    actual_at_line,
                    ..
                } => {
                    println!("  FAIL  {spec_id} (line {expected_line})");
                    println!("         expected: {expected_pattern}");
                    println!("         actual:   {actual_at_line}");
                }
                AnchorResult::Error {
                    spec_id, message, ..
                } => {
                    println!("  ERR   {spec_id}: {message}");
                }
            }
        }

        if !report.uncovered.is_empty() {
            println!(
                "\nUncovered state field accesses: {}",
                report.uncovered.len()
            );
            for u in report.uncovered.iter().take(20) {
                println!("  line {:4}: {:20}  {}", u.line, u.field, u.content);
            }
            if report.uncovered.len() > 20 {
                println!("  ... and {} more", report.uncovered.len() - 20);
            }
        }
    }

    if report.summary.mismatch > 0 || report.summary.error > 0 {
        Err(format!(
            "{} mismatch(es) and {} error(s) found",
            report.summary.mismatch, report.summary.error
        ))
    } else {
        Ok(())
    }
}

fn extraction_check(args: ExtractionCheckArgs) -> Result<(), String> {
    let mut failures = 0;

    for path in &args.files {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;

        let info = mununu_core::adapter::extraction::validate::check_provenance(&content);
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        if args.require_generated && !info.is_generated() {
            println!("FAIL: {name} — missing @generated-from header");
            failures += 1;
        } else if args.require_model_source && !info.is_specification_model() {
            println!("FAIL: {name} — missing @model-source header");
            failures += 1;
        } else if info.is_generated() {
            println!(
                "OK:   {name} — @generated-from: {}",
                info.generated_from.as_deref().unwrap_or("?")
            );
        } else if info.is_specification_model() {
            println!(
                "OK:   {name} — @model-source: {}",
                info.model_source.as_deref().unwrap_or("?")
            );
        } else {
            println!("WARN: {name} — no provenance header");
        }
    }

    if failures > 0 {
        Err(format!("{failures} file(s) missing required headers"))
    } else {
        Ok(())
    }
}

fn prepare_output_dir(path: &Path, force: bool) -> Result<(), String> {
    if path.exists() {
        if !path.is_dir() {
            return Err(format!(
                "output path '{}' exists but is not a directory",
                path.display()
            ));
        }
        if force {
            if let Ok(mut entries) = fs::read_dir(path)
                && entries.next().is_some()
            {
                tracing::warn!(
                    path = %path.display(),
                    "output directory not empty; existing files may be overwritten"
                );
            }
        } else {
            let mut entries = fs::read_dir(path)
                .map_err(|err| format!("failed to inspect '{}': {err}", path.display()))?;
            if entries.next().is_some() {
                return Err(format!(
                    "output directory '{}' is not empty (use --force to overwrite)",
                    path.display()
                ));
            }
        }
    } else {
        fs::create_dir_all(path).map_err(|err| {
            format!(
                "failed to create output directory '{}': {err}",
                path.display()
            )
        })?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct AutomatonSummaryOutput {
    name: String,
    state_count: usize,
    transition_count: usize,
}

#[derive(Debug, Serialize)]
struct ContextSummaryOutput {
    context: String,
    sidecar_count: usize,
    automata: Vec<AutomatonSummaryOutput>,
    controllers: Vec<String>,
    formulas: Vec<String>,
    guard_predicates: BTreeMap<String, Vec<String>>,
}

fn parse_context_file(path: &Path) -> Result<ContextDoc, String> {
    let source = fs::read_to_string(path)
        .map_err(|err| format!("failed to read '{}': {err}", path.display()))?;
    parse_context_doc(&source).map_err(|err| format!("failed to parse '{}': {err}", path.display()))
}

/// Read a source file, optionally translating it from an external format first.
fn load_with_adapter_mode(
    path: &Path,
    adapter: Option<&str>,
    mode: Option<&str>,
) -> Result<ContextDoc, String> {
    let source = fs::read_to_string(path)
        .map_err(|err| format!("failed to read '{}': {err}", path.display()))?;

    let ctxdsl_source = match adapter {
        Some("tlsf") => {
            use mununu_core::adapter::{AdapterOptions, FormatAdapter};
            let options = AdapterOptions::default();
            let output = mununu_core::adapter::tlsf::TlsfAdapter::translate(&source, &options)
                .map_err(|e| format!("TLSF adapter error: {e}"))?;
            for w in &output.warnings {
                eprintln!("adapter warning: {}", w.message);
            }
            eprintln!(
                "Translated TLSF: {} signals, {} states, {} properties",
                output.source_info.signal_count,
                output.source_info.state_count,
                output.source_info.property_count,
            );
            output.ctxdsl
        }
        Some("aiger") => {
            use mununu_core::adapter::{AdapterOptions, FormatAdapter};
            let options = AdapterOptions::default();
            let output = mununu_core::adapter::aiger::AigerAdapter::translate(&source, &options)
                .map_err(|e| format!("AIGER adapter error: {e}"))?;
            for w in &output.warnings {
                eprintln!("adapter warning: {}", w.message);
            }
            eprintln!(
                "Translated AIGER: {} signals, {} states, {} properties",
                output.source_info.signal_count,
                output.source_info.state_count,
                output.source_info.property_count,
            );
            output.ctxdsl
        }
        Some("promela") => {
            use mununu_core::adapter::{AdapterOptions, FormatAdapter};
            let options = AdapterOptions::default();
            let output =
                mununu_core::adapter::promela::PromelaAdapter::translate(&source, &options)
                    .map_err(|e| format!("Promela adapter error: {e}"))?;
            for w in &output.warnings {
                eprintln!("adapter warning: {}", w.message);
            }
            eprintln!(
                "Translated Promela: {} signals, {} states, {} properties",
                output.source_info.signal_count,
                output.source_info.state_count,
                output.source_info.property_count,
            );
            output.ctxdsl
        }
        Some("systemverilog") | Some("sv") => {
            use mununu_core::adapter::{AdapterOptions, FormatAdapter};
            let options = AdapterOptions::default();
            let output = mununu_core::adapter::systemverilog::SystemVerilogAdapter::translate(
                &source, &options,
            )
            .map_err(|e| format!("SystemVerilog adapter error: {e}"))?;
            for w in &output.warnings {
                eprintln!("adapter warning: {}", w.message);
            }
            eprintln!(
                "Translated SystemVerilog: {} signals, {} states, {} properties",
                output.source_info.signal_count,
                output.source_info.state_count,
                output.source_info.property_count,
            );
            output.ctxdsl
        }
        Some("xstate") => {
            use mununu_core::adapter::{AdapterOptions, FormatAdapter};
            let options = AdapterOptions::default();
            let output = mununu_core::adapter::xstate::XStateAdapter::translate(&source, &options)
                .map_err(|e| format!("XState adapter error: {e}"))?;
            for w in &output.warnings {
                eprintln!("adapter warning: {}", w.message);
            }
            eprintln!(
                "Translated XState: {} events, {} states, {} properties",
                output.source_info.signal_count,
                output.source_info.state_count,
                output.source_info.property_count,
            );
            output.ctxdsl
        }
        Some("extraction") => {
            use mununu_core::adapter::{AdapterOptions, FormatAdapter};
            let options = AdapterOptions {
                mode: mode.map(|s| s.to_string()),
                ..Default::default()
            };
            let output =
                mununu_core::adapter::extraction::ExtractionAdapter::translate(&source, &options)
                    .map_err(|e| format!("Extraction adapter error: {e}"))?;
            for w in &output.warnings {
                eprintln!("adapter warning: {}", w.message);
            }
            eprintln!(
                "Translated Extraction (mode: {}): {} labels, {} states, {} properties",
                mode.unwrap_or("vulnerable"),
                output.source_info.signal_count,
                output.source_info.state_count,
                output.source_info.property_count,
            );
            output.ctxdsl
        }
        Some("auto") => {
            let options = mununu_core::adapter::AdapterOptions {
                mode: mode.map(|s| s.to_string()),
                ..Default::default()
            };
            let output = mununu_core::adapter::auto_translate(&source, &options)
                .map_err(|e| format!("adapter error: {e}"))?;
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
            output.ctxdsl
        }
        Some(fmt) => {
            return Err(format!(
                "unknown adapter format '{fmt}'. Supported: tlsf, aiger, promela, xstate, systemverilog, extraction, auto"
            ));
        }
        None => {
            // Auto-detect by file extension if no adapter specified
            if let Some(fmt) = mununu_core::adapter::detect_format_by_extension(path) {
                eprintln!("Auto-detected format '{}' from extension", fmt);
                return load_with_adapter_mode(path, Some(fmt), mode);
            }
            source
        }
    };

    parse_context_doc(&ctxdsl_source)
        .map_err(|err| format!("failed to parse '{}': {err}", path.display()))
}

fn load_context_documents(
    context_path: &Path,
    sidecar_paths: &[PathBuf],
    adapter: Option<&str>,
) -> Result<(ContextDoc, Vec<ContextDoc>), String> {
    load_context_documents_mode(context_path, sidecar_paths, adapter, None)
}

fn load_context_documents_mode(
    context_path: &Path,
    sidecar_paths: &[PathBuf],
    adapter: Option<&str>,
    mode: Option<&str>,
) -> Result<(ContextDoc, Vec<ContextDoc>), String> {
    let context_doc = load_with_adapter_mode(context_path, adapter, mode)?;
    let mut sidecar_docs = Vec::with_capacity(sidecar_paths.len());
    for path in sidecar_paths {
        sidecar_docs.push(parse_context_file(path)?);
    }
    Ok((context_doc, sidecar_docs))
}

fn realize_documents(
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
fn print_context_structure(
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

fn build_context_summary(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
) -> ContextSummaryOutput {
    let mut automata_map: BTreeMap<String, AutomatonSummaryOutput> = BTreeMap::new();
    for doc in std::iter::once(context_doc).chain(sidecar_docs.iter()) {
        for automaton in &doc.automata {
            automata_map
                .entry(automaton.name.name.clone())
                .or_insert_with(|| AutomatonSummaryOutput {
                    name: automaton.name.name.clone(),
                    state_count: automaton.states.len(),
                    transition_count: automaton.transitions.len(),
                });
        }
    }

    let automata = automata_map.into_values().collect();

    let mut controllers = BTreeSet::new();
    let mut formulas = BTreeSet::new();
    for doc in std::iter::once(context_doc).chain(sidecar_docs.iter()) {
        for controller in &doc.controllers {
            controllers.insert(controller.name.name.clone());
        }
        for formula in &doc.mu_formulas {
            formulas.insert(formula.name.name.clone());
        }
    }

    let mut guard_predicates = BTreeMap::new();
    for (automaton, predicates) in realized.predicates.iter() {
        let mut list: Vec<String> = predicates.iter().cloned().collect();
        list.sort();
        guard_predicates.insert(automaton.clone(), list);
    }

    ContextSummaryOutput {
        context: context_doc.name.name.clone(),
        sidecar_count: sidecar_docs.len(),
        automata,
        controllers: controllers.into_iter().collect(),
        formulas: formulas.into_iter().collect(),
        guard_predicates,
    }
}

fn context_merge(args: ContextMergeArgs) -> Result<(), String> {
    if args.files.is_empty() {
        return Err("at least one context file must be provided".into());
    }

    let context_path = args.files[0].clone();
    let sidecar_paths: Vec<PathBuf> = args.files.iter().skip(1).cloned().collect();
    let (context_doc, sidecar_docs) = load_context_documents(&context_path, &sidecar_paths, None)?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let summary = build_context_summary(&context_doc, &sidecar_docs, &realized);

    println!("Merged context '{}'", summary.context);
    println!("  Sidecars: {}", summary.sidecar_count);
    println!("  Automata:");
    if summary.automata.is_empty() {
        println!("    (none)");
    } else {
        for automaton in &summary.automata {
            println!(
                "    - {} (states: {}, transitions: {})",
                automaton.name, automaton.state_count, automaton.transition_count
            );
        }
    }
    if summary.formulas.is_empty() {
        println!("  μ-formulas: none");
    } else {
        println!("  μ-formulas: {}", summary.formulas.join(", "));
    }
    if summary.controllers.is_empty() {
        println!("  Controllers: none");
    } else {
        println!("  Controllers: {}", summary.controllers.join(", "));
    }

    if summary.guard_predicates.is_empty() {
        println!("  Guard predicates: none");
    } else {
        println!("  Guard predicates:");
        for (automaton, predicates) in &summary.guard_predicates {
            if predicates.is_empty() {
                println!("    - {}: none", automaton);
            } else {
                println!("    - {}: {}", automaton, predicates.join(", "));
            }
        }
    }

    if let Some(output_dir) = args.output {
        prepare_output_dir(&output_dir, args.force)?;
        let mut copied = 0usize;
        for input in std::iter::once(&context_path).chain(sidecar_paths.iter()) {
            let Some(filename) = input.file_name() else {
                return Err(format!(
                    "cannot determine file name for '{}'",
                    input.display()
                ));
            };
            let destination = output_dir.join(filename);
            fs::copy(input, &destination).map_err(|err| {
                format!(
                    "failed to copy '{}' to '{}': {err}",
                    input.display(),
                    destination.display()
                )
            })?;
            copied += 1;
        }
        println!("Copied {} file(s) to {}", copied, output_dir.display());
    }

    Ok(())
}

fn context_summarize(args: ContextSummarizeArgs) -> Result<(), String> {
    let (context_doc, sidecar_docs) = load_context_documents(&args.context, &args.sidecars, None)?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let summary = build_context_summary(&context_doc, &sidecar_docs, &realized);
    let json = serde_json::to_string_pretty(&summary)
        .map_err(|err| format!("failed to serialise summary: {err}"))?;
    println!("{json}");

    // Print structure if requested
    if let Some(output_path) = args.print_structure {
        print_context_structure(&realized.context, output_path)?;
    }

    Ok(())
}

fn context_predicates(args: ContextPredicatesArgs) -> Result<(), String> {
    let (context_doc, sidecar_docs) = load_context_documents(&args.context, &args.sidecars, None)?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let mut automata: Vec<String> = realized.predicates.keys().cloned().collect();
    automata.sort();

    let filter = args.automaton.as_deref();
    let mut reported = false;

    for automaton in automata {
        if let Some(filter) = filter
            && automaton != filter
        {
            continue;
        }

        let Some(predicates) = realized.predicate_names(&automaton) else {
            continue;
        };

        let mut names: Vec<String> = predicates.iter().cloned().collect();
        names.sort();

        println!("Automaton: {}", automaton);
        if names.is_empty() {
            println!("  (no guard predicates)");
        } else {
            for predicate in names {
                let guard = realized
                    .predicate_formula(&automaton, &predicate)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if guard.is_empty() {
                    println!("  - {}", predicate);
                } else {
                    println!("  - {} => {}", predicate, guard);
                }
            }
        }
        reported = true;
    }

    if let Some(filter_name) = filter
        && !reported
    {
        return Err(format!(
            "automaton '{}' has no registered predicates",
            filter_name
        ));
    }

    if !reported {
        println!("No guard predicates registered.");
    }

    Ok(())
}

fn context_eval(args: ContextEvalArgs) -> Result<(), String> {
    let (context_doc, sidecar_docs) = load_context_documents_mode(
        &args.context,
        &args.sidecars,
        args.adapter.as_deref(),
        args.mode.as_deref(),
    )?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let formula = realized
        .formulas
        .get(&args.formula)
        .ok_or_else(|| format!("unknown formula '{}' in realised context", args.formula))?;
    let clts = realized
        .context
        .clts(&args.automaton)
        .ok_or_else(|| format!("unknown automaton '{}' in realised context", args.automaton))?;

    // Apply adaptation: hiding + minimization (before evaluation)
    if !args.hide.is_empty() {
        let hide_set: std::collections::HashSet<String> = args.hide.iter().cloned().collect();
        let (hidden_clts, stats) =
            mununu_core::composition::hide::hide_labels_with_stats(clts, &hide_set)
                .map_err(|e| format!("label hiding failed: {e}"))?;
        eprintln!(
            "Hidden {} label(s) out of {} total",
            stats.labels_hidden, stats.total_labels
        );

        if args.minimize {
            if let Some((_minimized, report)) =
                mununu_core::composition::minimize::minimize_bisimulation(&hidden_clts, None)
                    .map_err(|e| format!("minimization failed: {e}"))?
            {
                eprintln!(
                    "Minimized: {} → {} states ({} removed), {} → {} transitions",
                    report.states_before,
                    report.states_after,
                    report.states_before - report.states_after,
                    report.transitions_before,
                    report.transitions_after,
                );
            } else {
                eprintln!("Minimization: already minimal (no reduction)");
            }
        }
        // Note: evaluation still runs on the original context because
        // the adapted CLTS is not registered in the realized context.
        // Full integration requires registering the adapted CLTS.
        // For now, hiding and minimization report statistics only.
        eprintln!("(Adaptation applied — evaluation runs on original context)");
    } else if args.minimize {
        if let Some((_minimized, report)) =
            mununu_core::composition::minimize::minimize_bisimulation(clts, None)
                .map_err(|e| format!("minimization failed: {e}"))?
        {
            eprintln!(
                "Minimized: {} → {} states ({} removed), {} → {} transitions",
                report.states_before,
                report.states_after,
                report.states_before - report.states_after,
                report.transitions_before,
                report.transitions_after,
            );
        } else {
            eprintln!("Minimization: already minimal (no reduction)");
        }
        eprintln!("(Adaptation applied — evaluation runs on original context)");
    }

    let env = realized.environment_for(&args.automaton);

    let mut options = EvaluationOptions::default();
    if args.no_partitions {
        options.use_partitions = false;
    }

    let result = realized
        .context
        .evaluate_mu(&args.automaton, &formula.formula, &env, Some(&options))
        .map_err(|err| format!("μ-calculus evaluation failed: {err}"))?;

    let mut satisfying = Vec::new();
    for state_id in clts.states() {
        if result
            .get(state_id.index())
            .map(|bit| *bit)
            .unwrap_or(false)
            && let Some(name) = clts.state_name(state_id)
        {
            satisfying.push(name.to_string());
        }
    }
    satisfying.sort();

    let initial_states: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|state_id| clts.state_name(*state_id).map(|name| name.to_string()))
        .collect();

    let mut initial_satisfying: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|state_id| {
            if result
                .get(state_id.index())
                .map(|bit| *bit)
                .unwrap_or(false)
            {
                clts.state_name(*state_id).map(|name| name.to_string())
            } else {
                None
            }
        })
        .collect();
    initial_satisfying.sort();

    println!(
        "Formula '{}' over automaton '{}':",
        args.formula, args.automaton
    );
    println!(
        "  States satisfying: {}/{}",
        satisfying.len(),
        clts.state_count()
    );
    if satisfying.is_empty() {
        println!("    (none)");
    } else {
        println!("    {}", satisfying.join(", "));
    }
    println!(
        "  Initial states satisfying: {}/{}",
        initial_satisfying.len(),
        initial_states.len()
    );
    if initial_satisfying.is_empty() {
        println!("    (none)");
    } else {
        println!("    {}", initial_satisfying.join(", "));
    }
    println!(
        "  Guard partitions: {}",
        if options.use_partitions {
            "enabled"
        } else {
            "disabled"
        }
    );

    // Print structure if requested
    if let Some(output_path) = args.print_structure {
        print_context_structure(&realized.context, output_path)?;
    }

    Ok(())
}

fn context_synthesize(args: ContextSynthesizeArgs) -> Result<(), String> {
    let (context_doc, sidecar_docs) = load_context_documents_mode(
        &args.context,
        &args.sidecars,
        args.adapter.as_deref(),
        args.mode.as_deref(),
    )?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let realized_formula = realized
        .formulas
        .get(&args.formula)
        .ok_or_else(|| format!("unknown formula '{}' in realised context", args.formula))?;
    if realized.context.clts(&args.automaton).is_none() {
        return Err(format!(
            "unknown automaton '{}' in realised context",
            args.automaton
        ));
    }
    let env = realized.environment_for(&args.automaton);

    let mut eval_options = EvaluationOptions::default();
    if args.no_partitions {
        eval_options.use_partitions = false;
    }

    let mut diagnostics = DiagnosticsOptions::default();
    let mut diagnostics_enabled = false;
    if args.counterexample {
        diagnostics.counterexample = true;
        diagnostics_enabled = true;
    }
    if args.deadlock_traces {
        diagnostics.deadlock_traces = true;
        diagnostics_enabled = true;
    }
    if let Some(limit) = args.max_counter_traces {
        diagnostics.max_counter_traces = Some(limit);
        diagnostics_enabled = true;
    }
    if args.no_proof_obligations {
        diagnostics.proof_obligations = false;
        diagnostics_enabled = true;
    }

    // Always enable diagnostics if proof_obligations is true (default), to ensure they are generated
    // even when no other diagnostics flags are set
    if diagnostics.proof_obligations {
        diagnostics_enabled = true;
    }

    let diagnostics_ref = diagnostics_enabled.then_some(diagnostics);

    let synthesis = realized
        .context
        .synthesise_controller_with_options(
            &args.automaton,
            &realized_formula.formula,
            &env,
            ControllerSynthesisOptions {
                evaluation: Some(&eval_options),
                diagnostics: diagnostics_ref.as_ref(),
                minimize: args.minimize,
                extract_strategy: args.extract_strategy,
            },
        )
        .map_err(|err| format!("controller synthesis failed: {err}"))?;

    let controller = &synthesis.controller;
    let mut controllable_alphabet = controller.alphabet();
    controllable_alphabet.sort();
    let initial_count = controller.initial_states().len();
    let state_count = controller.state_count();

    println!(
        "Controller synthesis for formula '{}' over automaton '{}':",
        args.formula, args.automaton
    );
    println!(
        "  Realizable: {}",
        if synthesis.realizable { "yes" } else { "no" }
    );
    println!(
        "  Controller states: {} (initial: {})",
        state_count, initial_count
    );
    println!(
        "  Alphabet: {}",
        if controllable_alphabet.is_empty() {
            "(none)".to_owned()
        } else {
            controllable_alphabet.join(", ")
        }
    );
    println!(
        "  Structural hash: 0x{hash:016x}",
        hash = controller.structural_hash()
    );

    render_controller_diagnostics(&synthesis.diagnostics);

    if let Some(path) = args.dump_json.as_ref() {
        write_controller_json(path, &args.automaton, &args.formula, &synthesis)?;
        println!("  JSON summary written to {}", path.display());
    }

    if let Some(path) = args.emit_dsl.as_ref() {
        write_controller_ctxdsl(
            path,
            &args.automaton,
            &args.formula,
            realized_formula.raw.as_str(),
            controller,
        )?;
        println!("  Controller DSL written to {}", path.display());
    }

    if let Some(path) = args.dump_diagnostics.as_ref() {
        ensure_parent_dir(path)
            .map_err(|err| format!("failed to prepare diagnostics path: {err}"))?;
        synthesis
            .diagnostics
            .write_sidecar_dsl(path)
            .map_err(|err| format!("failed to write diagnostics sidecar: {err}"))?;
        println!("  Diagnostics sidecar written to {}", path.display());
    }

    // Emit controller in native format if requested
    if let Some(format) = args.output_format.as_deref() {
        if synthesis.realizable {
            let native_content = match format {
                "xstate" => {
                    use mununu_core::adapter::xstate::emit_controller::controller_to_xstate_json;
                    controller_to_xstate_json(controller, &args.automaton, true)
                }
                "systemverilog" | "sv" => {
                    use mununu_core::adapter::systemverilog::emit_controller::controller_to_systemverilog;
                    controller_to_systemverilog(controller, &args.automaton, true)
                }
                "ctxdsl" => String::new(), // already handled by --emit-dsl
                other => {
                    return Err(format!(
                        "unknown output format '{other}'. Supported: ctxdsl, xstate, systemverilog"
                    ));
                }
            };

            if !native_content.is_empty() {
                if let Some(path) = args.emit_native.as_ref() {
                    ensure_parent_dir(path)
                        .map_err(|err| format!("failed to prepare native output path: {err}"))?;
                    std::fs::write(path, &native_content)
                        .map_err(|err| format!("failed to write native controller: {err}"))?;
                    println!("  Controller ({format}) written to {}", path.display());
                } else {
                    println!("\n--- Controller ({format}) ---\n{native_content}");
                }
            }
        } else {
            eprintln!("  Note: --output-format ignored (specification is unrealizable)");
        }
    }

    // Print structure if requested
    if let Some(output_path) = args.print_structure {
        print_context_structure(&realized.context, output_path)?;
    }

    Ok(())
}

fn render_controller_diagnostics(diagnostics: &ControllerDiagnostics) {
    let has_details = !diagnostics.messages.is_empty()
        || !diagnostics.violating_initials.is_empty()
        || diagnostics.counterexample_trace.is_some()
        || !diagnostics.deadlock_traces.is_empty()
        || !diagnostics.counterstrategy_traces.is_empty()
        || diagnostics.minimization.is_some()
        || !diagnostics.proof_obligations.is_empty()
        || diagnostics.counterstrategy.is_some();

    if !has_details {
        println!("  Diagnostics: no additional notes recorded.");
        return;
    }

    println!("  Diagnostics:");
    for message in &diagnostics.messages {
        println!("    note: {message}");
    }
    if !diagnostics.violating_initials.is_empty() {
        println!(
            "    violating initials: {}",
            diagnostics.violating_initials.join(", ")
        );
    }
    if let Some(trace) = &diagnostics.counterexample_trace {
        println!("    counterexample trace: {}", trace.join(" -> "));
    }
    if !diagnostics.deadlock_traces.is_empty() {
        for (idx, trace) in diagnostics.deadlock_traces.iter().enumerate() {
            println!("    deadlock trace #{idx}: {}", trace.join(" -> "));
        }
    }
    if !diagnostics.counterstrategy_traces.is_empty() {
        for (idx, trace) in diagnostics.counterstrategy_traces.iter().enumerate() {
            println!("    counterstrategy trace #{idx}: {}", trace.join(" -> "));
        }
    }
    if !diagnostics.lasso_traces.is_empty() {
        for (idx, lasso) in diagnostics.lasso_traces.iter().enumerate() {
            if lasso.cycle.is_empty() {
                println!("    lasso trace #{idx}: {}", lasso.prefix.join(" -> "));
            } else {
                println!(
                    "    lasso trace #{idx}: {} -> ({})^ω",
                    lasso.prefix.join(" -> "),
                    lasso.cycle.join(" -> ")
                );
            }
        }
    }
    if let Some(report) = &diagnostics.minimization {
        println!(
            "    minimisation removed {} states and {} transitions",
            report.removed_states, report.removed_transitions
        );
        if !report.merged_states.is_empty() {
            println!("    merged states: {}", report.merged_states.join(", "));
        }
    }
    if !diagnostics.proof_obligations.is_empty() {
        println!(
            "    proof obligations: {}",
            diagnostics.proof_obligations.len()
        );
    }
    if let Some(strategy) = &diagnostics.counterstrategy {
        println!("    counterstrategy states: {}", strategy.states.join(", "));
    }
}

fn context_graph(args: ContextGraphArgs) -> Result<(), String> {
    let (context_doc, sidecar_docs) = load_context_documents(&args.context, &args.sidecars, None)?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;

    // Collect all automata to visualize
    let mut automata_to_visualize: Vec<String> = Vec::new();
    if let Some(automaton_name) = &args.automaton {
        if realized.context.clts(automaton_name).is_none() {
            return Err(format!(
                "unknown automaton '{}' in realised context",
                automaton_name
            ));
        }
        automata_to_visualize.push(automaton_name.clone());
    } else {
        // Get all automata from the context
        for doc in std::iter::once(&context_doc).chain(sidecar_docs.iter()) {
            for automaton in &doc.automata {
                if realized.context.clts(&automaton.name.name).is_some()
                    && !automata_to_visualize.contains(&automaton.name.name)
                {
                    automata_to_visualize.push(automaton.name.name.clone());
                }
            }
        }
    }

    if automata_to_visualize.is_empty() {
        return Err("no automata found to visualize".to_string());
    }

    // Generate Cytoscape elements based on output type
    let mut all_elements = Vec::new();

    match args.r#type {
        GraphOutputType::Dsl => {
            let elements = dsl_automata_to_cytoscape(
                &context_doc,
                &sidecar_docs,
                &realized,
                &automata_to_visualize,
            )?;
            all_elements.extend(elements);
        }
        GraphOutputType::Unrolled => {
            let elements = unrolled_automata_to_cytoscape(
                &context_doc,
                &sidecar_docs,
                &realized,
                &automata_to_visualize,
            )?;
            all_elements.extend(elements);
        }
        GraphOutputType::Both => {
            let dsl_elements = dsl_automata_to_cytoscape(
                &context_doc,
                &sidecar_docs,
                &realized,
                &automata_to_visualize,
            )?;
            all_elements.extend(dsl_elements);
            // Skip unrolled graph if automata have no variables to unroll
            if let Ok(unrolled_elements) = unrolled_automata_to_cytoscape(
                &context_doc,
                &sidecar_docs,
                &realized,
                &automata_to_visualize,
            ) {
                all_elements.extend(unrolled_elements);
            }
        }
    }

    // Generate counterstrategy graph if requested
    if args.counterstrategy {
        let formula_name = args
            .formula
            .as_ref()
            .ok_or("--counterstrategy requires --formula")?;
        let automaton_name = args
            .automaton
            .as_ref()
            .ok_or("--counterstrategy requires --automaton")?;

        let rf = realized
            .formulas
            .get(formula_name)
            .ok_or_else(|| format!("unknown formula '{formula_name}'"))?;
        let clts = realized
            .context
            .clts(automaton_name)
            .ok_or_else(|| format!("unknown automaton '{automaton_name}'"))?;
        let env = realized.environment_for(automaton_name);
        let eval_options = mununu_core::mu_calculus::EvaluationOptions::default();

        // Invert formula and evaluate to get environment winning region
        let inverted = mununu_core::mu_calculus::invert::invert(&rf.formula);
        let inv_bv = realized
            .context
            .evaluate_mu(automaton_name, &inverted, &env, Some(&eval_options))
            .map_err(|e| format!("counterstrategy evaluation failed: {e}"))?;

        let winning_set: std::collections::HashSet<usize> = clts
            .states()
            .filter(|sid| inv_bv.get(sid.index()).map(|bit| *bit).unwrap_or(false))
            .map(|sid| sid.index())
            .collect();

        let cs_name = format!("{automaton_name}_counterstrategy");
        let cs_cytoscape = counterstrategy_to_cytoscape(clts, &cs_name, &winning_set);

        all_elements.extend(cs_cytoscape);

        let winning_names: Vec<String> = clts
            .states()
            .filter(|sid| winning_set.contains(&sid.index()))
            .filter_map(|sid| clts.state_name(sid).map(|n| n.to_string()))
            .collect();
        println!(
            "Counterstrategy: environment wins from {} states: {}",
            winning_names.len(),
            winning_names.join(", ")
        );
    }

    // Generate HTML
    let html = generate_cytoscape_html(&all_elements)?;

    // Write to file
    ensure_parent_dir(&args.output)
        .map_err(|e| format!("failed to create output directory: {}", e))?;
    fs::write(&args.output, html).map_err(|e| {
        format!(
            "failed to write output file '{}': {}",
            args.output.display(),
            e
        )
    })?;

    println!("Graph visualization written to {}", args.output.display());

    Ok(())
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

// Cytoscape element structures
#[derive(Serialize, Debug, Clone)]
struct CytoscapeElement {
    data: CytoscapeData,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<CytoscapePosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    classes: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(untagged)]
#[allow(non_snake_case)]
enum CytoscapeData {
    Node {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        vars: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actions: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        isStart: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        isDead: Option<bool>,
    },
    Edge {
        id: String,
        source: String,
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actionType: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        guard: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect: Option<String>,
    },
}

#[derive(Serialize, Debug, Clone)]
struct CytoscapePosition {
    x: f64,
    y: f64,
}

fn counterstrategy_to_cytoscape(
    clts: &mununu_core::clts::Clts<
        mununu_core::clts::DefaultStateIdx,
        mununu_core::clts::DefaultLabelIdx,
    >,
    automaton_name: &str,
    winning_set: &std::collections::HashSet<usize>,
) -> Vec<CytoscapeElement> {
    let mut elements = Vec::new();
    let mut x_pos = 100.0;

    // Compound node
    elements.push(CytoscapeElement {
        data: CytoscapeData::Node {
            id: automaton_name.to_string(),
            label: Some(format!(
                "Counterstrategy: {}",
                automaton_name.replace("_counterstrategy", "")
            )),
            parent: None,
            vars: None,
            actions: None,
            note: None,
            isStart: None,
            isDead: None,
        },
        position: None,
        classes: None,
    });

    // State nodes
    for state_id in clts.states() {
        if !winning_set.contains(&state_id.index()) {
            continue;
        }
        let name = clts.state_name(state_id).unwrap_or("?").to_string();
        let node_id = format!("{}_{}", automaton_name, name);
        let is_initial = clts.initial_states().contains(&state_id);

        let mut classes = vec!["env-winning"];
        if is_initial {
            classes.push("start");
        }

        elements.push(CytoscapeElement {
            data: CytoscapeData::Node {
                id: node_id,
                label: Some(name),
                parent: Some(automaton_name.to_string()),
                vars: None,
                actions: None,
                note: None,
                isStart: Some(is_initial),
                isDead: Some(false),
            },
            position: Some(CytoscapePosition { x: x_pos, y: 100.0 }),
            classes: Some(classes.join(" ")),
        });
        x_pos += 250.0;
    }

    // Transitions between winning states
    for state_id in clts.states() {
        if !winning_set.contains(&state_id.index()) {
            continue;
        }
        let source = clts.state_name(state_id).unwrap_or("?").to_string();
        let source_id = format!("{}_{}", automaton_name, source);

        for transition in clts.outgoing(state_id) {
            if !winning_set.contains(&transition.target().index()) {
                continue;
            }
            let target = clts
                .state_name(transition.target())
                .unwrap_or("?")
                .to_string();
            let target_id = format!("{}_{}", automaton_name, target);

            let label: Vec<String> = transition
                .labels()
                .iter()
                .filter_map(|lid| {
                    clts.label_payload(*lid).and_then(|vals| {
                        let joined = vals
                            .iter()
                            .filter(|v| !v.is_empty())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ");
                        if joined.is_empty() {
                            None
                        } else {
                            Some(joined)
                        }
                    })
                })
                .collect();

            elements.push(CytoscapeElement {
                data: CytoscapeData::Edge {
                    id: format!("{}_t{}", source_id, elements.len()),
                    source: source_id.clone(),
                    target: target_id,
                    label: if label.is_empty() {
                        None
                    } else {
                        Some(label.join(" | "))
                    },
                    action: None,
                    actionType: if transition.is_uncontrollable(clts) {
                        Some("uncontrollable".to_string())
                    } else {
                        Some("controllable".to_string())
                    },
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
        }
    }

    elements
}

fn dsl_automata_to_cytoscape(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automata_names: &[String],
) -> Result<Vec<CytoscapeElement>, String> {
    let mut elements = Vec::new();
    let mut y_offset = 0.0;
    let state_spacing = 250.0;

    for automaton_name in automata_names {
        // Find the automaton in documents
        let automaton = std::iter::once(context_doc)
            .chain(sidecar_docs.iter())
            .find_map(|doc| doc.automata.iter().find(|a| a.name.name == *automaton_name))
            .ok_or_else(|| format!("automaton '{}' not found in documents", automaton_name))?;

        // Get the realized CLTS
        let clts = realized.context.clts(automaton_name).ok_or_else(|| {
            format!(
                "automaton '{}' not found in realized context",
                automaton_name
            )
        })?;

        // Collect variable names
        let var_names: Vec<String> = automaton
            .variables
            .iter()
            .map(|v| v.name.name.clone())
            .collect();

        // Collect action names with controllability
        let mut action_info: Vec<String> = Vec::new();
        for label_ref in &automaton.alphabet {
            let label_name = &label_ref.name.name;
            let is_controllable = automaton
                .controllable
                .iter()
                .any(|c| c.name.name == *label_name);
            let is_internal = automaton
                .internal
                .iter()
                .any(|i| i.name.name == *label_name);

            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };
            action_info.push(format!("{} ({})", label_name, action_type));
        }

        // Create automaton compound node
        let automaton_id = automaton_name.clone();
        elements.push(CytoscapeElement {
            data: CytoscapeData::Node {
                id: automaton_id.clone(),
                parent: None,
                label: Some(format!("Automaton {}", automaton_name)),
                vars: if var_names.is_empty() {
                    None
                } else {
                    Some(json!(var_names))
                },
                actions: if action_info.is_empty() {
                    None
                } else {
                    Some(json!(action_info))
                },
                note: None,
                isStart: None,
                isDead: None,
            },
            position: None,
            classes: None,
        });

        // Collect states and their positions
        let mut state_positions: HashMap<String, (f64, f64)> = HashMap::new();
        let mut x_pos = 100.0;

        for state_decl in &automaton.states {
            let state_name = &state_decl.name.name;
            let state_id = format!("{}_{}", automaton_name, state_name);
            let is_initial = state_decl.is_initial;

            // Check if state is terminal (dead) - states with no outgoing transitions
            let is_dead = clts
                .state_id(state_name)
                .map(|sid| clts.outgoing(sid).is_empty())
                .unwrap_or(false);

            // Get variable values for this state
            let state_var_str = if var_names.is_empty() {
                None
            } else {
                clts.state_id(state_name).ok().and_then(|sid| {
                    let vars = clts.state_variables(sid);
                    if vars.is_empty() {
                        None
                    } else {
                        Some(vars.join(", "))
                    }
                })
            };

            let position = CytoscapePosition {
                x: x_pos,
                y: y_offset + 100.0,
            };
            state_positions.insert(state_name.clone(), (x_pos, y_offset + 100.0));

            let mut classes = vec!["state"];
            if is_initial {
                classes.push("start");
            }
            if is_dead {
                classes.push("dead");
            }

            elements.push(CytoscapeElement {
                data: CytoscapeData::Node {
                    id: state_id.clone(),
                    parent: Some(automaton_id.clone()),
                    label: Some(format!("{}_{}", automaton_name, state_name)),
                    vars: state_var_str.map(|s| json!(s)),
                    actions: None,
                    note: if is_initial {
                        Some("Initial state".to_string())
                    } else if is_dead {
                        Some("Terminal state".to_string())
                    } else {
                        None
                    },
                    isStart: Some(is_initial),
                    isDead: Some(is_dead),
                },
                position: Some(position),
                classes: Some(classes.join(" ")),
            });

            // Add entry arrow for initial states
            if is_initial {
                let entry_id = format!("{}_entry", automaton_name);
                elements.push(CytoscapeElement {
                    data: CytoscapeData::Node {
                        id: entry_id.clone(),
                        parent: Some(automaton_id.clone()),
                        label: None,
                        vars: None,
                        actions: None,
                        note: None,
                        isStart: None,
                        isDead: None,
                    },
                    position: Some(CytoscapePosition {
                        x: 40.0,
                        y: y_offset + 100.0,
                    }),
                    classes: Some("entry".to_string()),
                });

                elements.push(CytoscapeElement {
                    data: CytoscapeData::Edge {
                        id: format!("{}_entry_edge", automaton_name),
                        source: entry_id,
                        target: state_id.clone(),
                        label: None,
                        action: None,
                        actionType: Some("start-arrow".to_string()),
                        guard: None,
                        effect: None,
                    },
                    position: None,
                    classes: None,
                });
            }

            x_pos += state_spacing;
        }

        // Add transitions
        for transition in &automaton.transitions {
            let source_name = match &transition.source {
                StateSelector::Named(state_ref) => match state_ref {
                    StateRef::Simple(ident) => ident.name.clone(),
                    StateRef::Indexed { name, .. } => name.name.clone(),
                },
                _ => continue, // Skip group/wildcard selectors for now
            };

            let target_name = match &transition.target {
                StateSelector::Named(state_ref) => match state_ref {
                    StateRef::Simple(ident) => ident.name.clone(),
                    StateRef::Indexed { name, .. } => name.name.clone(),
                },
                _ => continue, // Skip group/wildcard selectors for now
            };

            let primary_label = match &transition.label {
                TransitionLabel::Named { name, .. } => name.name.clone(),
                TransitionLabel::Epsilon(_) => "ε".to_string(),
            };
            let mut all_label_names = vec![primary_label.clone()];
            for additional in &transition.additional_labels {
                if let TransitionLabel::Named { name, .. } = additional {
                    all_label_names.push(name.name.clone());
                }
            }
            let label_name = all_label_names.join(", ");

            // Determine action type
            let is_controllable = all_label_names
                .iter()
                .any(|l| automaton.controllable.iter().any(|c| c.name.name == *l));
            let is_internal = all_label_names
                .iter()
                .any(|l| automaton.internal.iter().any(|i| i.name.name == *l));
            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };

            // Format guard
            let guard_str = transition
                .guard
                .as_ref()
                .map(expr_to_string)
                .unwrap_or_default();

            // Format effects
            let effect_str = if transition.effects.is_empty() {
                String::new()
            } else {
                transition
                    .effects
                    .iter()
                    .map(|a| format!("{}' = {}", a.target.name, expr_to_string(&a.expr)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            // Build label
            let mut label_parts = vec![label_name.clone()];
            if !guard_str.is_empty() {
                label_parts.push(format!("[{}]", guard_str));
            }
            let effect_str_for_label = effect_str.clone();
            if !effect_str_for_label.is_empty() {
                label_parts.push(effect_str_for_label);
            }
            let transition_label = label_parts.join("\n");

            let transition_id = format!("{}_{}_t{}", automaton_name, source_name, elements.len());
            let source_id = format!("{}_{}", automaton_name, source_name);
            let target_id = format!("{}_{}", automaton_name, target_name);

            elements.push(CytoscapeElement {
                data: CytoscapeData::Edge {
                    id: transition_id,
                    source: source_id,
                    target: target_id,
                    label: Some(transition_label),
                    action: Some(label_name),
                    actionType: Some(action_type.to_string()),
                    guard: if guard_str.is_empty() {
                        None
                    } else {
                        Some(guard_str)
                    },
                    effect: if effect_str.is_empty() {
                        None
                    } else {
                        Some(effect_str)
                    },
                },
                position: None,
                classes: None,
            });
        }

        // Move to next automaton row
        y_offset += 200.0;
    }

    Ok(elements)
}

fn unrolled_automata_to_cytoscape(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automata_names: &[String],
) -> Result<Vec<CytoscapeElement>, String> {
    let mut elements = Vec::new();
    let mut y_offset = 0.0;
    let state_spacing = 250.0;

    for automaton_name in automata_names {
        // Find the automaton in documents
        let automaton = std::iter::once(context_doc)
            .chain(sidecar_docs.iter())
            .find_map(|doc| doc.automata.iter().find(|a| a.name.name == *automaton_name))
            .ok_or_else(|| format!("automaton '{}' not found in documents", automaton_name))?;

        // Get the realized CLTS for reference
        let _clts = realized.context.clts(automaton_name).ok_or_else(|| {
            format!(
                "automaton '{}' not found in realized context",
                automaton_name
            )
        })?;

        // Convert DSL automaton to unrolling format
        let original_states: Vec<OriginalState> = automaton
            .states
            .iter()
            .map(|s| OriginalState {
                name: s.name.name.clone(),
                initial: s.is_initial,
            })
            .collect();

        let original_transitions: Vec<OriginalTransition> = automaton
            .transitions
            .iter()
            .filter_map(|t| {
                let source_name = match &t.source {
                    StateSelector::Named(state_ref) => match state_ref {
                        StateRef::Simple(ident) => ident.name.clone(),
                        StateRef::Indexed { name, .. } => name.name.clone(),
                    },
                    _ => return None, // Skip group/wildcard selectors
                };

                let target_name = match &t.target {
                    StateSelector::Named(state_ref) => match state_ref {
                        StateRef::Simple(ident) => ident.name.clone(),
                        StateRef::Indexed { name, .. } => name.name.clone(),
                    },
                    _ => return None, // Skip group/wildcard selectors
                };

                let label_name = match &t.label {
                    TransitionLabel::Named { name, .. } => name.name.clone(),
                    TransitionLabel::Epsilon(_) => "ε".to_string(),
                };

                let guard_str = t
                    .guard
                    .as_ref()
                    .map(|e| strip_outer_parens(&expr_to_string(e)))
                    .unwrap_or_default();

                let effects: Vec<Effect> = t
                    .effects
                    .iter()
                    .map(|a| Effect {
                        target: a.target.name.clone(),
                        value_expr: strip_outer_parens(&expr_to_string(&a.expr)),
                    })
                    .collect();

                Some(OriginalTransition {
                    from: source_name,
                    to: target_name,
                    label: label_name,
                    guard: if guard_str.is_empty() {
                        None
                    } else {
                        Some(guard_str)
                    },
                    effects,
                })
            })
            .collect();

        // Check if automaton has variables to unroll
        if automaton.variables.is_empty() {
            return Err(format!(
                "automaton '{}' has no variables to unroll. Unrolling requires variable declarations in the DSL.",
                automaton_name
            ));
        }

        let variables: Vec<VariableDecl> = automaton
            .variables
            .iter()
            .map(|v| {
                // Extract literal value from expression for initial value
                let initial_str = extract_literal_value(&v.init, &v.ty)
                    .unwrap_or_else(|| strip_outer_parens(&expr_to_string(&v.init)));

                VariableDecl {
                    name: v.name.name.clone(),
                    ty: match &v.ty {
                        mununu_core::context_dsl::ast::TypeName::Bool => "bool".to_string(),
                        mununu_core::context_dsl::ast::TypeName::I64 => "i64".to_string(),
                        mununu_core::context_dsl::ast::TypeName::Enum(_) => "i64".to_string(),
                    },
                    initial: Some(initial_str),
                }
            })
            .collect();

        // Perform unrolling with default options
        // The unrolling algorithm will handle state space explosion by applying
        // interval abstraction and widening when approaching limits
        let unrolling_options = UnrollingOptions::default();

        let unrolled = unroll_states(
            original_states,
            original_transitions,
            variables,
            unrolling_options,
        )
        .map_err(|e| format!("failed to unroll automaton '{}': {}", automaton_name, e))?;

        // Collect variable names for display
        let var_names: Vec<String> = automaton
            .variables
            .iter()
            .map(|v| v.name.name.clone())
            .collect();

        // Collect action names
        let mut action_info: Vec<String> = Vec::new();
        for label_ref in &automaton.alphabet {
            let label_name = &label_ref.name.name;
            let is_controllable = automaton
                .controllable
                .iter()
                .any(|c| c.name.name == *label_name);
            let is_internal = automaton
                .internal
                .iter()
                .any(|i| i.name.name == *label_name);

            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };
            action_info.push(format!("{} ({})", label_name, action_type));
        }

        // Create automaton compound node (with "Unrolled" suffix)
        let automaton_id = format!("{}_unrolled", automaton_name);
        elements.push(CytoscapeElement {
            data: CytoscapeData::Node {
                id: automaton_id.clone(),
                parent: None,
                label: Some(format!("Automaton {} (Unrolled)", automaton_name)),
                vars: if var_names.is_empty() {
                    None
                } else {
                    Some(json!(var_names))
                },
                actions: if action_info.is_empty() {
                    None
                } else {
                    Some(json!(action_info))
                },
                note: None,
                isStart: None,
                isDead: None,
            },
            position: None,
            classes: None,
        });

        // Collect states and their positions
        let mut state_positions: HashMap<String, (f64, f64)> = HashMap::new();
        let mut x_pos = 100.0;
        let mut initial_states = HashSet::new();

        // Find initial states - states at initial locations with initial variable values
        let initial_location_names: HashSet<String> = automaton
            .states
            .iter()
            .filter(|s| s.is_initial)
            .map(|s| s.name.name.clone())
            .collect();

        // Collect states that have incoming transitions to determine which are initial
        let states_with_incoming: HashSet<String> = unrolled
            .transitions
            .iter()
            .map(|t| t.to.state_name())
            .collect();

        for state in &unrolled.states {
            // A state is initial if:
            // 1. It's at an initial location, AND
            // 2. It has no incoming transitions (it's a true initial state)
            if initial_location_names.contains(&state.location) {
                let state_name = state.state_name();
                if !states_with_incoming.contains(&state_name) {
                    initial_states.insert(state_name);
                }
            }
        }

        // Create states
        for state in &unrolled.states {
            let state_name = state.state_name();
            let state_id = format!("{}_unrolled_{}", automaton_name, state_name);
            let is_initial = initial_states.contains(&state_name);

            // Check if state is terminal (dead) - states with no outgoing transitions
            let is_dead = unrolled
                .transitions
                .iter()
                .all(|t| t.from.state_name() != state_name);

            // Get variable values for this state
            let state_var_str = if state.variables.is_empty() {
                None
            } else {
                let var_parts: Vec<String> = state
                    .variables
                    .iter()
                    .map(|(name, value)| format!("{} = {}", name, value))
                    .collect();
                Some(var_parts.join(", "))
            };

            let position = CytoscapePosition {
                x: x_pos,
                y: y_offset + 100.0,
            };
            state_positions.insert(state_name.clone(), (x_pos, y_offset + 100.0));

            let mut classes = vec!["state"];
            if is_initial {
                classes.push("start");
            }
            if is_dead {
                classes.push("dead");
            }

            elements.push(CytoscapeElement {
                data: CytoscapeData::Node {
                    id: state_id.clone(),
                    parent: Some(automaton_id.clone()),
                    label: Some(state_name.clone()),
                    vars: state_var_str.map(|s| json!(s)),
                    actions: None,
                    note: if is_initial {
                        Some("Initial state".to_string())
                    } else if is_dead {
                        Some("Terminal state".to_string())
                    } else {
                        None
                    },
                    isStart: Some(is_initial),
                    isDead: Some(is_dead),
                },
                position: Some(position),
                classes: Some(classes.join(" ")),
            });

            // Add entry arrow for initial states
            if is_initial {
                let entry_id = format!("{}_unrolled_entry_{}", automaton_name, state_name);
                elements.push(CytoscapeElement {
                    data: CytoscapeData::Node {
                        id: entry_id.clone(),
                        parent: Some(automaton_id.clone()),
                        label: None,
                        vars: None,
                        actions: None,
                        note: None,
                        isStart: None,
                        isDead: None,
                    },
                    position: Some(CytoscapePosition {
                        x: 40.0,
                        y: y_offset + 100.0,
                    }),
                    classes: Some("entry".to_string()),
                });

                elements.push(CytoscapeElement {
                    data: CytoscapeData::Edge {
                        id: format!("{}_unrolled_entry_edge_{}", automaton_name, state_name),
                        source: entry_id,
                        target: state_id.clone(),
                        label: None,
                        action: None,
                        actionType: Some("start-arrow".to_string()),
                        guard: None,
                        effect: None,
                    },
                    position: None,
                    classes: None,
                });
            }

            x_pos += state_spacing;
        }

        // Add transitions
        for (idx, transition) in unrolled.transitions.iter().enumerate() {
            let from_name = transition.from.state_name();
            let to_name = transition.to.state_name();
            let label_name = transition.label.clone();

            // Determine action type
            let is_controllable = automaton
                .controllable
                .iter()
                .any(|c| c.name.name == label_name);
            let is_internal = automaton.internal.iter().any(|i| i.name.name == label_name);
            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };

            let transition_id = format!("{}_unrolled_t{}", automaton_name, idx);
            let source_id = format!("{}_unrolled_{}", automaton_name, from_name);
            let target_id = format!("{}_unrolled_{}", automaton_name, to_name);

            elements.push(CytoscapeElement {
                data: CytoscapeData::Edge {
                    id: transition_id,
                    source: source_id,
                    target: target_id,
                    label: Some(label_name.clone()),
                    action: Some(label_name),
                    actionType: Some(action_type.to_string()),
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
        }

        // Move to next automaton row
        y_offset += 200.0;
    }

    Ok(elements)
}

fn generate_cytoscape_html(elements: &[CytoscapeElement]) -> Result<String, String> {
    let elements_json = serde_json::to_string(elements)
        .map_err(|e| format!("failed to serialize elements: {}", e))?;

    let html_template = r###"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <title>Cytoscape Automata Visualization</title>
  <style>
    html, body {
      margin: 0;
      padding: 0;
      height: 100%;
      overflow: hidden;
      font-family: system-ui, sans-serif;
    }
    #cy {
      width: 100%;
      height: 100%;
      display: block;
    }
  </style>
</head>
<body>
  <div id="cy"></div>

  <script src="https://unpkg.com/cytoscape@3.30.0/dist/cytoscape.min.js"></script>
  <script src="https://unpkg.com/dagre@0.8.5/dist/dagre.min.js"></script>
  <script src="https://unpkg.com/cytoscape-dagre@2.5.0/cytoscape-dagre.js"></script>
  <script>
    // Register dagre extension
    cytoscape.use(cytoscapeDagre);

    const elements = ELEMENTS_PLACEHOLDER;

    const cy = cytoscape({
      container: document.getElementById("cy"),
      elements,
      style: [
        {
          selector: "node[^parent]",
          style: {
            "shape": "round-rectangle",
            "background-opacity": 0,
            "border-width": 2,
            "border-style": "dashed",
            "border-color": "#000",
            "padding": "40px",
            "label": ele => {
              const v = (ele.data("vars") || []).join(", ");
              const a = (ele.data("actions") || []).join(", ");
              return ele.data("label") + "\n" + "vars: " + v + "\n" + "actions: " + a;
            },
            "font-size": 11,
            "text-wrap": "wrap",
            "text-max-width": 220,
            "text-halign": "left",
            "text-valign": "top",
            "text-margin-x": 10,
            "text-margin-y": 10,
            "text-background-opacity": 1,
            "text-background-color": "#ffffff",
            "text-background-shape": "round-rectangle",
            "text-outline-width": 0
          }
        },
        {
          selector: "node.state",
          style: {
            "shape": "ellipse",
            "background-color": "#ffffff",
            "border-width": 1,
            "border-style": "solid",
            "border-color": "#000000",
            "width": 60,
            "height": 60,
            "label": "data(label)",
            "font-size": 12,
            "text-wrap": "wrap",
            "text-max-width": 70,
            "text-halign": "center",
            "text-valign": "center"
          }
        },
        {
          selector: "node.start",
          style: {
            "border-width": 4,
            "border-style": "double"
          }
        },
        {
          selector: "node.dead",
          style: {
            "shape": "round-rectangle",
            "border-width": 3,
            "border-style": "solid"
          }
        },
        {
          selector: "node.entry",
          style: {
            "width": 1,
            "height": 1,
            "opacity": 0
          }
        },
        {
          selector: "edge[actionType = 'start-arrow']",
          style: {
            "curve-style": "unbundled-bezier",
            "control-point-distances": [40],
            "control-point-weights": [0.5],
            "line-color": "#000",
            "target-arrow-color": "#000",
            "target-arrow-shape": "triangle",
            "width": 2
          }
        },
        {
          selector: "edge:not([actionType = 'start-arrow'])",
          style: {
            "curve-style": "unbundled-bezier",
            "control-point-distances": [60],
            "control-point-weights": [0.5],
            "line-color": "#000000",
            "target-arrow-color": "#000000",
            "target-arrow-shape": "triangle",
            "width": 2,
            "label": "data(label)",
            "font-size": 11,
            "text-wrap": "wrap",
            "text-max-width": 140,
            "text-background-opacity": 1,
            "text-background-color": "#ffffff",
            "text-background-shape": "round-rectangle",
            "text-margin-y": -6
          }
        },
        {
          selector: "edge[actionType = 'controllable']",
          style: {
            "line-style": "solid",
            "target-arrow-shape": "triangle"
          }
        },
        {
          selector: "edge[actionType = 'uncontrollable']",
          style: {
            "line-style": "dashed",
            "target-arrow-shape": "vee"
          }
        },
        {
          selector: "edge[actionType = 'internal']",
          style: {
            "line-style": "dotted",
            "target-arrow-shape": "triangle"
          }
        }
      ],
      layout: {
        name: "dagre",
        rankDir: "TB",
        spacingFactor: 1.25,
        nodeSep: 50,
        edgeSep: 20,
        rankSep: 80,
        padding: 40,
        animate: true,
        animationDuration: 1000,
        animationEasing: "ease-out"
      }
    });
  </script>
</body>
</html>"###;

    let html = html_template.replace("ELEMENTS_PLACEHOLDER", &elements_json);
    Ok(html)
}

/// Strips outer parentheses from an expression string if they wrap the entire expression.
/// This is needed because the unrolling parser expects simple expressions without
/// unnecessary parentheses. Recursively strips multiple layers of outer parentheses.
fn strip_outer_parens(s: &str) -> String {
    let mut trimmed = s.trim();
    loop {
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            // Check if the parentheses are balanced and wrap the entire expression
            let mut depth = 0;
            let mut found_outer = false;
            let mut is_fully_wrapped = false;

            for (i, ch) in trimmed.chars().enumerate() {
                match ch {
                    '(' => {
                        depth += 1;
                        if i == 0 {
                            found_outer = true;
                        }
                    }
                    ')' => {
                        depth -= 1;
                        if depth == 0 && i == trimmed.len() - 1 && found_outer {
                            // The outer parentheses wrap the entire expression
                            is_fully_wrapped = true;
                            break;
                        }
                        if depth < 0 {
                            // Unbalanced, return original
                            return trimmed.to_string();
                        }
                    }
                    _ => {}
                }
            }

            if is_fully_wrapped {
                // Strip one layer and continue
                trimmed = trimmed[1..trimmed.len() - 1].trim();
            } else {
                // Not fully wrapped, return as is
                break;
            }
        } else {
            // No outer parentheses, we're done
            break;
        }
    }
    trimmed.to_string()
}

/// Extracts a literal value from an expression if it's a simple constant.
/// Returns None if the expression is not a simple constant.
fn extract_literal_value(
    expr: &Expr,
    ty: &mununu_core::context_dsl::ast::TypeName,
) -> Option<String> {
    match &expr.kind {
        ExprKind::Integer(value) => Some(value.to_string()),
        ExprKind::Ident(ident) => {
            // Check for boolean literals
            if matches!(ty, mununu_core::context_dsl::ast::TypeName::Bool) {
                if ident.name.eq_ignore_ascii_case("true") {
                    return Some("true".to_string());
                } else if ident.name.eq_ignore_ascii_case("false") {
                    return Some("false".to_string());
                }
            }
            None
        }
        ExprKind::Group(inner) => extract_literal_value(inner, ty),
        _ => None, // Complex expressions can't be extracted as literals
    }
}

// Helper function to convert Expr to string
// For unrolling, we want minimal parentheses to avoid parsing issues
fn expr_to_string(expr: &Expr) -> String {
    expr_to_string_inner(expr, false)
}

// Internal function with precedence tracking to minimize parentheses
fn expr_to_string_inner(expr: &Expr, in_binary: bool) -> String {
    match &expr.kind {
        ExprKind::Integer(value) => value.to_string(),
        ExprKind::Ident(ident) => {
            // Check if identifier is a boolean literal keyword
            if ident.name.eq_ignore_ascii_case("true") {
                "true".to_string()
            } else if ident.name.eq_ignore_ascii_case("false") {
                "false".to_string()
            } else {
                ident.name.clone()
            }
        }
        ExprKind::Index {
            target,
            expr: idx_expr,
        } => {
            format!("{}[{}]", target.name, expr_to_string_inner(idx_expr, false))
        }
        ExprKind::Unary { op, expr: inner } => {
            let op_str = match op {
                UnaryOp::Not => "!",
                UnaryOp::Neg => "-",
            };
            format!("{}{}", op_str, expr_to_string_inner(inner, true))
        }
        ExprKind::Binary { left, op, right } => {
            let op_str = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
            };
            // For unrolling, we want minimal parentheses - only wrap if needed
            // Comparison operators have lower precedence, so we don't need parentheses for them
            let needs_parens = in_binary && matches!(op, BinaryOp::Add | BinaryOp::Sub);
            let left_str = expr_to_string_inner(
                left,
                matches!(op, BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod),
            );
            let right_str = expr_to_string_inner(
                right,
                matches!(op, BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod),
            );

            if needs_parens {
                format!("({}{}{})", left_str, op_str, right_str)
            } else {
                format!("{}{}{}", left_str, op_str, right_str)
            }
        }
        ExprKind::Group(inner) => expr_to_string_inner(inner, in_binary),
    }
}

#[derive(Serialize)]
struct ControllerSynthesisJson {
    automaton: String,
    formula: String,
    realizable: bool,
    controller: ControllerJson,
    diagnostics: Value,
}

#[derive(Serialize)]
struct ControllerJson {
    state_count: usize,
    initial_state_count: usize,
    structural_hash: u64,
    controllable_alphabet: Vec<String>,
    states: Vec<ControllerStateJson>,
    transitions: Vec<ControllerTransitionJson>,
}

#[derive(Serialize)]
struct ControllerStateJson {
    name: String,
    initial: bool,
    variables: Vec<String>,
}

#[derive(Serialize)]
struct ControllerTransitionJson {
    source: String,
    target: String,
    controllable: bool,
    labels: Vec<Vec<String>>,
}

fn write_controller_json(
    path: &Path,
    automaton: &str,
    formula: &str,
    synthesis: &ControllerSynthesis,
) -> Result<(), String> {
    ensure_parent_dir(path)
        .map_err(|err| format!("failed to prepare JSON output directory: {err}"))?;

    let controller = &synthesis.controller;
    let mut controllable_alphabet = controller.alphabet();
    controllable_alphabet.sort();

    let initial_states = controller.initial_states();
    let mut states = Vec::new();
    for state in controller.states() {
        let idx = state.index();
        let name = controller
            .state_name(state)
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("state_{idx}"));
        let variables = controller.state_variables(state);
        states.push(ControllerStateJson {
            name,
            initial: initial_states.contains(&state),
            variables,
        });
    }
    states.sort_by(|a, b| a.name.cmp(&b.name));

    let mut transitions = Vec::new();
    for state in controller.states() {
        let source_name = controller
            .state_name(state)
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("state_{}", state.index()));
        for transition in controller.outgoing(state) {
            let target = transition.target();
            let target_name = controller
                .state_name(target)
                .map(|name| name.to_owned())
                .unwrap_or_else(|| format!("state_{}", target.index()));
            let mut labels: Vec<Vec<String>> = Vec::new();
            for label_id in transition.labels() {
                let mut payload = controller
                    .label_payload(*label_id)
                    .map(|values| values.to_vec())
                    .unwrap_or_default();
                payload.sort();
                labels.push(payload);
            }
            transitions.push(ControllerTransitionJson {
                source: source_name.clone(),
                target: target_name,
                controllable: transition.is_controllable(controller),
                labels,
            });
        }
    }
    transitions.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.target.cmp(&b.target))
    });

    let payload = ControllerSynthesisJson {
        automaton: automaton.to_owned(),
        formula: formula.to_owned(),
        realizable: synthesis.realizable,
        controller: ControllerJson {
            state_count: controller.state_count(),
            initial_state_count: initial_states.len(),
            structural_hash: controller.structural_hash(),
            controllable_alphabet,
            states,
            transitions,
        },
        diagnostics: synthesis.diagnostics.to_json_value(),
    };

    let rendered = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialise controller summary: {err}"))?;
    fs::write(path, rendered).map_err(|err| format!("failed to write controller JSON: {err}"))?;
    Ok(())
}

fn write_controller_ctxdsl(
    path: &Path,
    automaton: &str,
    formula: &str,
    raw_formula: &str,
    controller: &Clts,
) -> Result<(), String> {
    ensure_parent_dir(path)
        .map_err(|err| format!("failed to prepare DSL output directory: {err}"))?;

    let mut ordered_labels: BTreeMap<usize, LabelId<DefaultLabelIdx>> = BTreeMap::new();
    for state in controller.states() {
        for transition in controller.outgoing(state) {
            for label_id in transition.labels() {
                ordered_labels.entry(label_id.index()).or_insert(*label_id);
            }
        }
    }

    let mut label_names = HashMap::new();
    let mut label_entries = Vec::new();
    let mut seen_label_idents = HashSet::new();
    for (index, label_id) in ordered_labels {
        let mut payload = controller
            .label_payload(label_id)
            .map(|values| values.to_vec())
            .unwrap_or_default();
        payload.sort();
        let base = if payload.is_empty() {
            format!("label_{index}")
        } else {
            payload.join("_")
        };
        let mut ident = sanitize_identifier_cli(&base);
        if ident.is_empty() {
            ident = format!("label_{index}");
        }
        let original_ident = ident.clone();
        if !seen_label_idents.insert(ident.clone()) {
            let mut counter = 1usize;
            loop {
                let candidate = format!("{original_ident}_{counter}");
                if seen_label_idents.insert(candidate.clone()) {
                    ident = candidate;
                    break;
                }
                counter += 1;
            }
        }
        label_names.insert(label_id, ident.clone());
        label_entries.push((label_id, ident, payload));
    }

    let mut state_idents = Vec::with_capacity(controller.state_count());
    let mut raw_state_names = Vec::with_capacity(controller.state_count());
    let mut seen_state_idents = HashSet::new();
    for state in controller.states() {
        let idx = state.index();
        if state_idents.len() <= idx {
            state_idents.resize(idx + 1, String::new());
            raw_state_names.resize(idx + 1, String::new());
        }
        let raw = controller
            .state_name(state)
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("state_{idx}"));
        let mut ident = sanitize_identifier_cli(&raw);
        if ident.is_empty() {
            ident = format!("state_{idx}");
        }
        let original_ident = ident.clone();
        if !seen_state_idents.insert(ident.clone()) {
            let mut counter = 1usize;
            loop {
                let candidate = format!("{original_ident}_{counter}");
                if seen_state_idents.insert(candidate.clone()) {
                    ident = candidate;
                    break;
                }
                counter += 1;
            }
        }
        state_idents[idx] = ident;
        raw_state_names[idx] = raw;
    }

    let context_ident = sanitize_identifier_cli(&format!("{}_{}_controller", automaton, formula));
    let automaton_ident = format!("{context_ident}_automaton");

    let file = fs::File::create(path)
        .map_err(|err| format!("failed to create controller DSL file: {err}"))?;
    let mut writer = io::BufWriter::new(file);
    writeln!(
        writer,
        "// Synthesised controller derived from automaton '{}' and formula '{}'",
        automaton, formula
    )
    .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "context {context_ident} {{")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;

    if !label_entries.is_empty() {
        writeln!(writer, "    alphabet {{")
            .map_err(|err| format!("failed to write controller DSL: {err}"))?;
        for (_, ident, payload) in &label_entries {
            if payload.is_empty() {
                writeln!(writer, "        label {ident}; // ε")
                    .map_err(|err| format!("failed to write controller DSL: {err}"))?;
            } else {
                writeln!(
                    writer,
                    "        label {ident}; // original symbols: {}",
                    payload.join(", ")
                )
                .map_err(|err| format!("failed to write controller DSL: {err}"))?;
            }
        }
        writeln!(writer, "    }}")
            .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    }

    writeln!(writer, "    automata {{")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "        automaton {automaton_ident} {{")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "            states {{")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    for state in controller.states() {
        let idx = state.index();
        let name = &state_idents[idx];
        let raw = &raw_state_names[idx];
        let mut line = format!("                state {name}");
        if controller.initial_states().contains(&state) {
            line.push_str(" initial");
        }
        line.push(';');
        if raw != name {
            line.push_str(" // original: ");
            line.push_str(raw);
        }
        writeln!(writer, "{line}")
            .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    }
    writeln!(writer, "            }}")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "            transitions {{")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    for state in controller.states() {
        let source_name = &state_idents[state.index()];
        for transition in controller.outgoing(state) {
            let target_name = &state_idents[transition.target().index()];
            let mut clauses = Vec::new();
            for label_id in transition.labels() {
                if let Some(name) = label_names.get(label_id) {
                    clauses.push(format!("label {name}"));
                }
            }
            let labels_clause = if clauses.is_empty() {
                "epsilon".to_owned()
            } else {
                clauses.join(", ")
            };
            let mut line = format!(
                "                transition {source} -> {target} on {labels};",
                source = source_name,
                target = target_name,
                labels = labels_clause
            );
            if transition.is_controllable(controller) {
                line.push_str(" // controllable");
            } else {
                line.push_str(" // uncontrollable");
            }
            writeln!(writer, "{line}")
                .map_err(|err| format!("failed to write controller DSL: {err}"))?;
        }
    }
    writeln!(writer, "            }}")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "        }}")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "    }}").map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "    mu_formulas {{")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(
        writer,
        "        formula {} {{",
        sanitize_identifier_cli(formula)
    )
    .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "            over {automaton_ident};")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "            body = {raw_formula};")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "        }}")
        .map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "    }}").map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writeln!(writer, "}}").map_err(|err| format!("failed to write controller DSL: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("failed to finalise controller DSL: {err}"))?;
    Ok(())
}

fn sanitize_identifier_cli(value: &str) -> String {
    let mut ident: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if ident.is_empty() {
        ident.push_str("mediator");
    }
    if ident.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    ident
}
