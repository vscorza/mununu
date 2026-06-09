mod loader;
mod render;

use crate::loader::{
    EvalContextParams, PreparedEvalContext, load_context_documents, load_context_documents_mode,
    prepare_eval_context, print_context_structure, realize_documents, validate_preprocessor,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use mununu_core::clts::{Clts, DefaultLabelIdx, LabelId};
use mununu_core::context::{ControllerSynthesis, ControllerSynthesisOptions, DiagnosticsOptions};
use mununu_core::context_dsl::ast::TransitionModalitySpec;
use mununu_core::context_dsl::{ContextDoc, RealizedContext, parse as parse_context_doc};
use mununu_core::mu_calculus::EvaluationOptions;
use render::graph::{
    counterstrategy_to_cytoscape, dsl_automata_to_cytoscape, generate_cytoscape_html,
    unrolled_automata_to_cytoscape,
};
use render::text::{
    render_controller_diagnostics, render_discharge_verdict_text, render_proposal_provenance,
};
use serde::Serialize;
use serde_json::{self, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt, util::SubscriberInitExt};

// SV init helpers live in mununu-core::adapter::systemverilog::annotation
// (used via wildcard import inside sv_init/sv_init_multi functions)
use mununu_core::adapter::systemverilog::annotation as sv_annotation;

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
    /// SystemVerilog analysis tools (discover significant values).
    Sv {
        #[command(subcommand)]
        command: Box<SvCommand>,
    },
    /// List available property templates.
    Templates(TemplatesArgs),
    /// Browse / emit shipped parameterised CTXDSL component templates
    /// (PLIC, watchdog, tracked-memory).
    Library {
        #[command(subcommand)]
        command: Box<LibraryCommand>,
    },
    /// Contract assume-guarantee tooling (validate discharge graphs, etc.).
    Contract {
        #[command(subcommand)]
        command: Box<ContractCommand>,
    },
    /// HW/SW codesign tools — register-map sidecars + coupling synthesis.
    Codesign {
        #[command(subcommand)]
        command: Box<CodesignCommand>,
    },
    /// Run the general N-source verification framework against a
    /// `verify.toml` project config. Each source is dispatched
    /// through its adapter, alphabet bindings are applied,
    /// composition is realised, and every declared property is
    /// evaluated. See `crates/mununu-core/src/verify/` for the
    /// pipeline.
    Verify(VerifyArgs),
    /// Memory-soundness tooling — surface declared
    /// `[sources.memory_abstraction]` postures and flag property
    /// formulas that reference memory in ways the declared posture
    /// cannot soundly support. Advisory by default; `--strict`
    /// converts warnings into a non-zero exit.
    Memory {
        #[command(subcommand)]
        command: Box<MemoryCommand>,
    },
    /// Start HTTP API server
    Server {
        /// Server address (default: 127.0.0.1:8080)
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
    },
    /// BTOR2-direct analysis tools (Phase A.4 predicate-image discovery).
    Btor2 {
        #[command(subcommand)]
        command: Box<Btor2Command>,
    },
}

#[derive(Subcommand, Debug)]
enum Btor2Command {
    /// Run SMT predicate-image discovery on a BTOR2 file.
    ///
    /// For each user-named state cell, enumerate the values reachable
    /// in one transition step under any current state + input
    /// combination. Writes the resulting `discovered_values` map to
    /// the path-adjacent `<stem>.mununu.json` (or `--output` if
    /// provided), so the existing BTOR2 bit-blast / sidecar resolver
    /// pipeline picks the abstraction up without further wiring.
    ///
    /// See `docs/design/auto-extraction-architecture.md` §2 Stage 4
    /// for the soundness contract.
    Discover(Btor2DiscoverArgs),
    /// Lift a BTOR2 source into the KMTS-aware shape (R.2).
    ///
    /// Runs the existing 2-valued BTOR2 adapter and post-hoc
    /// enriches the output with per-state 3-valued AP labellings
    /// derived from the bit-blaster's `state_valuations`. Prints a
    /// summary JSON: number of predicates synthesised, total
    /// `(state, predicate)` labellings, per-automaton state count.
    /// Used by the R.2 fixture sweep to validate the lifter shape
    /// stays consistent across runs.
    LiftKmts(Btor2LiftKmtsArgs),
    /// R.5 Item 3 sub-item 3.5 (2026-06-04) — Run the R.5 CEGAR
    /// refinement loop on a BTOR2 fixture + μ-calculus formula.
    ///
    /// Selects a predicate-discovery source (`wp` heuristic by
    /// default, or `craig` for Craig interpolation via the
    /// CVC5 subprocess). When `craig` is selected but the CVC5
    /// binary isn't found at runtime, the loop emits a structured
    /// warning + falls back to the `wp` heuristic automatically.
    /// See `docs/external-tools.md` for CVC5 install instructions.
    Cegar(Btor2CegarArgs),
}

/// R.5 Item 3 sub-item 3.5 (2026-06-04) — predicate-source
/// selector for `mununu btor2 cegar`. Mirrors the values of
/// [`mununu_core::adapter::btor2::cegar::PredicateSource`] but
/// omits the `Manual` variant (callback-only; not selectable
/// from the CLI).
#[derive(Clone, Debug, Copy, clap::ValueEnum)]
enum PredicateSourceArg {
    /// Weakest-precondition heuristic (default). No external deps.
    Wp,
    /// Craig interpolation via CVC5 subprocess. Requires CVC5 ≥ 1.0
    /// installed locally; falls back to `wp` with a warning when
    /// the binary is absent.
    Craig,
}

/// R.2.5b session-1 follow-up (2026-06-06) — must-edge inference
/// selector for `mununu btor2 cegar`. Mirrors the values of
/// [`mununu_core::adapter::btor2::kmts_lift::MustEdgeInference`].
#[derive(Clone, Debug, Copy, clap::ValueEnum, Default)]
enum MustEdgeInferenceArg {
    /// Pre-R.2.5b behaviour (default). Only MayOnly edges emitted;
    /// no must / hyper-must inference.
    #[default]
    Off,
    /// Sampling-derived must-edge inference. The lifter's post-pass
    /// promotes MayOnly → Sharp when all sampled paths agree on
    /// a single target cube, and emits MustHyperOnly with the
    /// target set when paths diverge. SOUNDNESS: sampling-based;
    /// SMT-backed proof is queued for R.2.5b session 2. Verdicts
    /// depending on the inferred must-edges carry an
    /// `[R.2.5b-sampling-must]` AdapterWarning.
    SamplingConfluence,
    /// R.2.5b session 2 (2026-06-08). SMT-backed must-edge inference
    /// via Z3 BV theory using the **stronger ∀∀ form** (`∀ state.
    /// ∀ input. transition ⟹ next ⊨ tgt` — deterministic into tgt
    /// regardless of input). SOUNDNESS: SMT-proved (no sampling).
    /// Verdicts carry an `[R.2.5b-smt-must]` AdapterWarning.
    SmtPerTarget,
    /// R.2.5b session-2 follow-up (2026-06-09). SMT-backed must-edge
    /// inference using the **canonical ∀∃ form** (`∀ state ⊨ src.
    /// ∃ input. next ⊨ tgt`) via Z3 quantifier alternation. Strictly
    /// more permissive than `smt-per-target` (every ∀∀-Must is also
    /// ∀∃-Must; the ∀∃ form additionally promotes edges where SOME
    /// input per state reaches tgt). Verdicts carry an
    /// `[R.2.5b-smt-must-standard]` AdapterWarning.
    SmtPerTargetStandard,
    /// R.2.5b session-2 follow-up (2026-06-09). SMT-backed
    /// **hyper-must** inference. For each source cube, tries the
    /// per-target ∀∃ singleton checks first; if no singleton proves
    /// Must, runs a full-target-set ∀∃ check and emits
    /// `MustHyperOnly` with the full sampled target set on Must.
    /// Verdicts carry an `[R.2.5b-smt-must-hyper]` AdapterWarning.
    SmtHyperMust,
}

#[derive(Args, Debug)]
struct Btor2CegarArgs {
    /// Path to the BTOR2 input file.
    #[arg(value_name = "BTOR2_FILE")]
    file: PathBuf,
    /// μ-calculus formula to evaluate over the lifted KMTS.
    /// Example: `'nu X. < step > X'` for a simple liveness
    /// formula. Wrap in single quotes to avoid shell expansion.
    #[arg(long, value_name = "FORMULA")]
    formula: String,
    /// Initial predicate set as `name:register=value` triples.
    /// May be repeated. At least one is required to bootstrap
    /// the cube space.
    /// Example: `--predicate "p_idle:state=0" --predicate "p_busy:state=1"`.
    #[arg(long = "predicate", value_name = "NAME:REG=VALUE")]
    predicates: Vec<String>,
    /// Predicate-discovery source for the CEGAR loop.
    #[arg(long, value_enum, default_value_t = PredicateSourceArg::Wp)]
    predicate_source: PredicateSourceArg,
    /// Explicit CVC5 binary path (overrides `MUNUNU_CVC5_PATH`
    /// env var + `$PATH` discovery). Only consulted when
    /// `--predicate-source craig`.
    #[arg(long, value_name = "PATH")]
    cvc5_path: Option<PathBuf>,
    /// Maximum number of CEGAR refinement iterations before
    /// terminating with `BoundedIterationsReached`. Default 16.
    #[arg(long, default_value_t = 16)]
    max_iterations: usize,
    /// R.2.5b session-1 follow-up (2026-06-06) — must-edge
    /// inference policy for the per-iteration `predicate_cube_lift`.
    /// Defaults to `off`. Set to `sampling-confluence` to opt the
    /// lifter into sampling-derived Sharp / MustHyperOnly edge
    /// emission. The lift result then includes an
    /// `[R.2.5b-sampling-must]` AdapterWarning whenever the
    /// post-pass fires.
    #[arg(long, value_enum, default_value_t = MustEdgeInferenceArg::Off)]
    must_edge_inference: MustEdgeInferenceArg,
    /// R-S8 session 2 (2026-06-08) — under-constrained constant's
    /// admissible value set. Format: `REGISTER=v1,v2,v3`. May be
    /// repeated. Bridges to the R-Y7 symbolic-init path by
    /// expanding the predicate-cube lifter's initial-state set
    /// to all cubes admissible under the listed values.
    /// Example: `--config-values 'boot_fsm=0,1,2'
    /// --config-values 'mode=0,1'`.
    ///
    /// When a sidecar declares `signals[].config_values`, the
    /// sidecar value takes precedence for that register (the CLI
    /// flag's values are merged in; sidecar values override CLI
    /// values per the resolver's last-write-wins for the same
    /// key).
    #[arg(long = "config-values", value_name = "REG=v1,v2,...")]
    config_values: Vec<String>,
    /// Emit the trace summary as JSON on stdout instead of the
    /// human-readable format.
    #[arg(long)]
    json: bool,
    /// R.6.6 / V.6 (2026-06-09) — name of a BTOR2 input symbol the
    /// controller drives. Repeated to declare multiple controllable
    /// inputs. When non-empty, the predicate-cube lifter partitions
    /// boolean inputs into env (uncontrollable) + ctrl (controllable)
    /// classes per-symbol-name + emits per-combo dual-label
    /// transitions `[env_cN, ctrl_cM]` with appropriate
    /// `LabelControllability` tags. The Skolem grouping in the
    /// R.6.3 modal step then partitions correctly along the
    /// controllability axis (∀ env-combo, ∃ ctrl-combo for the
    /// synthesis idiom).
    ///
    /// When omitted (default empty): the lifter emits the legacy
    /// single-`step` label shape (pre-R.6.6 behaviour, no
    /// controllability axis on labels).
    ///
    /// Example: `--controllable-input ctrl_g0 --controllable-input ctrl_g1`.
    #[arg(long = "controllable-input", value_name = "INPUT_NAME")]
    controllable_inputs: Vec<String>,
}

#[derive(Args, Debug)]
struct Btor2DiscoverArgs {
    /// Path to the BTOR2 input file.
    #[arg(value_name = "BTOR2_FILE")]
    file: PathBuf,
    /// Where to write the updated sidecar. Defaults to
    /// `<stem>.mununu.json` next to the BTOR2 input.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
    /// Maximum number of distinct values to enumerate per state cell.
    /// Default matches `ImageOptions::default().cap_edges = 4096`.
    #[arg(long, default_value_t = 4096)]
    cap_edges: usize,
}

#[derive(Args, Debug)]
struct Btor2LiftKmtsArgs {
    /// Path to the BTOR2 input file.
    #[arg(value_name = "BTOR2_FILE")]
    file: PathBuf,
    /// Cap on the number of predicates the lifter synthesises.
    /// Each `(register, value)` pair in the bit-blaster's state
    /// valuations becomes one predicate; `None` (the default)
    /// means no cap.
    #[arg(long, value_name = "N")]
    max_predicates: Option<usize>,
}

#[derive(Subcommand, Debug)]
enum MemoryCommand {
    /// Analyse a `verify.toml` for memory-abstraction-posture issues.
    Check(MemoryCheckArgs),
}

#[derive(Args, Debug)]
struct MemoryCheckArgs {
    /// Path to the `verify.toml` project config.
    #[arg(value_name = "VERIFY_TOML")]
    config: PathBuf,
    /// Emit the report as JSON on stdout instead of the
    /// human-readable summary.
    #[arg(long)]
    json: bool,
    /// Exit with a non-zero status if any warning is raised.
    #[arg(long)]
    strict: bool,
}

#[derive(Subcommand, Debug)]
enum LibraryCommand {
    /// List every shipped library template + a one-line summary.
    List,
    /// Emit one template's CTXDSL body to stdout (or `--output`).
    Emit(LibraryEmitArgs),
}

