//! Shared Intermediate Representation (IR) for format adapters.
//!
//! The IR captures the common concepts across TLSF, AIGER, and Promela:
//! signals/variables, automata, compositions, temporal properties, and
//! controllability. Each adapter translates its format-specific AST into
//! the IR, and the shared emitter converts the IR into CTXDSL text.

use crate::context_dsl::ast::TransitionModalitySpec;
use crate::ltl::LtlFormula;

/// A reactive system specification in the shared intermediate representation.
#[derive(Debug, Clone)]
pub struct AdapterIR {
    /// Metadata from the source file.
    pub metadata: Metadata,
    /// Boolean signals or bounded variables that define the system's observable state.
    pub signals: Vec<Signal>,
    /// Explicit automata (used by Promela; empty for TLSF/AIGER).
    pub automata: Vec<AutomatonSpec>,
    /// Composition directives for combining automata.
    pub compositions: Vec<CompositionSpec>,
    /// Temporal properties to verify or synthesize.
    pub properties: Vec<PropertySpec>,
    /// Controller synthesis target (if any).
    pub controller: Option<ControllerSpec>,
}

/// Source metadata.
#[derive(Debug, Clone)]
pub struct Metadata {
    /// Human-readable title from the source file.
    pub title: String,
    /// Source format identifier.
    pub source_format: super::SourceFormat,
    /// Description or comment from the source file.
    pub description: Option<String>,
    /// Game semantics (Mealy/Moore), if specified.
    pub game_semantics: Option<GameSemantics>,
    /// SYNTCOMP status (realizable/unrealizable), if known.
    pub known_status: Option<RealizabilityStatus>,
}

/// Game semantics for synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameSemantics {
    /// Output depends on current input + state.
    Mealy,
    /// Output depends on state only.
    Moore,
}

/// Known realizability status from the source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizabilityStatus {
    Realizable,
    Unrealizable,
}

/// A signal or variable in the source format.
#[derive(Debug, Clone)]
pub struct Signal {
    /// Signal name (e.g., "req", "grant", "bit0").
    pub name: String,
    /// Controllability classification.
    pub kind: SignalKind,
    /// Value domain.
    pub domain: SignalDomain,
    /// Whether this signal defines state dimensions, label dimensions, or both.
    pub role: SignalRole,
}

/// Controllability classification for a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// Environment-controlled signal. Maps to uncontrollable labels in CTXDSL.
    Input,
    /// System-controlled signal. Maps to controllable labels in CTXDSL.
    Output,
    /// No inherent controllability (user must annotate).
    /// Default: treated as uncontrollable, with a warning.
    Neutral,
    /// Internal to a process or subsystem. Maps to internal labels.
    Internal,
}

/// Value domain of a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalDomain {
    /// Boolean signal (0 or 1). Used by TLSF and AIGER.
    Boolean,
    /// Bounded integer variable. Used by Promela.
    BoundedInt { lower: i64, upper: i64 },
    /// Enumerated type (e.g., Promela mtype).
    Enum(Vec<String>),
}

impl SignalDomain {
    /// Number of distinct values in this domain.
    pub fn cardinality(&self) -> usize {
        match self {
            SignalDomain::Boolean => 2,
            SignalDomain::BoundedInt { lower, upper } => (upper - lower + 1) as usize,
            SignalDomain::Enum(variants) => variants.len(),
        }
    }
}

/// Role of a signal in the state/label partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalRole {
    /// Signal defines a state dimension.
    State,
    /// Signal defines a label dimension.
    Label,
    /// Signal is both a state and label dimension (TLSF signals).
    StateAndLabel,
}

/// An explicit automaton specification.
#[derive(Debug, Clone)]
pub struct AutomatonSpec {
    /// Automaton name.
    pub name: String,
    /// States of the automaton.
    pub states: Vec<StateSpec>,
    /// Transitions of the automaton.
    pub transitions: Vec<TransitionSpec>,
    /// Labels declared as controllable within this automaton.
    pub controllable_labels: Vec<String>,
    /// Labels declared as internal within this automaton.
    pub internal_labels: Vec<String>,
}

