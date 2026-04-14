//! A2A (Agent-to-Agent) adapter.
//!
//! Translates A2A Agent Card JSON into CTXDSL via XState JSON as an
//! intermediate format. Each agent gets a task-lifecycle state machine
//! (idle → queued → in_progress → completed|failed). Multiple agents
//! are composed as parallel regions with mutex properties.

use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, FormatAdapter, SourceFormat,
    SourceInfo,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// A2A adapter implementing [`FormatAdapter`].
pub struct A2aAdapter;

// ---------------------------------------------------------------------------
// JSON AST types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AgentCard {
    #[serde(default = "default_agent_name")]
    name: String,
    #[serde(default)]
    skills: Vec<SkillDef>,
}

fn default_agent_name() -> String {
    "agent".to_string()
}

#[derive(Debug, Deserialize)]
struct SkillDef {
    id: Option<String>,
    name: Option<String>,
}

/// Input can be a single agent card or an array of cards.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum A2aInput {
    Multiple(Vec<AgentCard>),
    Single(AgentCard),
}

// ---------------------------------------------------------------------------
// FormatAdapter impl
// ---------------------------------------------------------------------------

impl FormatAdapter for A2aAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        // Single card: {"name": ..., "skills": [...]}
        // Multiple cards: [{"name": ..., "skills": [...]}, ...]
        if trimmed.starts_with('{') {
            return trimmed.contains("\"name\"")
                && trimmed.contains("\"skills\"")
                && !trimmed.contains("\"agents\"")
                && !trimmed.contains("\"nodes\"")
                && !trimmed.contains("\"states\"");
        }
        if trimmed.starts_with('[') {
            return trimmed.contains("\"name\"") && trimmed.contains("\"skills\"");
        }
        false
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let cards: Vec<AgentCard> = match serde_json::from_str::<A2aInput>(content) {
            Ok(A2aInput::Single(card)) => vec![card],
            Ok(A2aInput::Multiple(cards)) => cards,
            Err(e) => {
                return Err(AdapterError {
                    kind: AdapterErrorKind::ParseError,
                    message: format!("A2A JSON parse error: {e}"),
                    location: None,
                });
            }
        };

        if cards.is_empty() {
            return Err(AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: "A2A input has no agent cards".to_string(),
                location: None,
            });
        }

        let machine_id = options.context_name.as_deref().unwrap_or("a2a_protocol");

        let xstate_json = build_xstate_json(&cards, machine_id);
        let xstate_str = serde_json::to_string(&xstate_json).unwrap();

        let mut output =
            super::xstate::XStateAdapter::translate(&xstate_str, options).map_err(|e| {
                AdapterError {
                    kind: AdapterErrorKind::EmitError,
                    message: format!("A2A→XState translation failed: {e}"),
                    location: None,
                }
            })?;

        output.source_info = SourceInfo {
            format: SourceFormat::A2a,
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

fn build_xstate_json(cards: &[AgentCard], machine_id: &str) -> Value {
    let mut ctrl = BTreeSet::new();
    let mut unctrl = BTreeSet::new();
    let mut agent_regions = serde_json::Map::new();
    let mut agent_names = Vec::new();

    for card in cards {
        let aname = sanitize(&card.name);
        agent_names.push(aname.clone());

        let idle_s = format!("idle_{aname}");
        let queued_s = format!("queued_{aname}");
        let in_progress_s = format!("in_progress_{aname}");
        let completed_s = format!("completed_{aname}");
        let failed_s = format!("failed_{aname}");

        // Skill invocation events (controllable)
        let mut invoke_on = serde_json::Map::new();
        if card.skills.is_empty() {
            let ev = format!("INVOKE_{}", aname.to_uppercase());
            invoke_on.insert(ev.clone(), json!(&queued_s));
            ctrl.insert(ev);
        } else {
            for skill in &card.skills {
                let sid = sanitize(
                    skill
                        .id
                        .as_deref()
                        .or(skill.name.as_deref())
                        .unwrap_or("skill"),
                );
                let ev = format!("INVOKE_{}_{}", aname.to_uppercase(), sid.to_uppercase());
                invoke_on.insert(ev.clone(), json!(&queued_s));
                ctrl.insert(ev);
            }
        }

        let cancel_ev = format!("CANCEL_{}", aname.to_uppercase());
        let start_ev = format!("START_{}", aname.to_uppercase());
        let complete_ev = format!("COMPLETED_{}", aname.to_uppercase());
        let fail_ev = format!("FAILED_{}", aname.to_uppercase());
        let timeout_ev = format!("TIMEOUT_{}", aname.to_uppercase());
        let reset_ev = format!("RESET_{}", aname.to_uppercase());

        ctrl.insert(cancel_ev.clone());
        ctrl.insert(reset_ev.clone());
        unctrl.insert(start_ev.clone());
        unctrl.insert(complete_ev.clone());
        unctrl.insert(fail_ev.clone());
        unctrl.insert(timeout_ev.clone());

        let a_states = json!({
            &idle_s: {"on": Value::Object(invoke_on)},
            &queued_s: {"on": {
                &start_ev: &in_progress_s,
                &cancel_ev: &idle_s,
                &timeout_ev: &failed_s
            }},
            &in_progress_s: {"on": {
                &complete_ev: &completed_s,
                &fail_ev: &failed_s,
                &timeout_ev: &failed_s
            }},
            &completed_s: {"on": {&reset_ev: &idle_s}},
            &failed_s: {"on": {&reset_ev: &idle_s}}
        });

        agent_regions.insert(
            aname.clone(),
            json!({"initial": &idle_s, "states": a_states}),
        );
    }

    // Build top-level structure
    let (states, initial) = if cards.len() == 1 {
        let aname = &agent_names[0];
        let region = agent_regions.remove(aname).unwrap();
        let s = region.get("states").cloned().unwrap_or(json!({}));
        let i = region
            .get("initial")
            .and_then(|v| v.as_str())
            .unwrap_or("idle")
            .to_string();
        (s, i)
    } else {
        let states = json!({
            "system": {
                "type": "parallel",
                "states": Value::Object(agent_regions)
            }
        });
        (states, "system".to_string())
    };

    // Properties
    let mut props = vec![json!({
        "name": "safety_invariant",
        "formula": "nu X. ([] X)",
        "role": "guarantee"
    })];

    // Mutex properties for multi-agent
    if cards.len() >= 2 {
        for i in 0..agent_names.len() {
            for j in (i + 1)..agent_names.len() {
                props.push(json!({
                    "name": format!("mutex_{}_{}", agent_names[i], agent_names[j]),
                    "formula": format!(
                        "nu X. ((!in_progress_{} || !in_progress_{}) && ([] X))",
                        agent_names[i], agent_names[j]
                    ),
                    "role": "guarantee"
                }));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_a2a_json() {
        let single = r#"{"name": "agent1", "skills": [{"id": "search", "name": "Search"}]}"#;
        assert!(A2aAdapter::detect(single));

        let array = r#"[{"name": "a", "skills": []}, {"name": "b", "skills": []}]"#;
        assert!(A2aAdapter::detect(array));

        // Should not detect other formats
        assert!(!A2aAdapter::detect(r#"{"agents": [], "tasks": []}"#));
        assert!(!A2aAdapter::detect(r#"{"nodes": [], "edges": []}"#));
    }

    #[test]
    fn translate_single_agent() {
        let input = r#"{
            "name": "researcher",
            "skills": [
                {"id": "web_search", "name": "Web Search"},
                {"id": "summarize", "name": "Summarize"}
            ]
        }"#;
        let options = AdapterOptions::default();
        let output = A2aAdapter::translate(input, &options).unwrap();
        assert!(!output.ctxdsl.is_empty());
        assert_eq!(output.source_info.format, SourceFormat::A2a);
        assert!(output.ctxdsl.contains("idle_researcher"));
        assert!(output.ctxdsl.contains("INVOKE_RESEARCHER_WEB_SEARCH"));
    }

    #[test]
    fn translate_multi_agent() {
        let input = r#"[
            {"name": "researcher", "skills": [{"id": "search"}]},
            {"name": "writer", "skills": [{"id": "draft"}]}
        ]"#;
        let options = AdapterOptions::default();
        let output = A2aAdapter::translate(input, &options).unwrap();
        assert!(!output.ctxdsl.is_empty());
        // Should have parallel composition
        assert!(output.ctxdsl.contains("researcher"));
        assert!(output.ctxdsl.contains("writer"));
    }

    #[test]
    fn reject_empty_cards() {
        let input = r#"[]"#;
        let options = AdapterOptions::default();
        let result = A2aAdapter::translate(input, &options);
        assert!(result.is_err());
    }
}
