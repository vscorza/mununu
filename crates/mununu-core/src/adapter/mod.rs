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

pub mod aiger;
pub mod emit;
pub mod extraction;
pub mod ir;
pub mod promela;
pub mod systemverilog;
pub mod tlsf;
pub mod xstate;

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

    match path.extension().and_then(|e| e.to_str()) {
        Some("tlsf") => Some("tlsf"),
        Some("aag") | Some("aig") => Some("aiger"),
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
        message: "Could not detect source format. Supported formats: TLSF (.tlsf), AIGER (.aag/.aig), Promela (.pml), XState (.xstate), SystemVerilog (.sv/.v), Extraction (.espec.json)".into(),
        location: None,
    })
}
