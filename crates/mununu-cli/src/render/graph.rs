use mununu_core::abstraction::unrolling::{
    Effect, OriginalState, OriginalTransition, UnrollingOptions, VariableDecl, unroll_states,
};
use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::context_dsl::ast::{
    BinaryOp, Expr, ExprKind, StateRef, StateSelector, TransitionLabel, UnaryOp,
};
use mununu_core::context_dsl::{ContextDoc, RealizedContext};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;

// Cytoscape element structures
#[derive(Serialize, Debug, Clone)]
pub(crate) struct CytoscapeElement {
    pub(crate) data: CytoscapeData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<CytoscapePosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) classes: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(untagged)]
#[allow(non_snake_case)]
pub(crate) enum CytoscapeData {
    Node {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        vars: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actions: Option<serde_json::Value>,
        /// Structured per-state valuations (`{key: value}`). Sourced from
        /// `Clts::state_valuation()`. Rendered inline under the state label
        /// by the Cytoscape style function in `generate_cytoscape_html`.
        #[serde(skip_serializing_if = "Option::is_none")]
        valuations: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        isStart: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        isDead: Option<bool>,
    },
    Edge {
        id: String,
        source: String,
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actionType: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        guard: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect: Option<String>,
    },
}

#[derive(Serialize, Debug, Clone)]
pub(crate) struct CytoscapePosition {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// Collect action names with controllability annotation from an automaton's
/// alphabet.  Shared by both DSL and unrolled Cytoscape builders.
fn collect_action_info(automaton: &mununu_core::context_dsl::ast::Automaton) -> Vec<String> {
    automaton
        .alphabet
        .iter()
        .map(|label_ref| {
            let label_name = &label_ref.name.name;
            let is_controllable = automaton
                .controllable
                .iter()
                .any(|c| c.name.name == *label_name);
            let is_internal = automaton
                .internal
                .iter()
                .any(|i| i.name.name == *label_name);
            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };
            format!("{} ({})", label_name, action_type)
        })
        .collect()
}

/// Build the Cytoscape compound node that wraps an automaton's states and edges.
/// Shape is identical for both DSL and unrolled builders; only `id` and `label`
/// differ at the call site.
fn build_automaton_compound_node(
    id: String,
    label: String,
    var_names: &[String],
    action_info: &[String],
) -> CytoscapeElement {
    CytoscapeElement {
        data: CytoscapeData::Node {
            id,
            parent: None,
            label: Some(label),
            vars: if var_names.is_empty() {
                None
            } else {
                Some(json!(var_names))
            },
            actions: if action_info.is_empty() {
                None
            } else {
                Some(json!(action_info))
            },
            valuations: None,
            note: None,
            isStart: None,
            isDead: None,
        },
        position: None,
        classes: None,
    }
}

/// Classify a label as controllable/internal/uncontrollable from an automaton's
/// declarations.  Used by both Cytoscape builders for edge `actionType`.
fn classify_action(
    automaton: &mununu_core::context_dsl::ast::Automaton,
    label_names: &[String],
) -> &'static str {
    let is_internal = label_names
        .iter()
        .any(|l| automaton.internal.iter().any(|i| i.name.name == *l));
    let is_controllable = label_names
        .iter()
        .any(|l| automaton.controllable.iter().any(|c| c.name.name == *l));
    if is_internal {
        "internal"
    } else if is_controllable {
        "controllable"
    } else {
        "uncontrollable"
    }
}

/// Parameters for [`build_state_elements`], grouped to stay under the
/// clippy `too_many_arguments` limit.
struct StateElementParams {
    state_id: String,
    automaton_id: String,
    label: String,
    is_initial: bool,
    is_dead: bool,
    state_var_str: Option<String>,
    /// Structured per-state valuations keyed by variable name. Sourced from
    /// `Clts::state_valuation()`. The HTML template renders them inline
    /// under the state label as `{key1=val1, key2=val2}`.
    state_valuations: Option<std::collections::BTreeMap<String, String>>,
    x: f64,
    y: f64,
    entry_node_id: Option<String>,
    entry_edge_id: Option<String>,
}