#[derive(Args, Debug)]
struct LibraryEmitArgs {
    /// Template name (e.g. `plic`, `watchdog`, `tracked_memory`).
    #[arg(value_name = "NAME")]
    name: String,
    /// Substitute `{instance_id}` with this value before emitting.
    /// When omitted, the placeholder is preserved verbatim — useful
    /// when feeding the output to a `[[sources]]` block with
    /// `count = N` (the verify framework substitutes per instance).
    #[arg(long, value_name = "ID")]
    instance_id: Option<String>,
    /// Output path. Defaults to stdout.
    #[arg(long, short = 'o', value_name = "PATH")]
    output: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum ContractCommand {
    /// Validate a contract set's discharge graph.
    Validate(ContractValidateArgs),
    /// Inspect a gap-marker report (diagnostics + sidecar + strict gate).
    Gaps(ContractGapsArgs),
    /// Discover a phase-1 contract from a black-box interface description.
    Discover(ContractDiscoverArgs),
    /// Emit interface + gap-report sidecars for a list of black-box modules.
    Sidecars(ContractSidecarsArgs),
    /// Query the contract corpus for matching entries.
    Query(ContractQueryArgs),
    /// HITL stage-4 — surface proposed clauses for human review.
    Review(ContractReviewArgs),
}

#[derive(Args, Debug)]
struct ContractReviewArgs {
    /// Path to the black-box interface JSON (Phase 1 discovery input).
    #[arg(value_name = "INTERFACE")]
    interface: PathBuf,
    /// Optional contract corpus root used to resolve
    /// `@mununu_interface contract://` URIs into reference proposals.
    #[arg(long, value_name = "DIR")]
    corpus: Option<PathBuf>,
    /// Emit the package as JSON to stdout.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct ContractQueryArgs {
    /// `<domain>/<name>` identifier.
    #[arg(value_name = "DOMAIN/NAME")]
    id: String,
    /// Corpus root directory.
    #[arg(long, value_name = "DIR", default_value = "corpus")]
    corpus: PathBuf,
    /// Parameters to match, as `key=jsonvalue` pairs.
    #[arg(long = "param", value_name = "KEY=VALUE", num_args = 0..)]
    params: Vec<String>,
    /// Output as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct VerifyArgs {
    /// Path to the `verify.toml` project config.
    #[arg(value_name = "VERIFY_TOML")]
    config: PathBuf,
    /// Override the base directory used to resolve relative paths in
    /// the config. Defaults to the config file's parent directory.
    #[arg(long, value_name = "DIR")]
    base_dir: Option<PathBuf>,
    /// Emit the full `VerifyReport` as JSON on stdout instead of the
    /// human-readable summary table.
    #[arg(long)]
    json: bool,
    /// Exit with a non-zero status if any property is unsatisfied.
    /// Default: exit 0 regardless of verdicts; the report distinguishes.
    #[arg(long)]
    strict: bool,
    /// Skip property evaluation and instead emit an introspection
    /// report: per-automaton alphabet + state names, the composition's
    /// union alphabet, and every declared per-state predicate. Use
    /// this before authoring property formulas to discover what
    /// labels and predicates the realized context actually exposes.
    /// Mutually exclusive with `--strict` (the report carries no
    /// verdicts to enforce against).
    #[arg(long = "print-alphabet")]
    print_alphabet: bool,
    /// For every violated property, print the counterexample witness
    /// attached by the orchestrator (initial state → labelled steps →
    /// termination). Default off so existing transcripts stay
    /// byte-stable; opt-in for debugging.
    #[arg(long = "print-counterexample")]
    print_counterexample: bool,
}

#[derive(Args, Debug)]
struct ContractSidecarsArgs {
    /// Path to a JSON file containing a list of `BlackBoxInterface` objects.
    #[arg(value_name = "INTERFACES")]
    interfaces: PathBuf,
    /// Directory to write the sidecar files; created if missing.
    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,
    /// Emit an additional `Fairness` gap marker per module.
    #[arg(long)]
    emit_fairness_gap: bool,
    /// Optional contract corpus root used to resolve
    /// `@mununu_interface contract://` URIs on each interface. When
    /// supplied, the resolution outcomes are embedded into the gap-
    /// report sidecar so HITL UX can show vendor-declared corpus refs.
    #[arg(long, value_name = "DIR")]
    corpus: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ContractValidateArgs {
    /// Path to the contract set JSON.
    #[arg(value_name = "CONTRACT_SET")]
    contract_set: PathBuf,
    /// Output the verdict as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct ContractDiscoverArgs {
    /// Path to the black-box interface JSON.
    #[arg(value_name = "INTERFACE")]
    interface: PathBuf,
    /// Force these labels to Controllable.
    #[arg(long, value_name = "LABELS")]
    force_controllable: Vec<String>,
    /// Force these labels to Uncontrollable.
    #[arg(long, value_name = "LABELS")]
    force_uncontrollable: Vec<String>,
    /// Emit an additional `Fairness` gap marker.
    #[arg(long)]
    emit_fairness_gap: bool,
    /// Output as JSON.
    #[arg(long)]
    json: bool,
    /// Fail with non-zero exit if any gap.
    #[arg(long)]
    strict_contracts: bool,
    /// Write `.contract.todo.json` sidecar next to the source.
    #[arg(long, value_name = "SOURCE")]
    write_sidecar: Option<PathBuf>,
    /// Optional contract corpus root used to resolve
    /// `@mununu_interface contract://` URIs on the interface.
    #[arg(long, value_name = "DIR")]
    corpus: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ContractGapsArgs {
    /// Path to the gap-marker report JSON.
    #[arg(value_name = "GAP_REPORT")]
    gap_report: PathBuf,
    /// Fail with a non-zero exit code if the report contains any gap.
    #[arg(long)]
    strict_contracts: bool,
    /// Write a `.contract.todo.json` skeleton next to the source file.
    #[arg(long, value_name = "SOURCE")]
    write_sidecar: Option<PathBuf>,
    /// Emit the report as JSON to stdout.
    #[arg(long)]
    json: bool,
}

// ============================================================================
// Codesign — Document C task C2 (slice 1)
// ============================================================================

#[derive(Subcommand, Debug)]
enum CodesignCommand {
    /// Emit the coupling CTXDSL fragment for a register-map sidecar.
    ///
    /// Reads the register-map JSON (Document C task C1 schema, see
    /// `tools/register_map_schema.json`) and emits a CTXDSL fragment
    /// containing the alphabet of rendezvous labels, a chaotic-stub
    /// peripheral automaton, and an asynchronous composition block.
    /// The user pastes the fragment into a hand-authored
    /// `context <name> { … }` block alongside their firmware
    /// automaton.
    Couple(CodesignCoupleArgs),
    /// Emit a standalone chaotic-stub CTXDSL document from a register-map
    /// sidecar.
    ///
    /// Generates the one-state-self-loops form: a single `Chaotic`
    /// state with self-loops on every rendezvous label derived from
    /// the register map. The output is a complete CTXDSL document
    /// (with its own `context { … }` wrapper) ready to reference as
    /// a `ctxdsl` source from a `verify.toml`. Sound for safety,
    /// optimistic for liveness (Doc C §C.5). See `docs/abstraction.md`.
    EmitChaoticStub(CodesignEmitChaoticStubArgs),
    /// Compose a register-map sidecar with a firmware CTXDSL and
    /// verify a property over the result.
    ///
    /// Document C task C4. Reads the register-map JSON and a
    /// firmware CTXDSL document, splices the coupling fragment into
    /// the firmware's outer context block, realises the composed
    /// document, and evaluates the named formula over the
    /// codesign-composed automaton. Counterexample classification
    /// via the C3 trace origin classifier is wired in too.
    Verify(CodesignVerifyArgs),
    /// Import a CMSIS-SVD file and emit one mununu register-map JSON
    /// per peripheral.
    ///
    /// Document C task C6. The `sv_signal` and `c_accessor` fields on
    /// each imported field start empty — CMSIS-SVD does not carry that
    /// information, and the user authors it post-import.
    ImportSvd(CodesignImportSvdArgs),
    /// Extract C function declarations + their `@mununu_*` annotations
    /// from a C source file via a `clang -ast-dump=json` shell-out.
    ///
    /// Document C task C5, slice 2.a. Emits one record per function
    /// (with attached annotations) plus a list of orphan annotations.
    /// Function bodies are NOT modelled into an automaton yet — that's
    /// slice 2.b.
    ExtractC(CodesignExtractCArgs),
    /// Emit a CMSIS-DEVICE-style C header for one peripheral from an
    /// SVD file. Phase L8 — lets `mununu codesign extract-c` consume
    /// upstream-style firmware C that uses `NRF_TWIM0->FIELD` struct
    /// member access. The output is one header file's worth of
    /// content; redirect to disk.
    EmitCmsisHeader(CodesignEmitCmsisHeaderArgs),
    /// Reconcile firmware and peripheral rendezvous-label alphabets.
    ///
    /// Reads two JSON files, each a `[ "label_1", "label_2", … ]`
    /// array. Returns the canonical shared alphabet when the two sets
    /// match exactly, or a structured mismatch report otherwise. This
    /// is the hard gate against alphabet drift between the
    /// C-extraction's firmware automaton and the SV-extraction's
    /// peripheral automaton (Doc C §C.5: silent over-approximation
    /// across a mismatched bus is unsound for safety).
    ReconcileLabels(CodesignReconcileLabelsArgs),
}

#[derive(Args, Debug)]
struct CodesignEmitCmsisHeaderArgs {
    /// Path to the CMSIS-SVD XML file. One of `--svd` or
    /// `--register-map` is required.
    #[arg(long, value_name = "SVD")]
    svd: Option<PathBuf>,
    /// Path to a register-map JSON sidecar (the format `mununu
    /// codesign couple` consumes). Use this when the SVD has
    /// vendor extensions the importer doesn't yet handle and the
    /// register map was authored / patched by hand.
    #[arg(long = "register-map", value_name = "JSON")]
    register_map: Option<PathBuf>,
    /// Peripheral name to emit. Default: all peripherals in the
    /// source (one struct + one macro per peripheral, concatenated).
    #[arg(long, value_name = "NAME")]
    peripheral: Option<String>,
    /// Vendor prefix prepended to peripheral names. E.g.
    /// `--vendor-prefix NRF_` produces `NRF_TWIM_Type` and `NRF_TWIM0`.
    #[arg(long = "vendor-prefix", value_name = "PREFIX", default_value = "")]
    vendor_prefix: String,
}

#[derive(Args, Debug)]
struct CodesignReconcileLabelsArgs {
    /// Path to the firmware-side label JSON (a `["label_1", …]` array).
    #[arg(value_name = "FIRMWARE_JSON")]
    firmware_labels: PathBuf,
    /// Path to the peripheral-side label JSON (same shape as
    /// `FIRMWARE_JSON`). Mutually exclusive with
    /// `--peripheral-register-map` — exactly one peripheral source
    /// must be supplied.
    #[arg(value_name = "PERIPHERAL_JSON", required = false)]
    peripheral_labels: Option<PathBuf>,
    /// Path to a register-map sidecar JSON. The peripheral-side
    /// alphabet is derived directly via
    /// `coupling::register_map_labels` (the same function the
    /// firmware emitter targets), so passing a register map here
    /// short-circuits any manual hand-authoring of the peripheral
    /// labels list. Mutually exclusive with `PERIPHERAL_JSON`.
    #[arg(long = "peripheral-register-map", value_name = "REGISTER_MAP_JSON")]
    peripheral_register_map: Option<PathBuf>,
    /// Output format. `human` (default): one section per outcome,
    /// human-readable. `json`: machine-parseable
    /// `{ "shared": […], "mismatch": null | { "firmware_only": […],
    /// "peripheral_only": […] } }`.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    format: String,
}

#[derive(Args, Debug)]
struct CodesignExtractCArgs {
    /// Path to the C source file (`.c` or `.h`).
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Path to the clang binary. Defaults to `clang` (PATH).
    #[arg(long, value_name = "PATH")]
    clang: Option<PathBuf>,
    /// Include paths (repeatable) passed to clang as `-I`.
    #[arg(long = "include", value_name = "DIR")]
    include_paths: Vec<PathBuf>,
    /// Preprocessor defines (repeatable), e.g. `--define HAVE_DMA=1`.
    #[arg(long = "define", value_name = "DEF")]
    defines: Vec<String>,
    /// Extra arguments to pass to clang verbatim (advanced).
    #[arg(long = "clang-arg", value_name = "ARG")]
    extra_clang_args: Vec<String>,
    /// Treat any warning (orphan annotation, unhandled kind) as a
    /// hard error.
    #[arg(long)]
    strict: bool,
    /// Path to a register-map JSON file. When supplied (slice 2.b),
    /// each function body is walked for register accesses and the
    /// matched accessors are emitted on the function's `accesses` field.
    #[arg(long = "register-map", value_name = "JSON")]
    register_map: Option<PathBuf>,
    /// When set together with `--register-map`, synthesise a linear
    /// CTXDSL automaton from each function's register-access sequence.
    /// The automaton is emitted on the function's `automaton_ctxdsl`
    /// field and synchronises with the peripheral chaotic stub on the
    /// `coupling`-module rendezvous labels.
    #[arg(long = "synthesize-automaton")]
    synthesize_automaton: bool,
    /// Phase L7: emit a top-level Driver automaton that non-
    /// deterministically dispatches to each non-ISR entry point.
    /// Useful for driver files with multiple entry points the
    /// application calls in arbitrary order. Disabled by default.
    #[arg(long = "driver-mode")]
    driver_mode: bool,
    /// Phase L8: include the bundled vendor-neutral CMSIS-minimal
    /// stubs (`__IO`, `__NOP`, NVIC no-ops, …) in the clang
    /// invocation. Use together with `--include` pointing at an
    /// SVD-derived CMSIS header (emit via `mununu codesign
    /// emit-cmsis-header`) when the C source uses
    /// `PERIPHERAL->FIELD` struct-member access.
    #[arg(long = "cmsis-stubs")]
    cmsis_stubs: bool,
}

#[derive(Args, Debug)]
struct CodesignImportSvdArgs {
    /// Path to the CMSIS-SVD XML file.
    #[arg(value_name = "SVD")]
    svd: PathBuf,
    /// Output directory for the imported register-map JSON files.
    /// One file per peripheral: `<peripheral>.json`. Default: write
    /// JSON to stdout (no files created).
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
    /// Import only the named peripheral (case-sensitive). Default:
    /// import every peripheral in the SVD.
    #[arg(long, value_name = "NAME")]
    peripheral: Option<String>,
    /// Treat any structural warning (derivedFrom not resolved,
    /// register array not expanded, unknown field access) as a hard
    /// error.
    #[arg(long)]
    strict: bool,
}

#[derive(Args, Debug)]
struct CodesignVerifyArgs {
    /// Path to the register-map JSON sidecar (Document C task C1).
    #[arg(value_name = "REGISTER_MAP")]
    register_map: PathBuf,
    /// Path to the firmware CTXDSL document. Must contain a single
    /// `context <name> { … }` block with at least one firmware
    /// automaton; the coupling fragment is spliced into that block.
    #[arg(value_name = "FIRMWARE_CTXDSL")]
    firmware: PathBuf,
    /// Name of the formula to evaluate (declared in the firmware
    /// document's `mu_formulas { … }` section).
    #[arg(long, value_name = "FORMULA")]
    formula: String,
    /// Composition or automaton name to evaluate the formula over.
    /// Default: the codesign-composition name emitted by the splicer
    /// (`<PERIPHERAL>System`).
    #[arg(long, value_name = "NAME")]
    automaton: Option<String>,
    /// Override the peripheral automaton name (default: uppercased
    /// peripheral name from the sidecar).
    #[arg(long, value_name = "NAME")]
    peripheral_automaton: Option<String>,
    /// Override the composition name (default: `<PERIPHERAL>System`).
    #[arg(long, value_name = "NAME")]
    composition_name: Option<String>,
    /// Emit the composed CTXDSL to this path (does not affect
    /// verification; useful for inspection / debugging).
    #[arg(long, value_name = "PATH")]
    emit_ctxdsl: Option<PathBuf>,
    /// Emit the result as JSON to stdout.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct CodesignEmitChaoticStubArgs {
    /// Path to the register-map JSON sidecar.
    #[arg(value_name = "REGISTER_MAP")]
    register_map: PathBuf,
    /// Output path. Defaults to stdout when omitted.
    #[arg(long, short = 'o', value_name = "PATH")]
    output: Option<PathBuf>,
    /// Override the peripheral automaton name (default: uppercased
    /// peripheral name from the sidecar). The context-block name is
    /// always `<AutomatonName>ChaoticStub`.
    #[arg(long, value_name = "NAME")]
    peripheral_automaton: Option<String>,
    /// Validate the register-map and exit non-zero if any issue is
    /// reported, instead of just printing warnings.
    #[arg(long)]
    strict: bool,
}

#[derive(Args, Debug)]
struct CodesignCoupleArgs {
    /// Path to the register-map JSON sidecar.
    #[arg(value_name = "REGISTER_MAP")]
    register_map: PathBuf,
    /// Override the peripheral automaton name (default: uppercased
    /// peripheral name from the sidecar).
    #[arg(long, value_name = "NAME")]
    peripheral_automaton: Option<String>,
    /// Override the composition name (default: `<PERIPHERAL>System`).
    #[arg(long, value_name = "NAME")]
    composition_name: Option<String>,
    /// Names of firmware automata to include in the composition.
    /// Pass multiple via repeated flags or a comma-separated list.
    #[arg(long = "firmware-member", value_name = "AUTOMATON")]
    firmware_members: Vec<String>,
    /// Validate the register-map and exit non-zero if any issue is
    /// reported, instead of just printing warnings.
    #[arg(long)]
    strict: bool,
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
enum SvCommand {
    /// Generate a skeleton .mununu.json sidecar from a SystemVerilog module.
    ///
    /// Scans the module's declarations and ports, assigns sensible defaults
    /// (1-bit → boolean, enum → enum, wide → discover), and writes the sidecar.
    Init(SvInitArgs),
    /// Discover significant register values via SMT analysis.
    ///
    /// Parses the SystemVerilog module, loads the .mununu.json sidecar,
    /// and uses z3 to find concrete values that make guard conditions
    /// satisfiable. Updates the sidecar's discovered_values section.
    Discover(SvDiscoverArgs),
    /// Preprocess SystemVerilog through sv2v into Verilog-2005.
    ///
    /// Standalone wrapper over the sv2v binary (zachjs/sv2v). Elaborates
    /// SV-2017 constructs (generates, interfaces, structs, always_ff,
    /// parameterized modules, …) into a Verilog-2005 subset, preserving
    /// module hierarchy and signal names. Output goes to --output or
    /// <stem>.elab.v next to the input.
    ///
    /// Used by the KMTS pipeline (R.0a) as the frontend normaliser
    /// before Yosys-no-flatten. Also exposed standalone for users who
    /// want to inspect or further-process the sv2v output. Same
    /// sv2v invocation as the Yosys path's `--preprocessor sv2v`.
    Preprocess(SvPreprocessArgs),
    /// Emit one BTOR2 per submodule (R.0b — KMTS pipeline frontend).
    ///
    /// Runs Yosys with `hierarchy -check` (no `flatten`) and emits one
    /// BTOR2 per submodule reachable from the top. The per-submodule
    /// BTOR2 files feed the R.2 KMTS lifter; the top-module netlist
    /// drives composition (see docs/design/native-sv-abstraction.md
    /// §3 + §4).
    ///
    /// Output: writes `<output-dir>/<module>.btor2` per submodule.
    /// Defaults to writing into the same directory as the input file
    /// when --output-dir is omitted. Also prints the per-submodule
    /// state-count / property-count summary to stdout.
    EmitBtor2PerModule(SvEmitBtor2PerModuleArgs),
    /// Compare the native + KMTS pipelines on one fixture (R.0c).
    ///
    /// Runs both extraction paths on the same SystemVerilog source,
    /// records per-pipeline shape (state count, property count,
    /// per-submodule breakdown), runs the SVA-elision gate via sv2v,
    /// and prints a JSON record. Used to seed
    /// `crates/mununu-core/tests/data/kmts_pipeline_baseline.json` and
    /// `sva_elision_gate.json`; the integration test
    /// `sv_compare_pipelines` consumes those files as the regression
    /// baseline that gates S.0–S.2b.
    ComparePipelines(SvComparePipelinesArgs),
}

#[derive(Args, Debug)]
struct SvInitArgs {
    /// Path to the SystemVerilog source file (.sv).
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Enable multi-module mode. The file must be a top module that
    /// instantiates sub-modules. Connections are derived from wire bindings.
    #[arg(long)]
    multi: bool,
    /// Output path for the .mununu.json file.
    /// Defaults to <stem>.mununu.json next to the .sv file.
    #[arg(long = "output", value_name = "FILE")]
    output: Option<PathBuf>,
    /// Overwrite existing sidecar file.
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct SvDiscoverArgs {
    /// Path to the SystemVerilog source file (.sv) or multi-module sidecar (.mununu.json).
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Path to the .mununu.json annotation file (for single-module mode).
    /// If omitted, looks for <stem>.mununu.json next to the .sv file.
    #[arg(long = "annotation", value_name = "FILE")]
    annotation: Option<PathBuf>,
    /// Write the updated sidecar to a different file instead of in-place.
    #[arg(long = "output", value_name = "FILE")]
    output: Option<PathBuf>,
    /// Maximum number of values to discover per signal (default: 32).
    #[arg(long = "max-values", default_value = "32")]
    max_values: usize,
    /// Run discovery on a multi-module sidecar (cross-module + per-module).
    #[arg(long)]
    multi: bool,
}

#[derive(Args, Debug)]
struct SvPreprocessArgs {
    /// Path(s) to the SystemVerilog source file(s) (.sv). At least one
    /// required. Multiple files are passed to sv2v in one invocation
    /// (sv2v resolves cross-file packages, interfaces, and parameter
    /// references in a single pass).
    #[arg(value_name = "FILE", num_args = 1..)]
    files: Vec<PathBuf>,
    /// Include directory for `\`include` resolution. Repeatable;
    /// forwarded to sv2v as `-I <dir>`.
    #[arg(short = 'I', long = "include-dir", value_name = "DIR")]
    include_dirs: Vec<PathBuf>,
    /// Output path for the elaborated Verilog-2005. Defaults to
    /// <first-stem>.elab.v next to the first input file.
    #[arg(long = "output", value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct SvEmitBtor2PerModuleArgs {
    /// Primary SystemVerilog source file (.sv) containing the top
    /// module or the multi-module design.
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Additional SV source files providing submodules or packages.
    /// Repeatable; each is read alongside the primary input.
    #[arg(long = "source", value_name = "FILE")]
    sources: Vec<PathBuf>,
    /// Explicit top-module name. When omitted, Yosys's
    /// `hierarchy -auto-top` picks the root.
    #[arg(long = "top", value_name = "NAME")]
    top: Option<String>,
    /// Output directory for the per-submodule BTOR2 files. Defaults
    /// to the directory of the primary input.
    #[arg(long = "output-dir", value_name = "DIR")]
    output_dir: Option<PathBuf>,
    /// Run sv2v as a preprocessor pass before Yosys. Required for
    /// SV-2017 constructs Yosys's built-in parser cannot accept (most
    /// notably the module-header `import pkg::*;` form). Same
    /// behaviour as the legacy `--preprocessor sv2v` flag on the
    /// single-BTOR2 path.
    #[arg(long = "preprocess-sv2v")]
    preprocess_sv2v: bool,
    /// Use Yosys's `setundef -anyseq` instead of the default
    /// `setundef -zero`. Preserves CWE-1245-class semantics at the
    /// cost of introducing `$anyseq` state cells.
    #[arg(long = "setundef-anyseq")]
    setundef_anyseq: bool,
    /// R-Y1 (§Phase 8) — Use Yosys's `setundef -anyconst` instead of
    /// the default `setundef -zero`. Adds one nondeterministic
    /// **constant input** per undef bit (NOT per-cycle state cells).
    /// Preserves CWE-1245-class bug-bearing semantics at zero extra
    /// state-cell cost — the intermediate between `-zero` (masks
    /// bugs) and `-anyseq` (state-space explosion). When both
    /// `--setundef-anyseq` and `--setundef-anyconst` are passed,
    /// `--setundef-anyseq` wins (strictly more permissive).
    #[arg(long = "setundef-anyconst")]
    setundef_anyconst: bool,
}

#[derive(Args, Debug)]
struct SvComparePipelinesArgs {
    /// Primary SystemVerilog source file (.sv). The native arm reads
    /// only this file; the KMTS arm reads it plus any --source entries.
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Additional SV source files for the KMTS arm's multi-file
    /// composition. Repeatable.
    #[arg(long = "source", value_name = "FILE")]
    sources: Vec<PathBuf>,
    /// Explicit top-module name. When omitted, the KMTS arm uses
    /// `hierarchy -auto-top`; the native arm uses the first declared
    /// module in the file.
    #[arg(long = "top", value_name = "NAME")]
    top: Option<String>,
}

#[derive(Args, Debug)]
struct TemplatesArgs {
    /// Filter templates by domain (rtl, agentic, software, synthesis).
    #[arg(long, value_name = "DOMAIN")]
    domain: Option<String>,
    /// Show details of a specific template by ID.
    #[arg(long, value_name = "ID")]
    id: Option<String>,
    /// Output as JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
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
    /// Translate from an external format before processing
    /// (tlsf, aiger, promela, xstate, systemverilog, extraction, auto).
    #[arg(long = "adapter", value_name = "FORMAT")]
    adapter: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    #[arg(long = "mode", value_name = "MODE")]
    mode: Option<String>,
    /// Optional source-language preprocessor to run before adapter
    /// translation. Currently supports: `sv2v` (lowers SV2009/2012
    /// constructs to Verilog-2005 before Yosys; required for modern
    /// open-source RTL — Caliptra-RTL, OpenTitan, ibex, etc.).
    /// Only honoured by `--adapter sv-yosys`.
    #[arg(long = "preprocessor", value_name = "NAME")]
    preprocessor: Option<String>,
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
    /// Translate from an external format before processing
    /// (tlsf, aiger, promela, xstate, systemverilog, extraction, auto).
    #[arg(long = "adapter", value_name = "FORMAT")]
    adapter: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    #[arg(long = "mode", value_name = "MODE")]
    mode: Option<String>,
    /// Optional source-language preprocessor to run before adapter
    /// translation. Currently supports: `sv2v`. Only honoured by
    /// `--adapter sv-yosys`.
    #[arg(long = "preprocessor", value_name = "NAME")]
    preprocessor: Option<String>,
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
    /// Translate from an external format before processing
    /// (tlsf, aiger, promela, xstate, systemverilog, extraction, auto).
    #[arg(long = "adapter", value_name = "FORMAT")]
    adapter: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    #[arg(long = "mode", value_name = "MODE")]
    mode: Option<String>,
    /// Optional source-language preprocessor to run before adapter
    /// translation. Currently supports: `sv2v`. Only honoured by
    /// `--adapter sv-yosys`.
    #[arg(long = "preprocessor", value_name = "NAME")]
    preprocessor: Option<String>,
    /// μ-calculus formula to evaluate (by name from the context).
    #[arg(long = "formula", value_name = "NAME", conflicts_with = "template")]
    formula: Option<String>,
    /// Instantiate a property template instead of selecting an existing formula.
    #[arg(long = "template", value_name = "ID", conflicts_with = "formula")]
    template: Option<String>,
    /// Template argument bindings (KEY=VALUE). Repeatable.
    #[arg(long = "template-arg", value_name = "KEY=VALUE", requires = "template")]
    template_args: Vec<String>,
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
    /// Stub .espec.json files to compose with the model (external library interfaces).
    #[arg(long = "stub", value_name = "FILE")]
    stubs: Vec<PathBuf>,
    /// Print the internal structure of the context to stdout or a file.
    #[arg(long = "print-structure", value_name = "FILE")]
    print_structure: Option<Option<PathBuf>>,
    /// Print the intermediate CTXDSL (after adapter translation) to stdout or a file.
    #[arg(long = "print-ctxdsl", value_name = "FILE")]
    print_ctxdsl: Option<Option<PathBuf>>,
    /// Print a per-property soundness summary after evaluation.
    #[arg(long = "soundness-report")]
    soundness_report: bool,
}

#[derive(Args, Debug)]
struct ContextSynthesizeArgs {
    /// Primary context document to load.
    #[arg(value_name = "CONTEXT")]
    context: PathBuf,
    /// Optional sidecar documents to merge.
    #[arg(long = "sidecar", value_name = "FILE")]
    sidecars: Vec<PathBuf>,
    /// Translate from an external format before processing
    /// (tlsf, aiger, promela, xstate, systemverilog, extraction, auto).
    #[arg(long = "adapter", value_name = "FORMAT")]
    adapter: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    #[arg(long = "mode", value_name = "MODE")]
    mode: Option<String>,
    /// Optional source-language preprocessor to run before adapter
    /// translation. Currently supports: `sv2v`. Only honoured by
    /// `--adapter sv-yosys`.
    #[arg(long = "preprocessor", value_name = "NAME")]
    preprocessor: Option<String>,
    /// μ-calculus formula to synthesise (by name from the context).
    #[arg(long = "formula", value_name = "NAME", conflicts_with = "template")]
    formula: Option<String>,
    /// Instantiate a property template instead of selecting an existing formula.
    #[arg(long = "template", value_name = "ID", conflicts_with = "formula")]
    template: Option<String>,
    /// Template argument bindings (KEY=VALUE). Repeatable.
    #[arg(long = "template-arg", value_name = "KEY=VALUE", requires = "template")]
    template_args: Vec<String>,
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
    /// Legacy flag — equivalent to `--mode functional`. When `--mode` is also
    /// provided, `--mode` wins.
    #[arg(long = "extract-strategy")]
    extract_strategy: bool,
    /// Controller extraction mode. Case-insensitive. One of:
    /// `projection` (default), `functional`, `permissive`,
    /// `signature-memory`, `product-game`, `parity-game`. Overrides
    /// `--extract-strategy` when set.
    #[arg(long = "controller-mode", value_name = "NAME")]
    controller_mode: Option<String>,
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
    /// Print the intermediate CTXDSL (after adapter translation) to stdout or a file.
    #[arg(long = "print-ctxdsl", value_name = "FILE")]
    print_ctxdsl: Option<Option<PathBuf>>,
    /// Output format for the synthesized controller: ctxdsl (default), xstate, systemverilog.
    #[arg(long = "output-format", value_name = "FORMAT")]
    output_format: Option<String>,
    /// Path where the native-format controller should be written (requires --output-format).
    #[arg(long = "emit-native", value_name = "FILE")]
    emit_native: Option<PathBuf>,
    /// Print a per-property soundness summary after synthesis.
    #[arg(long = "soundness-report")]
    soundness_report: bool,
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

/// Resolve the controller mode from the CLI flags.
///
/// Precedence:
/// 1. `--mode <name>` (if `Some`) is parsed case-insensitively.
/// 2. Else, `--extract-strategy` → `Functional` (legacy mapping).
/// 3. Else, `Projection` (default).
fn parse_cli_controller_mode(
    mode: &Option<String>,
    extract_strategy: bool,
) -> Result<mununu_core::context::ControllerMode, String> {
    use mununu_core::context::ControllerMode;
    if let Some(name) = mode {
        return ControllerMode::from_normalized_name(name).map_err(|other| {
            format!(
                "unknown controller mode '{other}' \
                (valid: projection, functional, permissive, signature-memory, product-game, parity-game)"
            )
        });
    }
    Ok(if extract_strategy {
        ControllerMode::Functional
    } else {
        ControllerMode::Projection
    })
}

#[cfg(test)]
mod cli_controller_mode_tests {
    use super::*;
    use mununu_core::context::ControllerMode;

    #[test]
    fn default_is_projection() {
        assert_eq!(
            parse_cli_controller_mode(&None, false).unwrap(),
            ControllerMode::Projection
        );
    }

    #[test]
    fn extract_strategy_maps_to_functional() {
        assert_eq!(
            parse_cli_controller_mode(&None, true).unwrap(),
            ControllerMode::Functional
        );
    }

    #[test]
    fn explicit_mode_wins_over_extract_strategy() {
        assert_eq!(
            parse_cli_controller_mode(&Some("projection".into()), true).unwrap(),
            ControllerMode::Projection
        );
    }

    #[test]
    fn parses_all_modes() {
        let cases = [
            ("projection", ControllerMode::Projection),
            ("functional", ControllerMode::Functional),
            ("permissive", ControllerMode::Permissive),
            ("signature-memory", ControllerMode::SignatureMemory),
            ("Signature_Memory", ControllerMode::SignatureMemory),
            ("product-game", ControllerMode::ProductGame),
            ("PARITY-GAME", ControllerMode::ParityGame),
            ("paritygame", ControllerMode::ParityGame),
        ];
        for (name, expected) in cases {
            assert_eq!(
                parse_cli_controller_mode(&Some(name.into()), false).unwrap(),
                expected,
                "name `{name}`"
            );
        }
    }

    #[test]
    fn rejects_unknown_mode() {
        let err = parse_cli_controller_mode(&Some("strict".into()), false).unwrap_err();
        assert!(err.contains("unknown controller mode"));
        assert!(err.contains("strict"));
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

fn handle_verify(args: VerifyArgs) -> Result<(), String> {
    use mununu_core::verify::config::VerifyConfig;
    use mununu_core::verify::report::PropertyFormulaSource;
    use mununu_core::verify::{inspect_project, verify_project};

    if args.print_alphabet && args.strict {
        return Err("--print-alphabet is incompatible with --strict (the introspection report carries no verdicts)".to_string());
    }

    let body = std::fs::read_to_string(&args.config)
        .map_err(|e| format!("failed to read {}: {e}", args.config.display()))?;
    let config = VerifyConfig::from_toml(&body)
        .map_err(|e| format!("failed to parse {} as TOML: {e}", args.config.display()))?;

    let base_dir = args.base_dir.clone().unwrap_or_else(|| {
        args.config
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    });

    if args.print_alphabet {
        let inspection = inspect_project(&config, &base_dir).map_err(|e| format!("{e}"))?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&inspection).map_err(|e| e.to_string())?
            );
        } else {
            print_inspection_human(&inspection);
        }
        return Ok(());
    }

    let report = verify_project(&config, &base_dir).map_err(|e| format!("{e}"))?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        println!("verify report — project `{}`:", report.project);
        println!(
            "  composition: {} {} {{ members = [{}] }}",
            report.composition.semantics,
            report.composition.name,
            report.composition.members.join(", "),
        );
        println!("  sources:");
        for s in &report.sources {
            println!(
                "    - {id} (adapter = {adapter}, automaton = {automaton})",
                id = s.id,
                adapter = s.adapter,
                automaton = s.automaton.as_deref().unwrap_or("(unresolved)"),
            );
        }
        println!("  properties ({}):", report.property_verdicts.len());
        for v in &report.property_verdicts {
            let source_str = match &v.formula_source {
                PropertyFormulaSource::Inline => "inline".to_string(),
                PropertyFormulaSource::Template { id, .. } => format!("template `{id}`"),
            };
            let verdict = if v.satisfied { "SATISFIED" } else { "VIOLATED" };
            println!(
                "    {name}: {verdict} ({sat}/{total} states, {init_sat}/{init} initial) [{source}, over = {over}]",
                name = v.name,
                verdict = verdict,
                sat = v.satisfying_states,
                total = v.total_states,
                init_sat = v.initial_satisfying.len(),
                init = v.initial_states.len(),
                source = source_str,
                over = v.over,
            );
            if args.print_counterexample
                && let Some(witness) = v.counterexample.as_ref()
            {
                println!("      counterexample (from {}):", witness.initial_state);
                for (i, step) in witness.steps.iter().enumerate() {
                    println!(
                        "        {idx}. --[{label}]--> {state}",
                        idx = i + 1,
                        label = step.label,
                        state = step.successor_state,
                    );
                }
                let term = match &witness.termination {
                    mununu_core::verify::report::TraceTermination::Sink => "sink".to_string(),
                    mununu_core::verify::report::TraceTermination::Cycle { return_to_step } => {
                        format!("cycle back to step {return_to_step}")
                    }
                    mununu_core::verify::report::TraceTermination::LengthLimit => {
                        "length-limit (truncated)".to_string()
                    }
                };
                println!("        ({term})");
            }
        }
    }

    if args.strict && report.property_verdicts.iter().any(|v| !v.satisfied) {
        return Err("one or more properties violated (--strict)".to_string());
    }
    Ok(())
}

fn handle_memory(command: MemoryCommand) -> Result<(), String> {
    match command {
        MemoryCommand::Check(args) => handle_memory_check(args),
    }
}

fn handle_memory_check(args: MemoryCheckArgs) -> Result<(), String> {
    use mununu_core::verify::config::VerifyConfig;
    use mununu_core::verify::memory_check::check_memory_postures;

    let body = std::fs::read_to_string(&args.config)
        .map_err(|e| format!("failed to read {}: {e}", args.config.display()))?;
    let config = VerifyConfig::from_toml(&body)
        .map_err(|e| format!("failed to parse {} as TOML: {e}", args.config.display()))?;
    let report = check_memory_postures(&config);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        print!("{}", report.format_human());
    }

    if args.strict && report.has_warnings() {
        return Err(format!(
            "{} warning(s) raised; --strict requested",
            report.warnings.len()
        ));
    }
    Ok(())
}

fn print_inspection_human(report: &mununu_core::verify::InspectionReport) {
    println!("inspect report — project `{}`:", report.project);
    println!(
        "  composition: {} {} {{ members = [{}] }}",
        report.composition.info.semantics,
        report.composition.info.name,
        report.composition.info.members.join(", "),
    );
    println!("  sources:");
    for s in &report.sources {
        println!(
            "    - {id} (adapter = {adapter}, automaton = {automaton})",
            id = s.id,
            adapter = s.adapter,
            automaton = s.automaton.as_deref().unwrap_or("(unresolved)"),
        );
    }
    println!("  automata ({}):", report.automata.len());
    for a in &report.automata {
        let src = a.source_id.as_deref().unwrap_or("-");
        println!(
            "    {name} (source = {src}, {n_states} states, {n_init} initial, {n_alpha} labels)",
            name = a.name,
            n_states = a.states.len(),
            n_init = a.initial_states.len(),
            n_alpha = a.alphabet.len(),
        );
        if !a.initial_states.is_empty() {
            println!("      initial: {}", a.initial_states.join(", "));
        }
        if !a.states.is_empty() {
            println!("      states:  {}", a.states.join(", "));
        }
        if !a.alphabet.is_empty() {
            println!("      labels:  {}", a.alphabet.join(", "));
        }
    }
    println!();
    println!(
        "  composition alphabet ({} labels):",
        report.composition.alphabet.len()
    );
    if !report.composition.alphabet.is_empty() {
        for chunk in report.composition.alphabet.chunks(4) {
            println!("    {}", chunk.join(", "));
        }
    }
    if !report.composition.state_names.is_empty() {
        println!(
            "  composition states ({}):",
            report.composition.state_names.len()
        );
        for chunk in report.composition.state_names.chunks(4) {
            println!("    {}", chunk.join(", "));
        }
    }
    if !report.composition.predicate_names.is_empty() {
        println!(
            "  declared predicates ({}):",
            report.composition.predicate_names.len()
        );
        for chunk in report.composition.predicate_names.chunks(4) {
            println!("    {}", chunk.join(", "));
        }
    }
}

fn dispatch(command: Commands) -> Result<(), String> {
    match command {
        Commands::Context { command } => handle_context(*command),
        Commands::Extraction { command } => handle_extraction(*command),
        Commands::Sv { command } => handle_sv(*command),
        Commands::Templates(args) => list_templates(args),
        Commands::Library { command } => handle_library(*command),
        Commands::Contract { command } => handle_contract(*command),
        Commands::Codesign { command } => handle_codesign(*command),
        Commands::Verify(args) => handle_verify(args),
        Commands::Memory { command } => handle_memory(*command),
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
        Commands::Btor2 { command } => handle_btor2(*command),
    }
}

fn btor2_lift_kmts(args: Btor2LiftKmtsArgs) -> Result<(), String> {
    use mununu_core::adapter::AdapterOptions;
    use mununu_core::adapter::btor2::{KmtsLiftOptions, lift_btor2_to_kmts};

    let content = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read BTOR2 '{}': {e}", args.file.display()))?;
    let opts = AdapterOptions::default();
    let lift_opts = KmtsLiftOptions {
        max_predicates: args.max_predicates,
        ..Default::default()
    };
    let result = lift_btor2_to_kmts(&content, &opts, &lift_opts)
        .map_err(|e| format!("btor2 lift-kmts: {}", e.message))?;

    let summary = serde_json::json!({
        "fixture": args.file.display().to_string(),
        "predicates_synthesised": result.predicates.len(),
        "labelling_count": result.labelling_count(),
        "automata": result.predicate_labellings.keys().collect::<Vec<_>>(),
        "predicate_names": result.predicates.iter().map(|p| &p.name).collect::<Vec<_>>(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).map_err(|e| format!("serialize summary: {e}"))?
    );
    Ok(())
}

fn handle_btor2(command: Btor2Command) -> Result<(), String> {
    match command {
        Btor2Command::Discover(args) => btor2_discover(args),
        Btor2Command::LiftKmts(args) => btor2_lift_kmts(args),
        Btor2Command::Cegar(args) => btor2_cegar(args),
    }
}

/// R.5 Item 3 sub-item 3.5 (2026-06-04) — CEGAR refinement
/// loop CLI handler.
///
/// R-S8 session 2 (2026-06-08) — parse the `--config-values
/// REG=v1,v2,v3` CLI flag(s) and build an `AdapterOptions` whose
/// `sidecar_json` declares one `signals[]` entry per flag entry
/// with the listed `config_values`. The CEGAR loop reads this via
/// `r_s8_encoder::sidecar_config_values` and threads it through
/// the predicate-cube lift's `config_values`.
///
/// Returns an `AdapterOptions::default()` when the flag is absent.
/// Errors on malformed entries (missing `=`, non-numeric values).
fn build_adapter_options_with_config_values(
    config_values_args: &[String],
) -> Result<mununu_core::adapter::AdapterOptions, String> {
    use mununu_core::adapter::AdapterOptions;
    if config_values_args.is_empty() {
        return Ok(AdapterOptions::default());
    }
    let mut signals = Vec::with_capacity(config_values_args.len());
    for entry in config_values_args {
        let (reg, values_str) = entry.split_once('=').ok_or_else(|| {
            format!("invalid --config-values entry {entry:?}: expected REG=v1,v2,v3 format")
        })?;
        let values: Vec<u64> = values_str
            .split(',')
            .map(|s| {
                s.trim()
                    .parse::<u64>()
                    .map_err(|e| format!("invalid value {s:?} in --config-values: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if values.is_empty() {
            return Err(format!(
                "--config-values {entry:?}: at least one value required"
            ));
        }
        signals.push(serde_json::json!({
            "name": reg,
            "config_values": values,
        }));
    }
    let synthetic = serde_json::json!({
        "module": "cli-cegar",
        "source": "cli-cegar.btor2",
        "signals": signals,
    });
    Ok(AdapterOptions {
        sidecar_json: Some(synthetic.to_string()),
        ..AdapterOptions::default()
    })
}

/// Parses the user-supplied formula + initial predicate set,
/// builds a [`CegarOptions`] with the selected predicate source,
/// and invokes [`cegar_refine_loop`] on the BTOR2 fixture.
/// Prints a human-readable or JSON summary of the resulting
/// [`CegarTrace`].
fn btor2_cegar(args: Btor2CegarArgs) -> Result<(), String> {
    use mununu_core::adapter::btor2::PredicateSpec;
    use mununu_core::adapter::btor2::cegar::{
        CegarOptions, LiftStrategy, PredicateSource, cegar_refine_loop,
    };
    use mununu_core::mu_calculus::{Environment, parser as mu_parser};

    if !args.file.exists() {
        return Err(format!(
            "BTOR2 input file does not exist: {}",
            args.file.display()
        ));
    }
    let content = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read BTOR2 '{}': {e}", args.file.display()))?;

    // Honor --cvc5-path via env var (the locate_cvc5 helper
    // reads MUNUNU_CVC5_PATH first). SAFETY: env vars are
    // process-global; this is fine for the CLI handler which
    // is single-threaded + runs once per invocation.
    if let Some(p) = &args.cvc5_path {
        unsafe {
            std::env::set_var("MUNUNU_CVC5_PATH", p);
        }
    }

    // Parse initial predicates from `NAME:REGISTER=VALUE` triples.
    let mut initial_predicates: Vec<PredicateSpec> = Vec::with_capacity(args.predicates.len());
    for raw in &args.predicates {
        let (name, rest) = raw.split_once(':').ok_or_else(|| {
            format!("predicate spec '{raw}' missing ':' separator (expected NAME:REGISTER=VALUE)")
        })?;
        let (register, value_str) = rest.split_once('=').ok_or_else(|| {
            format!("predicate spec '{raw}' missing '=' separator (expected NAME:REGISTER=VALUE)")
        })?;
        let value: u64 = value_str
            .parse()
            .map_err(|e| format!("predicate spec '{raw}' has non-numeric value: {e}"))?;
        initial_predicates.push(PredicateSpec {
            name: name.to_string(),
            register: register.to_string(),
            value,
        });
    }
    if initial_predicates.is_empty() {
        return Err(
            "at least one --predicate NAME:REGISTER=VALUE is required to bootstrap the cube space"
                .into(),
        );
    }

    // Parse the μ-calculus formula.
    let formula =
        mu_parser::parse(&args.formula).map_err(|e| format!("formula parse error: {e:?}"))?;

    // Build the environment with state_count = 2^|predicates|.
    let cube_count = 1usize << initial_predicates.len();
    let env = Environment::new(cube_count);

    // Map the CLI's PredicateSourceArg to the core's PredicateSource.
    let predicate_source = match args.predicate_source {
        PredicateSourceArg::Wp => PredicateSource::WeakestPrecondition,
        PredicateSourceArg::Craig => PredicateSource::CraigInterpolation,
    };

    // R.2.5b session-1 follow-up — map CLI MustEdgeInferenceArg to
    // the core MustEdgeInference enum.
    let must_edge_inference = match args.must_edge_inference {
        MustEdgeInferenceArg::Off => mununu_core::adapter::btor2::kmts_lift::MustEdgeInference::Off,
        MustEdgeInferenceArg::SamplingConfluence => {
            mununu_core::adapter::btor2::kmts_lift::MustEdgeInference::SamplingConfluence
        }
        MustEdgeInferenceArg::SmtPerTarget => {
            mununu_core::adapter::btor2::kmts_lift::MustEdgeInference::SmtPerTarget
        }
        MustEdgeInferenceArg::SmtPerTargetStandard => {
            mununu_core::adapter::btor2::kmts_lift::MustEdgeInference::SmtPerTargetStandard
        }
        MustEdgeInferenceArg::SmtHyperMust => {
            mununu_core::adapter::btor2::kmts_lift::MustEdgeInference::SmtHyperMust
        }
    };

    let cegar_opts = CegarOptions {
        max_iterations: args.max_iterations,
        predicate_source,
        max_cube_count: 1024,
        capture_approximants: false,
        enable_approximant_reuse: false,
        smart_uf_cap: true,
        lift_strategy: LiftStrategy::Eager,
        must_edge_inference,
    };

    // R-S8 session 2 (2026-06-08) — parse the `--config-values`
    // CLI flag(s) into a synthetic sidecar JSON that exercises the
    // shipped `sidecar_config_values` resolver bridge end-to-end.
    // Format: `REG=v1,v2,v3`. Builds an `SvAnnotation` with one
    // signal per flag entry; the CEGAR loop reads `sidecar_json`
    // via the bridge and threads `config_values` into the
    // predicate-cube lift.
    let mut adapter_options = build_adapter_options_with_config_values(&args.config_values)?;
    // R.6.6 / V.6 (2026-06-09) — thread the `--controllable-input`
    // CLI flag values into `AdapterOptions::controllable_inputs`,
    // which the predicate-cube lifter reads to partition boolean
    // inputs into env / ctrl classes + emit per-combo dual-label
    // transitions with the appropriate `LabelControllability` tags.
    adapter_options.controllable_inputs = args.controllable_inputs.clone();

    let trace = cegar_refine_loop(
        &formula,
        &content,
        initial_predicates,
        &env,
        &adapter_options,
        &cegar_opts,
    )
    .map_err(|e| format!("cegar refine loop: {}", e.message))?;

    if args.json {
        let summary = serde_json::json!({
            "fixture": args.file.display().to_string(),
            "formula": args.formula,
            "predicate_source": format!("{:?}", args.predicate_source),
            "iterations": trace.iterations.len(),
            "terminated_with": format!("{:?}", trace.terminated_with),
            "final_predicate_count": trace.final_predicates.len(),
            "approximant_reuse_enabled": trace.approximant_reuse_enabled,
            "lazy_lift_pending": trace.lazy_lift_pending,
            "warnings": trace.warnings.iter().map(|w| w.message.clone()).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary)
                .map_err(|e| format!("serialize summary: {e}"))?
        );
    } else {
        println!("CEGAR refinement loop completed");
        println!("  fixture:           {}", args.file.display());
        println!("  formula:           {}", args.formula);
        println!("  predicate_source:  {:?}", args.predicate_source);
        println!("  iterations:        {}", trace.iterations.len());
        println!("  terminated_with:   {:?}", trace.terminated_with);
        println!("  final predicates:  {}", trace.final_predicates.len());
        if !trace.warnings.is_empty() {
            println!("  warnings:");
            for w in &trace.warnings {
                println!("    - {}", w.message);
            }
        }
    }

    Ok(())
}

fn btor2_discover(args: Btor2DiscoverArgs) -> Result<(), String> {
    use mununu_core::adapter::sidecar::predicate_image::{
        ImageOptions, all_smt::discover_values_for_btor2_file,
    };
    use mununu_core::adapter::systemverilog::annotation::{
        DiscoveredValues, SvAnnotation, merge_discovered_values,
    };

    if !args.file.exists() {
        return Err(format!(
            "BTOR2 input file does not exist: {}",
            args.file.display()
        ));
    }
    let src = fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read '{}': {e}", args.file.display()))?;
    let file = mununu_core::adapter::btor2::parser::parse(&src)
        .map_err(|e| format!("BTOR2 parse error in '{}': {e}", args.file.display()))?;

    let opts = ImageOptions {
        cap_edges: args.cap_edges,
        ..ImageOptions::default()
    };

    eprintln!(
        "Running predicate-image discovery on {} (cap_edges={})...",
        args.file.display(),
        args.cap_edges
    );
    let results = discover_values_for_btor2_file(&file, &opts)
        .map_err(|e| format!("predicate-image discovery failed: {e}"))?;

    if results.is_empty() {
        eprintln!("No discovered values for any state cell.");
    } else {
        for (signal, discovered) in &results {
            eprintln!("  {} — {} value(s):", signal, discovered.values.len());
            for v in &discovered.values {
                let from = v.from.as_deref().unwrap_or("predicate-image");
                eprintln!("    {} = {} ({})", v.name, v.value, from);
            }
        }
    }

    // Resolve the output sidecar path: explicit `--output`, or
    // `<stem>.mununu.json` next to the input.
    let sidecar_path = match &args.output {
        Some(p) => p.clone(),
        None => {
            let stem = args
                .file
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    format!("Cannot derive sidecar stem from '{}'", args.file.display())
                })?;
            args.file.with_file_name(format!("{stem}.mununu.json"))
        }
    };

    // Load existing sidecar if present, else start from a minimal
    // annotation shape. Either way we merge in the discovered values.
    let mut annotation: SvAnnotation = if sidecar_path.exists() {
        let body = fs::read_to_string(&sidecar_path).map_err(|e| {
            format!(
                "Failed to read existing sidecar '{}': {e}",
                sidecar_path.display()
            )
        })?;
        serde_json::from_str(&body).map_err(|e| {
            format!(
                "Failed to parse existing sidecar '{}': {e}",
                sidecar_path.display()
            )
        })?
    } else {
        SvAnnotation {
            schema: Some("mununu_sv_annotation_v1".to_string()),
            module: args
                .file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("design")
                .to_string(),
            source: args
                .file
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string()),
            signals: Vec::new(),
            inputs: Vec::new(),
            controllable: Vec::new(),
            properties: Vec::new(),
            discovered_values: std::collections::HashMap::new(),
            parameters: std::collections::HashMap::new(),
            reset_sequence: None,
            memories: Vec::new(),
            uf_wrap: Vec::new(),
            uf_unwrap: Vec::new(),
        }
    };

    let results_typed: std::collections::HashMap<String, DiscoveredValues> = results;
    merge_discovered_values(&mut annotation.discovered_values, results_typed);

    let json = serde_json::to_string_pretty(&annotation)
        .map_err(|e| format!("Failed to serialise updated sidecar: {e}"))?;
    fs::write(&sidecar_path, json)
        .map_err(|e| format!("Failed to write sidecar '{}': {e}", sidecar_path.display()))?;
    eprintln!("Updated sidecar: {}", sidecar_path.display());

    Ok(())
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

fn handle_contract(command: ContractCommand) -> Result<(), String> {
    match command {
        ContractCommand::Validate(args) => contract_validate(args),
        ContractCommand::Gaps(args) => contract_gaps(args),
        ContractCommand::Discover(args) => contract_discover(args),
        ContractCommand::Sidecars(args) => contract_sidecars(args),
        ContractCommand::Query(args) => contract_query(args),
        ContractCommand::Review(args) => contract_review(args),
    }
}

fn handle_codesign(command: CodesignCommand) -> Result<(), String> {
    match command {
        CodesignCommand::EmitCmsisHeader(args) => codesign_emit_cmsis_header(args),
        CodesignCommand::Couple(args) => codesign_couple(args),
        CodesignCommand::EmitChaoticStub(args) => codesign_emit_chaotic_stub(args),
        CodesignCommand::Verify(args) => codesign_verify(args),
        CodesignCommand::ImportSvd(args) => codesign_import_svd(args),
        CodesignCommand::ExtractC(args) => codesign_extract_c(args),
        CodesignCommand::ReconcileLabels(args) => codesign_reconcile_labels(args),
    }
}

fn codesign_reconcile_labels(args: CodesignReconcileLabelsArgs) -> Result<(), String> {
    use mununu_core::codesign::reconcile::{
        ReconcileError, peripheral_labels_from_register_map, reconcile_label_alphabets,
    };
    use mununu_core::codesign::register_map::RegisterMap;
    use std::collections::BTreeSet;

    let load_label_list = |path: &PathBuf, side: &str| -> Result<BTreeSet<String>, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("failed to read {side} labels {}: {e}", path.display()))?;
        let labels: Vec<String> = serde_json::from_slice(&bytes).map_err(|e| {
            format!(
                "failed to parse {side} labels {} as JSON array of strings: {e}",
                path.display()
            )
        })?;
        Ok(labels.into_iter().collect())
    };

    let firmware = load_label_list(&args.firmware_labels, "firmware")?;

    let peripheral: BTreeSet<String> = match (
        &args.peripheral_labels,
        &args.peripheral_register_map,
    ) {
        (Some(_), Some(_)) => {
            return Err(
                "pass exactly one of <PERIPHERAL_JSON> or --peripheral-register-map, not both"
                    .to_string(),
            );
        }
        (None, None) => {
            return Err(
                "missing peripheral source: pass <PERIPHERAL_JSON> as the second positional arg or --peripheral-register-map <JSON>"
                    .to_string(),
            );
        }
        (Some(p), None) => load_label_list(p, "peripheral")?,
        (None, Some(rm_path)) => {
            let bytes = std::fs::read(rm_path)
                .map_err(|e| format!("failed to read register-map {}: {e}", rm_path.display()))?;
            let rm: RegisterMap = serde_json::from_slice(&bytes).map_err(|e| {
                format!(
                    "failed to parse register-map {} as JSON: {e}",
                    rm_path.display()
                )
            })?;
            let issues = rm.validate();
            if !issues.is_empty() {
                for issue in &issues {
                    eprintln!("register-map validation: {issue}");
                }
                return Err(format!(
                    "register-map {} has {} validation issue(s) — refusing to proceed",
                    rm_path.display(),
                    issues.len()
                ));
            }
            peripheral_labels_from_register_map(&rm)
        }
    };

    let result = reconcile_label_alphabets(&firmware, &peripheral);

    match args.format.as_str() {
        "json" => match &result {
            Ok(r) => {
                let payload = serde_json::json!({
                    "shared": r.shared,
                    "mismatch": serde_json::Value::Null,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload)
                        .map_err(|e| format!("serialize: {e}"))?
                );
                Ok(())
            }
            Err(ReconcileError::Mismatch(m)) => {
                let payload = serde_json::json!({
                    "shared": Vec::<String>::new(),
                    "mismatch": {
                        "firmware_only": m.firmware_only,
                        "peripheral_only": m.peripheral_only,
                    },
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload)
                        .map_err(|e| format!("serialize: {e}"))?
                );
                Err("label-alphabet mismatch".to_string())
            }
        },
        "human" => match &result {
            Ok(r) => {
                println!("alphabets reconcile ({} shared labels):", r.shared.len());
                for label in &r.shared {
                    println!("  - {label}");
                }
                Ok(())
            }
            Err(ReconcileError::Mismatch(m)) => {
                eprintln!("error: label-alphabet mismatch");
                if !m.firmware_only.is_empty() {
                    eprintln!("  firmware-only ({}):", m.firmware_only.len());
                    for label in &m.firmware_only {
                        eprintln!("    - {label}");
                    }
                }
                if !m.peripheral_only.is_empty() {
                    eprintln!("  peripheral-only ({}):", m.peripheral_only.len());
                    for label in &m.peripheral_only {
                        eprintln!("    - {label}");
                    }
                }
                Err("label-alphabet mismatch".to_string())
            }
        },
        other => Err(format!("unknown --format '{other}' (valid: human, json)")),
    }
}

fn codesign_extract_c(args: CodesignExtractCArgs) -> Result<(), String> {
    use mununu_core::codesign::c_extract_llvm::{LlvmExtractOptions, extract_c_via_llvm};
    use mununu_core::codesign::register_map::RegisterMap;

    let register_map = match args.register_map.as_ref() {
        Some(path) => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("failed to read register-map {}: {e}", path.display()))?;
            let rm: RegisterMap = serde_json::from_slice(&bytes).map_err(|e| {
                format!(
                    "failed to parse register-map {} as JSON: {e}",
                    path.display()
                )
            })?;
            let issues = rm.validate();
            if !issues.is_empty() {
                for issue in &issues {
                    eprintln!("register-map validation: {issue}");
                }
                return Err(format!(
                    "register-map {} has {} validation issue(s) \u{2014} refusing to proceed",
                    path.display(),
                    issues.len()
                ));
            }
            Some(rm)
        }
        None => {
            if args.synthesize_automaton {
                return Err("--synthesize-automaton requires --register-map <JSON>".to_string());
            }
            None
        }
    };

