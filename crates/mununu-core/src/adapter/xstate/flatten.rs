//! Hierarchy flattening for XState compound and parallel states.
//!
//! Converts a recursive XState state tree into a flat list of states and
//! transitions suitable for the AdapterIR's `AutomatonSpec`.
//!
//! - **Compound states**: children are flattened with parent prefix.
//!   Transitions targeting the parent redirect to its `initial` child.
//! - **Parallel states**: each region becomes a separate `FlatRegion`
//!   for synchronous composition.
//! - **Final states**: become absorbing (no outgoing transitions).

use super::ast::{TransitionConfig, XStateNode};
use std::collections::HashMap;

/// A flattened state with its fully-qualified name.
#[derive(Debug, Clone)]
pub struct FlatState {
    pub name: String,
    pub is_initial: bool,
    pub is_final: bool,
}

/// A flattened transition between fully-qualified state names.
#[derive(Debug, Clone)]
pub struct FlatTransition {
    pub source: String,
    pub target: String,
    pub event: String,
    pub guard: Option<String>,
}

/// A flat region — one automaton's worth of states and transitions.
/// Parallel states produce multiple regions.
#[derive(Debug, Clone)]
pub struct FlatRegion {
    pub name: String,
    pub states: Vec<FlatState>,
    pub transitions: Vec<FlatTransition>,
}

/// Result of flattening the entire machine.
#[derive(Debug, Clone)]
pub struct FlattenResult {
    /// Regions to be emitted as separate automata (one per parallel branch,
    /// or a single region if no parallelism).
    pub regions: Vec<FlatRegion>,
}

/// Flatten a top-level XState machine's states into `FlatRegion`s.
pub fn flatten_machine(
    states: &HashMap<String, XStateNode>,
    initial: Option<&str>,
    machine_id: &str,
) -> FlattenResult {
    let mut regions = Vec::new();
    let mut flat_states = Vec::new();
    let mut flat_transitions = Vec::new();

    for (name, node) in states {
        let is_init = initial.is_some_and(|i| i == name);

        if node.type_.as_deref() == Some("parallel") {
            // Each child of a parallel node becomes its own region
            if let Some(children) = &node.states {
                flatten_parallel(children, name, &mut regions);
            }
        } else if node.states.is_some() {
            // Compound state: flatten recursively
            flatten_compound(node, name, is_init, &mut flat_states, &mut flat_transitions);
        } else {
            // Simple or final state
            let is_final = node.type_.as_deref() == Some("final");
            flat_states.push(FlatState {
                name: name.clone(),
                is_initial: is_init,
                is_final,
            });
            collect_transitions(node, name, &mut flat_transitions);
        }
    }

    // If we collected any non-parallel states, they form the main region
    if !flat_states.is_empty() {
        // Resolve targets that point to compound states → redirect to initial child
        resolve_compound_targets(&flat_states, states, &mut flat_transitions);

        regions.push(FlatRegion {
            name: machine_id.to_string(),
            states: flat_states,
            transitions: flat_transitions,
        });
    }

    FlattenResult { regions }
}

/// Flatten a compound (nested) state into the parent's flat state list.
fn flatten_compound(
    node: &XStateNode,
    prefix: &str,
    parent_is_initial: bool,
    flat_states: &mut Vec<FlatState>,
    flat_transitions: &mut Vec<FlatTransition>,
) {
    let children = match &node.states {
        Some(c) => c,
        None => return,
    };

    let child_initial = node.initial.as_deref();

    for (child_name, child_node) in children {
        let full_name = format!("{prefix}_{child_name}");
        let is_init = parent_is_initial && child_initial.is_some_and(|i| i == child_name.as_str());

        if child_node.states.is_some() && child_node.type_.as_deref() != Some("parallel") {
            // Nested compound: recurse
            flatten_compound(
                child_node,
                &full_name,
                is_init,
                flat_states,
                flat_transitions,
            );
        } else {
            let is_final = child_node.type_.as_deref() == Some("final");
            flat_states.push(FlatState {
                name: full_name.clone(),
                is_initial: is_init,
                is_final,
            });
            // Collect transitions with prefixed source
            collect_transitions_prefixed(child_node, &full_name, prefix, flat_transitions);
        }
    }

    // Parent-level transitions apply to all children
    collect_transitions_for_all_children(node, prefix, children, flat_transitions);
}

