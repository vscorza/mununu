//! Native LangGraph adapter — translates LangGraph `StateGraph`
//! JSON exports into CTXDSL via the shared `AdapterIR`.
//!
//! Each LangGraph node maps to a CTXDSL state; each edge to a
//! transition with label `node_<from>_enter` (unconditional) or
//! `node_<from>_<condition>_enter` (conditional). The entry point
//! becomes the initial state.
//!
//! ## Controllability defaults
//!
//! - Node-enter labels emitted from non-`end` source nodes:
//!   **controllable** (the scheduler picks the next transition).
//! - Node-enter labels emitted from `end` source nodes:
//!   **uncontrollable** (the runtime decides when to terminate).
//! - `__mununu` overrides win in either direction.
//!
//! ## Detection
//!
//! [`LangGraphAdapter::detect`] is content-based:
//! - JSON object (starts with `{`)
//! - top-level `"nodes"` AND `"edges"` keys, OR
//! - top-level `"graph"` envelope wrapping the same shape

pub mod ast;
pub mod translate;

use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo,
};
use ast::LangGraphDocument;

/// LangGraph adapter implementing [`FormatAdapter`].
pub struct LangGraphAdapter;

impl FormatAdapter for LangGraphAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('{') {
            return false;
        }
        let has_nodes = trimmed.contains("\"nodes\"");
        let has_edges = trimmed.contains("\"edges\"");
        let has_graph_envelope = trimmed.contains("\"graph\"");
        has_nodes && (has_edges || has_graph_envelope)
    }

    fn translate(content: &str, _options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let doc: LangGraphDocument = serde_json::from_str(content).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("LangGraph JSON parse error: {e}"),
            location: None,
        })?;
        let graph = doc.into_state_graph();
        let graph_name = graph
            .name
            .clone()
            .unwrap_or_else(|| "StateGraph".to_string());

        let mut warnings: Vec<AdapterWarning> = Vec::new();
        let ir = translate::to_ir(graph, &mut warnings)?;

        let result = super::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let signal_count = 0;
        let state_count: usize = ir.automata.iter().map(|a| a.states.len()).sum();
        let property_count = ir.properties.len();

        Ok(AdapterOutput {
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::XState, // shared variant until SourceFormat::Langgraph lands
                title: Some(graph_name),
                signal_count,
                state_count,
                property_count,
            },
            sidecars: Vec::new(),
            state_valuations: Default::default(),
            transition_observations: Default::default(),
            partition_summary: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const LINEAR_CHAIN: &str = r#"
    {
      "name": "linear",
      "entry_point": "a",
      "nodes": [
        { "id": "a", "kind": "agent" },
        { "id": "b", "kind": "agent" },
        { "id": "c", "kind": "end" }
      ],
      "edges": [
        { "from": "a", "to": "b" },
        { "from": "b", "to": "c" }
      ]
    }
    "#;

    const CONDITIONAL: &str = r#"
    {
      "name": "branching",
      "entry_point": "classify",
      "nodes": [
        { "id": "classify", "kind": "agent" },
        { "id": "billing", "kind": "agent" },
        { "id": "tech", "kind": "agent" },
        { "id": "done", "kind": "end" }
      ],
      "edges": [
        { "from": "classify", "to": "billing", "condition": "is_billing" },
        { "from": "classify", "to": "tech", "condition": "is_tech" },
        { "from": "billing", "to": "done" },
        { "from": "tech", "to": "done" }
      ]
    }
    "#;

    const WRAPPED: &str = r#"
    {
      "graph": {
        "nodes": [{ "id": "only" }],
        "edges": []
      }
    }
    "#;

    #[test]
    fn detects_flat_langgraph_json() {
        assert!(LangGraphAdapter::detect(LINEAR_CHAIN));
    }

    #[test]
    fn detects_wrapped_langgraph_json() {
        assert!(LangGraphAdapter::detect(WRAPPED));
    }

    #[test]
    fn does_not_detect_crewai_json() {
        let crewai = r#"{"agents": [], "tasks": []}"#;
        assert!(!LangGraphAdapter::detect(crewai));
    }

    #[test]
    fn translates_linear_chain() {
        let out = LangGraphAdapter::translate(LINEAR_CHAIN, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("automaton linear"));
        assert!(out.ctxdsl.contains("state a initial"));
        assert!(out.ctxdsl.contains("state b"));
        assert!(out.ctxdsl.contains("state c"));
        assert!(out.ctxdsl.contains("on label node_a_enter"));
        assert!(out.ctxdsl.contains("on label node_b_enter"));
        // No warnings — clean translation.
        assert!(out.warnings.is_empty(), "got warnings: {:?}", out.warnings);
    }

    #[test]
    fn translates_conditional_edges_with_per_condition_labels() {
        let out = LangGraphAdapter::translate(CONDITIONAL, &AdapterOptions::default()).unwrap();
        // Each conditional edge gets a label suffixed by the condition.
        assert!(out.ctxdsl.contains("node_classify_is_billing_enter"));
        assert!(out.ctxdsl.contains("node_classify_is_tech_enter"));
        // Unconditional edges retain the plain shape.
        assert!(out.ctxdsl.contains("node_billing_enter"));
        assert!(out.ctxdsl.contains("node_tech_enter"));
    }

    #[test]
    fn empty_nodes_errors() {
        let bad = r#"{"nodes": [], "edges": []}"#;
        let err = LangGraphAdapter::translate(bad, &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::IrConsistencyError);
    }

    #[test]
    fn invalid_json_errors() {
        let err = LangGraphAdapter::translate("{not json", &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
    }

    #[test]
    fn node_id_with_dashes_sanitises_to_underscore() {
        let json = r#"
        {
          "nodes": [
            { "id": "classify-ticket" },
            { "id": "route-billing" }
          ],
          "edges": [
            { "from": "classify-ticket", "to": "route-billing" }
          ]
        }
        "#;
        let out = LangGraphAdapter::translate(json, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("classify_ticket"));
        assert!(out.ctxdsl.contains("route_billing"));
    }

    #[test]
    fn map_form_nodes_translate_like_array_form() {
        let map_form = r#"
        {
          "nodes": {
            "a": { "kind": "agent" },
            "b": { "kind": "end" }
          },
          "edges": [
            { "from": "a", "to": "b" }
          ]
        }
        "#;
        let out = LangGraphAdapter::translate(map_form, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("state a"));
        assert!(out.ctxdsl.contains("state b"));
        assert!(out.ctxdsl.contains("node_a_enter"));
    }

    #[test]
    fn isolated_state_warning_when_no_edges() {
        let nodes_only = r#"{"nodes": [{ "id": "alone" }], "edges": []}"#;
        let out = LangGraphAdapter::translate(nodes_only, &AdapterOptions::default()).unwrap();
        assert_eq!(out.warnings.len(), 1);
    }
}
