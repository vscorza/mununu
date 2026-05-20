//! Report + error types for the verify orchestrator (A2.4).

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Top-level result of [`crate::verify::orchestrator::verify_project`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    /// Project name from the config.
    pub project: String,
    /// One summary per source declared in the config.
    pub sources: Vec<SourceSummary>,
    /// Composition shape used for evaluation.
    pub composition: CompositionInfo,
    /// One verdict per `[[properties]]` entry.
    pub property_verdicts: Vec<PropertyVerdict>,
}

/// Per-source diagnostic information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSummary {
    /// `[[sources]].id` from the config.
    pub id: String,
    /// `[[sources]].adapter` from the config.
    pub adapter: String,
    /// Resolved automaton name used in the composition (post
    /// `AutomatonDiscovery`).
    pub automaton: Option<String>,
    /// Auto-partition telemetry (Phase A.3 step 3.6) — present for
    /// adapters that ran the cone-of-influence pass during translation
    /// (currently SV / BTOR2; extraction is preview-only). `None` for
    /// adapters that did not run the partition (xstate, ctxdsl, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_summary: Option<crate::adapter::partition::PartitionSummary>,
}

/// Composition shape used for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionInfo {
    /// `synchronous` / `asynchronous` / `superset`.
    pub semantics: String,
    /// Resolved composition name.
    pub name: String,
    /// Automaton names (post member resolution) that participate in
    /// the composition.
    pub members: Vec<String>,
}

/// Verdict for one property.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyVerdict {
    /// `[[properties]].name`.
    pub name: String,
    /// How the formula was sourced (template instantiation vs.
    /// inline mu-calculus).
    pub formula_source: PropertyFormulaSource,
    /// Concrete mu-calculus text the verifier evaluated.
    pub formula: String,
    /// Target automaton/composition name.
    pub over: String,
    /// `true` if every initial state of the target satisfies the
    /// formula. Mirrors the CLI verdict convention.
    pub satisfied: bool,
    /// Total number of states in the target automaton/composition.
    pub total_states: usize,
    /// Number of states satisfying the formula.
    pub satisfying_states: usize,
    /// Initial-state names.
    pub initial_states: Vec<String>,
    /// Subset of `initial_states` that satisfy the formula.
    pub initial_satisfying: Vec<String>,
    /// Witness path from a violating initial state, when the
    /// verdict is unsatisfied and a witness can be constructed. The
    /// orchestrator emits this opportunistically — `None` is also
    /// emitted for satisfied verdicts and when the walk hits a
    /// degenerate case (no violating initials, no outgoing
    /// transitions from violating initials). See [`TraceWitness`]
    /// for the trace shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<TraceWitness>,
}

/// A short witness path from a violating initial state to either a
/// sink, a cycle, or a length-capped step in the composed
/// automaton. The trace is **not** a counterstrategy — it is a
/// straightforward forward walk through the composition, biased
/// toward visiting states that violate the property formula. For
/// safety properties this surfaces "the system can reach this bad
/// state via these transitions"; for reachability properties this
/// surfaces "no path from here reaches the target".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceWitness {
    /// Initial composed-state name where the trace begins.
    pub initial_state: String,
    /// Sequence of transitions taken. `steps[i]` records the label
    /// fired and the state entered.
    pub steps: Vec<TraceStep>,
    /// Why the trace stopped.
    pub termination: TraceTermination,
}

/// One transition in a [`TraceWitness`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    /// Label payload of the fired transition. A single CTXDSL
    /// `transition s -> t on label a, label b;` produces one entry
    /// joining the labels with `,`.
    pub label: String,
    /// Composed-state name entered after firing this transition.
    pub successor_state: String,
}

/// Reason a [`TraceWitness`] stopped at a particular step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceTermination {
    /// The composed-state reached has no outgoing transitions.
    Sink,
    /// The walk revisited a state already in the trace; the
    /// `return_to_step` index is the step where the cycle joins
    /// back. `0` means the cycle closes onto the initial state.
    Cycle { return_to_step: usize },
    /// The orchestrator's length cap (currently 20 steps) was
    /// reached before any other terminator fired.
    LengthLimit,
}