    // Phase L8: when `--cmsis-stubs` is set, prepend the bundled
    // `cmsis-stubs/` directory (vendor-neutral CMSIS shims) to the
    // include-paths list. The directory ships inside the
    // mununu-core crate; we resolve it relative to the binary's
    // workspace root via the CARGO_MANIFEST_DIR-equivalent
    // environment variable, with a fallback to the conventional
    // path so this also works for installed binaries.
    let mut include_paths = args.include_paths;
    if args.cmsis_stubs {
        include_paths.insert(0, locate_cmsis_stubs());
    }

    let opts = LlvmExtractOptions {
        clang_path: args.clang,
        include_paths,
        defines: args.defines,
        extra_clang_args: args.extra_clang_args,
        register_map,
        synthesize_automaton: args.synthesize_automaton,
        driver_mode: args.driver_mode,
    };
    let extraction =
        extract_c_via_llvm(&args.file, &opts).map_err(|e| format!("C extraction failed: {e}"))?;
    if args.strict && !extraction.warnings.is_empty() {
        for w in &extraction.warnings {
            eprintln!("warning: {w}");
        }
        return Err(format!(
            "--strict: {} extraction warning(s) \u{2014} refusing to proceed",
            extraction.warnings.len()
        ));
    }
    for w in &extraction.warnings {
        eprintln!("warning: {w}");
    }
    let json = serde_json::to_string_pretty(&extraction)
        .map_err(|e| format!("failed to serialise extraction: {e}"))?;
    println!("{json}");
    Ok(())
}

fn codesign_emit_cmsis_header(args: CodesignEmitCmsisHeaderArgs) -> Result<(), String> {
    use mununu_core::codesign::cmsis_emit::{CmsisEmitOptions, emit_cmsis_header};
    use mununu_core::codesign::register_map::RegisterMap;
    use mununu_core::codesign::svd_import::import_svd;

    let owned_maps: Vec<RegisterMap> = match (&args.svd, &args.register_map) {
        (Some(svd), None) => {
            let body = std::fs::read_to_string(svd)
                .map_err(|e| format!("failed to read {}: {e}", svd.display()))?;
            let import = import_svd(&body).map_err(|e| format!("SVD import failed: {e}"))?;
            import.maps
        }
        (None, Some(rm_path)) => {
            let body = std::fs::read_to_string(rm_path)
                .map_err(|e| format!("failed to read {}: {e}", rm_path.display()))?;
            let rm: RegisterMap = serde_json::from_str(&body)
                .map_err(|e| format!("failed to parse register-map JSON: {e}"))?;
            vec![rm]
        }
        (Some(_), Some(_)) => {
            return Err("specify either --svd or --register-map, not both".to_string());
        }
        (None, None) => {
            return Err("--svd <FILE> or --register-map <JSON> is required".to_string());
        }
    };
    let maps: Vec<&RegisterMap> = match &args.peripheral {
        Some(name) => owned_maps
            .iter()
            .filter(|rm| &rm.peripheral == name)
            .collect(),
        None => owned_maps.iter().collect(),
    };
    if maps.is_empty() {
        return Err(match args.peripheral {
            Some(name) => format!("peripheral `{name}` not found"),
            None => "no peripherals in input".to_string(),
        });
    }
    let options = CmsisEmitOptions {
        vendor_prefix: &args.vendor_prefix,
        struct_type_name: None,
    };
    for rm in &maps {
        print!("{}", emit_cmsis_header(rm, &options));
        println!();
    }
    Ok(())
}

/// Phase L8: resolve the bundled `cmsis-stubs/` directory's path.
/// Tries the workspace-relative path first (dev builds running from
/// source), then a `share/mununu/cmsis-stubs` fallback (installed
/// binaries). Returns the first existing directory.
fn locate_cmsis_stubs() -> PathBuf {
    let candidates: &[PathBuf] = &[
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("mununu-core/cmsis-stubs"),
        PathBuf::from("crates/mununu-core/cmsis-stubs"),
        PathBuf::from("../share/mununu/cmsis-stubs"),
    ];
    for candidate in candidates {
        if candidate.is_dir() {
            return candidate.clone();
        }
    }
    // Fallback: the workspace-relative path even if it doesn't
    // exist (clang will fail with a clear "include not found" if
    // so; the user can override via --include).
    candidates[0].clone()
}

fn codesign_import_svd(args: CodesignImportSvdArgs) -> Result<(), String> {
    use mununu_core::codesign::svd_import::import_svd;

    let body = std::fs::read_to_string(&args.svd)
        .map_err(|e| format!("failed to read {}: {e}", args.svd.display()))?;
    let import = import_svd(&body).map_err(|e| format!("SVD import failed: {e}"))?;

    // Emit warnings to stderr so they're visible regardless of stdout
    // capture. Under --strict, any warning is a hard error.
    for w in &import.warnings {
        eprintln!("warning: {w}");
    }
    if args.strict && !import.warnings.is_empty() {
        return Err(format!(
            "--strict: {} SVD warning(s) — refusing to proceed",
            import.warnings.len()
        ));
    }

    // Filter by --peripheral if requested.
    let filtered: Vec<_> = match &args.peripheral {
        Some(name) => import
            .maps
            .into_iter()
            .filter(|m| m.peripheral == *name)
            .collect(),
        None => import.maps,
    };

    if filtered.is_empty() {
        return Err(match args.peripheral {
            Some(name) => format!("no peripheral named `{name}` in {}", args.svd.display()),
            None => format!(
                "SVD file {} contains no importable peripherals",
                args.svd.display()
            ),
        });
    }

    // Decide output mode: --out-dir writes files; default writes JSON
    // to stdout (one peripheral or a JSON array if multiple).
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
        for map in &filtered {
            let path = dir.join(format!("{}.json", sanitize_filename(&map.peripheral)));
            let json = serde_json::to_string_pretty(map)
                .map_err(|e| format!("failed to serialise {}: {e}", map.peripheral))?;
            std::fs::write(&path, json)
                .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
            eprintln!("wrote {} → {}", map.peripheral, path.display());
        }
        eprintln!(
            "imported {} peripheral(s) from {}",
            filtered.len(),
            args.svd.display()
        );
    } else if filtered.len() == 1 {
        let json = serde_json::to_string_pretty(&filtered[0])
            .map_err(|e| format!("failed to serialise: {e}"))?;
        println!("{json}");
    } else {
        let json = serde_json::to_string_pretty(&filtered)
            .map_err(|e| format!("failed to serialise: {e}"))?;
        println!("{json}");
    }

    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn codesign_verify(args: CodesignVerifyArgs) -> Result<(), String> {
    use mununu_core::codesign::compose::{ComposeOptions, compose_codesign_ctxdsl};
    use mununu_core::codesign::register_map::RegisterMap;

    let rm_body = std::fs::read_to_string(&args.register_map)
        .map_err(|e| format!("failed to read {}: {e}", args.register_map.display()))?;
    let rm: RegisterMap = serde_json::from_str(&rm_body)
        .map_err(|e| format!("failed to parse register-map JSON: {e}"))?;

    let firmware_text = std::fs::read_to_string(&args.firmware)
        .map_err(|e| format!("failed to read {}: {e}", args.firmware.display()))?;

    let opts = ComposeOptions {
        peripheral_automaton: args.peripheral_automaton.as_deref(),
        composition_name: args.composition_name.as_deref(),
        firmware_members_override: None,
    };
    let composed = compose_codesign_ctxdsl(&rm, &firmware_text, &opts)
        .map_err(|e| format!("codesign compose failed: {e}"))?;

    if let Some(out_path) = &args.emit_ctxdsl {
        std::fs::write(out_path, &composed.ctxdsl)
            .map_err(|e| format!("failed to write {}: {e}", out_path.display()))?;
        eprintln!("wrote composed CTXDSL to {}", out_path.display());
    }

    let automaton_name = args
        .automaton
        .clone()
        .unwrap_or_else(|| composed.composition_name.clone());

    // Parse + realise the composed document so we can evaluate the
    // user's formula. Reuses the same path the `context eval`
    // subcommand uses.
    let context_doc = parse_context_doc(&composed.ctxdsl).map_err(|e| {
        format!("composed CTXDSL failed to parse (this is a bug in codesign::compose): {e:?}")
    })?;
    let sidecar_docs: Vec<ContextDoc> = Vec::new();
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let formula = realized
        .formulas
        .get(&args.formula)
        .ok_or_else(|| format!("unknown formula '{}' in composed context", args.formula))?;
    let clts = realized.context.clts(&automaton_name).ok_or_else(|| {
        format!(
            "unknown automaton/composition '{automaton_name}' in composed context — expected one of: {}",
            realized.context.clts_names().join(", ")
        )
    })?;

    let env = realized.environment_for(&automaton_name);
    let options = mununu_core::mu_calculus::EvaluationOptions::default();
    let result =
        mununu_core::mu_calculus::evaluate_with_options(&formula.formula, clts, &env, &options)
            .map_err(|err| format!("μ-calculus evaluation failed: {err}"))?;

    let total_states = clts.state_count();
    let satisfying_count = (0..total_states)
        .filter(|i| result.get(*i).map(|b| *b).unwrap_or(false))
        .count();
    let initial_states: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|sid| clts.state_name(*sid).map(|s| s.to_string()))
        .collect();
    let initial_satisfying: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|sid| {
            if result.get(sid.index()).map(|bit| *bit).unwrap_or(false) {
                clts.state_name(*sid).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();
    let all_initials_satisfying = initial_satisfying.len() == initial_states.len();

    if args.json {
        let body = serde_json::json!({
            "register_map": {
                "peripheral": rm.peripheral,
                "base_address": rm.base_address,
                "registers": rm.registers.len(),
            },
            "composition": {
                "automaton": automaton_name,
                "peripheral_automaton": composed.peripheral_automaton,
                "firmware_members": composed.firmware_members,
            },
            "verdict": {
                "formula": args.formula,
                "automaton": automaton_name,
                "total_states": total_states,
                "satisfying_states": satisfying_count,
                "initial_states": initial_states,
                "initial_satisfying": initial_satisfying,
                "satisfied": all_initials_satisfying,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    println!(
        "codesign verify — peripheral `{}` (base {}), composition `{}`",
        rm.peripheral, rm.base_address, automaton_name
    );
    println!(
        "  composed with firmware member(s): {}",
        composed.firmware_members.join(", ")
    );
    println!();
    println!("  formula `{}` over `{automaton_name}`", args.formula);
    println!("    states satisfying: {satisfying_count}/{total_states}");
    println!(
        "    initial states satisfying: {}/{}",
        initial_satisfying.len(),
        initial_states.len()
    );
    if !initial_states.is_empty() {
        println!("      initials: {}", initial_states.join(", "));
        if !initial_satisfying.is_empty() {
            println!("      satisfying: {}", initial_satisfying.join(", "));
        }
    }
    if all_initials_satisfying {
        println!("    verdict: HOLDS");
    } else {
        println!("    verdict: VIOLATED at initial state(s)");
    }
    Ok(())
}

fn codesign_emit_chaotic_stub(args: CodesignEmitChaoticStubArgs) -> Result<(), String> {
    use mununu_core::codesign::coupling::{CouplingOptions, emit_chaotic_stub_ctxdsl};
    use mununu_core::codesign::register_map::RegisterMap;

    let body = std::fs::read_to_string(&args.register_map)
        .map_err(|e| format!("failed to read {}: {e}", args.register_map.display()))?;
    let map: RegisterMap = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse register-map JSON: {e}"))?;

    let issues = map.validate();
    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("warning: {issue}");
        }
        if args.strict {
            return Err(format!(
                "--strict: {} register-map issue(s) — refusing to proceed",
                issues.len()
            ));
        }
    }

    let opts = CouplingOptions {
        peripheral_automaton: args.peripheral_automaton.as_deref(),
        ..Default::default()
    };
    let stub = emit_chaotic_stub_ctxdsl(&map, &opts);

    match args.output {
        Some(path) => std::fs::write(&path, &stub)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?,
        None => print!("{stub}"),
    }
    Ok(())
}

fn codesign_couple(args: CodesignCoupleArgs) -> Result<(), String> {
    use mununu_core::codesign::coupling::{CouplingOptions, emit_coupling_fragment};
    use mununu_core::codesign::register_map::RegisterMap;

    let body = std::fs::read_to_string(&args.register_map)
        .map_err(|e| format!("failed to read {}: {e}", args.register_map.display()))?;
    let map: RegisterMap = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse register-map JSON: {e}"))?;

    let issues = map.validate();
    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("warning: {issue}");
        }
        if args.strict {
            return Err(format!(
                "--strict: {} register-map issue(s) — refusing to proceed",
                issues.len()
            ));
        }
    }

    let firmware_refs: Vec<&str> = args.firmware_members.iter().map(String::as_str).collect();
    let opts = CouplingOptions {
        peripheral_automaton: args.peripheral_automaton.as_deref(),
        composition_name: args.composition_name.as_deref(),
        firmware_members: &firmware_refs,
    };
    let fragment = emit_coupling_fragment(&map, &opts);
    print!("{fragment}");
    Ok(())
}

fn contract_review(args: ContractReviewArgs) -> Result<(), String> {
    use mununu_core::contract::discover::{BlackBoxInterface, DiscoverOptions};
    use mununu_core::contract::review::{ProposalCounts, build_review_package};
    use mununu_core::corpus::Corpus;

    let body = std::fs::read_to_string(&args.interface)
        .map_err(|e| format!("failed to read {}: {e}", args.interface.display()))?;
    let iface: BlackBoxInterface =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse interface JSON: {e}"))?;

    let corpus = match &args.corpus {
        Some(root) => Some(
            Corpus::load(root)
                .map_err(|e| format!("failed to load corpus at {}: {e}", root.display()))?,
        ),
        None => None,
    };

    let opts = DiscoverOptions {
        force_controllable: &[],
        force_uncontrollable: &[],
        emit_fairness_gap: false,
        corpus: corpus.as_ref(),
    };
    let pkg = build_review_package(&iface, &opts);
    let counts = ProposalCounts::from_package(&pkg);

    if args.json {
        let rendered = serde_json::to_string_pretty(&pkg)
            .map_err(|e| format!("failed to serialise package: {e}"))?;
        println!("{rendered}");
        return Ok(());
    }

    println!(
        "review for module `{}` — {} proposal(s) [{} assume, {} guarantee, {} corpus]",
        pkg.module,
        counts.total(),
        counts.source_comment_assumptions,
        counts.source_comment_guarantees,
        counts.corpus_references,
    );
    println!(
        "  alphabet: {} label(s), {} gap marker(s)",
        pkg.phase1.labels.len(),
        pkg.phase1.gaps.len(),
    );
    if pkg.proposed_clauses.is_empty() {
        println!(
            "  (no proposals — interface has no @mununu_* clauses or resolved corpus entries)"
        );
        return Ok(());
    }
    for (i, p) in pkg.proposed_clauses.iter().enumerate() {
        println!();
        println!("[{}] {} ({} on {})", i + 1, p.id, p.kind, p.owner,);
        if let Some(desc) = &p.description {
            println!("    formula : {desc}");
        }
        println!(
            "    source  : {}",
            render_proposal_provenance(&p.provenance)
        );
        if let Some(note) = &p.soundness_note {
            println!("    note    : {note}");
        }
    }
    Ok(())
}

fn contract_query(args: ContractQueryArgs) -> Result<(), String> {
    use mununu_core::corpus::Corpus;
    use std::collections::BTreeMap;

    let (domain, name) = match args.id.split_once('/') {
        Some((d, n)) if !d.is_empty() && !n.is_empty() => (d, n),
        _ => {
            return Err(format!(
                "expected DOMAIN/NAME, got '{}' — example: rtl_protocol/axi4_slave",
                args.id
            ));
        }
    };

    let mut params: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for raw in &args.params {
        let (key, value_str) = raw
            .split_once('=')
            .ok_or_else(|| format!("--param expects KEY=VALUE, got '{raw}'"))?;
        let value: serde_json::Value = serde_json::from_str(value_str)
            .or_else(|_| Ok::<_, ()>(serde_json::Value::String(value_str.to_string())))
            .unwrap();
        params.insert(key.to_string(), value);
    }

    let corpus = if args.corpus.exists() {
        Corpus::load(&args.corpus)
            .map_err(|e| format!("failed to load corpus from {}: {e}", args.corpus.display()))?
    } else {
        eprintln!(
            "warning: corpus directory '{}' does not exist; using empty corpus",
            args.corpus.display()
        );
        Corpus::empty()
    };

    let hits = corpus.query(domain, name, &params);

    if args.json {
        let body = serde_json::to_string_pretty(&hits)
            .map_err(|e| format!("failed to serialise corpus hits: {e}"))?;
        println!("{body}");
        return Ok(());
    }

    if hits.is_empty() {
        println!("no contract entries match {domain}/{name} with the supplied parameters");
        return Ok(());
    }
    println!(
        "found {} contract candidate(s) for {domain}/{name}:",
        hits.len()
    );
    for (i, entry) in hits.iter().enumerate() {
        let provenance = match &entry.provenance {
            mununu_core::corpus::Provenance::MununuVerified { verified_against } => {
                format!(
                    "mununu-verified ({})",
                    verified_against.as_deref().unwrap_or("no reference")
                )
            }
            mununu_core::corpus::Provenance::Vendor { name, .. } => format!("vendor:{name}"),
            mununu_core::corpus::Provenance::Community { .. } => "community".to_string(),
        };
        println!(
            "  {}. {} @ {}  [{}]",
            i + 1,
            entry.id,
            entry.version,
            provenance,
        );
        if let Some(desc) = &entry.description {
            println!("       {desc}");
        }
    }
    Ok(())
}

fn contract_sidecars(args: ContractSidecarsArgs) -> Result<(), String> {
    use mununu_core::contract::discover::{
        BlackBoxInterface, DiscoverOptions, build_blackbox_sidecars,
    };
    use mununu_core::corpus::Corpus;

    let body = std::fs::read_to_string(&args.interfaces)
        .map_err(|e| format!("failed to read {}: {e}", args.interfaces.display()))?;
    let interfaces: Vec<BlackBoxInterface> = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse interfaces JSON (expected array): {e}"))?;

    std::fs::create_dir_all(&args.out_dir)
        .map_err(|e| format!("failed to create {}: {e}", args.out_dir.display()))?;

    let corpus = match &args.corpus {
        Some(root) => Some(
            Corpus::load(root)
                .map_err(|e| format!("failed to load corpus at {}: {e}", root.display()))?,
        ),
        None => None,
    };

    let opts = DiscoverOptions {
        force_controllable: &[],
        force_uncontrollable: &[],
        emit_fairness_gap: args.emit_fairness_gap,
        corpus: corpus.as_ref(),
    };
    let sidecars = build_blackbox_sidecars(&interfaces, &opts);

    for sidecar in &sidecars {
        let target = args.out_dir.join(&sidecar.filename);
        std::fs::write(&target, &sidecar.content)
            .map_err(|e| format!("failed to write {}: {e}", target.display()))?;
        println!("wrote: {}", target.display());
    }
    println!(
        "wrote {} sidecar(s) for {} black-box module(s)",
        sidecars.len(),
        interfaces.len(),
    );
    Ok(())
}

fn contract_discover(args: ContractDiscoverArgs) -> Result<(), String> {
    use mununu_core::contract::discover::{BlackBoxInterface, DiscoverOptions, discover_phase1};
    use mununu_core::corpus::Corpus;

    let body = std::fs::read_to_string(&args.interface)
        .map_err(|e| format!("failed to read {}: {e}", args.interface.display()))?;
    let iface: BlackBoxInterface =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse interface JSON: {e}"))?;

    let corpus = match &args.corpus {
        Some(root) => Some(
            Corpus::load(root)
                .map_err(|e| format!("failed to load corpus at {}: {e}", root.display()))?,
        ),
        None => None,
    };

    let force_c: Vec<&str> = args.force_controllable.iter().map(|s| s.as_str()).collect();
    let force_u: Vec<&str> = args
        .force_uncontrollable
        .iter()
        .map(|s| s.as_str())
        .collect();
    let opts = DiscoverOptions {
        force_controllable: &force_c,
        force_uncontrollable: &force_u,
        emit_fairness_gap: args.emit_fairness_gap,
        corpus: corpus.as_ref(),
    };
    let output = discover_phase1(&iface, &opts);
    output.gaps.emit_diagnostics();

    if let Some(source) = &args.write_sidecar {
        let target = output
            .gaps
            .write_todo_sidecar(source)
            .map_err(|e| format!("failed to write contract.todo.json: {e}"))?;
        println!("wrote sidecar: {}", target.display());
    }

    if args.json {
        let rendered = serde_json::to_string_pretty(&output)
            .map_err(|e| format!("failed to serialise output: {e}"))?;
        println!("{rendered}");
    } else {
        println!(
            "phase-1 discovery: {} label(s), {} gap marker(s) for module `{}`",
            output.labels.len(),
            output.gaps.len(),
            output.module
        );
        for res in &output.corpus_resolutions {
            match res.status {
                mununu_core::contract::discover::ResolutionStatus::Resolved => {
                    let alt = match res.alternative_matched {
                        Some(true) => format!(
                            " [alt `{}` ok]",
                            res.parsed.alternative.as_deref().unwrap_or("")
                        ),
                        Some(false) => format!(
                            " [alt `{}` MISSING on entry]",
                            res.parsed.alternative.as_deref().unwrap_or("")
                        ),
                        None => String::new(),
                    };
                    println!(
                        "  corpus: resolved `{}` → {}{alt}",
                        res.raw_uri,
                        res.matched_ids.join(", "),
                    );
                }
                mununu_core::contract::discover::ResolutionStatus::NotFound => {
                    println!(
                        "  corpus: `{}` not found ({}/{}{})",
                        res.raw_uri,
                        res.parsed.domain,
                        res.parsed.name,
                        res.parsed
                            .version
                            .as_ref()
                            .map(|v| format!("@{v}"))
                            .unwrap_or_default(),
                    );
                }
                mununu_core::contract::discover::ResolutionStatus::NoCorpus => {
                    println!(
                        "  corpus: `{}` referenced but no corpus supplied (use --corpus)",
                        res.raw_uri,
                    );
                }
                mununu_core::contract::discover::ResolutionStatus::Malformed => {
                    println!("  corpus: `{}` malformed URI", res.raw_uri);
                }
                mununu_core::contract::discover::ResolutionStatus::SidecarReference => {
                    println!(
                        "  corpus: `{}` is a sidecar reference (skipped)",
                        res.raw_uri
                    );
                }
            }
        }
    }

    if args.strict_contracts && output.gaps.is_strict_failure() {
        return Err(format!(
            "--strict-contracts: {} unresolved contract gap(s) — refusing to proceed",
            output.gaps.len()
        ));
    }
    Ok(())
}

fn contract_gaps(args: ContractGapsArgs) -> Result<(), String> {
    use mununu_core::contract::gap::GapMarkerReport;

    let body = std::fs::read_to_string(&args.gap_report)
        .map_err(|e| format!("failed to read {}: {e}", args.gap_report.display()))?;
    let report: GapMarkerReport =
        serde_json::from_str(&body).map_err(|e| format!("failed to parse gap report JSON: {e}"))?;

    report.emit_diagnostics();

    if let Some(source) = &args.write_sidecar {
        let target = report
            .write_todo_sidecar(source)
            .map_err(|e| format!("failed to write contract.todo.json sidecar: {e}"))?;
        println!("wrote sidecar: {}", target.display());
    }

    if args.json {
        let rendered = serde_json::to_string_pretty(&report)
            .map_err(|e| format!("failed to serialise report: {e}"))?;
        println!("{rendered}");
    } else {
        println!(
            "gap report: {} marker(s) across {} module(s)",
            report.len(),
            report.by_module().len(),
        );
    }

    if args.strict_contracts && report.is_strict_failure() {
        return Err(format!(
            "--strict-contracts: {} unresolved contract gap(s) — refusing to proceed",
            report.len()
        ));
    }
    Ok(())
}

fn contract_validate(args: ContractValidateArgs) -> Result<(), String> {
    use mununu_core::contract::{ContractSet, discharge};

    let body = std::fs::read_to_string(&args.contract_set)
        .map_err(|e| format!("failed to read {}: {e}", args.contract_set.display()))?;
    let set: ContractSet = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse contract set JSON: {e}"))?;
    let verdict = discharge::validate(&set);

    if args.json {
        let rendered = serde_json::to_string_pretty(&verdict)
            .map_err(|e| format!("failed to serialise verdict: {e}"))?;
        println!("{rendered}");
        return Ok(());
    }

    render_discharge_verdict_text(&verdict);
    Ok(())
}

fn handle_sv(command: SvCommand) -> Result<(), String> {
    match command {
        SvCommand::Init(args) => {
            if args.multi {
                sv_init_multi(args)
            } else {
                sv_init(args)
            }
        }
        SvCommand::Discover(args) => sv_discover(args),
        SvCommand::Preprocess(args) => sv_preprocess(args),
        SvCommand::EmitBtor2PerModule(args) => sv_emit_btor2_per_module(args),
        SvCommand::ComparePipelines(args) => sv_compare_pipelines(args),
    }
}

fn sv_compare_pipelines(args: SvComparePipelinesArgs) -> Result<(), String> {
    use std::collections::HashMap;
    use std::fs;

    let content = fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read '{}': {e}", args.file.display()))?;
    let mut additional: HashMap<String, String> = HashMap::new();
    for src in &args.sources {
        let name = src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Invalid additional source path: {}", src.display()))?;
        let body = fs::read_to_string(src)
            .map_err(|e| format!("Failed to read additional source '{}': {e}", src.display()))?;
        additional.insert(name.to_string(), body);
    }
    let record = mununu_core::adapter::sv_pipeline_compare::compare_pipelines(
        &content,
        &args.file,
        &additional,
        args.top.as_deref(),
    );
    let json = serde_json::to_string_pretty(&record)
        .map_err(|e| format!("Failed to serialise comparison record: {e}"))?;
    println!("{json}");
    Ok(())
}

fn sv_emit_btor2_per_module(args: SvEmitBtor2PerModuleArgs) -> Result<(), String> {
    use std::collections::HashMap;
    use std::fs;

    let primary_content = fs::read_to_string(&args.file).map_err(|e| {
        format!(
            "Failed to read primary source '{}': {e}",
            args.file.display()
        )
    })?;
    let mut additional: HashMap<String, String> = HashMap::new();
    for src in &args.sources {
        let name = src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Invalid additional source path: {}", src.display()))?;
        let body = fs::read_to_string(src)
            .map_err(|e| format!("Failed to read additional source '{}': {e}", src.display()))?;
        additional.insert(name.to_string(), body);
    }
    let output_dir = args.output_dir.clone().unwrap_or_else(|| {
        args.file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    });

    let yopts = mununu_core::adapter::yosys::YosysOptions {
        top: args.top.clone(),
        additional_sources: additional.into_iter().collect(),
        primary_source_path: Some(args.file.display().to_string()),
        use_sv2v: args.preprocess_sv2v,
        setundef_anyseq: args.setundef_anyseq,
        setundef_anyconst: args.setundef_anyconst,
        per_module_btor: true,
        per_module_output_dir: Some(output_dir.clone()),
        ..Default::default()
    };
    let opts = mununu_core::adapter::AdapterOptions::default();
    let outputs =
        mununu_core::adapter::yosys::translate_sv_per_module(&primary_content, &opts, &yopts)
            .map_err(|e| format!("sv emit-btor2-per-module: {e}"))?;

    println!(
        "Emitted {} BTOR2 file(s) to {}",
        outputs.len(),
        output_dir.display()
    );
    for per_module in &outputs {
        let path_display = per_module
            .btor2_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(transient)".to_string());
        println!(
            "  {} -> {} (state_count={}, property_count={})",
            per_module.module_name,
            path_display,
            per_module.output.source_info.state_count,
            per_module.output.source_info.property_count,
        );
    }
    Ok(())
}

fn sv_preprocess(args: SvPreprocessArgs) -> Result<(), String> {
    if args.files.is_empty() {
        return Err("sv preprocess: at least one .sv input file required".to_string());
    }
    let output_path = args.output.unwrap_or_else(|| {
        let first = &args.files[0];
        let stem = first
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("module");
        first.with_file_name(format!("{stem}.elab.v"))
    });
    let sv2v =
        mununu_core::adapter::yosys::preprocess_sv(&args.files, &args.include_dirs, &output_path)
            .map_err(|e| format!("sv preprocess: {e}"))?;
    println!(
        "sv2v ({}) -> {} (inputs: {})",
        sv2v.display(),
        output_path.display(),
        args.files.len()
    );
    Ok(())
}

fn sv_init(args: SvInitArgs) -> Result<(), String> {
    use mununu_core::adapter::systemverilog::annotation::*;

    let source = fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read '{}': {e}", args.file.display()))?;

    let module = mununu_core::adapter::systemverilog::parser::parse(&source)
        .map_err(|e| format!("SV parse error: {e}"))?;

    let output_path = args.output.unwrap_or_else(|| {
        let stem = args
            .file
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("module");
        args.file.with_file_name(format!("{stem}.mununu.json"))
    });

    if output_path.exists() && !args.force {
        return Err(format!(
            "'{}' already exists. Use --force to overwrite.",
            output_path.display()
        ));
    }

    let signals = build_signal_annotations(&module);
    let inputs = build_input_annotations(&module);

    let ann = SvAnnotation {
        schema: Some("mununu_sv_annotation_v1".to_string()),
        module: module.name.clone(),
        source: Some(
            args.file
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("")
                .to_string(),
        ),
        signals,
        inputs,
        controllable: vec![],
        properties: vec![PropertyAnnotation {
            id: "safety".to_string(),
            formula: Some("nu X. ([] X)".to_string()),
            description: Some("No deadlock — all states have successors".to_string()),
            role: "guarantee".to_string(),
            template_ref: None,
        }],
        discovered_values: HashMap::new(),
        parameters: module
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.default_value))
            .collect(),
        reset_sequence: None,
        memories: Vec::new(),
        uf_wrap: Vec::new(),
        uf_unwrap: Vec::new(),
    };

