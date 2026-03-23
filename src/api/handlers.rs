//! HTTP handlers for REST API endpoints.
//!
//! This module provides async handlers that wrap existing CLI functions
//! and convert file-based operations to in-memory content processing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use axum::Json;
use tracing::info;

use crate::api::error::{ApiError, ApiResult};
use crate::api::graph::generate_graphs;
use crate::api::models::*;
use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelId};
use crate::context::{ControllerSynthesisOptions, DiagnosticsOptions as ContextDiagnosticsOptions};
use crate::context_dsl::{parse as parse_context_doc, realize_context};
use crate::guard::sanitize_identifier;
use crate::mu_calculus::EvaluationOptions;

/// Health check endpoint
pub async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "mununu-api"
    }))
}

/// Summarize context (automata, formulas, controllers)
pub async fn context_summarize_handler(
    Json(request): Json<ContextSummarizeRequest>,
) -> ApiResult<Json<ContextSummarizeResponse>> {
    let handler_start = Instant::now();

    // Parse context document
    let t0 = Instant::now();
    let context_doc =
        parse_context_doc(&request.context.content).map_err(|e| ApiError::BadRequest {
            message: format!("Failed to parse context: {}", e),
            details: Some(e.to_string()),
        })?;

    // Parse sidecar documents
    let sidecar_docs: Result<Vec<_>, _> = request
        .sidecars
        .iter()
        .map(|s| parse_context_doc(&s.content))
        .collect();

    let sidecar_docs = sidecar_docs.map_err(|e| ApiError::BadRequest {
        message: format!("Failed to parse sidecar: {}", e),
        details: Some(e.to_string()),
    })?;
    let parse_ms = t0.elapsed().as_millis();

    // Realize context
    let t1 = Instant::now();
    let realized =
        realize_context(&context_doc, &sidecar_docs).map_err(|e| ApiError::Internal {
            message: format!("Failed to realize context: {}", e),
            source: None,
        })?;
    let realize_ms = t1.elapsed().as_millis();
    info!(parse_ms, realize_ms, "summarize: parse+realize complete");

    // Build summary — include both direct automata and compositions
    let mut automata_names: Vec<String> = context_doc
        .automata
        .iter()
        .map(|a| a.name.name.clone())
        .collect();
    for doc in std::iter::once(&context_doc).chain(sidecar_docs.iter()) {
        for comp in &doc.compositions {
            automata_names.push(comp.name.name.clone());
        }
    }

    let automata: Vec<AutomatonSummary> = automata_names
        .iter()
        .filter_map(|name| {
            realized.context.clts(name).map(|clts| AutomatonSummary {
                name: name.clone(),
                states_count: clts.states().count(),
                transitions_count: clts.states().map(|sid| clts.outgoing(sid).len()).sum(),
            })
        })
        .collect();

    // Synthesize declared controllers and collect summaries
    let eval_options = EvaluationOptions::default();
    let mut controllers = Vec::new();
    for rc in realized.controllers.values() {
        let Some(rf) = realized.formulas.get(&rc.formula) else {
            continue;
        };
        if realized.context.clts(&rc.source).is_none() {
            continue;
        }
        let env = realized.environment_for(&rc.source);
        let (realizable, states_count, transitions_count) =
            match realized.context.synthesise_controller_with_options(
                &rc.source,
                &rf.formula,
                &env,
                ControllerSynthesisOptions {
                    evaluation: Some(&eval_options),
                    diagnostics: None,
                    minimize: rc.options.minimize(),
                },
            ) {
                Ok(syn) => (
                    syn.realizable,
                    if syn.realizable {
                        syn.controller.state_count()
                    } else {
                        0
                    },
                    if syn.realizable {
                        syn.controller
                            .states()
                            .map(|sid| syn.controller.outgoing(sid).len())
                            .sum()
                    } else {
                        0
                    },
                ),
                Err(_) => (false, 0, 0),
            };

        controllers.push(ControllerSummary {
            name: rc.name.clone(),
            source: rc.source.clone(),
            formula: rc.formula.clone(),
            realizable,
            states_count,
            transitions_count,
        });
    }

    let controllers_count = controllers.len();
    let summary = ContextSummary {
        context_name: context_doc.name.name.clone(),
        automata,
        formulas_count: realized.formulas.len(),
        controllers_count: realized.controllers.len(),
        controllers,
    };
    let total_ms = handler_start.elapsed().as_millis();
    info!(
        parse_ms,
        realize_ms,
        total_ms,
        controllers = controllers_count,
        "summarize: complete"
    );

    Ok(Json(ContextSummarizeResponse {
        success: true,
        summary,
    }))
}

