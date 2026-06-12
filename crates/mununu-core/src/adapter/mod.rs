//! Format adapter subsystem — translates external specification formats
//! into CTXDSL text via a shared intermediate representation.
//!
//! # Architecture
//!
//! ```text
//! Source file → Format Parser → Format AST → to_ir() → AdapterIR → emit() → CTXDSL text
//! ```
//!
//! Each adapter implements the [`FormatAdapter`] trait. The shared IR types
//! live in [`ir`], and the CTXDSL emitter in [`emit`].
//!
//! # Soundness Checklist for Adapter Implementors
//!
//! Every adapter must address the following before being considered complete:
//!
//! 1. **Unsupported constructs must warn.** Any source-language construct that
//!    is skipped or partially handled must emit an [`AdapterWarning`] with
//!    [`WarningKind::UnsupportedConstruct`] and note the soundness impact
//!    (over-approx or under-approx).
//!
//! 2. **State abstraction direction must be documented.** For every field or
//!    register abstracted into a finite domain, document whether the abstraction
//!    is an over-approximation (admits more behaviors) or under-approximation
//!    (admits fewer behaviors) using `// SOUNDNESS:` comments.
//!
//! 3. **Guard evaluation failures must be documented.** When `eval_expr` or
//!    guard evaluation returns `None`/default, document whether the fallback
//!    is conservative (over-approx: allows transition) or optimistic
//!    (under-approx: blocks transition).
//!
//! 4. **Controllability must be explicit.** Every label must have a clear
//!    rationale for its controllability classification. If heuristic-based,
//!    emit [`WarningKind::NeutralControllability`] so the user can override.
//!
//! 5. **Known-verdict regression test must exist.** At minimum one test with a
//!    known-safe and one known-unsafe property, verifying the adapter produces
//!    a model that gives the expected verdict.

pub mod aiger;
pub mod btor2;
pub mod crewai;
pub mod cvc5;
pub mod domain;
pub mod emit;
pub mod extraction;
pub mod ir;
pub mod langgraph;
pub mod microcode;
pub mod partition;
pub mod promela;
pub mod sidecar;
pub mod state_enum;
pub mod sv_pipeline_compare;
pub mod systemverilog;
pub mod templates;
pub mod tlsf;
pub mod vcd;
pub mod verilator;
pub mod xstate;
pub mod yosys;

use std::collections::HashMap;
use std::fmt;

/// Trait implemented by each format adapter.
pub trait FormatAdapter {
    /// Detect whether the input content is in this format.
    fn detect(content: &str) -> bool;

    /// Translate source content to CTXDSL text via the shared IR.
    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError>;
}

/// Options controlling adapter translation behavior.
#[derive(Debug, Clone, Default)]
pub struct AdapterOptions {
    /// Which input signals are controllable (for AIGER, Promela).
    pub controllable_inputs: Vec<String>,
    /// Variable bounds for Promela (overrides inferred bounds).
    pub variable_bounds: HashMap<String, (i64, i64)>,
    /// Context name for the output CTXDSL document.
    pub context_name: Option<String>,
    /// Mode for extraction adapter: "fixed" or "vulnerable".
    pub mode: Option<String>,
    /// Raw `.mununu.json` content. When present, the BTOR2 reader (and
    /// any future adapter) parses it through
    /// [`crate::adapter::sidecar::resolve_to_field_domain`] to bound
    /// per-state-cell value enumeration. The SV adapter loads its
    /// sidecar via filesystem convention (next to the .sv source); the
    /// BTOR2 reader takes the JSON in-memory because the BTOR2 source
    /// may itself live in memory (the Yosys driver case).
    pub sidecar_json: Option<String>,
    /// Direction map for top-module ports the upstream frontend
    /// captured before any flattening / inlining destroyed the
    /// module-boundary information. Used by the BTOR2 reader (when
    /// the yosys driver populates this from the pre-flatten
    /// `write_json`) to classify BTOR2 inputs by direction instead of
    /// defaulting every input to `Uncontrollable`.
    ///
    /// This is Document B task B1's plumbing: the §4 rule says
    /// "controllability follows the direction at the surrounding
    /// scope's boundary." When `port_directions` is non-empty, the
    /// reader classifies each input by looking up its name here; any
    /// input *not* in the map keeps the historical "Uncontrollable"
    /// default (the right call for BTOR2 inputs that originated as
    /// cut points from `cutpoint -blackbox`).
    pub port_directions: HashMap<String, crate::controllability::BoundaryDirection>,