    let json =
        serde_json::to_string_pretty(&ann).map_err(|e| format!("Failed to serialize: {e}"))?;
    fs::write(&output_path, json)
        .map_err(|e| format!("Failed to write '{}': {e}", output_path.display()))?;

    eprintln!("Generated sidecar: {}", output_path.display());
    eprintln!(
        "  {} signal(s), {} input(s), {} property/ies",
        ann.signals.len(),
        ann.inputs.len(),
        ann.properties.len()
    );
    eprintln!(
        "Review the file, then run: mununu sv discover {}",
        args.file.display()
    );

    Ok(())
}

fn sv_init_multi(args: SvInitArgs) -> Result<(), String> {
    let source = fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read '{}': {e}", args.file.display()))?;
    let module = mununu_core::adapter::systemverilog::parser::parse(&source)
        .map_err(|e| format!("SV parse error in '{}': {e}", args.file.display()))?;

    if module.instantiations.is_empty() {
        return Err(format!(
            "'{}' has no module instantiations. Provide a top module that \
             instantiates the sub-modules to verify, e.g.:\n\n\
             module top(input clk, input rst);\n    \
             module_a inst_a(.clk(clk), .out(wire_x));\n    \
             module_b inst_b(.clk(clk), .in(wire_x));\n\
             endmodule",
            args.file.display()
        ));
    }

    sv_init_multi_from_top(args, module)
}