/// Build the Cytoscape elements for a state node plus its entry arrow (if
/// initial).  Shared by both DSL and unrolled Cytoscape builders.
fn build_state_elements(p: StateElementParams) -> Vec<CytoscapeElement> {
    let mut elems = Vec::new();

    let mut classes = vec!["state"];
    if p.is_initial {
        classes.push("start");
    }
    if p.is_dead {
        classes.push("dead");
    }

    elems.push(CytoscapeElement {
        data: CytoscapeData::Node {
            id: p.state_id.clone(),
            parent: Some(p.automaton_id.clone()),
            label: Some(p.label),
            vars: p.state_var_str.map(|s| json!(s)),
            actions: None,
            valuations: p
                .state_valuations
                .filter(|m| !m.is_empty())
                .map(|m| json!(m)),
            note: if p.is_initial {
                Some("Initial state".to_string())
            } else if p.is_dead {
                Some("Terminal state".to_string())
            } else {
                None
            },
            isStart: Some(p.is_initial),
            isDead: Some(p.is_dead),
        },
        position: Some(CytoscapePosition { x: p.x, y: p.y }),
        classes: Some(classes.join(" ")),
    });

    if p.is_initial
        && let (Some(en_id), Some(ee_id)) = (p.entry_node_id, p.entry_edge_id)
    {
        elems.push(CytoscapeElement {
            data: CytoscapeData::Node {
                id: en_id.clone(),
                parent: Some(p.automaton_id),
                label: None,
                vars: None,
                actions: None,
                valuations: None,
                note: None,
                isStart: None,
                isDead: None,
            },
            position: Some(CytoscapePosition { x: 40.0, y: p.y }),
            classes: Some("entry".to_string()),
        });

        elems.push(CytoscapeElement {
            data: CytoscapeData::Edge {
                id: ee_id,
                source: en_id,
                target: p.state_id,
                label: None,
                action: None,
                actionType: Some("start-arrow".to_string()),
                guard: None,
                effect: None,
            },
            position: None,
            classes: None,
        });
    }

    elems
}

