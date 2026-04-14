//! CrewAI adapter.
//!
//! Translates CrewAI JSON dict representations (agents + tasks + process type)
//! into CTXDSL via XState JSON as an intermediate format. Supports:
//! - Sequential process: linear task chain with retry logic
//! - Hierarchical process: supervisor + parallel worker regions with delegation

use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, FormatAdapter, SourceFormat,
    SourceInfo,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// CrewAI adapter implementing [`FormatAdapter`].
pub struct CrewAiAdapter;

// ---------------------------------------------------------------------------
// JSON AST types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CrewDefinition {
    #[serde(default)]
    agents: Vec<AgentDef>,
    #[serde(default)]
    tasks: Vec<TaskDef>,
    #[serde(default = "default_process")]
    process: String,
}

fn default_process() -> String {
    "sequential".to_string()
}

#[derive(Debug, Deserialize)]
struct AgentDef {
    #[serde(default = "default_agent_role")]
    role: String,
    #[serde(default)]
    allow_delegation: bool,
    #[serde(default)]
    #[allow(dead_code)]
    tools: Vec<String>,
}

fn default_agent_role() -> String {
    "agent".to_string()
}

#[derive(Debug, Deserialize)]
struct TaskDef {
    name: Option<String>,
    #[serde(default)]
    agent_role: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    context: Vec<String>,
}

// ---------------------------------------------------------------------------
// FormatAdapter impl
// ---------------------------------------------------------------------------

impl FormatAdapter for CrewAiAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('{') {
            return false;
        }
        trimmed.contains("\"agents\"")
            && trimmed.contains("\"tasks\"")
            && trimmed.contains("\"process\"")
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let crew: CrewDefinition = serde_json::from_str(content).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("CrewAI JSON parse error: {e}"),
            location: None,
        })?;

        if crew.tasks.is_empty() {
            return Err(AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: "CrewAI definition has no tasks".to_string(),
                location: None,
            });
        }

        let machine_id = options.context_name.as_deref().unwrap_or("crewai_workflow");

        let xstate_json = build_xstate_json(&crew, machine_id);
        let xstate_str = serde_json::to_string(&xstate_json).unwrap();

        let mut output =
            super::xstate::XStateAdapter::translate(&xstate_str, options).map_err(|e| {
                AdapterError {
                    kind: AdapterErrorKind::EmitError,
                    message: format!("CrewAI→XState translation failed: {e}"),
                    location: None,
                }
            })?;

        output.source_info = SourceInfo {
            format: SourceFormat::CrewAi,
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
    re.replace_all(name, "_").trim_matches('_').to_lowercase()
}

fn build_xstate_json(crew: &CrewDefinition, machine_id: &str) -> Value {
    let (states, initial, ctrl, unctrl) = if crew.process == "hierarchical" {
        build_hierarchical(&crew.agents, &crew.tasks)
    } else {
        build_sequential(&crew.tasks)
    };

    let mut props = vec![json!({
        "name": "safety_invariant",
        "formula": "nu X. ([] X)",
        "role": "guarantee"
    })];

    if crew.process != "hierarchical" {
        props.push(json!({
            "name": "can_finish",
            "formula": "mu X. (done || <> X)",
            "role": "guarantee"
        }));
    }

    json!({
        "id": machine_id,
        "initial": initial,
        "states": states,
        "__mununu": {
            "controllable": ctrl.into_iter().collect::<Vec<_>>(),
            "uncontrollable": unctrl.into_iter().collect::<Vec<_>>(),
            "properties": props
        }
    })
}

fn build_sequential(tasks: &[TaskDef]) -> (Value, String, BTreeSet<String>, BTreeSet<String>) {
    let mut states = serde_json::Map::new();
    let mut ctrl = BTreeSet::new();
    let mut unctrl = BTreeSet::new();

    let task_states: Vec<(String, String)> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let role = sanitize(t.agent_role.as_deref().unwrap_or(&format!("agent_{i}")));
            let tname_default = format!("task_{i}");
            let tname = t.name.as_deref().unwrap_or(&tname_default);
            let sname = format!("{}_{}", sanitize(tname), role);
            let tname_san = sanitize(tname);
            (sname, tname_san)
        })
        .collect();

    for (i, (sname, tname_san)) in task_states.iter().enumerate() {
        let complete_ev = format!("COMPLETE_{}", tname_san.to_uppercase());
        let fail_ev = format!("FAIL_{}", tname_san.to_uppercase());
        let retry_ev = format!("RETRY_{}", tname_san.to_uppercase());
        let fail_sname = format!("failed_{tname_san}");

        let next_sname = if i + 1 < task_states.len() {
            task_states[i + 1].0.clone()
        } else {
            "done".to_string()
        };

        states.insert(
            sname.clone(),
            json!({"on": { &complete_ev: next_sname, &fail_ev: &fail_sname }}),
        );
        states.insert(fail_sname, json!({"on": { &retry_ev: sname }}));

        unctrl.insert(complete_ev);
        unctrl.insert(fail_ev);
        ctrl.insert(retry_ev);
    }

    states.insert("done".to_string(), json!({}));

    let initial = task_states[0].0.clone();
    (Value::Object(states), initial, ctrl, unctrl)
}