/// Synthesize controller from ctxdsl specification
pub async fn context_synthesize_handler(
    Json(request): Json<ContextSynthesizeRequest>,
) -> ApiResult<Json<ContextSynthesizeResponse>> {
    // Parse context document
    let context_doc =
        parse_context_doc(&request.context.content).map_err(|e| ApiError::BadRequest {
            message: format!("Failed to parse context: {}", e),
            details: Some(e.to_string()),
        })?;

    // Parse sidecar documents
    let sidecar_docs: Result<Vec<_>, _> = request
        .sidecars
        .iter()
        .map(|s| parse_context_doc(&s.content))
        .collect();

    let sidecar_docs = sidecar_docs.map_err(|e| ApiError::BadRequest {
        message: format!("Failed to parse sidecar: {}", e),
        details: Some(e.to_string()),
    })?;

    // Realize context
    let realized =
        realize_context(&context_doc, &sidecar_docs).map_err(|e| ApiError::Internal {
            message: format!("Failed to realize context: {}", e),
            source: None,
        })?;

    // Get formula
    let realized_formula =
        realized
            .formulas
            .get(&request.formula)
            .ok_or_else(|| ApiError::BadRequest {
                message: format!("Unknown formula '{}'", request.formula),
                details: None,
            })?;

    // Verify automaton exists
    if realized.context.clts(&request.automaton).is_none() {
        return Err(ApiError::BadRequest {
            message: format!("Unknown automaton '{}'", request.automaton),
            details: None,
        });
    }

    let env = realized.environment_for(&request.automaton);

    // Build evaluation options
    let eval_options = EvaluationOptions::default();

    // Build diagnostics options
    let mut diagnostics = ContextDiagnosticsOptions::default();
    diagnostics.counterexample = request.options.diagnostics.counterexample;
    diagnostics.deadlock_traces = request.options.diagnostics.deadlock_traces;
    if let Some(max) = request.options.diagnostics.max_counter_traces {
        diagnostics.max_counter_traces = Some(max as usize);
    }

    let diagnostics_ref = Some(diagnostics);

    // Synthesize controller
    let synthesis = realized
        .context
        .synthesise_controller_with_options(
            &request.automaton,
            &realized_formula.formula,
            &env,
            ControllerSynthesisOptions {
                evaluation: Some(&eval_options),
                diagnostics: diagnostics_ref.as_ref(),
                minimize: request.options.minimize,
            },
        )
        .map_err(|e| ApiError::Internal {
            message: format!("Controller synthesis failed: {}", e),
            source: None,
        })?;

    // Convert controller to DSL if realizable
    let controller_content = if synthesis.realizable {
        let formula_raw = &realized_formula.raw;
        let content = serialize_controller_to_ctxdsl(
            &synthesis.controller,
            &request.automaton,
            &request.formula,
            formula_raw,
        )?;
        Some(FileContent {
            name: format!("{}_controller.ctxdsl", request.automaton),
            content,
        })
    } else {
        None
    };

    // Convert diagnostics
    let diagnostics = convert_diagnostics(&synthesis.diagnostics);

    Ok(Json(ContextSynthesizeResponse {
        success: true,
        realizable: synthesis.realizable,
        controller: controller_content,
        diagnostics,
    }))
}

