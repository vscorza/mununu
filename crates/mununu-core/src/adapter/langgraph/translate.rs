//! LangGraph JSON → `AdapterIR` translation.
//!
//! The whole `StateGraph` collapses to a single explicit automaton:
//!   - One CTXDSL state per LangGraph node
//!   - Initial state = `entry_point` (or the first node when absent)
//!   - One transition per edge, label = `node_<from>_enter` (or
//!     `node_<from>_<condition>_enter` for conditional edges)
//!
//! Asynchronous composition isn't needed at the adapter level — the
//! verify framework's orchestrator composes the LangGraph automaton
//! with other sources via `verify.toml`'s composition block.

use crate::adapter::ir::{AdapterIR, AutomatonSpec, Metadata, StateSpec, TransitionSpec};
use crate::adapter::langgraph::ast::{MununuAnnotations, Node, StateGraph};
use crate::adapter::{AdapterError, AdapterErrorKind, AdapterWarning, SourceFormat, WarningKind};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Translate a parsed [`StateGraph`] into the shared `AdapterIR`.
pub fn to_ir(
    graph: StateGraph,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let graph_name = graph
        .name
        .as_deref()
        .map(sanitise_ident)
        .unwrap_or_else(|| "StateGraph".to_string());

    let nodes_vec: Vec<Node> = graph.nodes.into_vec();
    if nodes_vec.is_empty() {
        return Err(AdapterError {
            kind: AdapterErrorKind::IrConsistencyError,
            message: "LangGraph StateGraph has no nodes — nothing to translate.".to_string(),
            location: None,
        });
    }

    // Resolve entry point. Falls back to the first node when absent.
    let entry = graph
        .entry_point
        .clone()
        .unwrap_or_else(|| nodes_vec[0].id.clone());
    if !nodes_vec.iter().any(|n| n.id == entry) {
        warnings.push(AdapterWarning {
            kind: WarningKind::UnsupportedConstruct,
            message: format!(
                "entry_point = \"{entry}\" references a node not present in the graph; using the first node instead."
            ),
            location: None,
        });
    }

    let (override_ctrl, override_internal) = derive_controllability(&graph.mununu);

    // ---------- states --------------------------------------------------
    let states: Vec<StateSpec> = nodes_vec
        .iter()
        .map(|n| StateSpec {
            name: sanitise_ident(&n.id),
            is_initial: n.id == entry,
            valuations: None,
        })
        .collect();

    // ---------- transitions ---------------------------------------------
    let mut transitions: Vec<TransitionSpec> = Vec::with_capacity(graph.edges.len());
    let mut all_labels: Vec<String> = Vec::with_capacity(graph.edges.len());
    for edge in &graph.edges {
        let from = sanitise_ident(&edge.from);
        let to = sanitise_ident(&edge.to);
        let label = match &edge.condition {
            Some(cond) => {
                let c = sanitise_ident(cond);
                format!("node_{from}_{c}_enter")
            }
            None => format!("node_{from}_enter"),
        };
        if !all_labels.contains(&label) {
            all_labels.push(label.clone());
        }
        transitions.push(TransitionSpec {
            source: from,
            target: to,
            labels: vec![label],
            modality: crate::context_dsl::ast::TransitionModalitySpec::Sharp,

            additional_targets: Vec::new(),
        });
    }

    if transitions.is_empty() {
        warnings.push(AdapterWarning {
            kind: WarningKind::UnsupportedConstruct,
            message: "LangGraph StateGraph has nodes but no edges — emitting an isolated-state automaton.".to_string(),
            location: None,
        });
    }

    // ---------- controllability classification --------------------------
    // Default: node-enter labels are controllable (the scheduler /
    // graph runtime chooses the next node). Labels matching a node
    // marked `kind = "end"` are uncontrollable (the runtime decides
    // when to terminate). `__mununu` overrides win.
    let end_nodes: std::collections::HashSet<String> = nodes_vec
        .iter()
        .filter(|n| n.kind.eq_ignore_ascii_case("end"))
        .map(|n| sanitise_ident(&n.id))
        .collect();
    let mut controllable_labels: Vec<String> = Vec::new();
    let mut internal_labels: Vec<String> = Vec::new();
    for label in &all_labels {
        let label_refers_to_end_source = end_nodes.iter().any(|node_id| {
            label == &format!("node_{node_id}_enter")
                || label.starts_with(&format!("node_{node_id}_"))
        });
        if override_internal.contains(label) {
            internal_labels.push(label.clone());
        } else if override_ctrl.contains(label) || !label_refers_to_end_source {
            controllable_labels.push(label.clone());
        }
    }

    // Composition isn't auto-generated; the verify framework composes
    // this automaton with other sources via the `verify.toml`.
    Ok(AdapterIR {
        metadata: Metadata {
            title: graph_name.clone(),
            source_format: SourceFormat::XState, // shared variant until SourceFormat::Langgraph lands
            description: Some(format!(
                "Translated from LangGraph StateGraph '{}' ({} nodes, {} edges)",
                graph_name,
                nodes_vec.len(),
                graph.edges.len(),
            )),
            game_semantics: None,
            known_status: None,
        },
        signals: Vec::new(),
        automata: vec![AutomatonSpec {
            name: graph_name,
            states,
            transitions,
            controllable_labels,
            internal_labels,
        }],
        compositions: Vec::new(),
        properties: Vec::new(),
        controller: None,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sanitise a LangGraph node id / name into a CTXDSL identifier.
pub(crate) fn sanitise_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for c in s.chars() {
        let ok = if first {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        };
        out.push(if ok { c } else { '_' });
        first = false;
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn derive_controllability(mununu: &Option<MununuAnnotations>) -> (Vec<String>, Vec<String>) {
    match mununu {
        Some(m) => (m.controllable.clone(), m.internal.clone()),
        None => (Vec::new(), Vec::new()),
    }
}
