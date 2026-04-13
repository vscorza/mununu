//! Emit a synthesized controller CLTS as XState v5 JSON.
//!
//! The output is a flat state machine (no hierarchy). States map to XState
//! states and transitions map to event-keyed transitions. Synthesis metadata
//! is included in the `__mununu` annotation block.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use serde_json::{Map, Value, json};

/// Convert a synthesized controller CLTS to an XState v5 JSON string.
///
/// The controller is a sub-CLTS produced by synthesis — it has the same state
/// names as the original but fewer transitions (those removed by the strategy).
pub fn controller_to_xstate_json(
    controller: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    machine_id: &str,
    realizable: bool,
) -> String {
    let mut states_map = Map::new();
    let mut initial_state: Option<String> = None;

    let initials = controller.initial_states();

    for state_id in controller.states() {
        let state_name = controller
            .state_name(state_id)
            .unwrap_or("unknown")
            .to_string();

        if initials.contains(&state_id) && initial_state.is_none() {
            initial_state = Some(state_name.clone());
        }

        // Collect outgoing transitions grouped by event (first label payload)
        let mut event_transitions: Map<String, Value> = Map::new();
        for transition in controller.outgoing(state_id) {
            let target_name = controller
                .state_name(transition.target())
                .unwrap_or("unknown")
                .to_string();

            // The event name is the first label's first payload string
            let event_name = transition
                .labels()
                .first()
                .and_then(|lid| controller.label_payload(*lid))
                .and_then(|payload| payload.first())
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            // If this event already has a target, convert to array
            if let Some(existing) = event_transitions.get(&event_name) {
                let mut arr = match existing {
                    Value::String(_) | Value::Object(_) => vec![existing.clone()],
                    Value::Array(a) => a.clone(),
                    _ => vec![],
                };
                arr.push(json!({ "target": target_name }));
                event_transitions.insert(event_name, Value::Array(arr));
            } else {
                event_transitions.insert(event_name, Value::String(target_name));
            }
        }

        let mut state_obj = Map::new();
        if !event_transitions.is_empty() {
            state_obj.insert("on".to_string(), Value::Object(event_transitions));
        }
        states_map.insert(state_name, Value::Object(state_obj));
    }

    let machine = json!({
        "id": format!("{machine_id}_controller"),
        "initial": initial_state.unwrap_or_default(),
        "states": states_map,
        "__mununu": {
            "synthesis_result": if realizable { "realizable" } else { "unrealizable" },
            "generated_by": "mununu"
        }
    });

    serde_json::to_string_pretty(&machine).unwrap_or_else(|_| "{}".to_string())
}
