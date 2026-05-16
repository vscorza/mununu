//! CrewAI JSON AST types — strictly typed deserialisation surface.
//!
//! Mirrors the public CrewAI serialisation shape closely enough that
//! exported `Crew` JSONs from CrewAI v0.50+ parse via serde without
//! coaxing. Unknown fields are ignored (`#[serde(default)]` everywhere)
//! so vendor-specific extensions survive translation without breaking
//! the schema check.
//!
//! The translator (`crewai::translate`) reads only the subset of
//! fields it can produce verifiable automata from today; the rest are
//! preserved on the AST for future feature work but otherwise unused.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level shape
// ---------------------------------------------------------------------------

/// Parsed CrewAI JSON document. Two-format support:
///   1. Top-level object with `agents` + `tasks` arrays + optional
///      `process` discriminator (the canonical CrewAI export shape).
///   2. Wrapped form with a `crew` key carrying the same fields (some
///      CrewAI versions wrap in an outer envelope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CrewaiDocument {
    Wrapped { crew: Crew },
    Flat(Crew),
}

impl CrewaiDocument {
    /// Project either shape into the underlying [`Crew`].
    pub fn into_crew(self) -> Crew {
        match self {
            CrewaiDocument::Wrapped { crew } => crew,
            CrewaiDocument::Flat(crew) => crew,
        }
    }
}

/// CrewAI `Crew` — top-level agentic-orchestration object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crew {
    /// Optional crew name. Surfaced in the emitted CTXDSL document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Agent definitions — at least one is required for translation.
    #[serde(default)]
    pub agents: Vec<Agent>,

    /// Task definitions. Each task references an `agent` by role.
    /// Order in this list is the sequential-process execution order.
    #[serde(default)]
    pub tasks: Vec<Task>,

    /// Process discipline. `"sequential"` (default), `"hierarchical"`,
    /// or `"consensual"`. Only sequential is fully translated today;
    /// hierarchical / consensual emit a structural warning and fall
    /// back to sequential.
    #[serde(default = "default_process")]
    pub process: String,

    /// Optional manager LLM specifier (for hierarchical crews).
    /// Preserved but unused by the translator today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_llm: Option<String>,

    /// Optional `__mununu` annotation block overriding controllability
    /// classifications or declaring properties. Same convention as the
    /// XState adapter's `__mununu` block.
    #[serde(default, rename = "__mununu", skip_serializing_if = "Option::is_none")]
    pub mununu: Option<MununuAnnotations>,
}

fn default_process() -> String {
    "sequential".to_string()
}

/// A CrewAI Agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Role identifier. Used to bind tasks to agents and to derive
    /// per-agent automaton names in the emitted CTXDSL.
    pub role: String,

    /// Human-readable goal. Preserved for diagnostics; not used by
    /// the translator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,

    /// Backstory text. Preserved for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backstory: Option<String>,

    /// Tools the agent has access to (free-form strings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,

    /// Whether the agent is allowed to delegate to other agents.
    /// `false` in sequential crews; `true` typically in hierarchical
    /// ones.
    #[serde(default)]
    pub allow_delegation: bool,
}

/// A CrewAI Task definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Human-readable description. Surfaced in CTXDSL comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Role of the agent assigned to this task. Must match an
    /// [`Agent::role`] in the same crew.
    pub agent: String,

    /// Expected output description. Preserved for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_output: Option<String>,

    /// Whether this task can run asynchronously (in parallel with
    /// the next one). When `true`, the sequential-process translator
    /// treats the task as initiating a fire-and-forget branch.
    #[serde(default)]
    pub async_execution: bool,

    /// Other tasks this task depends on. Used by hierarchical /
    /// consensual processes; ignored by sequential.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
}

/// `__mununu` block — same convention as the XState adapter.
///
/// Today the CrewAI adapter recognises only the controllability
/// override field; templated properties (e.g.
/// `bounded_handoff`, `no_delegation_cycle`) will land in a
/// follow-up slice alongside the property-template additions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MununuAnnotations {
    /// Override list of labels classified as controllable. Each label
    /// must appear in the emitted alphabet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,
    /// Override list of labels classified as uncontrollable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncontrollable: Vec<String>,
    /// Override list of labels classified as internal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal: Vec<String>,
}