    /// R-Y3 (§Phase 8) — opt-in BTOR2 init-line smart defaults for
    /// state cells without sidecar entries. When true, each
    /// unsidecared cell with a BTOR2 `Init` line is pinned to its
    /// init value (1-state abstraction) instead of falling back to
    /// full bit-blast (`2^width`). Cells without init lines retain
    /// full bit-blast.
    ///
    /// SOUNDNESS: under-approximation. Sound for liveness ("the
    /// reset state is reachable in the real design"); unsound for
    /// safety ("property violations that require deviation from the
    /// init value are silently masked"). Surfaced as an adapter
    /// warning naming the affected cells.
    ///
    /// The CLI also honours the env var
    /// `MUNUNU_BTOR2_SMART_INIT_DEFAULTS=1` as a fallback so existing
    /// `validate.sh` scripts can opt in without touching every
    /// AdapterOptions construction site. Default `false` — preserves
    /// legacy full-bit-blast behaviour for unsidecared cells.
    pub smart_init_defaults: bool,

    /// R-S2b.6 (§Phase 9 §9.1, 2026-06-11) — path to the original
    /// SystemVerilog source. When `Some` AND the sidecar declares
    /// a `simulate_reset` block AND a Verilator binary is
    /// discoverable, the BTOR2 bit-blaster runs a short concrete
    /// reset simulation via
    /// [`crate::adapter::verilator::run_reset_simulation`] and
    /// feeds the result through
    /// [`crate::adapter::btor2::bit_blast::apply_reset_simulation_seeding`].
    ///
    /// The path is populated by the caller (CLI / API / yosys
    /// driver) — the BTOR2 bit-blaster does not derive it.
    /// `None` (default) preserves the legacy "no simulation seeding"
    /// behaviour; Verilator is also skipped silently when the
    /// binary is absent (graceful fallback to other Phase 9
    /// strategies). See R-S2b.4 / R-S2b.5 for the prerequisite
    /// helpers + sidecar schema.
    pub sv_source_path: Option<std::path::PathBuf>,

    /// R-S6.6 (§Phase 9 §9.1, 2026-06-11) — filesystem path to
    /// the sidecar that produced `sidecar_json` (when known).
    /// Used by R-S6.6's orchestration to resolve VCD trace paths
    /// declared in `SvAnnotation::vcd_traces`: relative paths in
    /// the sidecar (e.g. `"regression/uart_tx.vcd"`) are resolved
    /// against this path's parent directory.
    ///
    /// When `None`, only absolute trace paths can be read; relative
    /// paths emit an `AdapterWarning` and fall through. Default
    /// `None` preserves the legacy behaviour. Populated by the CLI
    /// (`mununu sv extract --sidecar <path>`) and any API caller
    /// that has the sidecar file system path.
    pub sidecar_path: Option<std::path::PathBuf>,
}