pub(crate) fn counterstrategy_to_cytoscape(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    automaton_name: &str,
    winning_set: &std::collections::HashSet<usize>,
) -> Vec<CytoscapeElement> {
    let mut elements = Vec::new();
    let mut x_pos = 100.0;

    // Compound node
    elements.push(CytoscapeElement {
        data: CytoscapeData::Node {
            id: automaton_name.to_string(),
            label: Some(format!(
                "Counterstrategy: {}",
                automaton_name.replace("_counterstrategy", "")
            )),
            parent: None,
            vars: None,
            actions: None,
            valuations: None,
            note: None,
            isStart: None,
            isDead: None,
        },
        position: None,
        classes: None,
    });

    // State nodes
    for state_id in clts.states() {
        if !winning_set.contains(&state_id.index()) {
            continue;
        }
        let name = clts.state_name(state_id).unwrap_or("?").to_string();
        let node_id = format!("{}_{}", automaton_name, name);
        let is_initial = clts.initial_states().contains(&state_id);

        let mut classes = vec!["env-winning"];
        if is_initial {
            classes.push("start");
        }

        let val = clts
            .state_valuation(state_id)
            .filter(|m| !m.is_empty())
            .map(|m| json!(m));

        elements.push(CytoscapeElement {
            data: CytoscapeData::Node {
                id: node_id,
                label: Some(name),
                parent: Some(automaton_name.to_string()),
                vars: None,
                actions: None,
                valuations: val,
                note: None,
                isStart: Some(is_initial),
                isDead: Some(false),
            },
            position: Some(CytoscapePosition { x: x_pos, y: 100.0 }),
            classes: Some(classes.join(" ")),
        });
        x_pos += 250.0;
    }

    // Transitions between winning states
    for state_id in clts.states() {
        if !winning_set.contains(&state_id.index()) {
            continue;
        }
        let source = clts.state_name(state_id).unwrap_or("?").to_string();
        let source_id = format!("{}_{}", automaton_name, source);

        for transition in clts.outgoing(state_id) {
            if !winning_set.contains(&transition.target().index()) {
                continue;
            }
            let target = clts
                .state_name(transition.target())
                .unwrap_or("?")
                .to_string();
            let target_id = format!("{}_{}", automaton_name, target);

            let label: Vec<String> = transition
                .labels()
                .iter()
                .filter_map(|lid| {
                    clts.label_payload(*lid).and_then(|vals| {
                        let joined = vals
                            .iter()
                            .filter(|v| !v.is_empty())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ");
                        if joined.is_empty() {
                            None
                        } else {
                            Some(joined)
                        }
                    })
                })
                .collect();

            elements.push(CytoscapeElement {
                data: CytoscapeData::Edge {
                    id: format!("{}_t{}", source_id, elements.len()),
                    source: source_id.clone(),
                    target: target_id,
                    label: if label.is_empty() {
                        None
                    } else {
                        Some(label.join(" | "))
                    },
                    action: None,
                    actionType: if transition.is_uncontrollable(clts) {
                        Some("uncontrollable".to_string())
                    } else {
                        Some("controllable".to_string())
                    },
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
        }
    }

    elements
}

pub(crate) fn dsl_automata_to_cytoscape(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automata_names: &[String],
) -> Result<Vec<CytoscapeElement>, String> {
    let mut elements = Vec::new();
    let mut y_offset = 0.0;
    let state_spacing = 250.0;

    for automaton_name in automata_names {
        // Find the automaton in documents
        let automaton = std::iter::once(context_doc)
            .chain(sidecar_docs.iter())
            .find_map(|doc| doc.automata.iter().find(|a| a.name.name == *automaton_name))
            .ok_or_else(|| format!("automaton '{}' not found in documents", automaton_name))?;

        // Get the realized CLTS
        let clts = realized.context.clts(automaton_name).ok_or_else(|| {
            format!(
                "automaton '{}' not found in realized context",
                automaton_name
            )
        })?;

        // Collect variable names
        let var_names: Vec<String> = automaton
            .variables
            .iter()
            .map(|v| v.name.name.clone())
            .collect();

        let action_info = collect_action_info(automaton);

        // Create automaton compound node
        let automaton_id = automaton_name.clone();
        elements.push(build_automaton_compound_node(
            automaton_id.clone(),
            format!("Automaton {}", automaton_name),
            &var_names,
            &action_info,
        ));

        let mut x_pos = 100.0;

        for state_decl in &automaton.states {
            let state_name = &state_decl.name.name;
            let state_id = format!("{}_{}", automaton_name, state_name);
            let is_initial = state_decl.is_initial;

            // Check if state is terminal (dead) - states with no outgoing transitions
            let is_dead = clts
                .state_id(state_name)
                .map(|sid| clts.outgoing(sid).is_empty())
                .unwrap_or(false);

            // Get variable values for this state
            let state_var_str = if var_names.is_empty() {
                None
            } else {
                clts.state_id(state_name).ok().and_then(|sid| {
                    let vars = clts.state_variables(sid);
                    if vars.is_empty() {
                        None
                    } else {
                        Some(vars.join(", "))
                    }
                })
            };

            let state_valuations = clts
                .state_id(state_name)
                .ok()
                .and_then(|sid| clts.state_valuation(sid).cloned());

            elements.extend(build_state_elements(StateElementParams {
                state_id,
                automaton_id: automaton_id.clone(),
                label: format!("{}_{}", automaton_name, state_name),
                is_initial,
                is_dead,
                state_var_str,
                state_valuations,
                x: x_pos,
                y: y_offset + 100.0,
                entry_node_id: if is_initial {
                    Some(format!("{}_entry", automaton_name))
                } else {
                    None
                },
                entry_edge_id: if is_initial {
                    Some(format!("{}_entry_edge", automaton_name))
                } else {
                    None
                },
            }));

            x_pos += state_spacing;
        }

        // Add transitions
        for transition in &automaton.transitions {
            let source_name = match &transition.source {
                StateSelector::Named(state_ref) => match state_ref {
                    StateRef::Simple(ident) => ident.name.clone(),
                    StateRef::Indexed { name, .. } => name.name.clone(),
                },
                _ => continue, // Skip group/wildcard selectors for now
            };

            let target_name = match &transition.target {
                StateSelector::Named(state_ref) => match state_ref {
                    StateRef::Simple(ident) => ident.name.clone(),
                    StateRef::Indexed { name, .. } => name.name.clone(),
                },
                _ => continue, // Skip group/wildcard selectors for now
            };

            let primary_label = match &transition.label {
                TransitionLabel::Named { name, .. } => name.name.clone(),
                TransitionLabel::Epsilon(_) => "ε".to_string(),
            };
            let mut all_label_names = vec![primary_label.clone()];
            for additional in &transition.additional_labels {
                if let TransitionLabel::Named { name, .. } = additional {
                    all_label_names.push(name.name.clone());
                }
            }
            let label_name = all_label_names.join(", ");

            let action_type = classify_action(automaton, &all_label_names);

            // Format guard
            let guard_str = transition
                .guard
                .as_ref()
                .map(expr_to_string)
                .unwrap_or_default();

            // Format effects
            let effect_str = if transition.effects.is_empty() {
                String::new()
            } else {
                transition
                    .effects
                    .iter()
                    .map(|a| format!("{}' = {}", a.target.name, expr_to_string(&a.expr)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            // Build label
            let mut label_parts = vec![label_name.clone()];
            if !guard_str.is_empty() {
                label_parts.push(format!("[{}]", guard_str));
            }
            let effect_str_for_label = effect_str.clone();
            if !effect_str_for_label.is_empty() {
                label_parts.push(effect_str_for_label);
            }
            let transition_label = label_parts.join("\n");

            let transition_id = format!("{}_{}_t{}", automaton_name, source_name, elements.len());
            let source_id = format!("{}_{}", automaton_name, source_name);
            let target_id = format!("{}_{}", automaton_name, target_name);

            elements.push(CytoscapeElement {
                data: CytoscapeData::Edge {
                    id: transition_id,
                    source: source_id,
                    target: target_id,
                    label: Some(transition_label),
                    action: Some(label_name),
                    actionType: Some(action_type.to_string()),
                    guard: if guard_str.is_empty() {
                        None
                    } else {
                        Some(guard_str)
                    },
                    effect: if effect_str.is_empty() {
                        None
                    } else {
                        Some(effect_str)
                    },
                },
                position: None,
                classes: None,
            });
        }

        // Move to next automaton row
        y_offset += 200.0;
    }

    Ok(elements)
}

pub(crate) fn unrolled_automata_to_cytoscape(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automata_names: &[String],
) -> Result<Vec<CytoscapeElement>, String> {
    let mut elements = Vec::new();
    let mut y_offset = 0.0;
    let state_spacing = 250.0;

    for automaton_name in automata_names {
        // Find the automaton in documents
        let automaton = std::iter::once(context_doc)
            .chain(sidecar_docs.iter())
            .find_map(|doc| doc.automata.iter().find(|a| a.name.name == *automaton_name))
            .ok_or_else(|| format!("automaton '{}' not found in documents", automaton_name))?;

        // Get the realized CLTS for reference
        let _clts = realized.context.clts(automaton_name).ok_or_else(|| {
            format!(
                "automaton '{}' not found in realized context",
                automaton_name
            )
        })?;

        // Convert DSL automaton to unrolling format
        let original_states: Vec<OriginalState> = automaton
            .states
            .iter()
            .map(|s| OriginalState {
                name: s.name.name.clone(),
                initial: s.is_initial,
            })
            .collect();

        let original_transitions: Vec<OriginalTransition> = automaton
            .transitions
            .iter()
            .filter_map(|t| {
                let source_name = match &t.source {
                    StateSelector::Named(state_ref) => match state_ref {
                        StateRef::Simple(ident) => ident.name.clone(),
                        StateRef::Indexed { name, .. } => name.name.clone(),
                    },
                    _ => return None, // Skip group/wildcard selectors
                };

                let target_name = match &t.target {
                    StateSelector::Named(state_ref) => match state_ref {
                        StateRef::Simple(ident) => ident.name.clone(),
                        StateRef::Indexed { name, .. } => name.name.clone(),
                    },
                    _ => return None, // Skip group/wildcard selectors
                };

                let label_name = match &t.label {
                    TransitionLabel::Named { name, .. } => name.name.clone(),
                    TransitionLabel::Epsilon(_) => "ε".to_string(),
                };

                let guard_str = t
                    .guard
                    .as_ref()
                    .map(|e| strip_outer_parens(&expr_to_string(e)))
                    .unwrap_or_default();

                let effects: Vec<Effect> = t
                    .effects
                    .iter()
                    .map(|a| Effect {
                        target: a.target.name.clone(),
                        value_expr: strip_outer_parens(&expr_to_string(&a.expr)),
                    })
                    .collect();

                Some(OriginalTransition {
                    from: source_name,
                    to: target_name,
                    label: label_name,
                    guard: if guard_str.is_empty() {
                        None
                    } else {
                        Some(guard_str)
                    },
                    effects,
                })
            })
            .collect();

        // Check if automaton has variables to unroll
        if automaton.variables.is_empty() {
            return Err(format!(
                "automaton '{}' has no variables to unroll. Unrolling requires variable declarations in the DSL.",
                automaton_name
            ));
        }

        let variables: Vec<VariableDecl> = automaton
            .variables
            .iter()
            .map(|v| {
                // Extract literal value from expression for initial value
                let initial_str = extract_literal_value(&v.init, &v.ty)
                    .unwrap_or_else(|| strip_outer_parens(&expr_to_string(&v.init)));

                VariableDecl {
                    name: v.name.name.clone(),
                    ty: match &v.ty {
                        mununu_core::context_dsl::ast::TypeName::Bool => "bool".to_string(),
                        mununu_core::context_dsl::ast::TypeName::I64 => "i64".to_string(),
                        mununu_core::context_dsl::ast::TypeName::Enum(_) => "i64".to_string(),
                    },
                    initial: Some(initial_str),
                }
            })
            .collect();

        // Perform unrolling with default options
        // The unrolling algorithm will handle state space explosion by applying
        // interval abstraction and widening when approaching limits
        let unrolling_options = UnrollingOptions::default();

        let unrolled = unroll_states(
            original_states,
            original_transitions,
            variables,
            unrolling_options,
        )
        .map_err(|e| format!("failed to unroll automaton '{}': {}", automaton_name, e))?;

        // Collect variable names for display
        let var_names: Vec<String> = automaton
            .variables
            .iter()
            .map(|v| v.name.name.clone())
            .collect();

        let action_info = collect_action_info(automaton);

        // Create automaton compound node (with "Unrolled" suffix)
        let automaton_id = format!("{}_unrolled", automaton_name);
        elements.push(build_automaton_compound_node(
            automaton_id.clone(),
            format!("Automaton {} (Unrolled)", automaton_name),
            &var_names,
            &action_info,
        ));

        let mut x_pos = 100.0;
        let mut initial_states = HashSet::new();

        // Find initial states - states at initial locations with initial variable values
        let initial_location_names: HashSet<String> = automaton
            .states
            .iter()
            .filter(|s| s.is_initial)
            .map(|s| s.name.name.clone())
            .collect();

        // Collect states that have incoming transitions to determine which are initial
        let states_with_incoming: HashSet<String> = unrolled
            .transitions
            .iter()
            .map(|t| t.to.state_name())
            .collect();

        for state in &unrolled.states {
            // A state is initial if:
            // 1. It's at an initial location, AND
            // 2. It has no incoming transitions (it's a true initial state)
            if initial_location_names.contains(&state.location) {
                let state_name = state.state_name();
                if !states_with_incoming.contains(&state_name) {
                    initial_states.insert(state_name);
                }
            }
        }

        // Create states
        for state in &unrolled.states {
            let state_name = state.state_name();
            let state_id = format!("{}_unrolled_{}", automaton_name, state_name);
            let is_initial = initial_states.contains(&state_name);

            // Check if state is terminal (dead) - states with no outgoing transitions
            let is_dead = unrolled
                .transitions
                .iter()
                .all(|t| t.from.state_name() != state_name);

            // Get variable values for this state
            let state_var_str = if state.variables.is_empty() {
                None
            } else {
                let var_parts: Vec<String> = state
                    .variables
                    .iter()
                    .map(|(name, value)| format!("{} = {}", name, value))
                    .collect();
                Some(var_parts.join(", "))
            };

            elements.extend(build_state_elements(StateElementParams {
                state_id,
                automaton_id: automaton_id.clone(),
                label: state_name.clone(),
                is_initial,
                is_dead,
                state_var_str,
                state_valuations: None,
                x: x_pos,
                y: y_offset + 100.0,
                entry_node_id: if is_initial {
                    Some(format!("{}_unrolled_entry_{}", automaton_name, state_name))
                } else {
                    None
                },
                entry_edge_id: if is_initial {
                    Some(format!(
                        "{}_unrolled_entry_edge_{}",
                        automaton_name, state_name
                    ))
                } else {
                    None
                },
            }));

            x_pos += state_spacing;
        }

        // Add transitions
        for (idx, transition) in unrolled.transitions.iter().enumerate() {
            let from_name = transition.from.state_name();
            let to_name = transition.to.state_name();
            let label_name = transition.label.clone();

            let action_type = classify_action(automaton, std::slice::from_ref(&label_name));

            let transition_id = format!("{}_unrolled_t{}", automaton_name, idx);
            let source_id = format!("{}_unrolled_{}", automaton_name, from_name);
            let target_id = format!("{}_unrolled_{}", automaton_name, to_name);

            elements.push(CytoscapeElement {
                data: CytoscapeData::Edge {
                    id: transition_id,
                    source: source_id,
                    target: target_id,
                    label: Some(label_name.clone()),
                    action: Some(label_name),
                    actionType: Some(action_type.to_string()),
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
        }

        // Move to next automaton row
        y_offset += 200.0;
    }

    Ok(elements)
}

pub(crate) fn generate_cytoscape_html(elements: &[CytoscapeElement]) -> Result<String, String> {
    let elements_json = serde_json::to_string(elements)
        .map_err(|e| format!("failed to serialize elements: {}", e))?;

    let html_template = r###"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <title>Cytoscape Automata Visualization</title>
  <style>
    html, body {
      margin: 0;
      padding: 0;
      height: 100%;
      overflow: hidden;
      font-family: system-ui, sans-serif;
    }
    #cy {
      width: 100%;
      height: 100%;
      display: block;
    }
  </style>
</head>
<body>
  <div id="cy"></div>

  <script src="https://unpkg.com/cytoscape@3.30.0/dist/cytoscape.min.js"></script>
  <script src="https://unpkg.com/dagre@0.8.5/dist/dagre.min.js"></script>
  <script src="https://unpkg.com/cytoscape-dagre@2.5.0/cytoscape-dagre.js"></script>
  <script>
    // Register dagre extension
    cytoscape.use(cytoscapeDagre);

    const elements = ELEMENTS_PLACEHOLDER;

    const cy = cytoscape({
      container: document.getElementById("cy"),
      elements,
      style: [
        {
          selector: "node[^parent]",
          style: {
            "shape": "round-rectangle",
            "background-opacity": 0,
            "border-width": 2,
            "border-style": "dashed",
            "border-color": "#000",
            "padding": "40px",
            "label": ele => {
              const v = (ele.data("vars") || []).join(", ");
              const a = (ele.data("actions") || []).join(", ");
              return ele.data("label") + "\n" + "vars: " + v + "\n" + "actions: " + a;
            },
            "font-size": 11,
            "text-wrap": "wrap",
            "text-max-width": 220,
            "text-halign": "left",
            "text-valign": "top",
            "text-margin-x": 10,
            "text-margin-y": 10,
            "text-background-opacity": 1,
            "text-background-color": "#ffffff",
            "text-background-shape": "round-rectangle",
            "text-outline-width": 0
          }
        },
        {
          selector: "node.state",
          style: {
            "shape": "round-rectangle",
            "background-color": "#ffffff",
            "border-width": 1,
            "border-style": "solid",
            "border-color": "#000000",
            "width": "label",
            "height": "label",
            "padding": "8px",
            "label": ele => {
              const base = ele.data("label") || "";
              const v = ele.data("valuations");
              if (v && typeof v === "object" && Object.keys(v).length > 0) {
                const pairs = Object.keys(v).map(k => k + "=" + v[k]).join(", ");
                return base + "\n{" + pairs + "}";
              }
              return base;
            },
            "font-size": 12,
            "text-wrap": "wrap",
            "text-max-width": 200,
            "text-halign": "center",
            "text-valign": "center"
          }
        },
        {
          selector: "node.start",
          style: {
            "border-width": 4,
            "border-style": "double"
          }
        },
        {
          selector: "node.dead",
          style: {
            "shape": "round-rectangle",
            "border-width": 3,
            "border-style": "solid"
          }
        },
        {
          selector: "node.entry",
          style: {
            "width": 1,
            "height": 1,
            "opacity": 0
          }
        },
        {
          selector: "edge[actionType = 'start-arrow']",
          style: {
            "curve-style": "unbundled-bezier",
            "control-point-distances": [40],
            "control-point-weights": [0.5],
            "line-color": "#000",
            "target-arrow-color": "#000",
            "target-arrow-shape": "triangle",
            "width": 2
          }
        },
        {
          selector: "edge:not([actionType = 'start-arrow'])",
          style: {
            "curve-style": "unbundled-bezier",
            "control-point-distances": [60],
            "control-point-weights": [0.5],
            "line-color": "#000000",
            "target-arrow-color": "#000000",
            "target-arrow-shape": "triangle",
            "width": 2,
            "label": "data(label)",
            "font-size": 11,
            "text-wrap": "wrap",
            "text-max-width": 140,
            "text-background-opacity": 1,
            "text-background-color": "#ffffff",
            "text-background-shape": "round-rectangle",
            "text-margin-y": -6
          }
        },
        {
          selector: "edge[actionType = 'controllable']",
          style: {
            "line-style": "solid",
            "target-arrow-shape": "triangle"
          }
        },
        {
          selector: "edge[actionType = 'uncontrollable']",
          style: {
            "line-style": "dashed",
            "target-arrow-shape": "vee"
          }
        },
        {
          selector: "edge[actionType = 'internal']",
          style: {
            "line-style": "dotted",
            "target-arrow-shape": "triangle"
          }
        }
      ],
      layout: {
        name: "dagre",
        rankDir: "TB",
        spacingFactor: 1.25,
        nodeSep: 50,
        edgeSep: 20,
        rankSep: 80,
        padding: 40,
        animate: true,
        animationDuration: 1000,
        animationEasing: "ease-out"
      }
    });
  </script>
</body>
</html>"###;

    let html = html_template.replace("ELEMENTS_PLACEHOLDER", &elements_json);
    Ok(html)
}

/// Strips outer parentheses from an expression string if they wrap the entire expression.
/// This is needed because the unrolling parser expects simple expressions without
/// unnecessary parentheses. Recursively strips multiple layers of outer parentheses.
fn strip_outer_parens(s: &str) -> String {
    let mut trimmed = s.trim();
    loop {
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
            // Check if the parentheses are balanced and wrap the entire expression
            let mut depth = 0;
            let mut found_outer = false;
            let mut is_fully_wrapped = false;

            for (i, ch) in trimmed.chars().enumerate() {
                match ch {
                    '(' => {
                        depth += 1;
                        if i == 0 {
                            found_outer = true;
                        }
                    }
                    ')' => {
                        depth -= 1;
                        if depth == 0 && i == trimmed.len() - 1 && found_outer {
                            // The outer parentheses wrap the entire expression
                            is_fully_wrapped = true;
                            break;
                        }
                        if depth < 0 {
                            // Unbalanced, return original
                            return trimmed.to_string();
                        }
                    }
                    _ => {}
                }
            }

            if is_fully_wrapped {
                // Strip one layer and continue
                trimmed = trimmed[1..trimmed.len() - 1].trim();
            } else {
                // Not fully wrapped, return as is
                break;
            }
        } else {
            // No outer parentheses, we're done
            break;
        }
    }
    trimmed.to_string()
}

/// Extracts a literal value from an expression if it's a simple constant.
/// Returns None if the expression is not a simple constant.
fn extract_literal_value(
    expr: &Expr,
    ty: &mununu_core::context_dsl::ast::TypeName,
) -> Option<String> {
    match &expr.kind {
        ExprKind::Integer(value) => Some(value.to_string()),
        ExprKind::Ident(ident) => {
            // Check for boolean literals
            if matches!(ty, mununu_core::context_dsl::ast::TypeName::Bool) {
                if ident.name.eq_ignore_ascii_case("true") {
                    return Some("true".to_string());
                } else if ident.name.eq_ignore_ascii_case("false") {
                    return Some("false".to_string());
                }
            }
            None
        }
        ExprKind::Group(inner) => extract_literal_value(inner, ty),
        _ => None, // Complex expressions can't be extracted as literals
    }
}

// Helper function to convert Expr to string
// For unrolling, we want minimal parentheses to avoid parsing issues
fn expr_to_string(expr: &Expr) -> String {
    expr_to_string_inner(expr, false)
}

// Internal function with precedence tracking to minimize parentheses
fn expr_to_string_inner(expr: &Expr, in_binary: bool) -> String {
    match &expr.kind {
        ExprKind::Integer(value) => value.to_string(),
        ExprKind::Ident(ident) => {
            // Check if identifier is a boolean literal keyword
            if ident.name.eq_ignore_ascii_case("true") {
                "true".to_string()
            } else if ident.name.eq_ignore_ascii_case("false") {
                "false".to_string()
            } else {
                ident.name.clone()
            }
        }
        ExprKind::Index {
            target,
            expr: idx_expr,
        } => {
            format!("{}[{}]", target.name, expr_to_string_inner(idx_expr, false))
        }
        ExprKind::Unary { op, expr: inner } => {
            let op_str = match op {
                UnaryOp::Not => "!",
                UnaryOp::Neg => "-",
            };
            format!("{}{}", op_str, expr_to_string_inner(inner, true))
        }
        ExprKind::Binary { left, op, right } => {
            let op_str = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
            };
            // For unrolling, we want minimal parentheses - only wrap if needed
            // Comparison operators have lower precedence, so we don't need parentheses for them
            let needs_parens = in_binary && matches!(op, BinaryOp::Add | BinaryOp::Sub);
            let left_str = expr_to_string_inner(
                left,
                matches!(op, BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod),
            );
            let right_str = expr_to_string_inner(
                right,
                matches!(op, BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod),
            );

            if needs_parens {
                format!("({}{}{})", left_str, op_str, right_str)
            } else {
                format!("{}{}{}", left_str, op_str, right_str)
            }
        }
        ExprKind::Group(inner) => expr_to_string_inner(inner, in_binary),
    }
}