/// Top-module mode: derive connections from wire bindings in instantiations.
fn sv_init_multi_from_top(
    args: SvInitArgs,
    top_module: mununu_core::adapter::systemverilog::ast::Module,
) -> Result<(), String> {
    use mununu_core::adapter::systemverilog::annotation::*;
    use mununu_core::adapter::systemverilog::ast::PortDirection;

    let top_dir = args.file.parent().unwrap_or(std::path::Path::new("."));

    eprintln!(
        "Top-module mode: '{}' has {} instantiation(s)",
        top_module.name,
        top_module.instantiations.len()
    );

    // Step 1: Locate and parse each instantiated sub-module
    struct SubModule {
        module: mununu_core::adapter::systemverilog::ast::Module,
        path: PathBuf,
        signals: Vec<SignalAnnotation>,
        inputs: Vec<InputAnnotation>,
        parameters: HashMap<String, i64>,
    }

    let mut sub_modules: HashMap<String, SubModule> = HashMap::new();

    for inst in &top_module.instantiations {
        if sub_modules.contains_key(&inst.module_type) {
            continue; // Already parsed this module type
        }
        // Sanitize module type to prevent path traversal (e.g., "../../etc/passwd")
        let safe_module_name = std::path::Path::new(&inst.module_type)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let sv_path = top_dir.join(format!("{}.sv", safe_module_name));
        if !sv_path.exists() {
            eprintln!(
                "  warning: cannot find '{}' for module type '{}' — skipping",
                sv_path.display(),
                inst.module_type
            );
            continue;
        }
        let source = fs::read_to_string(&sv_path)
            .map_err(|e| format!("Failed to read '{}': {e}", sv_path.display()))?;
        let module = mununu_core::adapter::systemverilog::parser::parse(&source)
            .map_err(|e| format!("SV parse error in '{}': {e}", sv_path.display()))?;

        let signals = build_signal_annotations(&module);
        let inputs = build_input_annotations(&module);

        let parameters = module
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.default_value))
            .collect();

        sub_modules.insert(
            inst.module_type.clone(),
            SubModule {
                module,
                path: sv_path,
                signals,
                inputs,
                parameters,
            },
        );
    }

    // Step 2: Build wire map from instantiation port bindings
    // wire_name → Vec<(instance_name, module_type, port_name)>
    let mut wire_map: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for inst in &top_module.instantiations {
        for conn in &inst.port_connections {
            wire_map.entry(conn.signal_name.clone()).or_default().push((
                inst.instance_name.clone(),
                inst.module_type.clone(),
                conn.port_name.clone(),
            ));
        }
    }

    // Step 3: Derive connections from shared wires
    let mut connections = Vec::new();
    let mut connected_inputs: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for (wire_name, bindings) in &wire_map {
        // Skip clk/rst wires
        if *wire_name == "clk" || *wire_name == "rst" || *wire_name == "rst_n" {
            continue;
        }

        // Find output drivers and input receivers
        let mut outputs = Vec::new();
        let mut inputs_on_wire = Vec::new();

        for (inst_name, mod_type, port_name) in bindings {
            if let Some(sub) = sub_modules.get(mod_type)
                && let Some(port) = sub.module.ports.iter().find(|p| p.name == *port_name)
            {
                match port.direction {
                    PortDirection::Output => {
                        outputs.push((inst_name, mod_type, port_name, port.width))
                    }
                    PortDirection::Input => {
                        inputs_on_wire.push((inst_name, mod_type, port_name, port.width))
                    }
                    _ => {}
                }
            }
        }

        // Create connection for each output→input pair on the same wire
        for (out_inst, out_mod, out_port, width) in &outputs {
            for (in_inst, in_mod, in_port, _) in &inputs_on_wire {
                let (abstraction, bound) = sv_annotation::abstract_width(*width);
                connections.push(ConnectionSpec {
                    from: format!("{}.{}", out_mod, out_port),
                    to: format!("{}.{}", in_mod, in_port),
                    abstraction,
                    bound,
                    variants: None,
                    value_map: None,
                    note: Some(format!(
                        "wire '{}': {}.{} → {}.{}",
                        wire_name, out_inst, out_port, in_inst, in_port
                    )),
                });
                connected_inputs.insert((in_mod.to_string(), in_port.to_string()));
            }
        }
    }

    // Step 4: Build module entries
    let mut module_entries = Vec::new();
    let mut seen_types = std::collections::HashSet::new();
    for inst in &top_module.instantiations {
        if !seen_types.insert(inst.module_type.clone()) {
            continue; // Skip duplicate module types
        }
        if let Some(sub) = sub_modules.get(&inst.module_type) {
            let remaining_inputs: Vec<InputAnnotation> = sub
                .inputs
                .iter()
                .filter(|inp| {
                    !connected_inputs.contains(&(inst.module_type.clone(), inp.name.clone()))
                })
                .cloned()
                .collect();
            module_entries.push(ModuleEntry {
                name: inst.module_type.clone(),
                source: sub
                    .path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("")
                    .to_string(),
                clock_domain: None,
                signals: sub.signals.clone(),
                inputs: remaining_inputs,
                controllable: vec![],
                parameters: sub.parameters.clone(),
                discovered_values: HashMap::new(),
            });
        }
    }

    // Step 5: Emit sidecar
    let ann = MultiModuleSvAnnotation {
        schema: Some("mununu_sv_multi_v1".to_string()),
        modules: module_entries,
        connections,
        composition: Some(CompositionConfig {
            mode: "synchronous".to_string(),
            name: "system".to_string(),
        }),
        properties: vec![PropertyAnnotation {
            id: "safety".to_string(),
            formula: Some("nu X. ([] X)".to_string()),
            description: Some("No deadlock — all states have successors".to_string()),
            role: "guarantee".to_string(),
            template_ref: None,
        }],
        discovered_values: HashMap::new(),
        blackbox_modules: Vec::new(),
    };

    let output_path = args.output.unwrap_or_else(|| {
        let stem = args
            .file
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("system");
        args.file
            .with_file_name(format!("{stem}_system.mununu.json"))
    });
    if output_path.exists() && !args.force {
        return Err(format!(
            "'{}' already exists. Use --force to overwrite.",
            output_path.display()
        ));
    }

    let json =
        serde_json::to_string_pretty(&ann).map_err(|e| format!("Failed to serialize: {e}"))?;
    fs::write(&output_path, json)
        .map_err(|e| format!("Failed to write '{}': {e}", output_path.display()))?;

    eprintln!(
        "Generated multi-module sidecar (from top): {}",
        output_path.display()
    );
    eprintln!(
        "  {} module(s), {} connection(s)",
        ann.modules.len(),
        ann.connections.len()
    );
    for conn in &ann.connections {
        eprintln!(
            "  connection: {} → {} ({})",
            conn.from,
            conn.to,
            conn.note.as_deref().unwrap_or("")
        );
    }

    Ok(())
}