/// How a property's formula was sourced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PropertyFormulaSource {
    /// Inline mu-calculus in the config (no template).
    Inline,
    /// Instantiated from a property template.
    Template {
        /// Template ID.
        id: String,
        /// Arguments bound to the template's parameters.
        args: BTreeMap<String, String>,
    },
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors raised by [`crate::verify::orchestrator::verify_project`].
#[derive(Debug)]
pub enum VerifyError {
    /// `VerifyConfig::validate` returned non-empty. The orchestrator
    /// refuses to proceed.
    ConfigValidationFailed(Vec<crate::verify::config::ConfigIssue>),
    /// `AlphabetBinding::from_config` raised an error (e.g.
    /// register-map sidecar not readable).
    AlphabetBinding(crate::verify::binding::BindingError),
    /// `assemble_unified_ctxdsl` raised an error.
    Assemble(crate::verify::assemble::AssembleError),
    /// An I/O error while reading a source file.
    SourceReadFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A source's `adapter` name isn't recognised by the dispatcher.
    UnknownAdapter { source_id: String, adapter: String },
    /// The dispatched adapter failed to translate the source.
    AdapterTranslationFailed {
        source_id: String,
        adapter: String,
        message: String,
    },
    /// A property's `template_ref` failed to instantiate (unknown id,
    /// missing required arg, etc.).
    TemplateInstantiationFailed { property: String, message: String },
    /// The assembled CTXDSL document failed to parse. Indicates a bug
    /// in the assembler or an adapter emitting non-parseable output.
    AssembledCtxdslParseFailed { message: String, snippet: String },
    /// `realize_documents` rejected the assembled document.
    RealizeFailed { message: String },
    /// A property names an `over` automaton/composition that the
    /// realised context doesn't expose.
    UnknownAutomaton {
        property: String,
        over: String,
        known: Vec<String>,
    },
    /// `mu_calculus::evaluate_with_options` returned an error.
    EvaluationFailed { property: String, message: String },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::ConfigValidationFailed(issues) => {
                writeln!(f, "verify config has {} validation issue(s):", issues.len())?;
                for i in issues {
                    writeln!(f, "  - {i}")?;
                }
                Ok(())
            }
            VerifyError::AlphabetBinding(e) => write!(f, "alphabet-binding setup failed: {e}"),
            VerifyError::Assemble(e) => write!(f, "CTXDSL assembly failed: {e}"),
            VerifyError::SourceReadFailed { path, source } => {
                write!(f, "failed to read source file {}: {source}", path.display())
            }
            VerifyError::UnknownAdapter { source_id, adapter } => write!(
                f,
                "source `{source_id}`: unknown adapter `{adapter}` (orchestrator dispatch doesn't know how to call this adapter yet)"
            ),
            VerifyError::AdapterTranslationFailed {
                source_id,
                adapter,
                message,
            } => write!(
                f,
                "source `{source_id}` adapter `{adapter}` failed to translate: {message}"
            ),
            VerifyError::TemplateInstantiationFailed { property, message } => write!(
                f,
                "property `{property}`: template instantiation failed: {message}"
            ),
            VerifyError::AssembledCtxdslParseFailed { message, .. } => write!(
                f,
                "assembled CTXDSL failed to parse: {message}\nthis is a bug in the assembler or an adapter emitting non-parseable output"
            ),
            VerifyError::RealizeFailed { message } => {
                write!(f, "context realization failed: {message}")
            }
            VerifyError::UnknownAutomaton {
                property,
                over,
                known,
            } => write!(
                f,
                "property `{property}`: `over = \"{over}\"` is not declared in the assembled context (known: {})",
                known.join(", ")
            ),
            VerifyError::EvaluationFailed { property, message } => {
                write!(
                    f,
                    "property `{property}`: μ-calculus evaluation failed: {message}"
                )
            }
        }
    }
}

impl std::error::Error for VerifyError {}
