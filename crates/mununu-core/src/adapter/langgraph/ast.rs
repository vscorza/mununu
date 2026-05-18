//! LangGraph JSON AST — strictly typed deserialisation surface.
//!
//! Matches LangGraph's `StateGraph` JSON serialisation closely
//! enough that exported graphs parse via serde without coaxing.
//! Unknown fields are ignored via `#[serde(default)]` so vendor
//! extensions don't break the schema check.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Top-level shape
// ---------------------------------------------------------------------------

/// Parsed LangGraph JSON document. Three accepted shapes:
///   1. Flat `{nodes, edges}` (the canonical compiled-graph export).
///   2. Wrapped `{graph: {nodes, edges}}` envelope.
///   3. StateGraph form with `{state_schema, entry_point, nodes, edges}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LangGraphDocument {
    Wrapped { graph: StateGraph },
    Flat(StateGraph),
}

impl LangGraphDocument {
    pub fn into_state_graph(self) -> StateGraph {
        match self {
            LangGraphDocument::Wrapped { graph } => graph,
            LangGraphDocument::Flat(graph) => graph,
        }
    }
}

/// LangGraph `StateGraph` — agent workflow definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateGraph {
    /// Optional graph name. Surfaced in the emitted CTXDSL document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Entry-point node id. If absent, the translator uses the first
    /// node listed in `nodes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<String>,

    /// Optional state schema (typed state variables LangGraph
    /// threads through nodes). Preserved but unused by today's
    /// translator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_schema: Option<BTreeMap<String, serde_json::Value>>,

    /// Nodes — `id → node spec`. LangGraph's JSON shape varies by
    /// version; we accept either an array (with each entry carrying
    /// an `id` field) or an object map.
    #[serde(default)]
    pub nodes: Nodes,

    /// Edges between nodes. Each edge fires on an optional
    /// `condition` predicate name.
    #[serde(default)]
    pub edges: Vec<Edge>,

    /// Optional `__mununu` annotation block (same convention as
    /// CrewAI / XState adapters).
    #[serde(default, rename = "__mununu", skip_serializing_if = "Option::is_none")]
    pub mununu: Option<MununuAnnotations>,
}

/// Node container — accepts array or object form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Nodes {
    Array(Vec<Node>),
    Map(BTreeMap<String, NodeSpec>),
}

impl Default for Nodes {
    fn default() -> Self {
        Nodes::Array(Vec::new())
    }
}

impl Nodes {
    /// Flatten to `Vec<Node>` for translator consumption.
    pub fn into_vec(self) -> Vec<Node> {
        match self {
            Nodes::Array(v) => v,
            Nodes::Map(m) => m
                .into_iter()
                .map(|(id, spec)| Node {
                    id,
                    kind: spec.kind,
                    function: spec.function,
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Nodes::Array(v) => v.is_empty(),
            Nodes::Map(m) => m.is_empty(),
        }
    }
}

/// One node in array form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    /// `agent`, `tool`, `conditional`, `end`, or vendor-specific.
    /// Default `"agent"` when unspecified.
    #[serde(default = "default_node_kind")]
    pub kind: String,
    /// Optional function/tool reference. Preserved for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

/// Per-id node spec (object form).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSpec {
    #[serde(default = "default_node_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

fn default_node_kind() -> String {
    "agent".to_string()
}

/// An edge between two nodes.
///
/// LangGraph supports both unconditional edges (`from → to`) and
/// conditional edges (`from → {target_1: cond_1, target_2: cond_2,
/// …}`). The conditional form serialises to multiple `Edge`
/// entries sharing `from` with distinct `condition` values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// Optional condition expression / predicate. When absent the
    /// edge is unconditional. The translator emits the condition as
    /// part of the transition label (`cond_<name>`) for now;
    /// state-schema-driven guard predicates land in a follow-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

/// `__mununu` block — same convention as the XState / CrewAI adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MununuAnnotations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncontrollable: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal: Vec<String>,
}