fn sv_discover(args: SvDiscoverArgs) -> Result<(), String> {
    // Check if this is a multi-module sidecar
    if args.multi
        || args
            .file
            .extension()
            .is_some_and(|ext| ext == "json" || ext == "mununu.json")
    {
        return sv_discover_multi(args);
    }

    // Step 1: Parse the SV file
    let source = fs::read_to_string(&args.file)
        .map_err(|e| format!("Failed to read '{}': {e}", args.file.display()))?;

    let _module = mununu_core::adapter::systemverilog::parser::parse(&source)
        .map_err(|e| format!("SV parse error: {e}"))?;

    eprintln!("Parsed module: {}", _module.name);

    // Step 2: Load or find the sidecar annotation
    let sidecar_path = if let Some(ref p) = args.annotation {
        p.clone()
    } else {
        mununu_core::adapter::systemverilog::annotation::find_sidecar(&args.file).ok_or_else(
            || {
                let stem = args
                    .file
                    .file_stem()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or("");
                format!(
                    "No .mununu.json sidecar found. Create one at '{stem}.mununu.json' \
                     or pass --annotation <path>."
                )
            },
        )?
    };

    let annotation =
        mununu_core::adapter::systemverilog::annotation::load_annotation(&sidecar_path)
            .map_err(|e| format!("Failed to load sidecar: {e}"))?;

    eprintln!("Loaded annotation: {}", sidecar_path.display());

    use mununu_core::adapter::systemverilog::annotation::SignalAbstraction;

    // Count signals + inputs marked for discovery
    let discover_count = annotation
        .signals
        .iter()
        .filter(|s| s.preserve && s.abstraction == SignalAbstraction::Discover)
        .count()
        + annotation
            .inputs
            .iter()
            .filter(|i| i.preserve && i.abstraction == SignalAbstraction::Discover)
            .count();

    if discover_count == 0 {
        eprintln!("No signals marked for discovery (abstraction: \"discover\"). Nothing to do.");
        return Ok(());
    }

    eprintln!("Discovering values for {} signal(s)...", discover_count);

    let mut annotation = annotation;
    let results = mununu_core::adapter::systemverilog::kripke_smt::discover_significant_values(
        &_module,
        &annotation,
    );

    if results.is_empty() {
        eprintln!("No significant values discovered.");
    } else {
        for (signal, discovered) in &results {
            eprintln!("  {} — {} value(s):", signal, discovered.values.len());
            for v in &discovered.values {
                let from = v.from.as_deref().unwrap_or("unknown");
                eprintln!("    {} = {} ({})", v.name, v.value, from);
            }
        }

        // Merge into the annotation, preserving user-given names
        sv_annotation::merge_discovered_values(&mut annotation.discovered_values, results);
    }

    // Write the updated sidecar
    let output_path = args.output.as_ref().unwrap_or(&sidecar_path);
    let json = serde_json::to_string_pretty(&annotation)
        .map_err(|e| format!("Failed to serialize annotation: {e}"))?;
    fs::write(output_path, json)
        .map_err(|e| format!("Failed to write '{}': {e}", output_path.display()))?;
    eprintln!("Updated sidecar: {}", output_path.display());

    Ok(())
}