/// Generate graph data for visualization
pub async fn context_graphs_handler(
    Json(request): Json<ContextGraphsRequest>,
) -> ApiResult<Json<ContextGraphsResponse>> {
    // Parse context document
    let context_doc =
        parse_context_doc(&request.context.content).map_err(|e| ApiError::BadRequest {
            message: format!("Failed to parse context: {}", e),
            details: Some(e.to_string()),
        })?;

    // Parse sidecar documents
    let sidecar_docs: Result<Vec<_>, _> = request
        .sidecars
        .iter()
        .map(|s| parse_context_doc(&s.content))
        .collect();

    let sidecar_docs = sidecar_docs.map_err(|e| ApiError::BadRequest {
        message: format!("Failed to parse sidecar: {}", e),
        details: Some(e.to_string()),
    })?;

    // Realize context
    let realized =
        realize_context(&context_doc, &sidecar_docs).map_err(|e| ApiError::Internal {
            message: format!("Failed to realize context: {}", e),
            source: None,
        })?;

    // Generate graphs
    let (mut graphs, context_summary) = generate_graphs(
        &context_doc,
        &sidecar_docs,
        &realized,
        request.automaton.as_deref(),
        &request.graph_types,
    )
    .map_err(|e| ApiError::Internal {
        message: format!("Failed to generate graphs: {}", e),
        source: None,
    })?;

    // Synthesize declared controllers and add their graphs
    if request.include_controllers {
        let eval_options = EvaluationOptions::default();
        for rc in realized.controllers.values() {
            let Some(rf) = realized.formulas.get(&rc.formula) else {
                continue;
            };
            if realized.context.clts(&rc.source).is_none() {
                continue;
            }
            let env = realized.environment_for(&rc.source);
            let Ok(syn) = realized.context.synthesise_controller_with_options(
                &rc.source,
                &rf.formula,
                &env,
                ControllerSynthesisOptions {
                    evaluation: Some(&eval_options),
                    diagnostics: None,
                    minimize: request
                        .minimize_controllers
                        .unwrap_or(rc.options.minimize()),
                },
            ) else {
                continue;
            };
            if !syn.realizable {
                continue;
            }
            let controller_name = format!("{}_controller", rc.name);
            let source_clts = realized.context.clts(&rc.source).unwrap();
            if let Ok(elements) = crate::api::graph::controller_to_graph_elements(
                &syn.controller,
                source_clts,
                &controller_name,
            ) {
                let metadata =
                    crate::api::graph::calculate_graph_metadata_pub(&elements, &controller_name);
                graphs.push(GraphData {
                    automaton: controller_name,
                    graph_type: GraphTypeResponse::Controller,
                    elements,
                    metadata,
                });
            }
        }
    }

    Ok(Json(ContextGraphsResponse {
        success: true,
        context: context_summary,
        graphs,
    }))
}