/// A sidecar file the adapter produced alongside its CTXDSL output.
///
/// Used by the contract subsystem (Document A § A5 / Document B § B.3
/// row "Black-box submodule handling"): when an adapter encounters a
/// black-box module it cannot fully extract, it emits a phase-1 contract
/// description (`BlackBoxInterface`) and a phase-1 gap-marker report
/// (`GapMarkerReport`) as side outputs. Callers (CLI / API / UI) write
/// the sidecars to disk next to the primary CTXDSL file so the rest of
/// the `mununu contract …` workflow can consume them without the user
/// hand-authoring JSON.
///
/// The adapter does **not** decide where the file lands — it returns the
/// suggested filename (relative to the primary CTXDSL output's parent
/// directory) and the rendered content. The caller is responsible for
/// the final write path and any conflict resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterSidecar {
    /// Suggested filename, relative to the primary output's parent
    /// directory. Example: `"ddr3_phy.interface.json"`.
    pub filename: String,
    /// Rendered file content (pretty-printed JSON for the contract
    /// sidecars; the caller writes it byte-for-byte).
    pub content: String,
    /// Why this sidecar was emitted. Carried through to user-facing
    /// reports so the operator can see at a glance which adapter slot
    /// produced what.
    pub origin: SidecarOrigin,
}

/// Why an `AdapterSidecar` exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarOrigin {
    /// A `BlackBoxInterface` description for a module the adapter
    /// classified as black-box (un-elaborated submodule, vendor IP,
    /// `(* blackbox *)` attribute).
    BlackBoxInterface,
    /// A `GapMarkerReport` for the same module, prefilled with the
    /// chaotic-stub default gaps.
    BlackBoxGapReport,
}

/// Output from a successful adapter translation.
#[derive(Debug, Clone)]
pub struct AdapterOutput {
    /// The generated CTXDSL text.
    pub ctxdsl: String,
    /// Warnings about unsupported constructs, neutral controllability, etc.
    pub warnings: Vec<AdapterWarning>,
    /// Metadata about the translation.
    pub source_info: SourceInfo,
    /// Sidecar files the adapter produced alongside the CTXDSL.
    /// Currently used by the contract subsystem to emit
    /// `BlackBoxInterface` + `GapMarkerReport` JSONs when the adapter
    /// encounters a black-box submodule. See [`AdapterSidecar`] for the
    /// shape.
    #[doc(hidden)] // kept hidden until B3 lands at least one producer
    pub sidecars: Vec<AdapterSidecar>,
    /// Structured state valuations from cross-product enumeration.
    /// Keyed by `automaton_name → state_name → { variable → display_value }`.
    /// Populated by adapters that enumerate states from register/field domains
    /// (SV Kripke, extraction). Used to wire structured predicate matching
    /// on the CLTS without encoding valuations in the CTXDSL text format.
    pub state_valuations: std::collections::HashMap<
        String,
        std::collections::HashMap<String, std::collections::BTreeMap<String, String>>,
    >,
    /// Per-transition signal observations, keyed by automaton name.
    /// The inner vector mirrors the order of transitions emitted in the
    /// CTXDSL text — adapters that populate it (currently the BTOR2
    /// reader, for Mealy outputs) record `signal → value` pairs that
    /// depend on the input combination of the transition.
    ///
    /// These are *display-only* metadata, never consulted by the
    /// formal evaluator. The CLI / UI trace renderer queries them when
    /// rendering counterexamples and counterstrategies so the user
    /// sees Mealy output values per cycle.
    pub transition_observations: std::collections::HashMap<String, Vec<TransitionObservation>>,
    /// Auto-partition telemetry (Phase A.3 step 3.6) — populated by
    /// adapters that run `crate::adapter::partition::classify` during
    /// translation. Surfaced in CLI / API summaries so users can see
    /// how many signals the cone-of-influence pass dropped without
    /// re-deriving the count from warnings. `None` when the adapter
    /// did not run the partition (e.g. agentic / xstate paths).
    #[doc(hidden)]
    pub partition_summary: Option<partition::PartitionSummary>,
}

/// A single per-transition observation row, emitted by adapters that
/// expose Mealy-style outputs. Used only for trace presentation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransitionObservation {
    pub source: String,
    pub target: String,
    /// Labels on the transition, mirroring `IRTransition.labels`. The
    /// renderer matches an observation row to a CLTS transition by
    /// `(source, target, sorted-labels)`.
    pub labels: Vec<String>,
    /// `signal_name → display_value` for signals whose value depends
    /// on the input portion of this transition.
    pub observations: std::collections::BTreeMap<String, String>,
}

