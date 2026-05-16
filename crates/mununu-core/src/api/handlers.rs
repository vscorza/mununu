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
use crate::context_dsl::RealizedContext;
use crate::guard::sanitize_identifier;
use crate::mu_calculus::{Environment, EvaluationOptions, Formula};

/// Health check endpoint
pub async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "mununu-api"
    }))
}

/// List available property templates.
pub async fn templates_handler(
    query: axum::extract::Query<TemplatesQuery>,
) -> Json<serde_json::Value> {
    let registry = crate::adapter::templates::TemplateRegistry::builtin();

    if let Some(domain_str) = &query.domain {
        use crate::adapter::templates::TemplateDomain;
        let domain = match domain_str.as_str() {
            "rtl" => Some(TemplateDomain::Rtl),
            "agentic" => Some(TemplateDomain::Agentic),
            "software" => Some(TemplateDomain::Software),
            "synthesis" => Some(TemplateDomain::Synthesis),
            "universal" => Some(TemplateDomain::Universal),
            _ => None,
        };
        if let Some(d) = domain {
            let filtered: Vec<_> = registry.for_domain(d);
            return Json(serde_json::to_value(filtered).unwrap_or_default());
        }
    }

    let catalog = registry.catalog();
    Json(serde_json::to_value(catalog).unwrap_or_default())
}

/// Query parameters for the templates endpoint.
#[derive(Debug, serde::Deserialize)]
pub struct TemplatesQuery {
    pub domain: Option<String>,
}