/// Verify context by evaluating μ-calculus formulas over automata
pub async fn context_verify_handler(
    Json(request): Json<ContextVerifyRequest>,
) -> ApiResult<Json<ContextVerifyResponse>> {
    let handler_start = Instant::now();
    let counterstrategy_requested = request.counterstrategy;

    // Parse context document
    let t0 = Instant::now();
    let context_doc =
        parse_context_doc(&request.context.content).map_err(|e| ApiError::BadRequest {
            message: format!("Failed to parse context: {}", e),
            details: Some(e.to_string()),
        })?;

    // Parse sidecar documents
    let sidecar_docs: Result<Vec<_>, _> = request
        .sidecars
        .iter()
        .map(|s| parse_context_doc(&s.content))
        .collect();

    let sidecar_docs = sidecar_docs.map_err(|e| ApiError::BadRequest {
        message: format!("Failed to parse sidecar: {}", e),
        details: Some(e.to_string()),
    })?;
    let parse_ms = t0.elapsed().as_millis();

    // Realize context
    let t1 = Instant::now();
    let realized =
        realize_context(&context_doc, &sidecar_docs).map_err(|e| ApiError::Internal {
            message: format!("Failed to realize context: {}", e),
            source: None,
        })?;
    let realize_ms = t1.elapsed().as_millis();
    info!(
        parse_ms,
        realize_ms,
        counterstrategy = counterstrategy_requested,
        "verify: parse+realize complete"
    );

    // Collect formula–automaton pairs to evaluate
    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Some(ref formula_name) = request.formula {
        // Specific formula requested
        if !realized.formulas.contains_key(formula_name) {
            return Err(ApiError::BadRequest {
                message: format!("Unknown formula '{}'", formula_name),
                details: None,
            });
        }
        if let Some(ref automaton_name) = request.automaton {
            pairs.push((formula_name.clone(), automaton_name.clone()));
        } else {
            // Use the formula's target automata
            let rf = &realized.formulas[formula_name];
            let automata = resolve_targets(&rf.targets, &realized);
            for a in automata {
                pairs.push((formula_name.clone(), a));
            }
        }
    } else {
        // Evaluate ALL explicit user-defined formulas (skip auto-generated structural predicates)
        for (name, rf) in &realized.formulas {
            if rf
                .meta
                .comment
                .as_ref()
                .is_some_and(|c| c.contains("\"type\":\"structural\""))
            {
                continue;
            }
            if let Some(ref automaton_name) = request.automaton {
                pairs.push((name.clone(), automaton_name.clone()));
            } else {
                let automata = resolve_targets(&rf.targets, &realized);
                for a in automata {
                    pairs.push((name.clone(), a));
                }
            }
        }
    }

    // Sort for deterministic output
    pairs.sort();

    let eval_options = EvaluationOptions::default();
    let mut results = Vec::new();
    let t2 = Instant::now();

    for (formula_name, automaton_name) in &pairs {
        let rf = &realized.formulas[formula_name];
        let clts = realized
            .context
            .clts(automaton_name)
            .ok_or_else(|| ApiError::BadRequest {
                message: format!("Unknown automaton '{}'", automaton_name),
                details: None,
            })?;
        let env = realized.environment_for(automaton_name);

        let bitvec = realized
            .context
            .evaluate_mu(automaton_name, &rf.formula, &env, Some(&eval_options))
            .map_err(|e| ApiError::Internal {
                message: format!(
                    "μ-calculus evaluation failed for formula '{}' on '{}': {}",
                    formula_name, automaton_name, e
                ),
                source: None,
            })?;

        let mut satisfying_state_names = Vec::new();
        for state_id in clts.states() {
            if bitvec
                .get(state_id.index())
                .map(|bit| *bit)
                .unwrap_or(false)
            {
                if let Some(name) = clts.state_name(state_id) {
                    satisfying_state_names.push(name.to_string());
                }
            }
        }
        satisfying_state_names.sort();

        let initial_states: Vec<String> = clts
            .initial_states()
            .iter()
            .filter_map(|sid| clts.state_name(*sid).map(|n| n.to_string()))
            .collect();

        let mut initial_satisfying = Vec::new();
        let mut initial_violating = Vec::new();
        for sid in clts.initial_states() {
            if let Some(name) = clts.state_name(*sid) {
                if bitvec.get(sid.index()).map(|bit| *bit).unwrap_or(false) {
                    initial_satisfying.push(name.to_string());
                } else {
                    initial_violating.push(name.to_string());
                }
            }
        }
        initial_satisfying.sort();
        initial_violating.sort();

        let satisfied = initial_violating.is_empty() && !initial_states.is_empty();

        // Compute counterstrategy for failed formulas when requested
        let counterstrategy = if request.counterstrategy && !satisfied {
            use crate::mu_calculus::invert;

            let inverted = invert::invert(&rf.formula);
            let inverted_bitvec = realized
                .context
                .evaluate_mu(automaton_name, &inverted, &env, Some(&eval_options))
                .ok();

            inverted_bitvec.map(|inv_bv| {
                // Collect environment winning states
                let winning_set: HashSet<usize> = clts
                    .states()
                    .filter(|sid| inv_bv.get(sid.index()).map(|bit| *bit).unwrap_or(false))
                    .map(|sid| sid.index())
                    .collect();

                let mut env_winning: Vec<String> = clts
                    .states()
                    .filter(|sid| winning_set.contains(&sid.index()))
                    .filter_map(|sid| clts.state_name(sid).map(|n| n.to_string()))
                    .collect();
                env_winning.sort();

                // Build graph elements directly from original CLTS, filtered
                let cs_name = format!("{}_counterstrategy", automaton_name);
                let graph_elements = crate::api::graph::counterstrategy_to_graph_elements(
                    clts,
                    &cs_name,
                    &winning_set,
                );

                CounterstrategyResult {
                    environment_winning_states: env_winning,
                    graph_elements,
                    inverted_formula: format!("{:?}", inverted),
                    minimized: false,
                }
            })
        } else {
            None
        };

        results.push(FormulaVerificationResult {
            formula_name: formula_name.clone(),
            automaton: automaton_name.clone(),
            satisfied,
            total_states: clts.state_count(),
            satisfying_states: satisfying_state_names.len(),
            initial_states,
            initial_satisfying,
            initial_violating,
            satisfying_state_names,
            counterstrategy,
        });
    }

    let eval_ms = t2.elapsed().as_millis();
    let total_ms = handler_start.elapsed().as_millis();
    info!(
        parse_ms,
        realize_ms,
        eval_ms,
        total_ms,
        formulas = pairs.len(),
        counterstrategy = counterstrategy_requested,
        "verify: complete"
    );

    let all_satisfied = results.iter().all(|r| r.satisfied);

    Ok(Json(ContextVerifyResponse {
        success: true,
        all_satisfied,
        results,
    }))
}