/// A warning produced during translation.
#[derive(Debug, Clone)]
pub struct AdapterWarning {
    pub kind: WarningKind,
    pub message: String,
    pub location: Option<SourceLocation>,
}

/// Classification of translation warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    /// A source-language construct is not supported and was skipped.
    UnsupportedConstruct,
    /// A signal has no inherent controllability; defaulting to uncontrollable.
    NeutralControllability,
    /// The generated state space is large (>10k states).
    LargeStateSpace,
    /// A variable bound overflows the practical state-space limit.
    BoundOverflow,
    /// The translation is an approximation (e.g., Promela liveness under unfairness).
    ApproximateTranslation,
}

/// Source location for diagnostics.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
}

/// Metadata about the translated source.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub format: SourceFormat,
    pub title: Option<String>,
    pub signal_count: usize,
    pub state_count: usize,
    pub property_count: usize,
}

/// Identifies the source format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Tlsf,
    Aiger,
    Btor2,
    Promela,
    XState,
    SystemVerilog,
    Extraction,
}

impl fmt::Display for SourceFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceFormat::Tlsf => write!(f, "TLSF"),
            SourceFormat::Aiger => write!(f, "AIGER"),
            SourceFormat::Btor2 => write!(f, "BTOR2"),
            SourceFormat::Promela => write!(f, "Promela"),
            SourceFormat::XState => write!(f, "XState"),
            SourceFormat::SystemVerilog => write!(f, "SystemVerilog"),
            SourceFormat::Extraction => write!(f, "Extraction"),
        }
    }
}

/// Error during adapter translation.
#[derive(Debug, Clone)]
pub struct AdapterError {
    pub kind: AdapterErrorKind,
    pub message: String,
    pub location: Option<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterErrorKind {
    /// Syntax error in the source format.
    ParseError,
    /// Unsupported construct that cannot be skipped.
    UnsupportedConstruct,
    /// State-space explosion (too many signals/latches).
    StateSpaceOverflow,
    /// Internal consistency error in the IR.
    IrConsistencyError,
    /// CTXDSL emission failed.
    EmitError,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(loc) = &self.location {
            write!(f, "{}:{}: {}", loc.line, loc.column, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for AdapterError {}

/// Detect the source format from a file extension.
pub fn detect_format_by_extension(path: &std::path::Path) -> Option<&'static str> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem.ends_with(".espec") {
        return Some("extraction");
    }
    if stem.ends_with(".xstate") {
        return Some("xstate");
    }
    if stem.ends_with(".crewai") {
        return Some("crewai");
    }
    if stem.ends_with(".langgraph") {
        return Some("langgraph");
    }
    if stem.ends_with(".microcode") {
        return Some("microcode");
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some("tlsf") => Some("tlsf"),
        Some("aag") | Some("aig") => Some("aiger"),
        Some("btor") | Some("btor2") => Some("btor2"),
        Some("pml") | Some("promela") => Some("promela"),
        Some("xstate") => Some("xstate"),
        Some("crewai") => Some("crewai"),
        Some("langgraph") => Some("langgraph"),
        Some("microcode") => Some("microcode"),
        Some("sv") | Some("v") => Some("systemverilog"),
        _ => None,
    }
}

/// Auto-detect the format of the given content and translate it.
pub fn auto_translate(
    content: &str,
    options: &AdapterOptions,
) -> Result<AdapterOutput, AdapterError> {
    if tlsf::TlsfAdapter::detect(content) {
        return tlsf::TlsfAdapter::translate(content, options);
    }
    if aiger::AigerAdapter::detect(content) {
        return aiger::AigerAdapter::translate(content, options);
    }
    if btor2::Btor2Adapter::detect(content) {
        return btor2::Btor2Adapter::translate(content, options);
    }
    if promela::PromelaAdapter::detect(content) {
        return promela::PromelaAdapter::translate(content, options);
    }
    if xstate::XStateAdapter::detect(content) {
        return xstate::XStateAdapter::translate(content, options);
    }
    // CrewAI / LangGraph come after XState — XState's `states` + `initial`
    // shape is distinct from CrewAI's `agents` + `tasks` and LangGraph's
    // `nodes` + `edges`, so order avoids false positives.
    if crewai::CrewaiAdapter::detect(content) {
        return crewai::CrewaiAdapter::translate(content, options);
    }
    if langgraph::LangGraphAdapter::detect(content) {
        return langgraph::LangGraphAdapter::translate(content, options);
    }
    // Microcode after LangGraph — microcode's `steps` array plus
    // `regs`/`mem`/`interrupts` resource declarations make it
    // unambiguous against LangGraph's `nodes` + `edges` shape.
    if microcode::MicrocodeAdapter::detect(content) {
        return microcode::MicrocodeAdapter::translate(content, options);
    }
    if systemverilog::SystemVerilogAdapter::detect(content) {
        return systemverilog::SystemVerilogAdapter::translate(content, options);
    }
    if extraction::ExtractionAdapter::detect(content) {
        return extraction::ExtractionAdapter::translate(content, options);
    }

    Err(AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: "Could not detect source format. Supported formats: TLSF (.tlsf), AIGER (.aag/.aig), BTOR2 (.btor/.btor2), Promela (.pml), XState (.xstate or .xstate.json), CrewAI (.crewai.json), LangGraph (.langgraph.json), SystemVerilog (.sv/.v via Yosys frontend), Extraction (.espec.json)".into(),
        location: None,
    })
}

#[cfg(test)]
mod tests {
    use super::detect_format_by_extension;
    use std::path::Path;

