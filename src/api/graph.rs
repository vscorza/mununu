//! Graph generation for context visualization.
//!
//! This module extracts graph generation logic from the CLI for use in the REST API.
//! It converts automata into graph elements that can be serialized to JSON for visualization.

use std::collections::HashSet;

use crate::abstraction::unrolling::{
    Effect, OriginalState, OriginalTransition, UnrollingOptions, VariableDecl, unroll_states,
};
use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use crate::context_dsl::ast::{
    BinaryOp, Expr, ExprKind, StateRef, StateSelector, TransitionLabel, TypeName, UnaryOp,
};
use crate::context_dsl::{ContextDoc, RealizedContext};

use crate::api::models::{
    AutomatonSummary, ContextSummary, GraphData, GraphElement, GraphElementData, GraphMetadata,
    GraphPosition, GraphType, GraphTypeResponse,
};

/// Generate graphs from context based on request parameters.
///
/// This function generates one or more graphs (one per automaton, potentially multiple types per automaton)
/// and returns them with context summary information.
pub fn generate_graphs(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automaton: Option<&str>,
    graph_types: &[GraphType],
) -> Result<(Vec<GraphData>, ContextSummary), String> {
    // Determine which automata to visualize (including compositions)
    let all_docs = std::iter::once(context_doc).chain(sidecar_docs.iter());

    // Collect direct automata names
    let direct_automata: HashSet<String> = all_docs
        .clone()
        .flat_map(|doc| doc.automata.iter().map(|a| a.name.name.clone()))
        .collect();

    // Collect composition names
    let composition_names: HashSet<String> = all_docs
        .flat_map(|doc| doc.compositions.iter().map(|c| c.name.name.clone()))
        .collect();

    let automata_to_visualize: Vec<String> = if let Some(automaton_name) = automaton {
        vec![automaton_name.to_string()]
    } else {
        let mut names: Vec<String> = context_doc
            .automata
            .iter()
            .map(|a| a.name.name.clone())
            .collect();
        for doc in std::iter::once(context_doc).chain(sidecar_docs.iter()) {
            for comp in &doc.compositions {
                names.push(comp.name.name.clone());
            }
        }
        names
    };

    if automata_to_visualize.is_empty() {
        return Err("No automata found in context".to_string());
    }

    let mut graphs = Vec::new();

    for automaton_name in &automata_to_visualize {
        let is_composition =
            composition_names.contains(automaton_name) && !direct_automata.contains(automaton_name);

        for &graph_type in graph_types {
            let (elements, metadata) = if is_composition {
                // Compositions: generate graph from realized CLTS directly
                let elements = realized_clts_to_graph_elements(realized, automaton_name)?;
                let metadata = calculate_graph_metadata(&elements, automaton_name);
                (elements, metadata)
            } else {
                match graph_type {
                    GraphType::Dsl => {
                        let elements = dsl_automata_to_graph_elements(
                            context_doc,
                            sidecar_docs,
                            realized,
                            std::slice::from_ref(automaton_name),
                        )?;
                        let metadata = calculate_graph_metadata(&elements, automaton_name);
                        (elements, metadata)
                    }
                    GraphType::Unrolled => {
                        match unrolled_automata_to_graph_elements(
                            context_doc,
                            sidecar_docs,
                            realized,
                            std::slice::from_ref(automaton_name),
                        ) {
                            Ok(elements) => {
                                let metadata = calculate_graph_metadata(&elements, automaton_name);
                                (elements, metadata)
                            }
                            Err(_) => continue,
                        }
                    }
                }
            };

            graphs.push(GraphData {
                automaton: automaton_name.clone(),
                graph_type: GraphTypeResponse::from(graph_type),
                elements,
                metadata,
            });

            // Compositions only get one graph (from realized CLTS), not DSL + unrolled
            if is_composition {
                break;
            }
        }
    }

    Ok((graphs, calculate_context_summary(context_doc, realized)))
}

/// Calculate context summary from realized context
fn calculate_context_summary(
    context_doc: &ContextDoc,
    realized: &RealizedContext,
) -> ContextSummary {
    let automata: Vec<AutomatonSummary> = context_doc
        .automata
        .iter()
        .filter_map(|a| {
            realized
                .context
                .clts(&a.name.name)
                .map(|clts| AutomatonSummary {
                    name: a.name.name.clone(),
                    states_count: clts.states().count(),
                    transitions_count: clts.states().map(|sid| clts.outgoing(sid).len()).sum(),
                })
        })
        .collect();

    ContextSummary {
        context_name: context_doc.name.name.clone(),
        automata,
        formulas_count: realized.formulas.len(),
        controllers_count: realized.controllers.len(),
        controllers: Vec::new(),
    }
}