/// Flatten parallel regions: each child of the parallel node becomes its own region.
fn flatten_parallel(
    children: &HashMap<String, XStateNode>,
    parallel_name: &str,
    regions: &mut Vec<FlatRegion>,
) {
    for (region_name, region_node) in children {
        let full_region_name = format!("{parallel_name}_{region_name}");
        let mut states = Vec::new();
        let mut transitions = Vec::new();

        let child_initial = region_node.initial.as_deref();

        if let Some(region_states) = &region_node.states {
            for (state_name, state_node) in region_states {
                let full_name = format!("{full_region_name}_{state_name}");
                let is_init = child_initial.is_some_and(|i| i == state_name.as_str());
                let is_final = state_node.type_.as_deref() == Some("final");

                states.push(FlatState {
                    name: full_name.clone(),
                    is_initial: is_init,
                    is_final,
                });
                collect_transitions_prefixed(
                    state_node,
                    &full_name,
                    &full_region_name,
                    &mut transitions,
                );
            }
        }

        regions.push(FlatRegion {
            name: full_region_name,
            states,
            transitions,
        });
    }
}

/// Collect transitions from a simple state node.
fn collect_transitions(node: &XStateNode, source: &str, out: &mut Vec<FlatTransition>) {
    for (event, config) in &node.on {
        match config {
            TransitionConfig::Simple(target) => {
                out.push(FlatTransition {
                    source: source.to_string(),
                    target: target.clone(),
                    event: event.clone(),
                    guard: None,
                });
            }
            TransitionConfig::Guarded(gt) => {
                if let Some(target) = &gt.target {
                    out.push(FlatTransition {
                        source: source.to_string(),
                        target: target.clone(),
                        event: event.clone(),
                        guard: gt.guard.clone(),
                    });
                }
            }
            TransitionConfig::Array(arr) => {
                for gt in arr {
                    if let Some(target) = &gt.target {
                        out.push(FlatTransition {
                            source: source.to_string(),
                            target: target.clone(),
                            event: event.clone(),
                            guard: gt.guard.clone(),
                        });
                    }
                }
            }
        }
    }
}

/// Collect transitions with prefixed source and target resolution.
fn collect_transitions_prefixed(
    node: &XStateNode,
    full_source: &str,
    prefix: &str,
    out: &mut Vec<FlatTransition>,
) {
    for (event, config) in &node.on {
        let targets = match config {
            TransitionConfig::Simple(t) => vec![(t.clone(), None)],
            TransitionConfig::Guarded(gt) => {
                if let Some(t) = &gt.target {
                    vec![(t.clone(), gt.guard.clone())]
                } else {
                    vec![]
                }
            }
            TransitionConfig::Array(arr) => arr
                .iter()
                .filter_map(|gt| gt.target.as_ref().map(|t| (t.clone(), gt.guard.clone())))
                .collect(),
        };

        for (raw_target, guard) in targets {
            // If target doesn't contain a dot or prefix, it's relative to the same parent
            let resolved_target = if raw_target.contains('.') {
                // Dot-separated path: convert to underscore
                raw_target.replace('.', "_")
            } else {
                // Simple name: prefix with parent
                format!("{prefix}_{raw_target}")
            };
            out.push(FlatTransition {
                source: full_source.to_string(),
                target: resolved_target,
                event: event.clone(),
                guard,
            });
        }
    }
}

/// Parent-level transitions on a compound state apply to all children.
fn collect_transitions_for_all_children(
    parent: &XStateNode,
    prefix: &str,
    children: &HashMap<String, XStateNode>,
    out: &mut Vec<FlatTransition>,
) {
    for (event, config) in &parent.on {
        let targets = match config {
            TransitionConfig::Simple(t) => vec![(t.clone(), None)],
            TransitionConfig::Guarded(gt) => {
                if let Some(t) = &gt.target {
                    vec![(t.clone(), gt.guard.clone())]
                } else {
                    vec![]
                }
            }
            TransitionConfig::Array(arr) => arr
                .iter()
                .filter_map(|gt| gt.target.as_ref().map(|t| (t.clone(), gt.guard.clone())))
                .collect(),
        };

        for (raw_target, guard) in &targets {
            for child_name in children.keys() {
                let full_source = format!("{prefix}_{child_name}");
                out.push(FlatTransition {
                    source: full_source,
                    target: raw_target.clone(),
                    event: event.clone(),
                    guard: guard.clone(),
                });
            }
        }
    }
}