/// A state in an explicit automaton.
#[derive(Debug, Clone)]
pub struct StateSpec {
    /// State name.
    pub name: String,
    /// Whether this is an initial state.
    pub is_initial: bool,
    /// Structured variable-value pairs that define this state.
    /// Populated by adapters that enumerate states from cross-product domains
    /// (SV Kripke, extraction). Enables structured predicate matching.
    pub valuations: Option<std::collections::BTreeMap<String, String>>,
}

/// A transition in an explicit automaton.
///
/// R.5 Item K sub-item K.3 (2026-06-05) — carries the CTXDSL-level
/// modality attribute (`[may]` / `[must]` / `[sharp]`). Adapters
/// producing KMTS-shaped output (the R.2.5 predicate-cube lift, the
/// R.5b UF abstraction lifter, and hand-authored CTXDSL via the
/// realize round-trip) set this field; legacy 2-valued adapters leave
/// it at the `Sharp` default. The CTXDSL emitter (`adapter::emit`)
/// reads this field to emit the `[may]` / `[must]` suffix; the
/// default `Sharp` emits no suffix (preserving pre-K.3 output
/// byte-for-byte).
#[derive(Debug, Clone)]
pub struct TransitionSpec {
    /// Source state name.
    pub source: String,
    /// Target state name.
    pub target: String,
    /// Labels on this transition.
    pub labels: Vec<String>,
    /// R.5 Item K sub-item K.3 (2026-06-05) — modality attribute.
    /// Defaults to `Sharp` for adapters that produce only 2-valued
    /// (CLTS) output. KMTS-aware adapters set this to `MayOnly` or
    /// `MustOnly` per the source's modality declaration.
    pub modality: TransitionModalitySpec,
}

/// Composition directive.
#[derive(Debug, Clone)]
pub enum CompositionSpec {
    /// Synchronous composition: shared labels fire together.
    Synchronous { name: String, members: Vec<String> },
    /// Asynchronous composition: shared labels fire together, independent actions interleave.
    Asynchronous { name: String, members: Vec<String> },
}

/// A temporal property from the source format.
#[derive(Debug, Clone)]
pub struct PropertySpec {
    /// Property name.
    pub name: String,
    /// Property classification.
    pub kind: PropertyKind,
    /// The formula.
    pub formula: PropertyFormula,
    /// Role in the assume-guarantee framework.
    pub role: PropertyRole,
    /// Explicit "over" target automaton or composition name.
    /// If `None`, the emitter uses the first automaton as default.
    pub over: Option<String>,
    /// Optional human-readable description of what the property asserts —
    /// e.g. the original assertion expression recovered from a chformal
    /// lowering. Emitted as a `// <description>` comment above the
    /// `formula` block in CTXDSL so users can distinguish properties
    /// whose state-name disjunction collapses to the same vacuous-true
    /// formula text.
    pub description: Option<String>,
}

/// Classification of a temporal property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyKind {
    Safety,
    Liveness,
    Fairness,
    AssumeGuarantee,
}

/// Formula representation in the IR.
#[derive(Debug, Clone)]
pub enum PropertyFormula {
    /// LTL formula (translated by mununu's `ltl::translate()` at runtime).
    Ltl(LtlFormula),
    /// Direct mu-calculus formula string.
    MuCalculus(String),
    /// State predicate: a set of states satisfying a condition.
    StatePredicate { name: String, states: Vec<String> },
}

/// Role of a property in the assume-guarantee framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyRole {
    /// Environment assumption (TLSF ASSUMPTIONS).
    Assumption,
    /// System guarantee (TLSF GUARANTEES).
    Guarantee,
    /// Invariant (TLSF INVARIANTS, wrapped in G by the emitter).
    Invariant,
    /// Standalone property.
    Standalone,
}

/// Controller synthesis target.
#[derive(Debug, Clone)]
pub struct ControllerSpec {
    /// Controller name.
    pub name: String,
    /// Source automaton name.
    pub source_automaton: String,
    /// Formula name to satisfy.
    pub formula_name: String,
}