/// Public wrapper for calculating graph metadata.
pub fn calculate_graph_metadata_pub(
    elements: &[GraphElement],
    automaton_name: &str,
) -> GraphMetadata {
    calculate_graph_metadata(elements, automaton_name)
}

/// Calculate metadata for a specific graph
fn calculate_graph_metadata(elements: &[GraphElement], automaton_name: &str) -> GraphMetadata {
    let mut states_count = 0;
    let mut transitions_count = 0;
    let mut initial_states = Vec::new();

    for element in elements {
        match &element.data {
            GraphElementData::Node { id, .. } => {
                // Count state nodes (exclude compound automaton nodes and entry nodes)
                if !id.starts_with(&format!("{}_entry", automaton_name))
                    && !id.eq(automaton_name)
                    && !id.starts_with(&format!("{}_unrolled_entry", automaton_name))
                    && !id.eq(&format!("{}_unrolled", automaton_name))
                {
                    states_count += 1;
                    // Check if this is an initial state
                    if let Some(classes) = &element.classes
                        && classes.contains("start")
                    {
                        initial_states.push(id.clone());
                    }
                }
            }
            GraphElementData::Edge { .. } => {
                transitions_count += 1;
            }
        }
    }

    GraphMetadata {
        states_count,
        transitions_count,
        initial_states,
    }
}

/// Generate graph elements from a realized CLTS (used for compositions)
fn realized_clts_to_graph_elements(
    realized: &RealizedContext,
    automaton_name: &str,
) -> Result<Vec<GraphElement>, String> {
    let clts = realized.context.clts(automaton_name).ok_or_else(|| {
        format!(
            "composed automaton '{}' not found in realized context",
            automaton_name
        )
    })?;
    clts_to_graph_elements(clts, automaton_name)
}

/// Convert a CLTS to graph elements for visualization.
pub fn clts_to_graph_elements(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    automaton_name: &str,
) -> Result<Vec<GraphElement>, String> {
    clts_to_graph_elements_with_labels(clts, automaton_name, None)
}

/// Convert a synthesized controller CLTS to graph elements, resolving label names
/// from the source CLTS (since the controller's label IDs originate from it).
pub fn controller_to_graph_elements(
    controller: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    source_clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    automaton_name: &str,
) -> Result<Vec<GraphElement>, String> {
    clts_to_graph_elements_with_labels(controller, automaton_name, Some(source_clts))
}