/// Resolve formula targets to a list of automaton names
fn resolve_targets(
    targets: &crate::context_dsl::FormulaTargetsKind,
    realized: &crate::context_dsl::RealizedContext,
) -> Vec<String> {
    use crate::context_dsl::FormulaTargetsKind;
    match targets {
        FormulaTargetsKind::All => realized.context.clts_names(),
        FormulaTargetsKind::Named(names) => names.clone(),
    }
}

/// Convert controller diagnostics to API format
fn convert_diagnostics(
    diagnostics: &crate::context::ControllerDiagnostics,
) -> SynthesisDiagnostics {
    SynthesisDiagnostics {
        messages: diagnostics.messages.clone(),
        violating_initials: diagnostics.violating_initials.clone(),
        counterexample_trace: diagnostics.counterexample_trace.clone(),
        counterstrategy_traces: diagnostics.counterstrategy_traces.clone(),
        deadlock_traces: diagnostics.deadlock_traces.clone(),
        minimization: diagnostics
            .minimization
            .as_ref()
            .map(|m| MinimizationReport {
                removed_states: m.removed_states,
                removed_transitions: m.removed_transitions,
                merged_states: m.merged_states.clone(),
            }),
        proof_obligations: diagnostics
            .proof_obligations
            .iter()
            .map(|po| ProofObligation {
                state: po.state.clone(),
                detail: po.detail.clone(),
            })
            .collect(),
    }
}