    #[test]
    fn detects_xstate_compound_extension() {
        assert_eq!(
            detect_format_by_extension(Path::new("support_pipeline.xstate.json")),
            Some("xstate")
        );
        assert_eq!(
            detect_format_by_extension(Path::new("/abs/path/auth_flow.xstate.json")),
            Some("xstate")
        );
    }

    #[test]
    fn detects_extraction_compound_extension() {
        assert_eq!(
            detect_format_by_extension(Path::new("game.espec.json")),
            Some("extraction")
        );
    }

    #[test]
    fn detects_simple_extensions() {
        assert_eq!(
            detect_format_by_extension(Path::new("design.sv")),
            Some("systemverilog")
        );
        assert_eq!(
            detect_format_by_extension(Path::new("model.tlsf")),
            Some("tlsf")
        );
        assert_eq!(
            detect_format_by_extension(Path::new("circuit.aag")),
            Some("aiger")
        );
        assert_eq!(
            detect_format_by_extension(Path::new("proc.pml")),
            Some("promela")
        );
    }

    #[test]
    fn detects_crewai_compound_extension() {
        assert_eq!(
            detect_format_by_extension(Path::new("research_crew.crewai.json")),
            Some("crewai")
        );
        assert_eq!(
            detect_format_by_extension(Path::new("/abs/path/crew.crewai")),
            Some("crewai")
        );
    }

    #[test]
    fn detects_langgraph_compound_extension() {
        assert_eq!(
            detect_format_by_extension(Path::new("workflow.langgraph.json")),
            Some("langgraph")
        );
        assert_eq!(
            detect_format_by_extension(Path::new("/abs/path/graph.langgraph")),
            Some("langgraph")
        );
    }

    #[test]
    fn returns_none_for_unknown_or_plain_json() {
        assert_eq!(detect_format_by_extension(Path::new("README.md")), None);
        // A plain .json file (not .xstate.json or .espec.json) should not auto-route.
        // Content-based detection via `auto_translate` is the right path here.
        assert_eq!(detect_format_by_extension(Path::new("payload.json")), None);
    }
}