fn build_hierarchical(
    agents: &[AgentDef],
    _tasks: &[TaskDef],
) -> (Value, String, BTreeSet<String>, BTreeSet<String>) {
    let mut ctrl = BTreeSet::new();
    let mut unctrl = BTreeSet::new();

    // Collect unique roles preserving order
    let mut unique_roles = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for a in agents {
        let r = sanitize(&a.role);
        if seen.insert(r.clone()) {
            unique_roles.push(r);
        }
    }

    // Supervisor region
    let mut dispatch_on = serde_json::Map::new();
    for r in &unique_roles {
        let ev = format!("ACTIVATE_{}", r.to_uppercase());
        dispatch_on.insert(ev.clone(), json!("waiting"));
        ctrl.insert(ev);
    }

    let sup_states = json!({
        "idle": {"on": {"TASK_ARRIVE": "dispatching"}},
        "dispatching": {"on": Value::Object(dispatch_on)},
        "waiting": {"on": {
            "TASK_COMPLETE": "idle",
            "AGENT_FAIL": "recovering",
            "TIMEOUT": "recovering"
        }},
        "recovering": {"on": {"RETRY": "dispatching"}}
    });

    unctrl.extend(
        ["TASK_ARRIVE", "TASK_COMPLETE", "AGENT_FAIL", "TIMEOUT"]
            .iter()
            .map(|s| s.to_string()),
    );
    ctrl.insert("RETRY".to_string());

    // Worker regions
    let mut parallel_states = serde_json::Map::new();
    parallel_states.insert(
        "supervisor".to_string(),
        json!({"initial": "idle", "states": sup_states}),
    );

    for r in &unique_roles {
        let act_ev = format!("ACTIVATE_{}", r.to_uppercase());
        let mut working_on = serde_json::Map::new();
        working_on.insert("TASK_COMPLETE".to_string(), json!(format!("idle_{r}")));
        working_on.insert("AGENT_FAIL".to_string(), json!(format!("idle_{r}")));

        // Delegation edges
        let agent_def = agents.iter().find(|a| sanitize(&a.role) == *r);
        if let Some(ad) = agent_def
            && ad.allow_delegation
        {
            for other_r in &unique_roles {
                if other_r != r {
                    let deleg_ev = format!(
                        "DELEGATE_{}_TO_{}",
                        r.to_uppercase(),
                        other_r.to_uppercase()
                    );
                    working_on.insert(deleg_ev.clone(), json!(format!("idle_{r}")));
                    ctrl.insert(deleg_ev);
                }
            }
        }

        let w_states = json!({
            format!("idle_{r}"): {"on": {&act_ev: format!("working_{r}")}},
            format!("working_{r}"): {"on": Value::Object(working_on)}
        });

        parallel_states.insert(
            r.clone(),
            json!({"initial": format!("idle_{r}"), "states": w_states}),
        );
    }

    let states = json!({
        "system": {
            "type": "parallel",
            "states": Value::Object(parallel_states)
        }
    });

    (states, "system".to_string(), ctrl, unctrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_crewai_json() {
        let input = r#"{"agents": [], "tasks": [], "process": "sequential"}"#;
        assert!(CrewAiAdapter::detect(input));
        assert!(!CrewAiAdapter::detect(r#"{"nodes": [], "edges": []}"#));
    }

    #[test]
    fn translate_sequential() {
        let input = r#"{
            "agents": [
                {"role": "researcher", "allow_delegation": false, "tools": []},
                {"role": "writer", "allow_delegation": false, "tools": []}
            ],
            "tasks": [
                {"name": "research", "agent_role": "researcher"},
                {"name": "write", "agent_role": "writer"}
            ],
            "process": "sequential"
        }"#;
        let options = AdapterOptions::default();
        let output = CrewAiAdapter::translate(input, &options).unwrap();
        assert!(!output.ctxdsl.is_empty());
        assert_eq!(output.source_info.format, SourceFormat::CrewAi);
        // Should contain the automaton states
        assert!(output.ctxdsl.contains("research_researcher"));
        assert!(output.ctxdsl.contains("write_writer"));
        assert!(output.ctxdsl.contains("done"));
    }

    #[test]
    fn translate_hierarchical() {
        let input = r#"{
            "agents": [
                {"role": "researcher", "allow_delegation": true, "tools": []},
                {"role": "writer", "allow_delegation": false, "tools": []}
            ],
            "tasks": [
                {"name": "research", "agent_role": "researcher"}
            ],
            "process": "hierarchical"
        }"#;
        let options = AdapterOptions::default();
        let output = CrewAiAdapter::translate(input, &options).unwrap();
        assert!(!output.ctxdsl.is_empty());
        // Hierarchical should have supervisor and worker states
        assert!(output.ctxdsl.contains("ACTIVATE_RESEARCHER"));
    }

    #[test]
    fn reject_empty_tasks() {
        let input = r#"{"agents": [], "tasks": [], "process": "sequential"}"#;
        let options = AdapterOptions::default();
        let result = CrewAiAdapter::translate(input, &options);
        assert!(result.is_err());
    }
}