/// Multi-module discovery: runs cross-module SMT discovery for connected
/// signals marked with `"abstraction": "discover"`, and per-module discovery
/// for non-connected signals.
fn sv_discover_multi(args: SvDiscoverArgs) -> Result<(), String> {
    use mununu_core::adapter::systemverilog::annotation::*;

    // Load the multi-module sidecar
    let sidecar_path = &args.file;
    let is_multi = is_multi_module(sidecar_path)
        .map_err(|e| format!("Failed to check sidecar format: {e}"))?;
    if !is_multi {
        return Err(format!(
            "'{}' is not a multi-module sidecar (missing \"modules\" key). \
             Use without --multi for single-module discovery.",
            sidecar_path.display()
        ));
    }

    #[allow(unused_mut)]
    let mut ann = load_multi_annotation(sidecar_path)
        .map_err(|e| format!("Failed to load multi-module sidecar: {e}"))?;

    #[allow(unused_variables)]
    let sidecar_dir = sidecar_path.parent().unwrap_or(std::path::Path::new("."));

    eprintln!(
        "Multi-module discovery: {} module(s), {} connection(s)",
        ann.modules.len(),
        ann.connections.len()
    );

    // Count discover targets
    let mut discover_count = 0;
    for module_entry in &ann.modules {
        discover_count += module_entry
            .signals
            .iter()
            .filter(|s| s.preserve && s.abstraction == SignalAbstraction::Discover)
            .count();
        discover_count += module_entry
            .inputs
            .iter()
            .filter(|i| i.preserve && i.abstraction == SignalAbstraction::Discover)
            .count();
    }
    for conn in &ann.connections {
        if conn.abstraction == SignalAbstraction::Discover {
            discover_count += 1;
        }
    }

    if discover_count == 0 {
        eprintln!("No signals or connections marked for discovery. Nothing to do.");
        return Ok(());
    }

    eprintln!("Discovering values for {} target(s)...", discover_count);

    {
        // Parse all sub-modules
        let mut parsed_modules: Vec<(mununu_core::adapter::systemverilog::ast::Module, String)> =
            Vec::new();
        let mut param_map: std::collections::HashMap<
            String,
            std::collections::HashMap<String, i64>,
        > = std::collections::HashMap::new();

        for module_entry in &ann.modules {
            // Sanitize source path to prevent path traversal from .mununu.json
            let safe_source = std::path::Path::new(&module_entry.source)
                .file_name()
                .unwrap_or_default();
            let sv_path = sidecar_dir.join(safe_source);
            let source = fs::read_to_string(&sv_path)
                .map_err(|e| format!("Failed to read '{}': {e}", sv_path.display()))?;
            let module = mununu_core::adapter::systemverilog::parser::parse(&source)
                .map_err(|e| format!("Parse error in '{}': {e}", sv_path.display()))?;
            param_map.insert(module_entry.name.clone(), module_entry.parameters.clone());
            parsed_modules.push((module, module_entry.name.clone()));
        }

        // Run per-module discovery for non-connected signals
        for (module, mod_name) in &parsed_modules {
            let Some(module_entry) = ann.modules.iter().find(|m| m.name == *mod_name) else {
                eprintln!(
                    "  warning: no annotation entry for module '{}' — skipping discovery",
                    mod_name
                );
                continue;
            };

            // Build a temporary single-module annotation for per-module discovery
            let temp_ann = SvAnnotation {
                schema: None,
                module: mod_name.clone(),
                source: Some(module_entry.source.clone()),
                signals: module_entry.signals.clone(),
                inputs: module_entry.inputs.clone(),
                controllable: module_entry.controllable.clone(),
                properties: vec![],
                discovered_values: module_entry.discovered_values.clone(),
                parameters: module_entry.parameters.clone(),
                reset_sequence: None,
                memories: Vec::new(),
                uf_wrap: Vec::new(),
                uf_unwrap: Vec::new(),
            };

            let results =
                mununu_core::adapter::systemverilog::kripke_smt::discover_significant_values(
                    module, &temp_ann,
                );

            if !results.is_empty() {
                eprintln!(
                    "  Module '{}': discovered {} signal(s)",
                    mod_name,
                    results.len()
                );
                for (signal, discovered) in &results {
                    eprintln!("    {} — {} value(s)", signal, discovered.values.len());
                    for v in &discovered.values {
                        eprintln!(
                            "      {} = {} ({})",
                            v.name,
                            v.value,
                            v.from.as_deref().unwrap_or("unknown")
                        );
                    }
                }
                // Merge into module's discovered_values
                let Some(entry) = ann.modules.iter_mut().find(|m| m.name == *mod_name) else {
                    continue;
                };
                sv_annotation::merge_discovered_values(&mut entry.discovered_values, results);
            }
        }

        // Run cross-module discovery for connected signals
        let module_refs: Vec<(&mununu_core::adapter::systemverilog::ast::Module, &str)> =
            parsed_modules
                .iter()
                .map(|(m, name)| (m, name.as_str()))
                .collect();

        let cross_results =
            mununu_core::adapter::systemverilog::kripke_smt::engine::discover_cross_module_values(
                &module_refs,
                &ann.connections,
                &param_map,
            );

        if !cross_results.is_empty() {
            eprintln!(
                "  Cross-module: discovered {} connection(s)",
                cross_results.len()
            );
            for (key, discovered) in &cross_results {
                eprintln!("    {} — {} value(s)", key, discovered.values.len());
                for v in &discovered.values {
                    eprintln!(
                        "      {} = {} ({})",
                        v.name,
                        v.value,
                        v.from.as_deref().unwrap_or("unknown")
                    );
                }
            }
            // Merge into top-level discovered_values
            sv_annotation::merge_discovered_values(&mut ann.discovered_values, cross_results);
        }

        // Write updated sidecar
        let output_path = args.output.as_ref().unwrap_or(sidecar_path);
        let json =
            serde_json::to_string_pretty(&ann).map_err(|e| format!("Failed to serialize: {e}"))?;
        fs::write(output_path, json)
            .map_err(|e| format!("Failed to write '{}': {e}", output_path.display()))?;
        eprintln!("Updated multi-module sidecar: {}", output_path.display());

        Ok(())
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

#[derive(Debug, Serialize, Default)]
struct ModalityBreakdown {
    sharp: usize,
    may_only: usize,
    must_hyper_only: usize,
}

#[derive(Debug, Serialize)]
struct AutomatonSummaryOutput {
    name: String,
    state_count: usize,
    transition_count: usize,
    /// R.5 Item K sub-item K.4 (2026-06-05) — per-automaton KMTS
    /// modality breakdown sourced from the CTXDSL AST. Sharp is the
    /// pre-K.4 default; non-zero `may_only` / `must_hyper_only`
    /// indicates the automaton declares KMTS-shaped transitions via
    /// the K.1 attribute syntax. Serialized verbatim into the
    /// `context summarize --format json` output.
    #[serde(skip_serializing_if = "modality_breakdown_is_sharp_only")]
    modality_breakdown: ModalityBreakdown,
}

/// R.5 Item K sub-item K.4 — serde skip predicate. Keeps pre-K.4
/// JSON byte-for-byte compatible: when an automaton only carries
/// Sharp transitions (the dominant case), the `modality_breakdown`
/// field is omitted entirely.
fn modality_breakdown_is_sharp_only(b: &ModalityBreakdown) -> bool {
    b.may_only == 0 && b.must_hyper_only == 0
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

fn build_context_summary(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
) -> ContextSummaryOutput {
    let mut automata_map: BTreeMap<String, AutomatonSummaryOutput> = BTreeMap::new();
    for doc in std::iter::once(context_doc).chain(sidecar_docs.iter()) {
        for automaton in &doc.automata {
            let mut breakdown = ModalityBreakdown::default();
            for t in &automaton.transitions {
                match t.modality {
                    TransitionModalitySpec::Sharp => breakdown.sharp += 1,
                    TransitionModalitySpec::MayOnly => breakdown.may_only += 1,
                    TransitionModalitySpec::MustOnly => breakdown.must_hyper_only += 1,
                }
            }
            automata_map
                .entry(automaton.name.name.clone())
                .or_insert_with(|| AutomatonSummaryOutput {
                    name: automaton.name.name.clone(),
                    state_count: automaton.states.len(),
                    transition_count: automaton.transitions.len(),
                    modality_breakdown: breakdown,
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
    let (context_doc, sidecar_docs, _) =
        load_context_documents(&context_path, &sidecar_paths, None)?;
    let realized = realize_documents(&context_doc, &sidecar_docs)?;
    let summary = build_context_summary(&context_doc, &sidecar_docs, &realized);

    println!("Merged context '{}'", summary.context);
    println!("  Sidecars: {}", summary.sidecar_count);
    println!("  Automata:");
    if summary.automata.is_empty() {
        println!("    (none)");
    } else {
        for automaton in &summary.automata {
            let breakdown = &automaton.modality_breakdown;
            let modality_suffix = if modality_breakdown_is_sharp_only(breakdown) {
                String::new()
            } else {
                // R.5 Item K sub-item K.4 (2026-06-05) — surface KMTS
                // modality breakdown when the automaton declares non-Sharp
                // transitions via the K.1 attribute syntax. Pre-K.4
                // automata (Sharp-only) print the original output
                // byte-for-byte.
                format!(
                    " [modality: sharp={}, may_only={}, must_hyper_only={}]",
                    breakdown.sharp, breakdown.may_only, breakdown.must_hyper_only
                )
            };
            println!(
                "    - {} (states: {}, transitions: {}){}",
                automaton.name, automaton.state_count, automaton.transition_count, modality_suffix
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
    let preprocessor = validate_preprocessor(args.preprocessor.as_deref())?;
    let (context_doc, sidecar_docs, _) = load_context_documents_mode(
        &args.context,
        &args.sidecars,
        args.adapter.as_deref(),
        args.mode.as_deref(),
        preprocessor,
    )?;
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
    let preprocessor = validate_preprocessor(args.preprocessor.as_deref())?;
    let (context_doc, sidecar_docs, _) = load_context_documents_mode(
        &args.context,
        &args.sidecars,
        args.adapter.as_deref(),
        args.mode.as_deref(),
        preprocessor,
    )?;
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
    let preprocessor = validate_preprocessor(args.preprocessor.as_deref())?;
    let PreparedEvalContext {
        realized,
        formula_name,
    } = prepare_eval_context(EvalContextParams {
        context: &args.context,
        sidecars: &args.sidecars,
        adapter: args.adapter.as_deref(),
        mode: args.mode.as_deref(),
        preprocessor,
        print_ctxdsl_path: args.print_ctxdsl.as_ref(),
        stubs: &args.stubs,
        formula: &args.formula,
        template: &args.template,
        template_args: &args.template_args,
        automaton: &args.automaton,
    })?;
    let formula = realized
        .formulas
        .get(&formula_name)
        .ok_or_else(|| format!("unknown formula '{}' in realised context", formula_name))?;
    let clts = realized
        .context
        .clts(&args.automaton)
        .ok_or_else(|| format!("unknown automaton '{}' in realised context", args.automaton))?;

    // Apply adaptation: hiding + minimization (before evaluation)
    // If adaptation is applied, evaluate on the adapted CLTS directly.
    let adapted_clts: Option<
        mununu_core::clts::Clts<
            mununu_core::clts::DefaultStateIdx,
            mununu_core::clts::DefaultLabelIdx,
        >,
    > = {
        let need_adaptation = !args.hide.is_empty() || args.minimize;
        if need_adaptation {
            let mut working = if !args.hide.is_empty() {
                let hide_set: std::collections::HashSet<String> =
                    args.hide.iter().cloned().collect();
                let (hidden, stats) =
                    mununu_core::composition::hide::hide_labels_with_stats(clts, &hide_set)
                        .map_err(|e| format!("label hiding failed: {e}"))?;
                eprintln!(
                    "Hidden {} label(s) out of {} total",
                    stats.labels_hidden, stats.total_labels
                );
                hidden
            } else {
                // Clone the original CLTS for minimization
                // (minimization needs ownership; we can't modify the realized context)
                // Use hide with empty set as identity clone
                mununu_core::composition::hide::hide_labels(clts, &std::collections::HashSet::new())
                    .map_err(|e| format!("CLTS copy failed: {e}"))?
            };

            if args.minimize {
                match mununu_core::composition::minimize::minimize_bisimulation(&working, None)
                    .map_err(|e| format!("minimization failed: {e}"))?
                {
                    Some((minimized, report)) => {
                        eprintln!(
                            "Minimized: {} → {} states ({} removed), {} → {} transitions",
                            report.states_before,
                            report.states_after,
                            report.states_before - report.states_after,
                            report.transitions_before,
                            report.transitions_after,
                        );
                        working = minimized;
                    }
                    None => {
                        eprintln!("Minimization: already minimal (no reduction)");
                    }
                }
            }

            Some(working)
        } else {
            None
        }
    };

    // Choose which CLTS to evaluate on
    let eval_clts = adapted_clts.as_ref().unwrap_or(clts);

    // Build environment matching the CLTS state count.
    // For adapted CLTSs, register state-name predicates so formulas
    // referencing state names (e.g., `!Closed`) resolve correctly.
    let env = if adapted_clts.is_some() {
        let sc = eval_clts.state_count();
        let mut env = mununu_core::mu_calculus::Environment::new(sc);
        for state_id in eval_clts.states() {
            if let Some(name) = eval_clts.state_name(state_id) {
                let mut bits = bitvec::vec::BitVec::<usize, bitvec::order::Lsb0>::repeat(false, sc);
                bits.set(state_id.index(), true);
                env = env.with_predicate(name.to_string(), bits);
            }
        }
        env
    } else {
        realized.environment_for(&args.automaton)
    };

    let mut options = EvaluationOptions::default();
    if args.no_partitions {
        options.use_partitions = false;
    }

    let result = mununu_core::mu_calculus::evaluate_with_options(
        &formula.formula,
        eval_clts,
        &env,
        &options,
    )
    .map_err(|err| format!("μ-calculus evaluation failed: {err}"))?;

    let mut satisfying = Vec::new();
    for state_id in eval_clts.states() {
        if result
            .get(state_id.index())
            .map(|bit| *bit)
            .unwrap_or(false)
            && let Some(name) = eval_clts.state_name(state_id)
        {
            satisfying.push(name.to_string());
        }
    }
    satisfying.sort();

    let initial_states: Vec<String> = eval_clts
        .initial_states()
        .iter()
        .filter_map(|state_id| eval_clts.state_name(*state_id).map(|name| name.to_string()))
        .collect();

    let mut initial_satisfying: Vec<String> = eval_clts
        .initial_states()
        .iter()
        .filter_map(|state_id| {
            if result
                .get(state_id.index())
                .map(|bit| *bit)
                .unwrap_or(false)
            {
                eval_clts.state_name(*state_id).map(|name| name.to_string())
            } else {
                None
            }
        })
        .collect();
    initial_satisfying.sort();

    // Counterexample-style output: when some initial states violate
    // the property, surface them by name AND by structured valuation
    // (`signal = value` pairs from the BTOR2 lifter's cross-product
    // enumeration). Lets the engineer read "init state s_init_5 has
    // boot_fsm_ns = UNMATCHED_5" directly from the verdict instead of
    // mentally decoding state IDs against the cell domains. Empty
    // when every initial state satisfies, or when no state carries a
    // structured valuation (most CTXDSL-only fixtures).
    let initial_violating: Vec<(String, Option<BTreeMap<String, String>>)> = eval_clts
        .initial_states()
        .iter()
        .filter_map(|state_id| {
            let satisfies = result
                .get(state_id.index())
                .map(|bit| *bit)
                .unwrap_or(false);
            if satisfies {
                return None;
            }
            let name = eval_clts.state_name(*state_id)?.to_string();
            let valuation = eval_clts.state_valuation(*state_id).cloned();
            Some((name, valuation))
        })
        .collect();

    // GAP-009: vacuous-property warning. Surfaced by the 2026-05-04
    // compositional validations on MCP-001 / MCP-005, where per-instance
    // automata collapsed to 1 state and verdicts numerically matched the
    // hand baselines by coincidence. The check + message are extracted
    // into `vacuity_warning_for_state_count` for unit testability;
    // surfaced via stderr alongside the existing soundness warnings.
    if let Some(msg) = vacuity_warning_for_state_count(eval_clts.state_count()) {
        eprintln!("{msg}");
    }

    println!(
        "Formula '{}' over automaton '{}':",
        formula_name, args.automaton
    );
    println!(
        "  States satisfying: {}/{}",
        satisfying.len(),
        eval_clts.state_count()
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

    // Counterexample-style output: surface violating initial states
    // with their structured valuations (when the adapter produced
    // them). For Caliptra-shape fixtures this prints lines like
    // `s_init_5  boot_fsm_ns = UNMATCHED_5, wait_count = ZERO` which
    // directly identifies the bug-bearing reset samples — closes the
    // diagnosability gap the §6.7 measurement-driven discipline + the
    // Substack-article roadmap section both flagged.
    if !initial_violating.is_empty() {
        println!(
            "  Initial states violating: {}/{}",
            initial_violating.len(),
            initial_states.len()
        );
        for (name, valuation) in &initial_violating {
            match valuation {
                Some(vals) if !vals.is_empty() => {
                    let pairs: Vec<String> =
                        vals.iter().map(|(k, v)| format!("{k} = {v}")).collect();
                    println!("    {name}  ({})", pairs.join(", "));
                }
                _ => println!("    {name}"),
            }
        }
    }
    println!(
        "  Guard partitions: {}",
        if options.use_partitions {
            "enabled"
        } else {
            "disabled"
        }
    );

    // Soundness trust-level warning for liveness on over-approximate models
    if formula.alternation_depth >= 2 {
        let has_noop = eval_clts.states().any(|state| {
            eval_clts.outgoing(state).iter().any(|t| {
                t.target() == state
                    && t.labels().iter().any(|l| {
                        eval_clts
                            .label_payload(*l)
                            .is_some_and(|syms| syms.iter().any(|s| s == "noop" || s == "tau"))
                    })
            })
        });
        if has_noop {
            eprintln!(
                "  [SOUNDNESS WARNING] Trust level: LOW — formula has alternation depth {} \
                 (class: {:?}) and model contains noop/tau self-loops. Liveness verdicts \
                 may not transfer to the real system (over-approximation admits spurious progress).",
                formula.alternation_depth, formula.property_class
            );
        } else {
            eprintln!(
                "  [SOUNDNESS NOTE] Formula has alternation depth {} (class: {:?}). \
                 Positional strategy extraction is best-effort for this class.",
                formula.alternation_depth, formula.property_class
            );
        }
    }

    if args.soundness_report {
        print_soundness_report(&formula_name, formula, eval_clts);
    }

    // Print structure if requested
    if let Some(output_path) = args.print_structure {
        print_context_structure(&realized.context, output_path)?;
    }

    Ok(())
}

fn context_synthesize(args: ContextSynthesizeArgs) -> Result<(), String> {
    let preprocessor = validate_preprocessor(args.preprocessor.as_deref())?;
    let PreparedEvalContext {
        realized,
        formula_name,
    } = prepare_eval_context(EvalContextParams {
        context: &args.context,
        sidecars: &args.sidecars,
        adapter: args.adapter.as_deref(),
        mode: args.mode.as_deref(),
        preprocessor,
        print_ctxdsl_path: args.print_ctxdsl.as_ref(),
        stubs: &[], // context_synthesize has no --stub support
        formula: &args.formula,
        template: &args.template,
        template_args: &args.template_args,
        automaton: &args.automaton,
    })?;
    let realized_formula = realized
        .formulas
        .get(&formula_name)
        .ok_or_else(|| format!("unknown formula '{}' in realised context", formula_name))?;
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
                mode: parse_cli_controller_mode(&args.controller_mode, args.extract_strategy)?,
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
        formula_name, args.automaton
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

    // Soundness trust-level warning for liveness on over-approximate models
    if realized_formula.alternation_depth >= 2 {
        eprintln!(
            "  [SOUNDNESS WARNING] Trust level: LOW — formula has alternation depth {} \
             (class: {:?}). The winning region is correct, but the positional controller \
             may not cycle through obligations for liveness/GR(1) properties.",
            realized_formula.alternation_depth, realized_formula.property_class
        );
    }

    if args.soundness_report {
        if let Some(synth_clts) = realized.context.clts(&args.automaton) {
            print_soundness_report(&formula_name, realized_formula, synth_clts);
        } else {
            eprintln!(
                "  warning: automaton '{}' not found — skipping soundness report",
                args.automaton
            );
        }
    }

    if let Some(path) = args.dump_json.as_ref() {
        write_controller_json(path, &args.automaton, &formula_name, &synthesis)?;
        println!("  JSON summary written to {}", path.display());
    }

    if let Some(path) = args.emit_dsl.as_ref() {
        write_controller_ctxdsl(
            path,
            &args.automaton,
            &formula_name,
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

fn print_soundness_report(
    formula_name: &str,
    formula: &mununu_core::context_dsl::RealizedFormula,
    clts: &Clts<mununu_core::clts::DefaultStateIdx, DefaultLabelIdx>,
) {
    let has_noop = clts.states().any(|state| {
        clts.outgoing(state).iter().any(|t| {
            t.target() == state
                && t.labels().iter().any(|l| {
                    clts.label_payload(*l)
                        .is_some_and(|syms| syms.iter().any(|s| s == "noop" || s == "tau"))
                })
        })
    });

    let abstraction_dir = if has_noop {
        "over-approximation (noop/tau self-loops present)"
    } else {
        "exact (no detected over-approximation artifacts)"
    };

    let trust = match (formula.alternation_depth, has_noop) {
        (0, _) => "HIGH — propositional formula, no fixpoint semantics",
        (1, _) => "HIGH — safety/reachability (alternation depth 1), memoryless strategy is sound",
        (_, true) => "LOW — liveness on over-approximate model; verdict may not transfer",
        (_, false) => {
            "MEDIUM — liveness formula; winning region correct but controller is best-effort"
        }
    };

    println!("\n  ─── Soundness Report ───");
    println!("  Property:          {formula_name}");
    println!("  Class:             {:?}", formula.property_class);
    println!("  Alternation depth: {}", formula.alternation_depth);
    println!("  Abstraction:       {abstraction_dir}");
    println!("  Trust level:       {trust}");

    if formula.alternation_depth >= 2 {
        println!("  Recommendation:    Verify liveness claims against the real system.");
        println!(
            "                     Consider adding fairness constraints for async compositions."
        );
    }
    println!();
}

fn context_graph(args: ContextGraphArgs) -> Result<(), String> {
    let (context_doc, sidecar_docs, _) =
        load_context_documents(&args.context, &args.sidecars, None)?;
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

// ---------------------------------------------------------------------------
// Property templates CLI
// ---------------------------------------------------------------------------

fn handle_library(command: LibraryCommand) -> Result<(), String> {
    use mununu_core::library;

    match command {
        LibraryCommand::List => {
            println!("Shipped parameterised CTXDSL component templates:");
            println!();
            for t in library::templates() {
                println!("  {:<18} — {}", t.name, t.summary);
            }
            println!();
            println!("Emit with: `mununu library emit <NAME> [--instance-id ID] [-o PATH]`");
            Ok(())
        }
        LibraryCommand::Emit(args) => {
            let t = library::lookup(&args.name).ok_or_else(|| {
                let names: Vec<&str> = library::templates().iter().map(|t| t.name).collect();
                format!(
                    "unknown library template `{}`. Available: {}",
                    args.name,
                    names.join(", ")
                )
            })?;
            let body = library::emit(t, args.instance_id.as_deref());
            match args.output {
                Some(path) => std::fs::write(&path, &body)
                    .map_err(|e| format!("failed to write {}: {e}", path.display()))?,
                None => print!("{body}"),
            }
            Ok(())
        }
    }
}

fn list_templates(args: TemplatesArgs) -> Result<(), String> {
    use mununu_core::adapter::templates::{TemplateDomain, TemplateRegistry};

    let registry = TemplateRegistry::builtin();

    // Show details for a single template
    if let Some(id) = &args.id {
        let tmpl = registry
            .get(id)
            .ok_or_else(|| format!("unknown template '{id}'"))?;
        if args.json {
            let json = serde_json::to_string_pretty(tmpl).map_err(|e| e.to_string())?;
            println!("{json}");
        } else {
            println!("Template: {} ({})", tmpl.id, tmpl.display_name);
            println!("  {}", tmpl.description);
            println!("  Kind: {}  Role: {}", tmpl.kind, tmpl.role);
            println!(
                "  Domains: {}",
                tmpl.domains
                    .iter()
                    .map(|d| format!("{d:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if tmpl.params.is_empty() {
                println!("  Parameters: (none)");
            } else {
                println!("  Parameters:");
                for p in &tmpl.params {
                    let req = if p.required { "required" } else { "optional" };
                    let default = p
                        .default
                        .as_deref()
                        .map(|d| format!(" [default: {d}]"))
                        .unwrap_or_default();
                    println!("    ${} — {} ({req}{default})", p.name, p.description);
                }
            }
            println!("  Formula: {}", tmpl.formula_pattern);
            if !tmpl.tags.is_empty() {
                println!("  Tags: {}", tmpl.tags.join(", "));
            }
        }
        return Ok(());
    }

    // Filter by domain
    let domain_filter: Option<TemplateDomain> = args.domain.as_deref().and_then(|d| match d {
        "rtl" => Some(TemplateDomain::Rtl),
        "agentic" => Some(TemplateDomain::Agentic),
        "software" => Some(TemplateDomain::Software),
        "synthesis" => Some(TemplateDomain::Synthesis),
        "universal" => Some(TemplateDomain::Universal),
        _ => None,
    });

    let templates = if let Some(domain) = domain_filter {
        registry.for_domain(domain)
    } else {
        registry.for_domain(TemplateDomain::Universal) // universal returns all
    };

    if args.json {
        let catalog = registry.catalog();
        let json = serde_json::to_string_pretty(&catalog).map_err(|e| e.to_string())?;
        println!("{json}");
    } else {
        let mut sorted: Vec<_> = templates;
        sorted.sort_by_key(|t| t.id.clone());
        for tmpl in &sorted {
            let params = if tmpl.params.is_empty() {
                String::new()
            } else {
                let names: Vec<_> = tmpl.params.iter().map(|p| format!("${}", p.name)).collect();
                format!("({})", names.join(", "))
            };
            println!("  {}{:<24} {}", tmpl.id, params, tmpl.description);
        }
    }

    Ok(())
}

/// Resolve the formula name for eval/synth: either from `--formula` or from `--template`.
///
/// When `--template` is provided, instantiates the template and injects a sidecar
/// CTXDSL document containing the formula so it can be found in the realized context.
fn resolve_formula_name(
    formula: &Option<String>,
    template: &Option<String>,
    template_args: &[String],
    automaton: &str,
    sidecar_docs: &mut Vec<ContextDoc>,
) -> Result<String, String> {
    if let Some(name) = formula {
        return Ok(name.clone());
    }

    let template_id = template
        .as_ref()
        .ok_or("either --formula or --template is required")?;

    // Parse template args
    let mut args_map = HashMap::new();
    for arg in template_args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| format!("invalid --template-arg '{arg}': expected KEY=VALUE"))?;
        args_map.insert(key.to_string(), value.to_string());
    }

    let registry = mununu_core::adapter::templates::TemplateRegistry::builtin();
    let tref = mununu_core::adapter::templates::TemplateRef {
        template: template_id.clone(),
        args: args_map,
    };
    let inst = registry
        .instantiate(&tref)
        .map_err(|e| format!("template instantiation failed: {e}"))?;

    // Generate a sidecar CTXDSL document with the formula
    let formula_name = inst.name.clone();
    let sidecar_ctxdsl = format!(
        "context __template_sidecar {{\n  mu_formulas {{\n    formula {formula_name} {{\n      over {automaton};\n      body = {};\n    }}\n  }}\n}}\n",
        inst.formula
    );
    let sidecar_doc = parse_context_doc(&sidecar_ctxdsl)
        .map_err(|e| format!("internal error: generated template CTXDSL failed to parse: {e}"))?;
    sidecar_docs.push(sidecar_doc);

    eprintln!(
        "Template '{}' → formula '{}': {}",
        template_id, formula_name, inst.formula
    );

    Ok(formula_name)
}

/// GAP-009: returns a warning message when the model is small enough that
/// any verdict is vacuously satisfied, otherwise None.
///
/// A 1-state model can only produce 0/1 or 1/1 verdicts regardless of
/// formula content — the property never gets a chance to discriminate.
/// A 0-state model is the empty case (typically a result of upstream
/// extraction failure) — verdicts are nominally 0/0, also vacuous.
///
/// The threshold is intentionally tight (≤ 1). A 2+-state model can in
/// principle witness real behavior, even if some properties are still
/// trivially satisfied; that case is left for a follow-up predicate-level
/// vacuity check (out of scope for GAP-009).
fn vacuity_warning_for_state_count(state_count: usize) -> Option<String> {
    if state_count <= 1 {
        Some(format!(
            "[mununu] WARN: model has {state_count} reachable state(s); verdicts are \
             vacuously satisfied regardless of formula content. The property \
             may appear to hold (or fail) without genuinely witnessing or \
             excluding the modeled behavior. Confirm the model has \
             sufficient state-space distinction before relying on this verdict."
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod vacuity_warning_tests {
    use super::vacuity_warning_for_state_count;

    /// 1-state and 0-state models trigger the vacuity warning.
    /// The MCP-001 / MCP-005 compositional validation pathology was
    /// per-instance automata collapsing to 1 state; the composed product
    /// stayed 1 state. Both verdicts (`no_clobber 0/1`, `clobber_reachable
    /// 1/1`) coincidentally matched the hand baselines but were vacuous.
    #[test]
    fn vacuity_warning_fires_for_one_state_model() {
        let warning = vacuity_warning_for_state_count(1)
            .expect("1-state models must trigger the vacuity warning");
        assert!(warning.contains("vacuously satisfied"));
        assert!(warning.contains("1 reachable state"));
    }

    #[test]
    fn vacuity_warning_fires_for_zero_state_model() {
        // The empty case — typically a result of upstream extraction
        // failure (e.g., class not found). Worth warning about even
        // though the user usually gets a different error first.
        let warning = vacuity_warning_for_state_count(0)
            .expect("0-state models must trigger the vacuity warning");
        assert!(warning.contains("vacuously satisfied"));
    }

    #[test]
    fn vacuity_warning_silent_for_normal_models() {
        // 2+ state models can witness real behavior; no warning.
        for n in [2, 4, 16, 100, 4096] {
            assert!(
                vacuity_warning_for_state_count(n).is_none(),
                "did not expect vacuity warning for {n}-state model",
            );
        }
    }
}