/// Summarize context (automata, formulas, controllers)
pub async fn context_summarize_handler(
    Json(request): Json<ContextSummarizeRequest>,
) -> ApiResult<Json<ContextSummarizeResponse>> {
    let handler_start = Instant::now();

    // Cache parse + realize. Subsequent calls with identical content reuse the
    // realized result, sidestepping CLTS construction / abstraction unrolling.
    let t0 = Instant::now();
    let sidecar_strs: Vec<&str> = request
        .sidecars
        .iter()
        .map(|s| s.content.as_str())
        .collect();
    let (entry, cache_hit) =
        crate::api::cache::get_or_realize(&request.context.content, &sidecar_strs).map_err(
            |e| ApiError::BadRequest {
                message: format!("Failed to parse/realize context: {}", e),
                details: Some(e.clone()),
            },
        )?;
    let realize_ms = t0.elapsed().as_millis();
    let context_doc = entry.context_doc.as_ref();
    let sidecar_docs = entry.sidecar_docs.as_ref();
    let realized = entry.realized.as_ref();
    info!(realize_ms, cache_hit, "summarize: parse+realize complete");

    // Build summary — include both direct automata and compositions
    let mut automata_names: Vec<String> = context_doc
        .automata
        .iter()
        .map(|a| a.name.name.clone())
        .collect();
    for doc in std::iter::once(context_doc).chain(sidecar_docs.iter()) {
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

    // Report declared controllers (declarations only — no synthesis execution).
    // Per CLAUDE.md governance: "Never run controller synthesis in summary/informational
    // endpoints." Synthesis is expensive (state-space exploration) and belongs only in
    // the synthesis endpoint.
    let mut controllers = Vec::new();
    for rc in realized.controllers.values() {
        controllers.push(ControllerSummary {
            name: rc.name.clone(),
            source: rc.source.clone(),
            formula: rc.formula.clone(),
            realizable: false,
            states_count: 0,
            transitions_count: 0,
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
        realize_ms,
        total_ms,
        cache_hit,
        controllers = controllers_count,
        "summarize: complete"
    );

    Ok(Json(ContextSummarizeResponse {
        success: true,
        summary,
    }))
}

/// Resolve the requested controller mode from the API options.
///
/// Precedence:
/// 1. `controller_mode` (if `Some`) is parsed case-insensitively.
/// 2. Else, `extract_strategy = true` → `Functional` (legacy mapping).
/// 3. Else, `Projection` (default).
///
/// Accepted names: `projection`, `functional`, `permissive`,
/// `signature-memory`, `product-game`, `parity-game`. The dashes may also
/// be underscores or removed (e.g., `parity_game` and `paritygame` work).
fn resolve_controller_mode(
    mode: &Option<String>,
    extract_strategy: bool,
) -> Result<crate::context::ControllerMode, ApiError> {
    use crate::context::ControllerMode;
    if let Some(name) = mode {
        let normalized: String = name
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
            .flat_map(|c| c.to_lowercase())
            .collect();
        return match normalized.as_str() {
            "projection" => Ok(ControllerMode::Projection),
            "functional" => Ok(ControllerMode::Functional),
            "permissive" => Ok(ControllerMode::Permissive),
            "signaturememory" => Ok(ControllerMode::SignatureMemory),
            "productgame" => Ok(ControllerMode::ProductGame),
            "paritygame" => Ok(ControllerMode::ParityGame),
            other => Err(ApiError::BadRequest {
                message: format!("Unknown controller_mode '{other}'"),
                details: Some(
                    "Valid: projection, functional, permissive, signature-memory, product-game, parity-game".into(),
                ),
            }),
        };
    }
    Ok(if extract_strategy {
        ControllerMode::Functional
    } else {
        ControllerMode::Projection
    })
}

/// Synthesize controller from ctxdsl specification
pub async fn context_synthesize_handler(
    Json(request): Json<ContextSynthesizeRequest>,
) -> ApiResult<Json<ContextSynthesizeResponse>> {
    let handler_start = Instant::now();

    // Resolve formula name + synthesized template sidecar BEFORE cache lookup,
    // so the cache key covers the template-ref instantiation. Repeated requests
    // with identical context + identical template params hit the cache.
    let (formula_name, synth_sidecar) = resolve_template_ref_for_cache(
        &request.formula,
        &request.template_ref,
        &request.automaton,
    )?;
    let formula_name = formula_name.ok_or_else(|| ApiError::BadRequest {
        message: "either 'formula' or 'template_ref' must be provided".to_string(),
        details: None,
    })?;

    // Build sidecar string list: original sidecars + (optionally) synthesized template sidecar
    let mut sidecar_strs: Vec<&str> = request
        .sidecars
        .iter()
        .map(|s| s.content.as_str())
        .collect();
    if let Some(ref s) = synth_sidecar {
        sidecar_strs.push(s.as_str());
    }

    let (entry, cache_hit) =
        crate::api::cache::get_or_realize(&request.context.content, &sidecar_strs).map_err(
            |e| ApiError::BadRequest {
                message: format!("Failed to parse/realize context: {}", e),
                details: Some(e.clone()),
            },
        )?;
    let realized = entry.realized.as_ref();
    info!(
        realize_ms = handler_start.elapsed().as_millis() as u64,
        cache_hit, "synthesize: parse+realize complete"
    );

    // Get formula
    let realized_formula =
        realized
            .formulas
            .get(&formula_name)
            .ok_or_else(|| ApiError::BadRequest {
                message: format!("Unknown formula '{}'", formula_name),
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
                extract_strategy: request.options.extract_strategy,
                mode: resolve_controller_mode(
                    &request.options.controller_mode,
                    request.options.extract_strategy,
                )?,
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
            &formula_name,
            formula_raw,
        )?;
        Some(FileContent {
            name: format!("{}_controller.ctxdsl", request.automaton),
            content,
        })
    } else {
        None
    };

    // Compute counterstrategy graph for unrealizable cases
    let counterstrategy = if !synthesis.realizable {
        compute_counterstrategy_result(
            realized,
            &request.automaton,
            &realized_formula.formula,
            &env,
            &eval_options,
            request.options.minimize,
        )
    } else {
        None
    };

    // Convert diagnostics
    let diagnostics = convert_diagnostics(&synthesis.diagnostics);

    // Emit controller in native format if requested
    let controller_native = if synthesis.realizable {
        match request.options.output_format.as_deref() {
            Some("xstate") => {
                use crate::adapter::xstate::emit_controller::controller_to_xstate_json;
                let json =
                    controller_to_xstate_json(&synthesis.controller, &request.automaton, true);
                Some(FileContent {
                    name: format!("{}_controller.json", request.automaton),
                    content: json,
                })
            }
            Some("systemverilog") | Some("sv") => {
                use crate::adapter::systemverilog::emit_controller::controller_to_systemverilog;
                let sv =
                    controller_to_systemverilog(&synthesis.controller, &request.automaton, true);
                Some(FileContent {
                    name: format!("{}_controller.sv", request.automaton),
                    content: sv,
                })
            }
            _ => None,
        }
    } else {
        None
    };

    Ok(Json(ContextSynthesizeResponse {
        success: true,
        realizable: synthesis.realizable,
        controller: controller_content,
        controller_native,
        diagnostics,
        counterstrategy,
    }))
}

/// Import an external format (XState, SystemVerilog, TLSF, AIGER, Promela) into CTXDSL.
pub async fn context_import_handler(
    Json(request): Json<ContextImportRequest>,
) -> ApiResult<Json<ContextImportResponse>> {
    use crate::adapter::{AdapterOptions, FormatAdapter};

    let options = AdapterOptions::default();

    let result = match request.format.as_str() {
        "tlsf" => crate::adapter::tlsf::TlsfAdapter::translate(&request.content, &options),
        "aiger" => crate::adapter::aiger::AigerAdapter::translate(&request.content, &options),
        "btor2" | "btor" => {
            crate::adapter::btor2::Btor2Adapter::translate(&request.content, &options)
        }
        "sv-yosys" | "yosys" => {
            // Yosys-driven SV elaboration via child process. Parity with the
            // CLI `--adapter sv-yosys` flag.
            let yopts = if !request.additional_sources.is_empty() {
                let mut additional = std::collections::HashMap::new();
                for src in &request.additional_sources {
                    additional.insert(src.name.clone(), src.content.clone());
                }
                crate::adapter::yosys::YosysOptions {
                    additional_sources: additional.into_iter().collect(),
                    ..Default::default()
                }
            } else {
                crate::adapter::yosys::YosysOptions::default()
            };
            crate::adapter::yosys::translate_sv(&request.content, &options, &yopts)
        }
        "promela" => crate::adapter::promela::PromelaAdapter::translate(&request.content, &options),
        "xstate" => crate::adapter::xstate::XStateAdapter::translate(&request.content, &options),
        "systemverilog" | "sv" => {
            // Check if a multi-module sidecar is provided
            if let Some(ref sidecar_json) = request.sidecar {
                let is_multi = sidecar_json.contains("mununu_sv_multi_v1")
                    || sidecar_json.contains("\"modules\"");
                if is_multi && !request.additional_sources.is_empty() {
                    // Multi-module path: build source map and use in-memory composition
                    let mut sources = std::collections::HashMap::new();
                    // The primary source might be the top module; sub-modules come from additional_sources
                    sources.insert(
                        request.filename.clone().unwrap_or_default(),
                        request.content.clone(),
                    );
                    for src in &request.additional_sources {
                        sources.insert(src.name.clone(), src.content.clone());
                    }
                    crate::adapter::systemverilog::SystemVerilogAdapter::translate_multi_module_content(
                        sidecar_json,
                        &sources,
                        &options,
                    )
                } else {
                    // Single-module with sidecar (future: pass sidecar to translate)
                    crate::adapter::systemverilog::SystemVerilogAdapter::translate(
                        &request.content,
                        &options,
                    )
                }
            } else {
                crate::adapter::systemverilog::SystemVerilogAdapter::translate(
                    &request.content,
                    &options,
                )
            }
        }
        "extraction" | "espec" => {
            let mut opts = options.clone();
            opts.mode = Some("vulnerable".to_string());
            crate::adapter::extraction::ExtractionAdapter::translate(&request.content, &opts)
        }
        "auto" | "" => crate::adapter::auto_translate(&request.content, &options),
        other => {
            return Err(ApiError::BadRequest {
                message: format!(
                    "Unknown format '{other}'. Supported: auto, tlsf, aiger, btor2, promela, xstate, systemverilog, sv-yosys, extraction"
                ),
                details: None,
            });
        }
    };

    let output = result.map_err(|e| ApiError::BadRequest {
        message: format!("Adapter translation failed: {e}"),
        details: None,
    })?;

    let state_valuations = if output.state_valuations.is_empty() {
        None
    } else {
        serde_json::to_value(&output.state_valuations).ok()
    };
    let transition_observations = if output.transition_observations.is_empty() {
        None
    } else {
        serde_json::to_value(&output.transition_observations).ok()
    };

    Ok(Json(ContextImportResponse {
        success: true,
        ctxdsl: output.ctxdsl,
        source_format: output.source_info.format.to_string(),
        warnings: output.warnings.iter().map(|w| w.message.clone()).collect(),
        signal_count: output.source_info.signal_count,
        state_count: output.source_info.state_count,
        property_count: output.source_info.property_count,
        state_valuations,
        transition_observations,
    }))
}

/// Generate graph data for visualization
pub async fn context_graphs_handler(
    Json(request): Json<ContextGraphsRequest>,
) -> ApiResult<Json<ContextGraphsResponse>> {
    let handler_start = Instant::now();

    // Cache parse + realize.
    let sidecar_strs: Vec<&str> = request
        .sidecars
        .iter()
        .map(|s| s.content.as_str())
        .collect();
    let (entry, cache_hit) =
        crate::api::cache::get_or_realize(&request.context.content, &sidecar_strs).map_err(
            |e| ApiError::BadRequest {
                message: format!("Failed to parse/realize context: {}", e),
                details: Some(e.clone()),
            },
        )?;
    let context_doc = entry.context_doc.as_ref();
    let sidecar_docs = entry.sidecar_docs.as_ref();
    let realized = entry.realized.as_ref();
    info!(
        realize_ms = handler_start.elapsed().as_millis() as u64,
        cache_hit, "graphs: parse+realize complete"
    );

    // Generate graphs
    let (mut graphs, context_summary) = generate_graphs(
        context_doc,
        sidecar_docs,
        realized,
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
                    extract_strategy: false,
                    mode: crate::context::ControllerMode::default(),
                },
            ) else {
                continue;
            };
            if !syn.realizable {
                continue;
            }
            let controller_name = format!("{}_controller", rc.name);
            let Some(source_clts) = realized.context.clts(&rc.source) else {
                continue;
            };
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

    let t0 = Instant::now();

    // Resolve template_ref BEFORE cache lookup so the cache key covers the
    // template instantiation. Repeated requests with the same context +
    // template params hit the cache.
    let (effective_formula, synth_sidecar) = resolve_template_ref_for_cache(
        &request.formula,
        &request.template_ref,
        request.automaton.as_deref().unwrap_or("__default"),
    )?;

    let mut sidecar_strs: Vec<&str> = request
        .sidecars
        .iter()
        .map(|s| s.content.as_str())
        .collect();
    if let Some(ref s) = synth_sidecar {
        sidecar_strs.push(s.as_str());
    }

    let (entry, cache_hit) =
        crate::api::cache::get_or_realize(&request.context.content, &sidecar_strs).map_err(
            |e| ApiError::BadRequest {
                message: format!("Failed to parse/realize context: {}", e),
                details: Some(e.clone()),
            },
        )?;
    let realized = entry.realized.as_ref();
    let realize_ms = t0.elapsed().as_millis();
    info!(
        realize_ms,
        cache_hit,
        counterstrategy = counterstrategy_requested,
        "verify: parse+realize complete"
    );

    // Collect formula–automaton pairs to evaluate
    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Some(ref formula_name) = effective_formula {
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
            let automata = resolve_targets(&rf.targets, realized);
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
                let automata = resolve_targets(&rf.targets, realized);
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
                && let Some(name) = clts.state_name(state_id)
            {
                satisfying_state_names.push(name.to_string());
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
            compute_counterstrategy_result(
                realized,
                automaton_name,
                &rf.formula,
                &env,
                &eval_options,
                request.minimize_counterstrategy,
            )
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
        realize_ms,
        eval_ms,
        total_ms,
        cache_hit,
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

/// Compute counterstrategy graph elements for an unsatisfied formula.
///
/// Inverts the formula, evaluates the inverted formula to find the environment's
/// winning region, and builds Cytoscape graph elements for visualization.
fn compute_counterstrategy_result(
    realized: &RealizedContext,
    automaton_name: &str,
    formula: &Formula,
    env: &Environment,
    eval_options: &EvaluationOptions,
    minimize: bool,
) -> Option<CounterstrategyResult> {
    use crate::mu_calculus::invert;

    let clts = realized.context.clts(automaton_name)?;
    let inverted = invert::invert(formula);

    let inverted_result = if minimize {
        realized
            .context
            .evaluate_mu_with_witnesses(automaton_name, &inverted, env, Some(eval_options))
            .ok()
            .map(|(bv, wm)| (bv, Some(wm)))
    } else {
        realized
            .context
            .evaluate_mu(automaton_name, &inverted, env, Some(eval_options))
            .ok()
            .map(|bv| (bv, None))
    };

    inverted_result.map(|(inv_bv, witness_map)| {
        let winning_set: HashSet<usize> = clts
            .states()
            .filter(|sid| inv_bv.get(sid.index()).map(|bit| *bit).unwrap_or(false))
            .map(|sid| sid.index())
            .collect();

        let cs_name = format!("{}_counterstrategy", automaton_name);
        // Graph generation applies strategy extraction and then filters to
        // states reachable from initials via the kept transitions.
        let (graph_elements, reachable) = crate::api::graph::counterstrategy_to_graph_elements(
            clts,
            &cs_name,
            &winning_set,
            minimize,
            witness_map.as_ref(),
        );

        // Report only reachable states in the winning list
        let mut env_winning: Vec<String> = clts
            .states()
            .filter(|sid| reachable.contains(&sid.index()))
            .filter_map(|sid| clts.state_name(sid).map(|n| n.to_string()))
            .collect();
        env_winning.sort();

        CounterstrategyResult {
            environment_winning_states: env_winning,
            graph_elements,
            inverted_formula: format!("{:?}", inverted),
            minimized: minimize,
        }
    })
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
        lasso_traces: diagnostics
            .lasso_traces
            .iter()
            .map(|lt| LassoTraceApi {
                prefix: lt.prefix.clone(),
                cycle: lt.cycle.clone(),
                prefix_labels: lt.prefix_labels.clone(),
                cycle_labels: lt.cycle_labels.clone(),
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

// ============================================================================
// Extraction Endpoints
// ============================================================================

/// List available domain profiles for extraction.
pub async fn extraction_domains_handler()
-> ApiResult<Json<super::models::ExtractionDomainsResponse>> {
    use crate::adapter::extraction::ast_extract::domain;

    let profiles = domain::available_profiles()
        .into_iter()
        .filter_map(|name| {
            let profile = domain::get_profile(name)?;
            Some(super::models::DomainProfileInfo {
                name: name.to_string(),
                language: profile.language.to_string(),
                description: profile.description.to_string(),
            })
        })
        .collect();

    Ok(Json(super::models::ExtractionDomainsResponse { profiles }))
}

/// Phase B — scan source for concurrency idioms and return findings
/// the caller can use to seed a `composition.instances[]` block. Pure
/// pass-through to `concurrency_detect::detect_concurrency`.
#[cfg(feature = "ast-extract")]
pub async fn extraction_propose_composition_handler(
    Json(request): Json<super::models::ProposeCompositionRequest>,
) -> ApiResult<Json<super::models::ProposeCompositionResponse>> {
    use crate::adapter::extraction::ast_extract::{concurrency_detect, parser};

    let lang_name = request
        .language
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest {
            message: "missing required field `language`".to_string(),
            details: Some("supported values: typescript, python, rust".to_string()),
        })?;
    let lang =
        parser::SourceLanguage::from_name(lang_name).ok_or_else(|| ApiError::BadRequest {
            message: format!("unknown language: {lang_name}"),
            details: Some("supported values: typescript, python, rust".to_string()),
        })?;

    let parsed = parser::parse_source(&request.source, lang).map_err(|e| ApiError::BadRequest {
        message: format!("parse error: {e}"),
        details: None,
    })?;

    let findings = concurrency_detect::detect_concurrency(&parsed);
    Ok(Json(super::models::ProposeCompositionResponse { findings }))
}

/// Stub when ast-extract feature is not enabled. Mirrors the pattern used
/// for `extraction_extract_handler` so route registration in `server.rs`
/// can stay unconditional.
#[cfg(not(feature = "ast-extract"))]
pub async fn extraction_propose_composition_handler(
    Json(_request): Json<super::models::ProposeCompositionRequest>,
) -> ApiResult<Json<super::models::ProposeCompositionResponse>> {
    Err(ApiError::BadRequest {
        message: "AST extraction not available. Build with --features ast-extract".to_string(),
        details: None,
    })
}

/// List the supported composition modes consumed by `composition.type`
/// in the extract config / espec. Static — derived from the
/// `CompositionSemantics` enum's variants and their soundness notes.
pub async fn extraction_composition_modes_handler()
-> ApiResult<Json<super::models::CompositionModesResponse>> {
    let modes = vec![
        super::models::CompositionModeInfo {
            name: "synchronous",
            description: "Shared-alphabet labels fire jointly across instances; \
                          independent labels collapse into a single joint step.",
        },
        super::models::CompositionModeInfo {
            name: "asynchronous",
            description: "Shared-alphabet labels fire jointly; independent labels \
                          interleave in either order without fairness constraints. \
                          Sound for safety; unsound for liveness without explicit \
                          fairness assumptions.",
        },
    ];
    Ok(Json(super::models::CompositionModesResponse { modes }))
}

/// Extract a model from source code using the AST-based extraction pipeline.
///
/// Requires the `ast-extract` feature flag.
#[cfg(feature = "ast-extract")]
pub async fn extraction_extract_handler(
    Json(request): Json<super::models::ExtractionExtractRequest>,
) -> ApiResult<Json<super::models::ExtractionExtractResponse>> {
    let language = request.language.as_deref().unwrap_or("typescript");

    let spec = crate::adapter::extraction::ast_extract::extract_from_source(
        &request.config,
        &request.source,
        language,
    )
    .map_err(|e| ApiError::BadRequest {
        message: format!("Extraction failed: {e}"),
        details: None,
    })?;

    let automata: Vec<super::models::ExtractionAutomatonInfo> = spec
        .model_config
        .automata
        .iter()
        .map(|a| super::models::ExtractionAutomatonInfo {
            id: a.id.clone(),
            state_count: a.states.len(),
            transition_count: a.transitions.len(),
        })
        .collect();

    let espec_json = serde_json::to_string_pretty(&spec).map_err(|e| ApiError::Internal {
        message: format!("Failed to serialize extraction result: {e}"),
        source: None,
    })?;

    Ok(Json(super::models::ExtractionExtractResponse {
        success: true,
        espec: espec_json,
        warnings: vec![],
        automata,
    }))
}

/// Stub handler when ast-extract feature is not enabled.
#[cfg(not(feature = "ast-extract"))]
pub async fn extraction_extract_handler(
    Json(_request): Json<super::models::ExtractionExtractRequest>,
) -> ApiResult<Json<super::models::ExtractionExtractResponse>> {
    Err(ApiError::BadRequest {
        message: "AST extraction not available. Build with --features ast-extract".to_string(),
        details: None,
    })
}

/// Initialize a SystemVerilog annotation sidecar from parsed module.
pub async fn sv_init_handler(
    Json(request): Json<SvInitRequest>,
) -> ApiResult<Json<SvInitResponse>> {
    use crate::adapter::systemverilog::parser;

    let module = parser::parse(&request.source.content).map_err(|e| ApiError::BadRequest {
        message: format!("SV parse error: {e}"),
        details: None,
    })?;

    // Multi-module path: additional sources provided
    if !request.additional_sources.is_empty() {
        use crate::adapter::systemverilog::annotation::{ParsedSubModule, generate_multi_sidecar};

        let mut sub_modules = std::collections::HashMap::new();
        for additional in &request.additional_sources {
            let sub_mod = parser::parse(&additional.content).map_err(|e| ApiError::BadRequest {
                message: format!("SV parse error in '{}': {e}", additional.name),
                details: None,
            })?;
            sub_modules.insert(
                sub_mod.name.clone(),
                ParsedSubModule {
                    module: sub_mod,
                    source_name: additional.name.clone(),
                },
            );
        }

        let multi_sidecar = generate_multi_sidecar(&module, &sub_modules);
        let sidecar_json =
            serde_json::to_string_pretty(&multi_sidecar).map_err(|e| ApiError::Internal {
                message: format!("Failed to serialize sidecar: {e}"),
                source: None,
            })?;

        let mut all_signals = Vec::new();
        let mut all_inputs = Vec::new();
        for entry in &multi_sidecar.modules {
            for s in &entry.signals {
                all_signals.push(SvSignalInfo {
                    name: format!("{}.{}", entry.name, s.name),
                    width: 0,
                    abstraction: format!("{:?}", s.abstraction).to_lowercase(),
                    preserve: s.preserve,
                    note: s.note.clone(),
                });
            }
            for i in &entry.inputs {
                all_inputs.push(SvInputInfo {
                    name: format!("{}.{}", entry.name, i.name),
                    abstraction: format!("{:?}", i.abstraction).to_lowercase(),
                });
            }
        }

        return Ok(Json(SvInitResponse {
            success: true,
            sidecar: sidecar_json,
            schema: "mununu_sv_multi_v1".to_string(),
            signals: all_signals,
            inputs: all_inputs,
            warnings: vec![],
        }));
    }

    // Single-module path
    use crate::adapter::systemverilog::annotation::generate_sidecar;

    let sidecar = generate_sidecar(&module);
    let sidecar_json = serde_json::to_string_pretty(&sidecar).map_err(|e| ApiError::Internal {
        message: format!("Failed to serialize sidecar: {e}"),
        source: None,
    })?;

    let signals: Vec<SvSignalInfo> = sidecar
        .signals
        .iter()
        .map(|s| SvSignalInfo {
            name: s.name.clone(),
            width: 0,
            abstraction: format!("{:?}", s.abstraction).to_lowercase(),
            preserve: s.preserve,
            note: s.note.clone(),
        })
        .collect();

    let inputs: Vec<SvInputInfo> = sidecar
        .inputs
        .iter()
        .map(|i| SvInputInfo {
            name: i.name.clone(),
            abstraction: format!("{:?}", i.abstraction).to_lowercase(),
        })
        .collect();

    Ok(Json(SvInitResponse {
        success: true,
        sidecar: sidecar_json,
        schema: "mununu_sv_annotation_v1".to_string(),
        signals,
        inputs,
        warnings: vec![],
    }))
}

/// Run SMT-based value discovery on a SystemVerilog module.
pub async fn sv_discover_handler(
    Json(request): Json<SvDiscoverRequest>,
) -> ApiResult<Json<SvDiscoverResponse>> {
    // Check if SMT feature is available
    if !cfg!(feature = "smt") {
        return Ok(Json(SvDiscoverResponse {
            success: false,
            sidecar: request.sidecar.clone(),
            discoveries: vec![],
            smt_available: false,
            warnings: vec![
                "SMT discovery not available: mununu was built without the 'smt' feature. \
                 Rebuild with `cargo build --features smt` to enable Z3-based value discovery."
                    .to_string(),
            ],
        }));
    }

    // Parse the SV source
    use crate::adapter::systemverilog::parser;
    let _module = parser::parse(&request.source.content).map_err(|e| ApiError::BadRequest {
        message: format!("SV parse error: {e}"),
        details: None,
    })?;

    // Parse the sidecar
    use crate::adapter::systemverilog::annotation::SvAnnotation;
    let _sidecar: SvAnnotation =
        serde_json::from_str(&request.sidecar).map_err(|e| ApiError::BadRequest {
            message: format!("Failed to parse sidecar JSON: {e}"),
            details: None,
        })?;

    // Run SMT discovery (only when feature is enabled)
    #[cfg(feature = "smt")]
    {
        use crate::adapter::systemverilog::annotation::merge_discovered_values;
        use crate::adapter::systemverilog::kripke_smt::engine;

        let mut sidecar = _sidecar;
        let results = engine::discover_significant_values(&_module, &sidecar);

        let discoveries: Vec<SvDiscoveryResult> = results
            .iter()
            .map(|(signal, dv)| SvDiscoveryResult {
                signal: signal.clone(),
                values_found: dv.values.len(),
            })
            .collect();

        merge_discovered_values(&mut sidecar.discovered_values, results);

        let updated_sidecar =
            serde_json::to_string_pretty(&sidecar).map_err(|e| ApiError::Internal {
                message: format!("Failed to serialize updated sidecar: {e}"),
                source: None,
            })?;

        Ok(Json(SvDiscoverResponse {
            success: true,
            sidecar: updated_sidecar,
            discoveries,
            smt_available: true,
            warnings: vec![],
        }))
    }

    #[cfg(not(feature = "smt"))]
    {
        // Stub when smt feature is disabled.
        unreachable!()
    }
}

/// Validate an extraction spec against source code.
pub async fn extraction_validate_handler(
    Json(request): Json<ExtractionValidateRequest>,
) -> ApiResult<Json<ExtractionValidateResponse>> {
    use crate::adapter::extraction::validate;

    let report =
        validate::validate_spec_content(&request.spec, &request.source, request.drift_window)
            .map_err(|e| ApiError::BadRequest {
                message: format!("Validation failed: {e}"),
                details: None,
            })?;

    let anchors: Vec<AnchorResultApi> = report
        .anchors
        .iter()
        .map(|a| match a {
            validate::AnchorResult::Exact {
                spec_id,
                section,
                line,
                ..
            } => AnchorResultApi {
                id: spec_id.clone(),
                section: section.clone(),
                status: "exact".to_string(),
                line: Some(*line),
                found_line: Some(*line),
                message: None,
            },
            validate::AnchorResult::Drifted {
                spec_id,
                section,
                expected_line,
                found_line,
                ..
            } => AnchorResultApi {
                id: spec_id.clone(),
                section: section.clone(),
                status: "drifted".to_string(),
                line: Some(*expected_line),
                found_line: Some(*found_line),
                message: Some(format!(
                    "Drifted from line {} to {}",
                    expected_line, found_line
                )),
            },
            validate::AnchorResult::Mismatch {
                spec_id,
                section,
                expected_line,
                expected_pattern,
                actual_at_line,
            } => AnchorResultApi {
                id: spec_id.clone(),
                section: section.clone(),
                status: "mismatch".to_string(),
                line: Some(*expected_line),
                found_line: None,
                message: Some(format!(
                    "Expected '{}', found '{}'",
                    expected_pattern, actual_at_line
                )),
            },
            validate::AnchorResult::Error {
                spec_id,
                section,
                message,
            } => AnchorResultApi {
                id: spec_id.clone(),
                section: section.clone(),
                status: "error".to_string(),
                line: None,
                found_line: None,
                message: Some(message.clone()),
            },
        })
        .collect();

    let uncovered: Vec<UncoveredAccessApi> = report
        .uncovered
        .iter()
        .map(|u| UncoveredAccessApi {
            line: u.line,
            field: u.field.clone(),
            content: u.content.clone(),
        })
        .collect();

    Ok(Json(ExtractionValidateResponse {
        success: true,
        summary: ValidationSummaryApi {
            total: report.summary.total,
            exact: report.summary.exact,
            drifted: report.summary.drifted,
            mismatch: report.summary.mismatch,
            error: report.summary.error,
            uncovered_accesses: report.summary.uncovered_accesses,
        },
        anchors,
        uncovered,
        commit_match: report.commit_match,
    }))
}

/// List predicate names per automaton in a context.
pub async fn context_predicates_handler(
    Json(request): Json<ContextPredicatesRequest>,
) -> ApiResult<Json<ContextPredicatesResponse>> {
    let instant = Instant::now();

    // Cache parse + realize.
    let sidecar_strs: Vec<&str> = request
        .sidecars
        .iter()
        .map(|s| s.content.as_str())
        .collect();
    let (entry, cache_hit) =
        crate::api::cache::get_or_realize(&request.context.content, &sidecar_strs).map_err(
            |e| ApiError::BadRequest {
                message: format!("Context parse/realize error: {e}"),
                details: None,
            },
        )?;
    let realized = entry.realized.as_ref();

    info!(
        elapsed_ms = instant.elapsed().as_millis() as u64,
        cache_hit, "predicates: realized"
    );

    let mut predicates: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for aut_name in realized.context.clts_names() {
        if let Some(ref filter) = request.automaton
            && &aut_name != filter
        {
            continue;
        }
        let preds: Vec<String> = realized
            .predicate_names(&aut_name)
            .map(|set| {
                let mut v: Vec<String> = set.iter().map(|s| s.to_string()).collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        predicates.insert(aut_name, preds);
    }

    Ok(Json(ContextPredicatesResponse {
        success: true,
        predicates,
    }))
}

// ---------------------------------------------------------------------------
// Template resolution helper for API handlers
// ---------------------------------------------------------------------------

/// Resolve a formula reference (direct name or template_ref) into a name + an
/// optional synthesized sidecar CTXDSL string.
///
/// - When `formula` is given: returns `(Some(name), None)`.
/// - When `template_ref` is given: instantiates the template, returns
///   `(Some(template_formula_name), Some(synth_sidecar_ctxdsl))` so the caller
///   can pass the synthesized string to `cache::get_or_realize` as an extra
///   sidecar (the cache key then covers the template instantiation, enabling
///   hits across repeated requests with identical template params).
/// - When neither is given: returns `(None, None)` — caller decides whether
///   that's an error.
fn resolve_template_ref_for_cache(
    formula: &Option<String>,
    template_ref: &Option<crate::adapter::templates::TemplateRef>,
    automaton: &str,
) -> Result<(Option<String>, Option<String>), ApiError> {
    if let Some(name) = formula {
        return Ok((Some(name.clone()), None));
    }

    let tref = match template_ref {
        Some(t) => t,
        None => return Ok((None, None)),
    };

    let registry = crate::adapter::templates::TemplateRegistry::builtin();
    let inst = registry
        .instantiate(tref)
        .map_err(|e| ApiError::BadRequest {
            message: format!("Template instantiation failed: {e}"),
            details: None,
        })?;

    let formula_name = inst.name.clone();
    let sidecar_ctxdsl = format!(
        "context __template_sidecar {{\n  mu_formulas {{\n    formula {formula_name} {{\n      over {automaton};\n      body = {};\n    }}\n  }}\n}}\n",
        inst.formula
    );
    Ok((Some(formula_name), Some(sidecar_ctxdsl)))
}

/// Validate an assume/guarantee contract set's discharge graph.
///
/// Mirrors the `mununu contract validate` CLI subcommand. Accepts a
/// `ContractSet` JSON body, runs the §3.x SCC analysis, returns the
/// verdict.
pub async fn contract_validate_handler(
    Json(set): Json<crate::contract::ContractSet>,
) -> ApiResult<Json<crate::contract::discharge::DischargeVerdict>> {
    let verdict = crate::contract::discharge::validate(&set);
    Ok(Json(verdict))
}

/// Request body for `POST /api/v1/contract/discover`.
#[derive(Debug, serde::Deserialize)]
pub struct ContractDiscoverRequest {
    pub interface: crate::contract::discover::BlackBoxInterface,
    #[serde(default)]
    pub force_controllable: Vec<String>,
    #[serde(default)]
    pub force_uncontrollable: Vec<String>,
    #[serde(default)]
    pub emit_fairness_gap: bool,
    /// Optional contract corpus root used to resolve
    /// `@mununu_interface contract://` URIs on the interface. Mirrors
    /// the CLI's `--corpus` flag for three-surface parity.
    #[serde(default)]
    pub corpus: Option<std::path::PathBuf>,
}

/// Run phase-1 contract discovery on a black-box interface description.
/// Mirrors `mununu contract discover`. The structured `tracing::warn!`
/// diagnostics still fire on the server side; the response carries the
/// full `Phase1Output` (labels + gap markers + corpus resolutions) for
/// the UI to render.
pub async fn contract_discover_handler(
    Json(request): Json<ContractDiscoverRequest>,
) -> ApiResult<Json<crate::contract::discover::Phase1Output>> {
    use crate::contract::discover::{DiscoverOptions, discover_phase1};
    use crate::corpus::Corpus;

    let force_c: Vec<&str> = request
        .force_controllable
        .iter()
        .map(|s| s.as_str())
        .collect();
    let force_u: Vec<&str> = request
        .force_uncontrollable
        .iter()
        .map(|s| s.as_str())
        .collect();
    let corpus = match &request.corpus {
        Some(root) => Some(Corpus::load(root).map_err(|e| ApiError::BadRequest {
            message: format!("failed to load corpus at {}: {e}", root.display()),
            details: None,
        })?),
        None => None,
    };
    let opts = DiscoverOptions {
        force_controllable: &force_c,
        force_uncontrollable: &force_u,
        emit_fairness_gap: request.emit_fairness_gap,
        corpus: corpus.as_ref(),
    };
    let output = discover_phase1(&request.interface, &opts);
    output.gaps.emit_diagnostics();
    Ok(Json(output))
}

/// Request body for `POST /api/v1/contract/query`.
#[derive(Debug, serde::Deserialize)]
pub struct ContractQueryRequest {
    /// `<domain>/<name>` identifier, e.g. `"rtl_protocol/axi4_slave"`.
    pub id: String,
    /// Filesystem path of the corpus root the server should load.
    pub corpus: std::path::PathBuf,
    /// Parameters to match against. Each value is a JSON value
    /// (number, string, bool, etc.).
    #[serde(default)]
    pub parameters: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Response from `POST /api/v1/contract/query`. The body carries the
/// ranked candidate list directly.
#[derive(Debug, serde::Serialize)]
pub struct ContractQueryResponse {
    pub candidates: Vec<crate::corpus::ContractEntry>,
}

/// Query the contract corpus (Document D task D2).
///
/// Loads the corpus rooted at `request.corpus`, runs the
/// `(domain, name, parameters)` query, and returns the ranked
/// candidate list.
pub async fn contract_query_handler(
    Json(request): Json<ContractQueryRequest>,
) -> ApiResult<Json<ContractQueryResponse>> {
    let (domain, name) = match request.id.split_once('/') {
        Some((d, n)) if !d.is_empty() && !n.is_empty() => (d.to_string(), n.to_string()),
        _ => {
            return Err(ApiError::BadRequest {
                message: format!(
                    "expected `<domain>/<name>`, got '{}' — example: rtl_protocol/axi4_slave",
                    request.id
                ),
                details: None,
            });
        }
    };

    let corpus = if request.corpus.exists() {
        crate::corpus::Corpus::load(&request.corpus).map_err(|e| ApiError::BadRequest {
            message: format!(
                "failed to load corpus from {}: {e}",
                request.corpus.display()
            ),
            details: None,
        })?
    } else {
        crate::corpus::Corpus::empty()
    };

    let candidates: Vec<crate::corpus::ContractEntry> = corpus
        .query(&domain, &name, &request.parameters)
        .into_iter()
        .cloned()
        .collect();

    Ok(Json(ContractQueryResponse { candidates }))
}

/// Request body for `POST /api/v1/contract/review`.
#[derive(Debug, serde::Deserialize)]
pub struct ContractReviewRequest {
    pub interface: crate::contract::discover::BlackBoxInterface,
    #[serde(default)]
    pub force_controllable: Vec<String>,
    #[serde(default)]
    pub force_uncontrollable: Vec<String>,
    #[serde(default)]
    pub emit_fairness_gap: bool,
    /// Optional contract corpus root used to resolve
    /// `@mununu_interface contract://` URIs into reference proposals.
    #[serde(default)]
    pub corpus: Option<std::path::PathBuf>,
}

/// HITL stage-4 review surface — Document A §A7 / Document D §D.8.
///
/// Wraps phase-2 discovery and adds a flat list of proposed clauses
/// extracted from `@mununu_assume` / `@mununu_guarantee` annotations
/// and resolved corpus references. The approve/edit/reject UX lives in
/// the CLI / UI surfaces.
pub async fn contract_review_handler(
    Json(request): Json<ContractReviewRequest>,
) -> ApiResult<Json<crate::contract::review::ReviewPackage>> {
    use crate::contract::discover::DiscoverOptions;
    use crate::contract::review::build_review_package;
    use crate::corpus::Corpus;

    let force_c: Vec<&str> = request
        .force_controllable
        .iter()
        .map(|s| s.as_str())
        .collect();
    let force_u: Vec<&str> = request
        .force_uncontrollable
        .iter()
        .map(|s| s.as_str())
        .collect();
    let corpus = match &request.corpus {
        Some(root) => Some(Corpus::load(root).map_err(|e| ApiError::BadRequest {
            message: format!("failed to load corpus at {}: {e}", root.display()),
            details: None,
        })?),
        None => None,
    };
    let opts = DiscoverOptions {
        force_controllable: &force_c,
        force_uncontrollable: &force_u,
        emit_fairness_gap: request.emit_fairness_gap,
        corpus: corpus.as_ref(),
    };
    let pkg = build_review_package(&request.interface, &opts);
    pkg.phase1.gaps.emit_diagnostics();
    Ok(Json(pkg))
}

// ============================================================================
// Codesign verify (Document C task C4)
// ============================================================================

/// Request body for `POST /api/v1/codesign/verify`.
#[derive(Debug, serde::Deserialize)]
pub struct CodesignVerifyRequest {
    /// Register-map sidecar contents as a parsed `RegisterMap` value.
    /// HTTP callers should JSON-encode the same shape that the CLI
    /// loads from `register_map.json` on disk.
    pub register_map: crate::codesign::register_map::RegisterMap,
    /// Firmware CTXDSL document text.
    pub firmware_ctxdsl: String,
    /// Formula name to evaluate.
    pub formula: String,
    /// Composition / automaton name to evaluate over. Defaults to the
    /// codesign composition emitted by the splicer
    /// (`<PERIPHERAL>System`).
    #[serde(default)]
    pub automaton: Option<String>,
    /// Optional override for the peripheral automaton name.
    #[serde(default)]
    pub peripheral_automaton: Option<String>,
    /// Optional override for the composition name.
    #[serde(default)]
    pub composition_name: Option<String>,
}

/// Response body for `POST /api/v1/codesign/verify`.
#[derive(Debug, serde::Serialize)]
pub struct CodesignVerifyResponse {
    /// Whether every initial state satisfies the formula.
    pub satisfied: bool,
    /// Total number of states in the composed automaton/composition.
    pub total_states: usize,
    /// Number of states satisfying the formula.
    pub satisfying_states: usize,
    /// Initial state names.
    pub initial_states: Vec<String>,
    /// Subset of `initial_states` that satisfy the formula.
    pub initial_satisfying: Vec<String>,
    /// Composition shape used for evaluation.
    pub composition: CodesignCompositionInfo,
    /// The composed CTXDSL the verifier ran against — useful for the
    /// UI to render alongside the verdict.
    pub composed_ctxdsl: String,
}

/// Composition-shape report for the response.
#[derive(Debug, serde::Serialize)]
pub struct CodesignCompositionInfo {
    pub peripheral_automaton: String,
    pub composition_name: String,
    pub firmware_members: Vec<String>,
    pub automaton: String,
}

/// HW/SW codesign verification handler — Document C task C4.
///
/// Reads a register-map sidecar + firmware CTXDSL, splices the
/// coupling fragment into the firmware document, realises the
/// composed context, and evaluates the named formula. Returns the
/// verdict plus the composed CTXDSL so the UI can render both.
pub async fn codesign_verify_handler(
    Json(request): Json<CodesignVerifyRequest>,
) -> ApiResult<Json<CodesignVerifyResponse>> {
    use crate::codesign::compose::{ComposeOptions, compose_codesign_ctxdsl};
    use crate::context_dsl::{parse, realize_context};

    let opts = ComposeOptions {
        peripheral_automaton: request.peripheral_automaton.as_deref(),
        composition_name: request.composition_name.as_deref(),
        firmware_members_override: None,
    };
    let composed = compose_codesign_ctxdsl(&request.register_map, &request.firmware_ctxdsl, &opts)
        .map_err(|e| ApiError::BadRequest {
            message: format!("codesign compose failed: {e}"),
            details: None,
        })?;

    let context_doc = parse(&composed.ctxdsl).map_err(|e| ApiError::Internal {
        message: format!("composed CTXDSL failed to parse: {e:?}"),
        source: None,
    })?;
    let realized = realize_context(&context_doc, &[]).map_err(|e| ApiError::Internal {
        message: format!("composed CTXDSL failed to realise: {e}"),
        source: None,
    })?;

    let formula = realized
        .formulas
        .get(&request.formula)
        .ok_or_else(|| ApiError::BadRequest {
            message: format!("unknown formula '{}' in composed context", request.formula),
            details: None,
        })?;

    let automaton_name = request
        .automaton
        .clone()
        .unwrap_or_else(|| composed.composition_name.clone());
    let clts = realized
        .context
        .clts(&automaton_name)
        .ok_or_else(|| ApiError::BadRequest {
            message: format!(
                "unknown automaton/composition '{automaton_name}' in composed context — expected one of: {}",
                realized.context.clts_names().join(", ")
            ),
            details: None,
        })?;

    let env = realized.environment_for(&automaton_name);
    let options = crate::mu_calculus::EvaluationOptions::default();
    let result = crate::mu_calculus::evaluate_with_options(&formula.formula, clts, &env, &options)
        .map_err(|e| ApiError::Internal {
            message: format!("μ-calculus evaluation failed: {e}"),
            source: None,
        })?;

    let total_states = clts.state_count();
    let satisfying_states = (0..total_states)
        .filter(|i| result.get(*i).map(|b| *b).unwrap_or(false))
        .count();
    let initial_states: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|sid| clts.state_name(*sid).map(str::to_string))
        .collect();
    let initial_satisfying: Vec<String> = clts
        .initial_states()
        .iter()
        .filter_map(|sid| {
            if result.get(sid.index()).map(|b| *b).unwrap_or(false) {
                clts.state_name(*sid).map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    let satisfied = !initial_states.is_empty() && initial_satisfying.len() == initial_states.len();

    Ok(Json(CodesignVerifyResponse {
        satisfied,
        total_states,
        satisfying_states,
        initial_states,
        initial_satisfying,
        composition: CodesignCompositionInfo {
            peripheral_automaton: composed.peripheral_automaton,
            composition_name: composed.composition_name,
            firmware_members: composed.firmware_members,
            automaton: automaton_name,
        },
        composed_ctxdsl: composed.ctxdsl,
    }))
}

// ============================================================================
// Verify project (general N-source verification framework, A2.5)
// ============================================================================

/// Request body for `POST /api/v1/verify`.
///
/// The HTTP variant always receives the config inline. Source files
/// are read from disk relative to `base_dir`. Inline-content sources
/// are a future extension (would let the HTTP caller stream the
/// whole project archive without on-disk paths).
#[derive(Debug, serde::Deserialize)]
pub struct VerifyProjectRequest {
    /// Parsed `verify.toml` payload.
    pub config: crate::verify::config::VerifyConfig,
    /// Directory the source paths in the config resolve against.
    /// Required — the server has no implicit "client working
    /// directory" the way the CLI does.
    pub base_dir: String,
}

/// HTTP handler for the general verify pipeline. Mirrors
/// `mununu verify` (CLI). Returns the structured
/// [`crate::verify::report::VerifyReport`] on success.
///
/// All structural errors (config validation, adapter dispatch
/// failures, parse + realize failures, evaluation failures) come back
/// as 400 Bad Request with the error message.
pub async fn verify_project_handler(
    Json(request): Json<VerifyProjectRequest>,
) -> ApiResult<Json<crate::verify::report::VerifyReport>> {
    let base_dir = std::path::PathBuf::from(&request.base_dir);
    crate::verify::verify_project(&request.config, &base_dir)
        .map(Json)
        .map_err(|e| ApiError::BadRequest {
            message: e.to_string(),
            details: None,
        })
}

// ============================================================================
// Codesign reconcile-labels (Doc C §C.5 hard gate)
// ============================================================================

/// Request body for `POST /api/v1/codesign/reconcile-labels`.
#[derive(Debug, serde::Deserialize)]
pub struct CodesignReconcileLabelsRequest {
    /// Firmware-side rendezvous labels (the C extraction's alphabet).
    pub firmware_labels: Vec<String>,
    /// Peripheral-side rendezvous labels (the SV extraction's
    /// alphabet — or, today, the alphabet derived from a register
    /// map by `coupling::register_map_labels`).
    pub peripheral_labels: Vec<String>,
}

/// Response body for `POST /api/v1/codesign/reconcile-labels`.
///
/// The handler always returns 200 OK with a populated body — the
/// `mismatch` field distinguishes the two outcomes:
///   - `mismatch == null`: alphabets agree; `shared` holds the
///     canonical sorted alphabet.
///   - `mismatch == { firmware_only, peripheral_only }`: at least one
///     side has labels the other doesn't. `shared` is empty.
#[derive(Debug, serde::Serialize)]
pub struct CodesignReconcileLabelsResponse {
    /// Shared canonical alphabet (sorted), or empty on mismatch.
    pub shared: Vec<String>,
    /// Mismatch report. `None` when the alphabets reconcile exactly.
    pub mismatch: Option<crate::codesign::reconcile::ReconcileMismatch>,
}

/// HW/SW codesign label-alphabet reconcile handler (Doc C §C.5).
///
/// Hard gate against silent over-approximation: refuses to compose
/// firmware ‖ peripheral when the two extractions disagree on the
/// rendezvous-label alphabet. The handler itself returns 200 OK and
/// reports the mismatch in the body; downstream orchestrators (the
/// future `verify-project` flow) consume the `mismatch` field and
/// fail their pipelines on a non-null value.
pub async fn codesign_reconcile_labels_handler(
    Json(request): Json<CodesignReconcileLabelsRequest>,
) -> ApiResult<Json<CodesignReconcileLabelsResponse>> {
    use crate::codesign::reconcile::{ReconcileError, reconcile_label_alphabets};
    use std::collections::BTreeSet;

    let firmware: BTreeSet<String> = request.firmware_labels.into_iter().collect();
    let peripheral: BTreeSet<String> = request.peripheral_labels.into_iter().collect();
    match reconcile_label_alphabets(&firmware, &peripheral) {
        Ok(r) => Ok(Json(CodesignReconcileLabelsResponse {
            shared: r.shared,
            mismatch: None,
        })),
        Err(ReconcileError::Mismatch(m)) => Ok(Json(CodesignReconcileLabelsResponse {
            shared: Vec::new(),
            mismatch: Some(m),
        })),
    }
}

#[cfg(test)]
mod contract_handler_tests {
    use super::*;
    use crate::contract::{
        ClauseKind, ClauseProvenance, ContractClause, ContractSet, DischargeEdge,
        discharge::DischargeVerdict,
    };

    fn clause(id: &str, kind: ClauseKind) -> ContractClause {
        ContractClause {
            id: id.to_string(),
            kind,
            owner: "test".to_string(),
            description: None,
            provenance: ClauseProvenance::UserAuthored,
            mu_rank: None,
        }
    }

    #[tokio::test]
    async fn contract_validate_handler_returns_acyclic_for_linear_set() {
        let set = ContractSet {
            clauses: vec![
                clause("G_a", ClauseKind::Guarantee),
                clause("A_b", ClauseKind::Assumption),
            ],
            discharges: vec![DischargeEdge {
                discharger: "G_a".to_string(),
                dischargee: "A_b".to_string(),
            }],
            environment_assumptions: vec![],
        };
        let Json(verdict) = contract_validate_handler(Json(set)).await.unwrap();
        assert!(matches!(verdict, DischargeVerdict::Acyclic { .. }));
    }

    #[tokio::test]
    async fn contract_validate_handler_returns_circular_for_self_loop() {
        let set = ContractSet {
            clauses: vec![clause("X", ClauseKind::Guarantee)],
            discharges: vec![DischargeEdge {
                discharger: "X".to_string(),
                dischargee: "X".to_string(),
            }],
            environment_assumptions: vec![],
        };
        let Json(verdict) = contract_validate_handler(Json(set)).await.unwrap();
        assert!(matches!(verdict, DischargeVerdict::Circular { .. }));
    }
}

#[cfg(test)]
mod controller_mode_tests {
    use super::*;
    use crate::context::ControllerMode;

    #[test]
    fn resolve_returns_projection_by_default() {
        assert_eq!(
            resolve_controller_mode(&None, false).unwrap(),
            ControllerMode::Projection
        );
    }

    #[test]
    fn resolve_legacy_extract_strategy_maps_to_functional() {
        assert_eq!(
            resolve_controller_mode(&None, true).unwrap(),
            ControllerMode::Functional
        );
    }

    #[test]
    fn resolve_explicit_mode_wins_over_extract_strategy() {
        assert_eq!(
            resolve_controller_mode(&Some("projection".into()), true).unwrap(),
            ControllerMode::Projection
        );
    }

    #[test]
    fn resolve_accepts_all_modes_case_insensitive() {
        let cases = [
            ("projection", ControllerMode::Projection),
            ("Functional", ControllerMode::Functional),
            ("PERMISSIVE", ControllerMode::Permissive),
            ("signature-memory", ControllerMode::SignatureMemory),
            ("signature_memory", ControllerMode::SignatureMemory),
            ("SignatureMemory", ControllerMode::SignatureMemory),
            ("product-game", ControllerMode::ProductGame),
            ("parity-game", ControllerMode::ParityGame),
            ("ParityGame", ControllerMode::ParityGame),
        ];
        for (name, expected) in cases {
            assert_eq!(
                resolve_controller_mode(&Some(name.into()), false).unwrap(),
                expected,
                "name `{name}` should map to {expected:?}"
            );
        }
    }

    #[test]
    fn resolve_rejects_unknown_mode() {
        let err = resolve_controller_mode(&Some("strict".into()), false).unwrap_err();
        match err {
            ApiError::BadRequest { message, .. } => {
                assert!(message.contains("Unknown controller_mode"));
                assert!(message.contains("strict"));
            }
            _ => panic!("expected BadRequest, got {err:?}"),
        }
    }
}

#[cfg(test)]
mod composition_modes_handler_tests {
    use super::*;

    /// Smoke test for the new `/api/v1/extraction/composition-modes`
    /// endpoint. Confirms both modes are present, in the expected order,
    /// with non-empty descriptions.
    #[tokio::test]
    async fn composition_modes_handler_returns_both_modes() {
        let response = extraction_composition_modes_handler()
            .await
            .expect("handler should not error");
        let Json(body) = response;
        assert_eq!(body.modes.len(), 2);
        assert_eq!(body.modes[0].name, "synchronous");
        assert_eq!(body.modes[1].name, "asynchronous");
        assert!(!body.modes[0].description.is_empty());
        assert!(!body.modes[1].description.is_empty());
        // The async description carries the soundness caveat — sound for
        // safety / unsound for liveness — which is the key thing UI users
        // need to see when picking a mode.
        assert!(body.modes[1].description.contains("liveness"));
    }
}

#[cfg(all(test, feature = "ast-extract"))]
mod compositional_extract_handler_tests {
    use super::*;
    use crate::api::models::ExtractionExtractRequest;

    /// End-to-end smoke test for the existing `/api/v1/extraction/extract`
    /// endpoint with a compositional config. The endpoint passes the
    /// schema through to `extract_from_source`, so the new
    /// `composition.instances` + `composition.shared` fields flow through
    /// transparently — but a regression here would be invisible without
    /// an explicit test, so we add one.
    #[tokio::test]
    async fn extraction_extract_handler_handles_composition() {
        let config = serde_json::json!({
            "$schema": "extraction_config_v1",
            "domain": "mcp_server",
            "language": "typescript",
            "source": { "file": "test.ts" },
            "targets": [
                {
                    "class": "Worker",
                    "state_fields": ["state"],
                    "methods": { "include": ["save"] }
                }
            ],
            "composition": {
                "type": "asynchronous",
                "name": "race",
                "instances": [
                    { "of": "Worker", "as": "worker_a" },
                    { "of": "Worker", "as": "worker_b" }
                ],
                "shared": ["ev_save"]
            }
        });
        let source = "class Worker {\n    private state: boolean = false;\n    public save(): void { this.state = true; }\n}\n";
        let req = ExtractionExtractRequest {
            config: config.to_string(),
            source: source.to_string(),
            language: Some("typescript".to_string()),
        };
        let response = extraction_extract_handler(Json(req))
            .await
            .expect("handler should succeed");
        let Json(body) = response;
        assert!(body.success);
        // The espec response carries the JSON-serialized
        // ExtractionSpec; deserialize and assert the composition shape.
        let spec: serde_json::Value =
            serde_json::from_str(&body.espec).expect("espec should be valid JSON");
        let automata = spec["model_config"]["automata"]
            .as_array()
            .expect("automata should be an array");
        assert_eq!(automata.len(), 2, "expected 2 instance automata");
        assert_eq!(automata[0]["id"], "worker_a");
        assert_eq!(automata[1]["id"], "worker_b");
        let comp = &spec["model_config"]["composition"];
        assert_eq!(comp["type"], "asynchronous");
        let members = comp["members"]
            .as_array()
            .expect("members should be an array");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0], "worker_a");
        assert_eq!(members[1], "worker_b");
    }
}