fn clts_to_graph_elements_with_labels(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    automaton_name: &str,
    label_source: Option<&Clts<DefaultStateIdx, DefaultLabelIdx>>,
) -> Result<Vec<GraphElement>, String> {
    let label_clts = label_source.unwrap_or(clts);
    let mut elements = Vec::new();
    let state_spacing = 250.0;
    let mut x_pos = 100.0;
    let y_offset = 0.0;

    // Create compound node
    let automaton_id = automaton_name.to_string();
    elements.push(GraphElement {
        data: GraphElementData::Node {
            id: automaton_id.clone(),
            label: String::new(),
            parent: None,
            vars: Vec::new(),
            actions: Vec::new(),
        },
        position: None,
        classes: None,
    });

    // Add state nodes
    for state_id in clts.states() {
        let state_name = clts.state_name(state_id).unwrap_or("?").to_string();
        let node_id = format!("{}_{}", automaton_name, state_name);
        let is_initial = clts.initial_states().contains(&state_id);

        let mut classes = Vec::new();
        if is_initial {
            classes.push("start");
        }
        // Check if dead (no outgoing transitions)
        if clts.outgoing(state_id).is_empty() {
            classes.push("dead");
        }

        elements.push(GraphElement {
            data: GraphElementData::Node {
                id: node_id.clone(),
                label: state_name.clone(),
                parent: Some(automaton_id.clone()),
                vars: Vec::new(),
                actions: Vec::new(),
            },
            position: Some(GraphPosition {
                x: x_pos,
                y: y_offset + 100.0,
            }),
            classes: Some(classes.join(" ")),
        });

        // Add entry arrow for initial states
        if is_initial {
            let entry_id = format!("{}_entry_{}", automaton_name, state_name);
            elements.push(GraphElement {
                data: GraphElementData::Node {
                    id: entry_id.clone(),
                    label: String::new(),
                    parent: Some(automaton_id.clone()),
                    vars: Vec::new(),
                    actions: Vec::new(),
                },
                position: Some(GraphPosition {
                    x: 40.0,
                    y: y_offset + 100.0,
                }),
                classes: Some("entry".to_string()),
            });

            elements.push(GraphElement {
                data: GraphElementData::Edge {
                    id: format!("{}_entry_edge_{}", automaton_name, state_name),
                    source: entry_id,
                    target: node_id,
                    label: None,
                    action: None,
                    action_type: Some("start-arrow".to_string()),
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
        }

        x_pos += state_spacing;
    }

    // Add transition edges
    let mut edge_idx = 0;
    for state_id in clts.states() {
        let source_name = clts.state_name(state_id).unwrap_or("?").to_string();
        let source_id = format!("{}_{}", automaton_name, source_name);

        for transition in clts.outgoing(state_id) {
            let target_name = clts
                .state_name(transition.target())
                .unwrap_or("?")
                .to_string();
            let target_id = format!("{}_{}", automaton_name, target_name);

            // Build label from label payloads
            let label_parts: Vec<String> = transition
                .labels()
                .iter()
                .map(|lid| {
                    let payload = label_clts.label_payload(*lid);
                    match payload {
                        Some(values)
                            if !values.is_empty() && !values.iter().all(|v| v.is_empty()) =>
                        {
                            values
                                .iter()
                                .filter(|v| !v.is_empty())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                        _ => format!("label_{}", lid.index()),
                    }
                })
                .collect();
            let label_text = if label_parts.is_empty() {
                None
            } else {
                Some(label_parts.join(" | "))
            };

            let is_controllable = transition.is_controllable(clts);
            let action_type = if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };

            elements.push(GraphElement {
                data: GraphElementData::Edge {
                    id: format!("{}_t{}", automaton_name, edge_idx),
                    source: source_id.clone(),
                    target: target_id,
                    label: label_text.clone(),
                    action: label_text,
                    action_type: Some(action_type.to_string()),
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
            edge_idx += 1;
        }
    }

    Ok(elements)
}

/// Convert DSL automata to graph elements
fn dsl_automata_to_graph_elements(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automata_names: &[String],
) -> Result<Vec<GraphElement>, String> {
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

        // Collect action names with controllability
        let mut action_info: Vec<String> = Vec::new();
        for label_ref in &automaton.alphabet {
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
            action_info.push(format!("{} ({})", label_name, action_type));
        }

        // Create automaton compound node
        let automaton_id = automaton_name.clone();
        elements.push(GraphElement {
            data: GraphElementData::Node {
                id: automaton_id.clone(),
                label: String::new(),
                parent: None,
                vars: var_names.clone(),
                actions: action_info.clone(),
            },
            position: None,
            classes: None,
        });

        // Collect states and their positions
        let mut x_pos = 100.0;

        for state_decl in &automaton.states {
            let state_name = &state_decl.name.name;
            let state_id = format!("{}_{}", automaton_name, state_name);
            let is_initial = state_decl.is_initial;

            // Check if state is terminal (dead)
            let is_dead = clts
                .state_id(state_name)
                .map(|sid| clts.outgoing(sid).is_empty())
                .unwrap_or(false);

            // Get variable values for this state
            let state_vars = if var_names.is_empty() {
                Vec::new()
            } else {
                clts.state_id(state_name)
                    .ok()
                    .map(|sid| clts.state_variables(sid))
                    .unwrap_or_default()
            };

            let position = GraphPosition {
                x: x_pos,
                y: y_offset + 100.0,
            };

            let mut classes = vec!["state"];
            if is_initial {
                classes.push("start");
            }
            if is_dead {
                classes.push("dead");
            }

            elements.push(GraphElement {
                data: GraphElementData::Node {
                    id: state_id.clone(),
                    label: state_name.to_string(),
                    parent: Some(automaton_id.clone()),
                    vars: state_vars,
                    actions: Vec::new(),
                },
                position: Some(position),
                classes: Some(classes.join(" ")),
            });

            // Add entry arrow for initial states
            if is_initial {
                let entry_id = format!("{}_entry", automaton_name);
                elements.push(GraphElement {
                    data: GraphElementData::Node {
                        id: entry_id.clone(),
                        label: String::new(),
                        parent: Some(automaton_id.clone()),
                        vars: Vec::new(),
                        actions: Vec::new(),
                    },
                    position: Some(GraphPosition {
                        x: 40.0,
                        y: y_offset + 100.0,
                    }),
                    classes: Some("entry".to_string()),
                });

                elements.push(GraphElement {
                    data: GraphElementData::Edge {
                        id: format!("{}_entry_edge", automaton_name),
                        source: entry_id,
                        target: state_id.clone(),
                        label: None,
                        action: None,
                        action_type: Some("start-arrow".to_string()),
                        guard: None,
                        effect: None,
                    },
                    position: None,
                    classes: None,
                });
            }

            x_pos += state_spacing;
        }

        // Add transitions
        for transition in &automaton.transitions {
            let source_name = match &transition.source {
                StateSelector::Named(state_ref) => match state_ref {
                    StateRef::Simple(ident) => ident.name.clone(),
                    StateRef::Indexed { name, .. } => name.name.clone(),
                },
                _ => continue,
            };

            let target_name = match &transition.target {
                StateSelector::Named(state_ref) => match state_ref {
                    StateRef::Simple(ident) => ident.name.clone(),
                    StateRef::Indexed { name, .. } => name.name.clone(),
                },
                _ => continue,
            };

            let primary_label = match &transition.label {
                TransitionLabel::Named { name, .. } => name.name.clone(),
                TransitionLabel::Epsilon(_) => "\u{03B5}".to_string(),
            };
            let mut all_label_names = vec![primary_label.clone()];
            for additional in &transition.additional_labels {
                if let TransitionLabel::Named { name, .. } = additional {
                    all_label_names.push(name.name.clone());
                }
            }
            let label_name = all_label_names.join(", ");

            // Determine action type
            let is_controllable = all_label_names
                .iter()
                .any(|l| automaton.controllable.iter().any(|c| c.name.name == *l));
            let is_internal = all_label_names
                .iter()
                .any(|l| automaton.internal.iter().any(|i| i.name.name == *l));
            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };

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

            elements.push(GraphElement {
                data: GraphElementData::Edge {
                    id: transition_id,
                    source: source_id,
                    target: target_id,
                    label: Some(transition_label),
                    action: Some(label_name),
                    action_type: Some(action_type.to_string()),
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

/// Convert unrolled automata to graph elements
fn unrolled_automata_to_graph_elements(
    context_doc: &ContextDoc,
    sidecar_docs: &[ContextDoc],
    realized: &RealizedContext,
    automata_names: &[String],
) -> Result<Vec<GraphElement>, String> {
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
                    _ => return None,
                };

                let target_name = match &t.target {
                    StateSelector::Named(state_ref) => match state_ref {
                        StateRef::Simple(ident) => ident.name.clone(),
                        StateRef::Indexed { name, .. } => name.name.clone(),
                    },
                    _ => return None,
                };

                let primary_label = match &t.label {
                    TransitionLabel::Named { name, .. } => name.name.clone(),
                    TransitionLabel::Epsilon(_) => "\u{03B5}".to_string(),
                };
                let mut all_label_names = vec![primary_label];
                for additional in &t.additional_labels {
                    if let TransitionLabel::Named { name, .. } = additional {
                        all_label_names.push(name.name.clone());
                    }
                }
                let label_name = all_label_names.join(", ");

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
                let initial_str = extract_literal_value(&v.init, &v.ty)
                    .unwrap_or_else(|| strip_outer_parens(&expr_to_string(&v.init)));

                VariableDecl {
                    name: v.name.name.clone(),
                    ty: match v.ty {
                        TypeName::Bool => "bool".to_string(),
                        TypeName::I64 => "i64".to_string(),
                    },
                    initial: Some(initial_str),
                }
            })
            .collect();

        // Perform unrolling with default options
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

        // Collect action names
        let mut action_info: Vec<String> = Vec::new();
        for label_ref in &automaton.alphabet {
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
            action_info.push(format!("{} ({})", label_name, action_type));
        }

        // Create automaton compound node (with "Unrolled" suffix)
        let automaton_id = format!("{}_unrolled", automaton_name);
        elements.push(GraphElement {
            data: GraphElementData::Node {
                id: automaton_id.clone(),
                label: String::new(),
                parent: None,
                vars: var_names.clone(),
                actions: action_info.clone(),
            },
            position: None,
            classes: None,
        });

        // Collect states and their positions
        let mut x_pos = 100.0;
        let mut initial_states = HashSet::new();

        // Find initial states
        let initial_location_names: HashSet<String> = automaton
            .states
            .iter()
            .filter(|s| s.is_initial)
            .map(|s| s.name.name.clone())
            .collect();

        let states_with_incoming: HashSet<String> = unrolled
            .transitions
            .iter()
            .map(|t| t.to.state_name())
            .collect();

        for state in &unrolled.states {
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

            let is_dead = unrolled
                .transitions
                .iter()
                .all(|t| t.from.state_name() != state_name);

            let state_vars = if state.variables.is_empty() {
                Vec::new()
            } else {
                state
                    .variables
                    .iter()
                    .map(|(name, value)| format!("{} = {}", name, value))
                    .collect()
            };

            let position = GraphPosition {
                x: x_pos,
                y: y_offset + 100.0,
            };

            let mut classes = vec!["state"];
            if is_initial {
                classes.push("start");
            }
            if is_dead {
                classes.push("dead");
            }

            elements.push(GraphElement {
                data: GraphElementData::Node {
                    id: state_id.clone(),
                    label: state_name.clone(),
                    parent: Some(automaton_id.clone()),
                    vars: state_vars,
                    actions: Vec::new(),
                },
                position: Some(position),
                classes: Some(classes.join(" ")),
            });

            // Add entry arrow for initial states
            if is_initial {
                let entry_id = format!("{}_unrolled_entry_{}", automaton_name, state_name);
                elements.push(GraphElement {
                    data: GraphElementData::Node {
                        id: entry_id.clone(),
                        label: String::new(),
                        parent: Some(automaton_id.clone()),
                        vars: Vec::new(),
                        actions: Vec::new(),
                    },
                    position: Some(GraphPosition {
                        x: 40.0,
                        y: y_offset + 100.0,
                    }),
                    classes: Some("entry".to_string()),
                });

                elements.push(GraphElement {
                    data: GraphElementData::Edge {
                        id: format!("{}_unrolled_entry_edge_{}", automaton_name, state_name),
                        source: entry_id,
                        target: state_id.clone(),
                        label: None,
                        action: None,
                        action_type: Some("start-arrow".to_string()),
                        guard: None,
                        effect: None,
                    },
                    position: None,
                    classes: None,
                });
            }

            x_pos += state_spacing;
        }

        // Add transitions
        for (idx, transition) in unrolled.transitions.iter().enumerate() {
            let from_name = transition.from.state_name();
            let to_name = transition.to.state_name();
            let label_name = transition.label.clone();

            let is_controllable = automaton
                .controllable
                .iter()
                .any(|c| c.name.name == label_name);
            let is_internal = automaton.internal.iter().any(|i| i.name.name == label_name);
            let action_type = if is_internal {
                "internal"
            } else if is_controllable {
                "controllable"
            } else {
                "uncontrollable"
            };

            let transition_id = format!("{}_unrolled_t{}", automaton_name, idx);
            let source_id = format!("{}_unrolled_{}", automaton_name, from_name);
            let target_id = format!("{}_unrolled_{}", automaton_name, to_name);

            elements.push(GraphElement {
                data: GraphElementData::Edge {
                    id: transition_id,
                    source: source_id,
                    target: target_id,
                    label: Some(label_name.clone()),
                    action: Some(label_name),
                    action_type: Some(action_type.to_string()),
                    guard: None,
                    effect: None,
                },
                position: None,
                classes: None,
            });
        }

        y_offset += 200.0;
    }

    Ok(elements)
}

/// Helper function to convert Expr to string
fn expr_to_string(expr: &Expr) -> String {
    expr_to_string_inner(expr, false)
}

/// Internal function with precedence tracking to minimize parentheses
fn expr_to_string_inner(expr: &Expr, in_binary: bool) -> String {
    match &expr.kind {
        ExprKind::Integer(value) => value.to_string(),
        ExprKind::Ident(ident) => {
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

/// Strips outer parentheses from a string if they wrap the entire expression
fn strip_outer_parens(s: &str) -> String {
    let mut trimmed = s.trim();
    loop {
        if trimmed.starts_with('(') && trimmed.ends_with(')') {
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
                            is_fully_wrapped = true;
                            break;
                        }
                        if depth < 0 {
                            return trimmed.to_string();
                        }
                    }
                    _ => {}
                }
            }

            if is_fully_wrapped {
                trimmed = trimmed[1..trimmed.len() - 1].trim();
            } else {
                break;
            }
        } else {
            break;
        }
    }
    trimmed.to_string()
}

/// Extracts a literal value from an expression if it's a simple constant.
fn extract_literal_value(expr: &Expr, ty: &TypeName) -> Option<String> {
    match &expr.kind {
        ExprKind::Integer(value) => Some(value.to_string()),
        ExprKind::Ident(ident) => {
            if matches!(ty, TypeName::Bool) {
                if ident.name.eq_ignore_ascii_case("true") {
                    return Some("true".to_string());
                } else if ident.name.eq_ignore_ascii_case("false") {
                    return Some("false".to_string());
                }
            }
            None
        }
        ExprKind::Group(inner) => extract_literal_value(inner, ty),
        _ => None,
    }
}