/// Serialize controller CLTS to ctxdsl format
fn serialize_controller_to_ctxdsl(
    controller: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    automaton: &str,
    formula: &str,
    raw_formula: &str,
) -> ApiResult<String> {
    use std::fmt::Write;

    let mut output = String::new();

    // Collect all labels
    let mut ordered_labels: BTreeMap<usize, LabelId<DefaultLabelIdx>> = BTreeMap::new();
    for state in controller.states() {
        for transition in controller.outgoing(state) {
            for label_id in transition.labels() {
                ordered_labels.entry(label_id.index()).or_insert(*label_id);
            }
        }
    }

    // Generate label names
    let mut label_names = HashMap::new();
    let mut label_entries = Vec::new();
    let mut seen_label_idents = HashSet::new();
    for (index, label_id) in ordered_labels {
        let mut payload = controller
            .label_payload(label_id)
            .map(|values| values.to_vec())
            .unwrap_or_default();
        payload.sort();
        let base = if payload.is_empty() {
            format!("label_{index}")
        } else {
            payload.join("_")
        };
        let mut ident = sanitize_identifier(&base);
        if ident.is_empty() {
            ident = format!("label_{index}");
        }
        let original_ident = ident.clone();
        if !seen_label_idents.insert(ident.clone()) {
            let mut counter = 1usize;
            loop {
                let candidate = format!("{original_ident}_{counter}");
                if seen_label_idents.insert(candidate.clone()) {
                    ident = candidate;
                    break;
                }
                counter += 1;
            }
        }
        label_names.insert(label_id, ident.clone());
        label_entries.push((label_id, ident, payload));
    }

    // Generate state names
    let mut state_idents = Vec::with_capacity(controller.state_count());
    let mut raw_state_names = Vec::with_capacity(controller.state_count());
    let mut seen_state_idents = HashSet::new();
    for state in controller.states() {
        let idx = state.index();
        if state_idents.len() <= idx {
            state_idents.resize(idx + 1, String::new());
            raw_state_names.resize(idx + 1, String::new());
        }
        let raw = controller
            .state_name(state)
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("state_{idx}"));
        let mut ident = sanitize_identifier(&raw);
        if ident.is_empty() {
            ident = format!("state_{idx}");
        }
        let original_ident = ident.clone();
        if !seen_state_idents.insert(ident.clone()) {
            let mut counter = 1usize;
            loop {
                let candidate = format!("{original_ident}_{counter}");
                if seen_state_idents.insert(candidate.clone()) {
                    ident = candidate;
                    break;
                }
                counter += 1;
            }
        }
        state_idents[idx] = ident;
        raw_state_names[idx] = raw;
    }

    // Generate context and automaton identifiers
    let context_ident = sanitize_identifier(&format!("{}_{}_controller", automaton, formula));
    let automaton_ident = format!("{context_ident}_automaton");

    // Write header comment
    writeln!(
        output,
        "// Synthesised controller derived from automaton '{}' and formula '{}'",
        automaton, formula
    )
    .map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;

    // Write context declaration
    writeln!(output, "context {context_ident} {{").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;

    // Write alphabet
    if !label_entries.is_empty() {
        writeln!(output, "    alphabet {{").map_err(|e| ApiError::Internal {
            message: format!("Failed to write controller DSL: {}", e),
            source: None,
        })?;
        for (_, ident, payload) in &label_entries {
            if payload.is_empty() {
                writeln!(output, "        label {ident};").map_err(|e| ApiError::Internal {
                    message: format!("Failed to write controller DSL: {}", e),
                    source: None,
                })?;
            } else {
                writeln!(
                    output,
                    "        label {ident}; // original symbols: {}",
                    payload.join(", ")
                )
                .map_err(|e| ApiError::Internal {
                    message: format!("Failed to write controller DSL: {}", e),
                    source: None,
                })?;
            }
        }
        writeln!(output, "    }}").map_err(|e| ApiError::Internal {
            message: format!("Failed to write controller DSL: {}", e),
            source: None,
        })?;
    }

    // Write automata section
    writeln!(output, "    automata {{").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "        automaton {automaton_ident} {{").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;

    // Write states
    writeln!(output, "            states {{").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    for state in controller.states() {
        let idx = state.index();
        let name = &state_idents[idx];
        let raw = &raw_state_names[idx];
        let mut line = format!("                state {name}");
        if controller.initial_states().contains(&state) {
            line.push_str(" initial");
        }
        line.push(';');
        if raw != name {
            line.push_str(" // original: ");
            line.push_str(raw);
        }
        writeln!(output, "{line}").map_err(|e| ApiError::Internal {
            message: format!("Failed to write controller DSL: {}", e),
            source: None,
        })?;
    }
    writeln!(output, "            }}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;

    // Write transitions
    writeln!(output, "            transitions {{").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    for state in controller.states() {
        let source_name = &state_idents[state.index()];
        for transition in controller.outgoing(state) {
            let target_name = &state_idents[transition.target().index()];
            let mut clauses = Vec::new();
            for label_id in transition.labels() {
                if let Some(name) = label_names.get(label_id) {
                    clauses.push(format!("label {name}"));
                }
            }
            let labels_clause = if clauses.is_empty() {
                "epsilon".to_owned()
            } else {
                clauses.join(", ")
            };
            let mut line = format!(
                "                transition {source} -> {target} on {labels};",
                source = source_name,
                target = target_name,
                labels = labels_clause
            );
            if transition.is_controllable(controller) {
                line.push_str(" // controllable");
            } else {
                line.push_str(" // uncontrollable");
            }
            writeln!(output, "{line}").map_err(|e| ApiError::Internal {
                message: format!("Failed to write controller DSL: {}", e),
                source: None,
            })?;
        }
    }
    writeln!(output, "            }}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "        }}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "    }}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;

    // Write mu_formulas section
    writeln!(output, "    mu_formulas {{").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(
        output,
        "        formula {} {{",
        sanitize_identifier(formula)
    )
    .map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "            over {automaton_ident};").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "            body = {raw_formula};").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "        }}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "    }}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;
    writeln!(output, "}}").map_err(|e| ApiError::Internal {
        message: format!("Failed to write controller DSL: {}", e),
        source: None,
    })?;

    Ok(output)
}