/// Resolve transitions that target a compound state name: redirect to
/// the compound state's initial child.
fn resolve_compound_targets(
    _flat_states: &[FlatState],
    original_states: &HashMap<String, XStateNode>,
    transitions: &mut [FlatTransition],
) {
    for t in transitions.iter_mut() {
        // Check if the target is a compound state (has children + initial)
        if let Some(node) = original_states.get(&t.target)
            && node.states.is_some()
            && let Some(initial) = &node.initial
        {
            t.target = format!("{}_{}", t.target, initial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::xstate::ast::XStateMachine;

    fn parse_and_flatten(json: &str) -> FlattenResult {
        let machine: XStateMachine = serde_json::from_str(json).unwrap();
        let id = machine.id.as_deref().unwrap_or("test");
        flatten_machine(&machine.states, machine.initial.as_deref(), id)
    }

    #[test]
    fn flatten_simple_states() {
        let result = parse_and_flatten(
            r#"{
            "id": "light",
            "initial": "green",
            "states": {
                "green": { "on": { "TIMER": "yellow" } },
                "yellow": { "on": { "TIMER": "red" } },
                "red": { "on": { "TIMER": "green" } }
            }
        }"#,
        );
        assert_eq!(result.regions.len(), 1);
        let region = &result.regions[0];
        assert_eq!(region.states.len(), 3);
        assert_eq!(region.transitions.len(), 3);
        assert!(
            region
                .states
                .iter()
                .any(|s| s.name == "green" && s.is_initial)
        );
    }

    #[test]
    fn flatten_compound_state() {
        let result = parse_and_flatten(
            r#"{
            "id": "auth",
            "initial": "auth",
            "states": {
                "auth": {
                    "initial": "idle",
                    "states": {
                        "idle": { "on": { "LOGIN": "pending" } },
                        "pending": { "on": { "OK": "done" } },
                        "done": {}
                    }
                }
            }
        }"#,
        );
        assert_eq!(result.regions.len(), 1);
        let region = &result.regions[0];
        assert_eq!(region.states.len(), 3);
        // States should be prefixed
        assert!(
            region
                .states
                .iter()
                .any(|s| s.name == "auth_idle" && s.is_initial)
        );
        assert!(region.states.iter().any(|s| s.name == "auth_pending"));
        assert!(region.states.iter().any(|s| s.name == "auth_done"));
    }

    #[test]
    fn flatten_parallel_produces_multiple_regions() {
        let result = parse_and_flatten(
            r#"{
            "id": "app",
            "initial": "main",
            "states": {
                "main": {
                    "type": "parallel",
                    "states": {
                        "display": {
                            "initial": "light",
                            "states": {
                                "light": { "on": { "TOGGLE_THEME": "dark" } },
                                "dark": { "on": { "TOGGLE_THEME": "light" } }
                            }
                        },
                        "audio": {
                            "initial": "muted",
                            "states": {
                                "muted": { "on": { "UNMUTE": "playing" } },
                                "playing": { "on": { "MUTE": "muted" } }
                            }
                        }
                    }
                }
            }
        }"#,
        );
        // Parallel → two regions
        assert_eq!(result.regions.len(), 2);
        let names: Vec<&str> = result.regions.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"main_display"));
        assert!(names.contains(&"main_audio"));
    }

    #[test]
    fn flatten_final_state_marked() {
        let result = parse_and_flatten(
            r#"{
            "id": "flow",
            "initial": "active",
            "states": {
                "active": { "on": { "DONE": "finished" } },
                "finished": { "type": "final" }
            }
        }"#,
        );
        let region = &result.regions[0];
        let finished = region.states.iter().find(|s| s.name == "finished").unwrap();
        assert!(finished.is_final);
    }
}
