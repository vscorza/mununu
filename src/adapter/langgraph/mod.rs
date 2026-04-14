//! LangGraph adapter.
//!
//! Translates LangGraph JSON dict representations (nodes + edges +
//! conditional_edges) into CTXDSL via XState JSON as an intermediate format.
//! Supports unconditional edges, conditional routing, and heuristic-based
//! controllability classification.

use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, FormatAdapter, SourceFormat,
    SourceInfo,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// LangGraph adapter implementing [`FormatAdapter`].
pub struct LangGraphAdapter;

// ---------------------------------------------------------------------------
// JSON AST types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GraphDefinition {
    #[serde(default)]
    nodes: Vec<String>,
    #[serde(default)]
    edges: Vec<(String, String)>,
    #[serde(default)]
    conditional_edges: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// FormatAdapter impl
// ---------------------------------------------------------------------------

impl FormatAdapter for LangGraphAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('{') {
            return false;
        }
        trimmed.contains("\"nodes\"")
            && trimmed.contains("\"edges\"")
            && !trimmed.contains("\"agents\"")
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let graph: GraphDefinition = serde_json::from_str(content).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("LangGraph JSON parse error: {e}"),
            location: None,
        })?;

        if graph.nodes.is_empty() {
            return Err(AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: "LangGraph definition has no nodes".to_string(),
                location: None,
            });
        }

        let machine_id = options
            .context_name
            .as_deref()
            .unwrap_or("langgraph_workflow");

        let xstate_json = build_xstate_json(&graph, machine_id);
        let xstate_str = serde_json::to_string(&xstate_json).unwrap();

        let mut output =
            super::xstate::XStateAdapter::translate(&xstate_str, options).map_err(|e| {
                AdapterError {
                    kind: AdapterErrorKind::EmitError,
                    message: format!("LangGraph→XState translation failed: {e}"),
                    location: None,
                }
            })?;

        output.source_info = SourceInfo {
            format: SourceFormat::LangGraph,
            title: Some(machine_id.to_string()),
            signal_count: output.source_info.signal_count,
            state_count: output.source_info.state_count,
            property_count: output.source_info.property_count,
        };

        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Build XState JSON
// ---------------------------------------------------------------------------

fn sanitize(name: &str) -> String {
    let re = Regex::new(r"[^a-zA-Z0-9_]").unwrap();
    re.replace_all(name, "_").trim_matches('_').to_string()
}

fn is_env_event(event_name: &str) -> bool {
    let re = Regex::new(
        r"(?i)human|user|tool_result|sensor|external|callback|webhook|timeout|error|fail",
    )
    .unwrap();
    re.is_match(event_name)
}

fn build_xstate_json(graph: &GraphDefinition, machine_id: &str) -> Value {
    // Determine initial state (target of __start__ edge)
    let initial = graph
        .edges
        .iter()
        .find(|(src, _)| src == "__start__" || src == "START")
        .map(|(_, tgt)| sanitize(tgt))
        .unwrap_or_else(|| sanitize(&graph.nodes[0]));

    let mut states = serde_json::Map::new();
    let mut all_events = Vec::new();

    for node in &graph.nodes {
        let sname = sanitize(node);
        if sname.starts_with("__") {
            continue;
        }

        let mut on = serde_json::Map::new();

        // Unconditional edges from this node
        for (src, tgt) in &graph.edges {
            if sanitize(src) != sname {
                continue;
            }
            let mut tgt_s = sanitize(tgt);
            if tgt_s.starts_with("__") {
                tgt_s = "__done__".to_string();
            }
            let event = format!("NEXT_{}", tgt_s.to_uppercase());
            on.insert(event.clone(), json!(tgt_s));
            all_events.push(event);
        }

        // Conditional edges from this node
        if let Some(cond) = graph.conditional_edges.get(node) {
            for (label, target) in cond {
                let mut tgt_s = sanitize(target);
                if tgt_s.starts_with("__") {
                    tgt_s = "__done__".to_string();
                }
                let event = format!("ROUTE_{}", sanitize(label).to_uppercase());
                on.insert(event.clone(), json!(tgt_s));
                all_events.push(event);
            }
        }

        if on.is_empty() {
            states.insert(sname, json!({}));
        } else {
            states.insert(sname, json!({"on": Value::Object(on)}));
        }
    }

    // Add terminal state if referenced
    let has_done = states.values().any(|v| {
        v.get("on")
            .and_then(|o| o.as_object())
            .map(|o| o.values().any(|t| t.as_str() == Some("__done__")))
            .unwrap_or(false)
    });
    if has_done {
        states.insert("__done__".to_string(), json!({}));
    }

    // Classify controllability via heuristics
    let mut ctrl = BTreeSet::new();
    let mut unctrl = BTreeSet::new();

    for ev in &all_events {
        if is_env_event(ev) {
            unctrl.insert(ev.clone());
        } else {
            ctrl.insert(ev.clone());
        }
    }

    // Fallback: if heuristics classified nothing as controllable, make ROUTE_ events controllable
    if ctrl.is_empty() {
        for ev in &all_events {
            if ev.starts_with("ROUTE_") {
                ctrl.insert(ev.clone());
            } else {
                unctrl.insert(ev.clone());
            }
        }
    }

    json!({
        "id": machine_id,
        "initial": initial,
        "states": Value::Object(states),
        "__mununu": {
            "controllable": ctrl.into_iter().collect::<Vec<_>>(),
            "uncontrollable": unctrl.into_iter().collect::<Vec<_>>(),
            "properties": [{
                "name": "safety_invariant",
                "formula": "nu X. ([] X)",
                "role": "guarantee"
            }]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_langgraph_json() {
        let input = r#"{"nodes": ["a"], "edges": [["__start__", "a"]]}"#;
        assert!(LangGraphAdapter::detect(input));
        // Should not detect CrewAI format
        assert!(!LangGraphAdapter::detect(
            r#"{"agents": [], "tasks": [], "process": "sequential"}"#
        ));
    }

    #[test]
    fn translate_simple_graph() {
        let input = r#"{
            "nodes": ["router", "billing", "tech"],
            "edges": [["__start__", "router"], ["billing", "__end__"], ["tech", "__end__"]],
            "conditional_edges": {"router": {"billing": "billing", "tech": "tech"}}
        }"#;
        let options = AdapterOptions::default();
        let output = LangGraphAdapter::translate(input, &options).unwrap();
        assert!(!output.ctxdsl.is_empty());
        assert_eq!(output.source_info.format, SourceFormat::LangGraph);
        assert!(output.ctxdsl.contains("router"));
        assert!(output.ctxdsl.contains("billing"));
        assert!(output.ctxdsl.contains("tech"));
    }

    #[test]
    fn env_event_heuristics() {
        assert!(is_env_event("NEXT_TIMEOUT_HANDLER"));
        assert!(is_env_event("ROUTE_USER_INPUT"));
        assert!(!is_env_event("ROUTE_BILLING"));
        assert!(!is_env_event("NEXT_TECH"));
    }

    #[test]
    fn reject_empty_nodes() {
        let input = r#"{"nodes": [], "edges": []}"#;
        let options = AdapterOptions::default();
        let result = LangGraphAdapter::translate(input, &options);
        assert!(result.is_err());
    }
}
