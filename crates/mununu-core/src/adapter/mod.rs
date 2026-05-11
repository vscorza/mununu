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
pub mod domain;
pub mod emit;
pub mod extraction;
pub mod gdscript;
pub mod ir;
pub mod promela;
pub mod sidecar;
pub mod state_enum;
pub mod systemverilog;
pub mod templates;
pub mod tlsf;
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

    match path.extension().and_then(|e| e.to_str()) {
        Some("tlsf") => Some("tlsf"),
        Some("aag") | Some("aig") => Some("aiger"),
        Some("btor") | Some("btor2") => Some("btor2"),
        Some("pml") | Some("promela") => Some("promela"),
        Some("xstate") => Some("xstate"),
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
    if systemverilog::SystemVerilogAdapter::detect(content) {
        return systemverilog::SystemVerilogAdapter::translate(content, options);
    }
    if extraction::ExtractionAdapter::detect(content) {
        return extraction::ExtractionAdapter::translate(content, options);
    }

    Err(AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: "Could not detect source format. Supported formats: TLSF (.tlsf), AIGER (.aag/.aig), BTOR2 (.btor/.btor2), Promela (.pml), XState (.xstate or .xstate.json), SystemVerilog (.sv/.v via Yosys frontend), Extraction (.espec.json)".into(),
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
    fn returns_none_for_unknown_or_plain_json() {
        assert_eq!(detect_format_by_extension(Path::new("README.md")), None);
        // A plain .json file (not .xstate.json or .espec.json) should not auto-route.
        // Content-based detection via `auto_translate` is the right path here.
        assert_eq!(detect_format_by_extension(Path::new("payload.json")), None);
    }
}
