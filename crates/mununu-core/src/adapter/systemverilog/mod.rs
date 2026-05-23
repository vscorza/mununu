//! SystemVerilog adapter.
//!
//! Translates behavioral SystemVerilog RTL into CTXDSL via FSM extraction
//! from `always_ff` blocks. Uses the explicit-automaton encoding path
//! (like Promela), producing named states and sparse transitions.
//!
//! Supported subset: module declarations, typedef enum, always_ff with
//! case/if-else, always_comb, assign statements, and `// @mununu` properties.

pub mod annotation;
pub mod ast;
pub mod emit_controller;
pub mod fsm;
pub mod kripke;
pub mod kripke_smt;
pub mod parser;
pub mod typedef_extract;

use super::ir::*;
use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo, WarningKind,
};
use ast::{Module, MununuPropertyKind, PortDirection};
use std::collections::HashSet;

/// Phase A.3 step 3.6 — recompute the partition for telemetry only.
/// Mirrors what `build_kripke_with_config` does internally; we
/// duplicate the computation here to avoid threading a return value
/// through every Kripke-builder call site. Returns `None` when the
/// partition was effectively skipped (no property seeds).
///
/// Width tracking is `None` for the SV adapter — register widths are
/// available via `extract_registers` but plumbing them here for a
/// recomputed second pass is not worth the cycles. Adapters that
/// natively track bit widths (BTOR2) populate `state_bits_before/after`.
fn partition_summary_for_sv(
    module: &Module,
    config: &annotation::MergedConfig,
) -> Option<crate::adapter::partition::PartitionSummary> {
    use crate::adapter::partition::{self, PartitionOptions, PartitionSummary};
    let seeds = kripke::collect_property_signals_from_config(config);
    if seeds.is_empty() {
        return None;
    }
    let partition = partition::classify(module, &seeds, &PartitionOptions::default());
    Some(PartitionSummary::from_partition(&partition, None))
}

/// SystemVerilog adapter implementing [`FormatAdapter`].
pub struct SystemVerilogAdapter;

impl SystemVerilogAdapter {
    /// Translate with an explicit file path, enabling `.mununu.json` sidecar loading.
    pub fn translate_with_path(
        content: &str,
        options: &AdapterOptions,
        sv_path: &std::path::Path,
    ) -> Result<AdapterOutput, AdapterError> {
        let (module, mut parse_warnings) = parser::parse_with_warnings(content)?;

        // Try to load sidecar
        let sidecar =
            annotation::find_sidecar(sv_path).and_then(|p| annotation::load_annotation(&p).ok());

        if let Some(ref ann) = sidecar {
            eprintln!("Loaded .mununu.json sidecar for module '{}'", module.name);
            if ann.module != module.name {
                eprintln!(
                    "warning: sidecar module name '{}' does not match SV module '{}'",
                    ann.module, module.name
                );
            }
            // Validate signal/input names against module declarations and ports
            let input_ports: std::collections::HashSet<&str> = module
                .ports
                .iter()
                .filter(|p| p.direction == ast::PortDirection::Input)
                .map(|p| p.name.as_str())
                .collect();
            let all_ports: std::collections::HashSet<&str> =
                module.ports.iter().map(|p| p.name.as_str()).collect();
            let all_decls: std::collections::HashSet<&str> = module
                .declarations
                .iter()
                .filter_map(|d| match d {
                    ast::Declaration::Logic { name, .. } => Some(name.as_str()),
                    ast::Declaration::Enum {
                        var_name: Some(v), ..
                    } => Some(v.as_str()),
                    _ => None,
                })
                .collect();

            for sig in &ann.signals {
                if input_ports.contains(sig.name.as_str()) {
                    eprintln!(
                        "warning: '{}' is an input port but listed under \"signals\" — \
                         move it to \"inputs\" for correct handling",
                        sig.name
                    );
                } else if !all_decls.contains(sig.name.as_str())
                    && !all_ports.contains(sig.name.as_str())
                {
                    eprintln!(
                        "warning: signal '{}' in sidecar not found in module declarations",
                        sig.name
                    );
                }
            }
            for inp in &ann.inputs {
                if !all_ports.contains(inp.name.as_str()) {
                    eprintln!(
                        "warning: input '{}' in sidecar not found in module ports",
                        inp.name
                    );
                }
            }
        }

        let config = annotation::merge_config(sidecar.as_ref(), &module);
        let mut warnings = Vec::new();
        warnings.append(&mut parse_warnings);
        let ir = to_ir_with_config(&module, options, &config, &mut warnings)?;

        let state_valuations = crate::adapter::emit::extract_state_valuations(&ir);

        let result = crate::adapter::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let property_count = ir.properties.len();

        // Phase A.3 step 3.6 — recompute the partition for telemetry.
        // The first-pass partition runs inside `build_kripke_with_config`
        // and informs which registers were dropped; we re-run here on
        // the same inputs to surface a `PartitionSummary` without
        // threading it through every Kripke-builder call site.
        let partition_summary = partition_summary_for_sv(&module, &config);

        Ok(AdapterOutput {
            sidecars: Vec::new(),
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::SystemVerilog,
                title: Some(module.name.clone()),
                signal_count: module.ports.len(),
                state_count: result.state_count,
                property_count,
            },
            state_valuations,
            transition_observations: Default::default(),
            partition_summary,
        })
    }

    /// Translate a multi-module sidecar into a composed AdapterOutput.
    ///
    /// Parses each referenced `.sv` file, builds a Kripke automaton per module,
    /// generates shared labels for connections, and emits a composed IR with
    /// a composition directive.
    pub fn translate_multi_module(
        sidecar_path: &std::path::Path,
        options: &AdapterOptions,
    ) -> Result<AdapterOutput, AdapterError> {
        let ann = annotation::load_multi_annotation(sidecar_path).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: e,
            location: None,
        })?;

        let sidecar_dir = sidecar_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();

        Self::translate_multi_module_inner(&ann, options, |source, module_name| {
            let sv_path = sidecar_dir.join(source);
            std::fs::read_to_string(&sv_path).map_err(|e| AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: format!(
                    "failed to read '{}' for module '{}': {e}",
                    sv_path.display(),
                    module_name
                ),
                location: None,
            })
        })
    }

    /// Translate a multi-module sidecar into a composed AdapterOutput (in-memory).
    ///
    /// Like [`translate_multi_module`] but takes the sidecar JSON and source
    /// contents directly — no filesystem access. Used by the API endpoint.
    ///
    /// `sources` maps source filenames (matching the sidecar's `"source"` fields)
    /// to their content strings.
    pub fn translate_multi_module_content(
        sidecar_json: &str,
        sources: &std::collections::HashMap<String, String>,
        options: &AdapterOptions,
    ) -> Result<AdapterOutput, AdapterError> {
        let ann: annotation::MultiModuleSvAnnotation =
            serde_json::from_str(sidecar_json).map_err(|e| AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: format!("Failed to parse multi-module sidecar: {e}"),
                location: None,
            })?;

        Self::translate_multi_module_inner(&ann, options, |source, module_name| {
            sources.get(source).cloned().ok_or_else(|| AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: format!("Source file '{source}' for module '{module_name}' not provided"),
                location: None,
            })
        })
    }

    /// Shared body for the disk-based and in-memory multi-module entry points.
    /// `get_source` is given (source_field, module_name) and must return the SV
    /// source string — disk read in one variant, HashMap lookup in the other.
    fn translate_multi_module_inner<F>(
        ann: &annotation::MultiModuleSvAnnotation,
        options: &AdapterOptions,
        get_source: F,
    ) -> Result<AdapterOutput, AdapterError>
    where
        F: Fn(&str, &str) -> Result<String, AdapterError>,
    {
        let mut all_automata: Vec<AutomatonSpec> = Vec::new();
        let mut all_warnings: Vec<AdapterWarning> = Vec::new();
        let mut all_state_valuations = std::collections::HashMap::new();
        let mut total_state_count = 0usize;
        let mut total_signal_count = 0usize;

        // Phase 1: Build each module's Kripke automaton independently
        for module_entry in &ann.modules {
            let sv_content = get_source(&module_entry.source, &module_entry.name)?;

            let (module, mut parse_warnings) =
                parser::parse_with_warnings(&sv_content).map_err(|e| AdapterError {
                    kind: e.kind,
                    message: format!("in module '{}': {}", module_entry.name, e.message),
                    location: e.location,
                })?;

            all_warnings.append(&mut parse_warnings);

            if module.name != module_entry.name {
                all_warnings.push(AdapterWarning {
                    kind: super::WarningKind::UnsupportedConstruct,
                    message: format!(
                        "sidecar module name '{}' does not match SV module '{}' in '{}'",
                        module_entry.name, module.name, module_entry.source
                    ),
                    location: None,
                });
            }

            // Build a per-module SvAnnotation to reuse merge_config
            let per_module_ann = build_per_module_annotation(module_entry, ann);
            let config = annotation::merge_config(Some(&per_module_ann), &module);

            let (automaton, _module_properties, state_count) =
                kripke::build_kripke_with_config(&module, &config, &mut all_warnings)?;

            // Override automaton name to match the module entry name
            let mut automaton = AutomatonSpec {
                name: module_entry.name.clone(),
                ..automaton
            };

            // Post-process: add shared output labels for connections where
            // this module is the driver. The output value is derived from
            // the source state's valuations (including combinational outputs
            // resolved via assign statements).
            annotate_driving_output_labels(
                &mut automaton,
                &module_entry.name,
                &ann.connections,
                &module,
            );

            // Extract state valuations for this automaton
            let ir_for_valuations = AdapterIR {
                metadata: Metadata {
                    title: module_entry.name.clone(),
                    source_format: SourceFormat::SystemVerilog,
                    description: None,
                    game_semantics: None,
                    known_status: None,
                },
                signals: vec![],
                automata: vec![automaton.clone()],
                compositions: vec![],
                properties: vec![],
                controller: None,
            };
            let valuations = crate::adapter::emit::extract_state_valuations(&ir_for_valuations);
            for (k, v) in valuations {
                all_state_valuations.insert(k, v);
            }

            total_state_count += state_count;
            total_signal_count += module.ports.len();
            all_automata.push(automaton);
        }

        // Phase 2: Build composition directive
        let comp_config = ann.composition.as_ref();
        let comp_name = comp_config
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "system".to_string());
        let comp_mode = comp_config
            .map(|c| c.mode.as_str())
            .unwrap_or("synchronous");

        let member_names: Vec<String> = ann.modules.iter().map(|m| m.name.clone()).collect();

        let composition = match comp_mode {
            "asynchronous" => super::ir::CompositionSpec::Asynchronous {
                name: comp_name.clone(),
                members: member_names,
            },
            _ => super::ir::CompositionSpec::Synchronous {
                name: comp_name.clone(),
                members: member_names,
            },
        };

        // Phase 3: Build properties targeting the composition
        let properties: Vec<PropertySpec> =
            resolve_sidecar_properties(&ann.properties, Some(comp_name.clone()));

        let property_count = properties.len();

        // Controller from first property (if any)
        let controller = properties.first().map(|p| ControllerSpec {
            name: "synth".to_string(),
            source_automaton: comp_name.clone(),
            formula_name: p.name.clone(),
        });

        let context_name = options.context_name.as_deref().unwrap_or(&comp_name);

        let ir = AdapterIR {
            metadata: Metadata {
                title: context_name.to_string(),
                source_format: SourceFormat::SystemVerilog,
                description: Some(format!(
                    "Multi-module composition of {} SystemVerilog modules",
                    ann.modules.len()
                )),
                game_semantics: None,
                known_status: None,
            },
            signals: vec![],
            automata: all_automata,
            compositions: vec![composition],
            properties,
            controller,
        };

        let result = crate::adapter::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        // Document B task B3 (custom-SV half): for each blackbox module
        // entry in the sidecar, parse its source to extract the port
        // list and auto-emit `<name>.interface.json` +
        // `<name>.gap_report.json` sidecars. The chaotic-stub semantics
        // are entirely conveyed by the emitted JSON; no Kripke is
        // built for the blackbox module (the user explicitly opted out
        // of modelling it).
        let mut sidecars = Vec::new();
        if !ann.blackbox_modules.is_empty() {
            let mut blackboxes = Vec::with_capacity(ann.blackbox_modules.len());
            for bb_entry in &ann.blackbox_modules {
                let sv_content = match get_source(&bb_entry.source, &bb_entry.name) {
                    Ok(s) => s,
                    Err(_e) => {
                        all_warnings.push(AdapterWarning {
                            kind: super::WarningKind::UnsupportedConstruct,
                            message: format!(
                                "blackbox module '{}' source '{}' could not be read; skipping sidecar emission for this module",
                                bb_entry.name, bb_entry.source
                            ),
                            location: None,
                        });
                        continue;
                    }
                };
                let (module, _parse_warnings) = match parser::parse_with_warnings(&sv_content) {
                    Ok(r) => r,
                    Err(e) => {
                        all_warnings.push(AdapterWarning {
                            kind: super::WarningKind::UnsupportedConstruct,
                            message: format!(
                                "blackbox module '{}' failed to parse: {}; skipping sidecar emission",
                                bb_entry.name, e.message
                            ),
                            location: None,
                        });
                        continue;
                    }
                };
                blackboxes.push(blackbox_interface_from_module(
                    &module,
                    &bb_entry.name,
                    &bb_entry.source,
                    &sv_content,
                ));
            }
            if !blackboxes.is_empty() {
                let opts = crate::contract::discover::DiscoverOptions {
                    force_controllable: &[],
                    force_uncontrollable: &[],
                    emit_fairness_gap: false,
                    corpus: None,
                };
                sidecars.extend(crate::contract::discover::build_blackbox_sidecars(
                    &blackboxes,
                    &opts,
                ));
            }
        }

        Ok(AdapterOutput {
            sidecars,
            ctxdsl: result.ctxdsl,
            warnings: all_warnings,
            source_info: SourceInfo {
                format: SourceFormat::SystemVerilog,
                title: Some(context_name.to_string()),
                signal_count: total_signal_count,
                state_count: total_state_count,
                property_count,
            },
            state_valuations: all_state_valuations,
            transition_observations: Default::default(),
            partition_summary: None,
        })
    }
}

/// Annotate transitions of a driving module with shared output labels.
///
/// For each connection where `module_name` is the `from` side, this function
/// looks at the source state's valuations for the output port and adds the
/// Build a `BlackBoxInterface` description from a parsed SV `Module`.
///
/// Used by `translate_multi_module_inner` when the sidecar's
/// `blackbox_modules` list names a module the user wants treated as a
/// closed-IP boundary — Document B task B3 custom-SV half. The
/// resulting `BlackBoxInterface` carries the same shape the yosys-side
/// auto-emission produces, so both pipelines feed the same JSON
/// downstream into `mununu contract discover` / `mununu contract gaps`.
fn blackbox_interface_from_module(
    module: &ast::Module,
    user_supplied_name: &str,
    source_path: &str,
    source_text: &str,
) -> crate::contract::discover::BlackBoxInterface {
    use crate::contract::discover::{BlackBoxInterface, PortDescriptor};
    use crate::controllability::BoundaryDirection;

    let ports: Vec<PortDescriptor> = module
        .ports
        .iter()
        .map(|p| {
            let direction = match p.direction {
                ast::PortDirection::Input => BoundaryDirection::Input,
                ast::PortDirection::Output => BoundaryDirection::Output,
                ast::PortDirection::Inout => BoundaryDirection::Inout,
            };
            PortDescriptor {
                name: p.name.clone(),
                direction,
                description: None,
            }
        })
        .collect();

    // Document D task D4 + Document A task A6: scan the raw SV
    // source for `@mununu_*` annotations (Verilog attribute syntax
    // plus `// @mununu_xxx` and `/* @mununu_xxx */` comments). The
    // custom-SV parser does not yet hand us the attributes on its
    // AST, so we scan the source text directly. Both the SV and the
    // yosys frontends feed the same `MununuAnnotation` shape into
    // `BlackBoxInterface.annotations`.
    let annotations = crate::mununu_annotations::extract_from_sv_source(source_text);

    BlackBoxInterface {
        name: user_supplied_name.to_string(),
        ports,
        source_file: Some(source_path.to_string()),
        source_line: None,
        annotations,
    }
}

/// shared label (e.g., `grant_1`) to each transition. This enables the
/// composition engine to synchronize with the receiving module.
///
/// Supports both registered outputs (directly in valuations) and combinational
/// outputs (resolved via `assign port = register` statements).
fn annotate_driving_output_labels(
    automaton: &mut AutomatonSpec,
    module_name: &str,
    connections: &[annotation::ConnectionSpec],
    module: &ast::Module,
) {
    // Find connections where this module is the driver
    let driving_connections: Vec<(&str, &str)> = connections
        .iter()
        .filter_map(|conn| {
            let (from_mod, from_port) = conn.parse_from()?;
            if from_mod == module_name {
                Some((from_port, from_port)) // shared label uses the driving port name
            } else {
                None
            }
        })
        .collect();

    if driving_connections.is_empty() {
        return;
    }

    // Build a lookup: state_name → valuations
    let state_valuations: std::collections::HashMap<
        &str,
        &std::collections::BTreeMap<String, String>,
    > = automaton
        .states
        .iter()
        .filter_map(|s| s.valuations.as_ref().map(|v| (s.name.as_str(), v)))
        .collect();

    // For each transition, compute driving output values using:
    // 1. Source state valuations (registers + combinational signals already computed)
    // 2. Input label values on this transition (parsed from label format "name_value")
    // 3. Assign expressions for outputs not directly in valuations
    for transition in &mut automaton.transitions {
        if let Some(valuations) = state_valuations.get(transition.source.as_str()) {
            for (port_name, label_prefix) in &driving_connections {
                // Try direct lookup in state valuations (works for registered outputs
                // and combinational signals already in state space)
                if let Some(value) = valuations.get(*port_name) {
                    let output_label = format!("{}_{}", label_prefix, value);
                    if !transition.labels.contains(&output_label) {
                        transition.labels.push(output_label);
                    }
                    continue;
                }

                // Fallback: evaluate the assign expression for this output port
                // using state valuations + input values from transition labels.
                // This handles `assign push = (state == SENDING) && !full` where
                // push depends on both register state and input signals.
                let output_value =
                    eval_assign_for_transition(port_name, valuations, &transition.labels, module);
                if let Some(value) = output_value {
                    let output_label = format!("{}_{}", label_prefix, value);
                    if !transition.labels.contains(&output_label) {
                        transition.labels.push(output_label);
                    }
                }
            }
        }
    }

    // Mark output labels as controllable (driving module owns them)
    for (_, label_prefix) in &driving_connections {
        let output_labels: HashSet<String> = automaton
            .transitions
            .iter()
            .flat_map(|t| t.labels.iter())
            .filter(|l| l.starts_with(label_prefix))
            .cloned()
            .collect();
        for label in output_labels {
            if !automaton.controllable_labels.contains(&label) {
                automaton.controllable_labels.push(label);
            }
        }
    }
}

/// Evaluate a combinational assign expression for a specific transition.
///
/// Builds a valuation from state variables + input labels on the transition,
/// then evaluates the assign expression to determine the output value.
///
/// Returns "T"/"F" for boolean outputs, or a numeric string for counters.
fn eval_assign_for_transition(
    port_name: &str,
    state_valuations: &std::collections::BTreeMap<String, String>,
    transition_labels: &[String],
    module: &ast::Module,
) -> Option<String> {
    use crate::adapter::domain::AbstractValue;

    // Find the assign for this port
    let assign = module
        .assigns
        .iter()
        .find(|a| a.target.name() == port_name)?;

    // Build a valuation map from state variables
    let mut values: std::collections::BTreeMap<String, AbstractValue> =
        std::collections::BTreeMap::new();
    for (k, v) in state_valuations {
        let av = if v == "T" {
            AbstractValue::Bool(true)
        } else if v == "F" {
            AbstractValue::Bool(false)
        } else if let Ok(n) = v.parse::<i64>() {
            AbstractValue::Counter(n)
        } else {
            AbstractValue::Variant(v.clone())
        };
        values.insert(k.clone(), av);
    }

    // Add input values from transition labels (format: "name_value")
    for label in transition_labels {
        if let Some(idx) = label.rfind('_') {
            let name = &label[..idx];
            let val_str = &label[idx + 1..];
            let av = if val_str == "T" {
                AbstractValue::Bool(true)
            } else if val_str == "F" {
                AbstractValue::Bool(false)
            } else if let Ok(n) = val_str.parse::<i64>() {
                AbstractValue::Counter(n)
            } else {
                AbstractValue::Variant(val_str.to_string())
            };
            values.insert(name.to_string(), av);
        }
    }

    // Evaluate the assign expression. Pass an empty registers slice: this is
    // an output-annotation pathway used after state-space construction, where
    // Variant operands have already been resolved into the state valuation
    // map. Phase 7's value_map lookup isn't needed here.
    let result = kripke::eval_expr_pub(&assign.value, &values, &[])?;
    Some(result.display_short())
}

/// Build a per-module `SvAnnotation` from a `ModuleEntry` and the multi-module
/// sidecar, incorporating connection-level abstractions into the input domains.
fn build_per_module_annotation(
    entry: &annotation::ModuleEntry,
    multi: &annotation::MultiModuleSvAnnotation,
) -> annotation::SvAnnotation {
    let mut inputs = entry.inputs.clone();

    // For each connection where this module is the receiver, add an input
    // annotation with the connection-level abstraction and a shared label name.
    for conn in &multi.connections {
        if let Some((to_mod, to_port)) = conn.parse_to()
            && to_mod == entry.name
        {
            // Check if the user already declared this input
            if inputs.iter().any(|i| i.name == to_port) {
                continue;
            }
            // Derive the shared label name from the driving port
            let shared_label = conn.parse_from().map(|(_, port)| port.to_string());

            // Add connection-derived input annotation
            inputs.push(annotation::InputAnnotation {
                name: to_port.to_string(),
                preserve: true,
                abstraction: conn.abstraction.clone(),
                bound: conn.bound,
                variants: conn.variants.clone(),
                value_map: conn.value_map.clone(),
                label_name: shared_label,
                init_policy: annotation::InitPolicy::Inherit,
            });
        }
    }

    // Merge discovered_values: module-local + cross-connection values that
    // affect this module's ports
    let mut discovered_values = entry.discovered_values.clone();
    for conn in &multi.connections {
        if let Some((from_mod, from_port)) = conn.parse_from()
            && let Some(disc) = multi
                .discovered_values
                .get(&format!("{from_mod}.{from_port}"))
        {
            // If this module is the receiver, map discovered values to the input port name
            if let Some((to_mod, to_port)) = conn.parse_to()
                && to_mod == entry.name
            {
                discovered_values.insert(to_port.to_string(), disc.clone());
            }
            // If this module is the driver, map discovered values to the output port name
            if from_mod == entry.name {
                discovered_values.insert(from_port.to_string(), disc.clone());
            }
        }
    }

    annotation::SvAnnotation {
        schema: Some("mununu_sv_annotation_v1".to_string()),
        module: entry.name.clone(),
        source: Some(entry.source.clone()),
        signals: entry.signals.clone(),
        inputs,
        controllable: entry.controllable.clone(),
        properties: vec![], // Properties come from the multi-module level
        discovered_values,
        parameters: entry.parameters.clone(),
    }
}

impl FormatAdapter for SystemVerilogAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        trimmed.contains("module")
            && (trimmed.contains("always_ff")
                || trimmed.contains("always_comb")
                || trimmed.contains("always @")
                || trimmed.contains("endmodule"))
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let (module, mut parse_warnings) = parser::parse_with_warnings(content)?;

        let mut warnings = Vec::new();
        warnings.append(&mut parse_warnings);
        // Build a minimal MergedConfig from the inline annotations so
        // the partition summary mirrors what build_kripke internally
        // computes. Mirrors the path inside translate_with_path.
        let config = annotation::merge_config(None, &module);
        let partition_summary = partition_summary_for_sv(&module, &config);
        let ir = to_ir(&module, options, &mut warnings)?;

        let state_valuations = crate::adapter::emit::extract_state_valuations(&ir);

        let result = crate::adapter::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let property_count = ir.properties.len();

        Ok(AdapterOutput {
            sidecars: Vec::new(),
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::SystemVerilog,
                title: Some(module.name.clone()),
                signal_count: module.ports.len(),
                state_count: result.state_count,
                property_count,
            },
            state_valuations,
            transition_observations: Default::default(),
            partition_summary,
        })
    }
}

/// Convert a parsed SystemVerilog module to AdapterIR.
fn to_ir(
    module: &Module,
    options: &AdapterOptions,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let module_name = options.context_name.as_deref().unwrap_or(&module.name);

    // Build the inline-merged config so auto-detected combinational outputs
    // (driven from `always_comb` or `assign`) flow into signal_domains and
    // the Kripke builder treats them correctly. Without this, an FSM that
    // emits a combinational output would silently drop the comb logic.
    let inline_config = annotation::merge_config(None, module);

    // Decide path: Kripke if forced, if there are inline domain annotations,
    // if there are auto-detected combinational outputs, or if no enum FSM is
    // found.
    let use_kripke = module.force_kripke
        || !module.domain_annotations.is_empty()
        || !inline_config.signal_domains.is_empty()
        || fsm::extract_fsm(module).is_none();

    if use_kripke {
        if !inline_config.signal_domains.is_empty() {
            return to_ir_kripke_with_config(module, module_name, &inline_config, warnings);
        }
        return to_ir_kripke(module, module_name, warnings);
    }

    // Extract FSM from always_ff + enum (existing path)
    let fsm = fsm::extract_fsm(module).ok_or_else(|| AdapterError {
        kind: AdapterErrorKind::UnsupportedConstruct,
        message: "Could not extract FSM: no typedef enum with always_ff case statement found"
            .to_string(),
        location: None,
    })?;

    // Estimate state space
    let state_bits = (fsm.states.len() as f64).log2().ceil() as usize;
    if state_bits > 18 {
        return Err(AdapterError {
            kind: AdapterErrorKind::StateSpaceOverflow,
            message: format!(
                "FSM has {} states ({} bits), exceeding the 18-bit limit",
                fsm.states.len(),
                state_bits
            ),
            location: None,
        });
    }
    if state_bits > 12 {
        warnings.push(AdapterWarning {
            kind: WarningKind::LargeStateSpace,
            message: format!(
                "FSM has {} states ({} bits) — synthesis may be slow",
                fsm.states.len(),
                state_bits
            ),
            location: None,
        });
    }

    // Determine controllability from ports
    let output_port_names: HashSet<String> = module
        .ports
        .iter()
        .filter(|p| p.direction == PortDirection::Output)
        .map(|p| p.name.clone())
        .collect();

    let input_port_names: HashSet<String> = module
        .ports
        .iter()
        .filter(|p| p.direction == PortDirection::Input)
        .map(|p| p.name.clone())
        .collect();

    // Build the AutomatonSpec from extracted FSM
    let states: Vec<StateSpec> = fsm
        .states
        .iter()
        .map(|s| StateSpec {
            name: s.name.clone(),
            is_initial: s.is_initial,
            valuations: None,
        })
        .collect();

    // Classify transition labels by controllability:
    // Transitions guarded by input signals are uncontrollable (environment-driven).
    // Unconditional transitions or those guarded by state are controllable.
    let mut all_labels: HashSet<String> = HashSet::new();
    let mut controllable_labels: Vec<String> = Vec::new();
    let mut seen_controllable: HashSet<String> = HashSet::new();

    let transitions: Vec<TransitionSpec> = fsm
        .transitions
        .iter()
        .map(|t| {
            all_labels.insert(t.label.clone());

            // A transition is controllable if its guard is NOT an input port
            let is_input_driven = t
                .guard
                .as_ref()
                .is_some_and(|g| guard_references_input(g, &input_port_names));

            if !is_input_driven && !seen_controllable.contains(&t.label) {
                controllable_labels.push(t.label.clone());
                seen_controllable.insert(t.label.clone());
            }

            TransitionSpec {
                source: t.source.clone(),
                target: t.target.clone(),
                labels: vec![t.label.clone()],
            }
        })
        .collect();

    let automaton = AutomatonSpec {
        name: module_name.to_string(),
        states,
        transitions,
        controllable_labels,
        internal_labels: vec![],
    };

    // Build properties from @mununu comments
    let properties: Vec<PropertySpec> = module
        .mununu_properties
        .iter()
        .map(|p| {
            let role = match p.kind {
                MununuPropertyKind::Ltl | MununuPropertyKind::Guarantee => PropertyRole::Guarantee,
                MununuPropertyKind::Assume => PropertyRole::Assumption,
            };
            PropertySpec {
                name: p.name.clone(),
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus(p.formula.clone()),
                role,
                over: None,
                description: None,
            }
        })
        .collect();

    // Controller spec from first property
    let controller = properties.first().map(|p| ControllerSpec {
        name: "synth".to_string(),
        source_automaton: module_name.to_string(),
        formula_name: p.name.clone(),
    });

    // Warn about unused output ports (info only)
    for port in &module.ports {
        if port.direction == PortDirection::Output && port.name != "clk" && port.name != "rst" {
            let used_in_transitions = fsm
                .transitions
                .iter()
                .any(|t| t.guard.as_ref().is_some_and(|g| g.contains(&port.name)));
            if !used_in_transitions {
                // Output port not referenced in FSM guards — this is normal
                // (outputs are typically driven by combinational logic)
            }
        }
    }

    let _ = &output_port_names; // suppress unused warning

    Ok(AdapterIR {
        metadata: Metadata {
            title: module_name.to_string(),
            source_format: SourceFormat::SystemVerilog,
            description: Some(format!(
                "Translated from SystemVerilog module '{}'",
                module.name
            )),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata: vec![automaton],
        compositions: vec![],
        properties,
        controller,
    })
}

/// Resolve sidecar `PropertyAnnotation` list into `PropertySpec` values,
/// handling both raw formulas and template references.
fn resolve_sidecar_properties(
    annotations: &[annotation::PropertyAnnotation],
    over: Option<String>,
) -> Vec<PropertySpec> {
    let registry = crate::adapter::templates::TemplateRegistry::builtin();
    annotations
        .iter()
        .filter_map(|p| {
            let role = match p.role.as_str() {
                "assumption" => PropertyRole::Assumption,
                "standalone" => PropertyRole::Standalone,
                _ => PropertyRole::Guarantee,
            };
            // Raw formula takes precedence over template_ref
            let formula_str = if let Some(f) = &p.formula {
                f.clone()
            } else if let Some(tref) = &p.template_ref {
                match registry.instantiate(tref) {
                    Ok(inst) => inst.formula,
                    Err(_) => return None,
                }
            } else {
                return None;
            };
            Some(PropertySpec {
                name: p.id.clone(),
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus(formula_str),
                role,
                over: over.clone(),
                description: None,
            })
        })
        .collect()
}

/// Check if a guard string references any input port name.
///
/// Uses word-boundary matching rather than `str::contains` to prevent false
/// positives when one port name is a substring of another identifier. Example:
/// a port named `req` should not match a guard that uses `request_count` (a
/// different signal). priority_roadmap §2.8 / Tier A2.
fn guard_references_input(guard: &str, input_ports: &HashSet<String>) -> bool {
    input_ports
        .iter()
        .any(|port| token_appears(guard, port.as_str()))
}

/// Return `true` iff `needle` appears in `haystack` as a complete identifier
/// token — bordered on both sides by either start/end of string or a non-
/// identifier character. SystemVerilog identifiers consist of `[A-Za-z0-9_$]`
/// (the `$` is for system tasks/functions; we treat it as an identifier
/// character for matching purposes).
fn token_appears(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut search_start = 0;
    while let Some(rel) = haystack[search_start..].find(needle) {
        let abs = search_start + rel;
        let end = abs + needle.len();
        let before_ok = abs == 0 || !is_ident_char(bytes[abs - 1]);
        let after_ok = end == bytes.len() || !is_ident_char(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        search_start = abs + 1;
    }
    false
}

#[inline]
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Convert a parsed SystemVerilog module to AdapterIR using a MergedConfig.
fn to_ir_with_config(
    module: &Module,
    options: &AdapterOptions,
    config: &annotation::MergedConfig,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let module_name = options.context_name.as_deref().unwrap_or(&module.name);

    let use_kripke = config.force_kripke
        || !config.signal_domains.is_empty()
        || fsm::extract_fsm(module).is_none();

    if use_kripke {
        return to_ir_kripke_with_config(module, module_name, config, warnings);
    }

    // Fall back to FSM path (delegates to original to_ir)
    to_ir(module, options, warnings)
}

/// Convert a parsed SystemVerilog module to AdapterIR via Kripke construction.
fn to_ir_kripke(
    module: &Module,
    module_name: &str,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let (automaton, properties, _state_count) = kripke::build_kripke(module, warnings)?;

    let controller = properties.first().map(|p| ControllerSpec {
        name: "synth".to_string(),
        source_automaton: module_name.to_string(),
        formula_name: p.name.clone(),
    });

    // Override automaton name to match context
    let automaton = AutomatonSpec {
        name: module_name.to_string(),
        ..automaton
    };

    Ok(AdapterIR {
        metadata: Metadata {
            title: module_name.to_string(),
            source_format: SourceFormat::SystemVerilog,
            description: Some(format!(
                "Kripke structure from SystemVerilog module '{}'",
                module.name
            )),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata: vec![automaton],
        compositions: vec![],
        properties,
        controller,
    })
}

/// Kripke construction using a MergedConfig (from sidecar or inline).
fn to_ir_kripke_with_config(
    module: &Module,
    module_name: &str,
    config: &annotation::MergedConfig,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let (automaton, properties, _state_count) =
        kripke::build_kripke_with_config(module, config, warnings)?;

    let controller = properties.first().map(|p| ControllerSpec {
        name: "synth".to_string(),
        source_automaton: module_name.to_string(),
        formula_name: p.name.clone(),
    });

    let automaton = AutomatonSpec {
        name: module_name.to_string(),
        ..automaton
    };

    Ok(AdapterIR {
        metadata: Metadata {
            title: module_name.to_string(),
            source_format: SourceFormat::SystemVerilog,
            description: Some(format!(
                "Kripke structure from SystemVerilog module '{}'",
                module.name
            )),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata: vec![automaton],
        compositions: vec![],
        properties,
        controller,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate_sv(sv: &str) -> AdapterOutput {
        let options = AdapterOptions::default();
        SystemVerilogAdapter::translate(sv, &options).expect("translation should succeed")
    }

    // ---------------------------------------------------------------
    // Kripke path integration tests
    // ---------------------------------------------------------------

    #[test]
    fn kripke_counter_module_translates() {
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            module counter(
                input logic clk, input logic rst,
                input logic en
            );
                logic [1:0] count;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) count <= 0;
                    else if (en) count <= count + 1;
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        assert_eq!(output.source_info.format, SourceFormat::SystemVerilog);
        assert_eq!(output.source_info.property_count, 1);
        assert!(output.ctxdsl.contains("automaton counter"));
        assert!(output.ctxdsl.contains("count_0"));
    }

    #[test]
    fn kripke_counter_parses_and_realizes() {
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            module counter(
                input logic clk, input logic rst,
                input logic en
            );
                logic [1:0] count;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) count <= 0;
                    else if (en) count <= count + 1;
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("counter").expect("counter automaton");
        // 2-bit counter = 4 in-domain states, plus 1 OOB sink for the
        // unguarded `count <= count + 1` overflow at count=3.
        assert!(clts.state_count() <= 5);
    }

    #[test]
    fn kripke_mixed_enum_and_counter() {
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            // @mununu domain retry_count: bounded_counter 0..2
            module retry(
                input logic clk, input logic rst,
                input logic req, input logic ack
            );
                typedef enum logic [1:0] {IDLE, TRYING, DONE} state_t;
                state_t state;
                logic [7:0] retry_count;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) begin
                        state <= IDLE;
                        retry_count <= 0;
                    end
                    else case (state)
                        IDLE: if (req) begin state <= TRYING; retry_count <= 0; end
                        TRYING: begin
                            if (ack) state <= DONE;
                            else retry_count <= retry_count + 1;
                        end
                        DONE: state <= IDLE;
                    endcase
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("retry").expect("retry automaton");
        // 3 enum states × 3 counter values = 9 max, but pruned by reachability;
        // plus up to 1 OOB sink if any unguarded counter increment overflows.
        assert!(clts.state_count() <= 10);
        assert!(clts.state_count() > 0);
    }

    #[test]
    fn kripke_fallback_when_no_enum() {
        // No typedef enum → should fall back to Kripke path automatically
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            module toggle(
                input logic clk, input logic rst,
                input logic trigger
            );
                logic flag;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) flag <= 0;
                    else if (trigger) flag <= !flag;
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("toggle").expect("toggle automaton");
        assert_eq!(clts.state_count(), 2); // flag=0, flag=1
    }

    #[test]
    fn kripke_domain_annotation_overrides() {
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            // @mununu domain data: ignored
            module proc(
                input logic clk, input logic rst
            );
                logic valid;
                logic [31:0] data;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) begin valid <= 0; data <= 0; end
                    else valid <= 1;
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        // data is ignored → state space is just valid (2 states)
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("proc").expect("proc automaton");
        assert_eq!(clts.state_count(), 2);
    }

    // ---------------------------------------------------------------
    // ALU and FIFO examples (Kripke path, non-trivial properties)
    // ---------------------------------------------------------------

    #[test]
    fn kripke_alu_example() {
        let sv = include_str!("../../../../../examples/systemverilog/alu.sv");
        let output = translate_sv(sv);

        // Must translate and parse
        let doc = crate::context_dsl::parse(&output.ctxdsl)
            .unwrap_or_else(|e| panic!("CTXDSL parse failed:\n{}\n\nError: {e}", output.ctxdsl));
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("alu").expect("alu automaton");

        // acc: 0..7 (8 values), cmd: 6 variants, operand: 0..3 (4 values)
        // But COI + reachability should prune significantly
        assert!(clts.state_count() > 0, "ALU should have reachable states");

        // Safety should be realizable (no deadlock, well-formed)
        let formula = realized.formulas.get("safety").expect("safety formula");
        let env = realized.environment_for("alu");
        let synth = realized
            .context
            .synthesise_controller("alu", &formula.formula, &env, None)
            .expect("synthesis should succeed");
        // The ALU has a reachable OOB sink (acc overflow). Once Phase C/4 emits
        // the OOB-marker valuation through CTXDSL, the mu-calculus evaluator
        // recognises the sink as a bottom state, and `nu X. ([] X)` does not
        // hold from `acc_0` because some path leads to the OOB sink. Synth
        // therefore reports unrealizable — the *right* answer for the ALU as
        // written. To make it realizable the user would need a wider `acc`
        // bound or a guard that prevents the overflowing transitions.
        assert!(
            !synth.realizable,
            "ALU safety should be unrealizable due to reachable OOB sink (states: {})",
            clts.state_count()
        );
    }

    #[test]
    fn kripke_fifo_example() {
        let sv = include_str!("../../../../../examples/systemverilog/fifo.sv");
        let output = translate_sv(sv);

        let mut doc = crate::context_dsl::parse(&output.ctxdsl)
            .unwrap_or_else(|e| panic!("CTXDSL parse failed:\n{}\n\nError: {e}", output.ctxdsl));
        // Inject structured valuations from the adapter output
        doc.state_valuations = output.state_valuations.clone();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("fifo").expect("fifo automaton");

        // state: 4 variants, fill: 0..4 (5 values) = 20 max before pruning
        assert!(
            clts.state_count() > 1,
            "FIFO should have multiple reachable states"
        );
        assert_eq!(
            clts.state_count(),
            20,
            "FIFO: 4 states x 5 fill levels = 20"
        );

        // Verify structured valuations are present on the CLTS
        assert!(
            clts.has_valuations(),
            "FIFO CLTS should have structured valuations"
        );
        // Verify a specific state's valuation
        let s0 = clts.state_id("fill_0_state_IDLE").unwrap();
        let val = clts
            .state_valuation(s0)
            .expect("state should have valuation");
        assert_eq!(val.get("fill").map(|s| s.as_str()), Some("0"));
        assert_eq!(val.get("state").map(|s| s.as_str()), Some("IDLE"));

        // Safety should be realizable
        let formula = realized.formulas.get("safety").expect("safety formula");
        let env = realized.environment_for("fifo");
        let synth = realized
            .context
            .synthesise_controller("fifo", &formula.formula, &env, None)
            .expect("synthesis should succeed");
        assert!(
            synth.realizable,
            "FIFO safety should be realizable (states: {})",
            clts.state_count()
        );
    }

    // ---------------------------------------------------------------
    // Sidecar pipeline tests (MergedConfig-driven)
    // ---------------------------------------------------------------

    #[test]
    fn sidecar_bounded_counter_register() {
        // A sidecar that preserves a 2-bit counter, no inline annotations
        let sv = r#"
            module cnt(input logic clk, input logic rst, input logic en);
                logic [1:0] count;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) count <= 0;
                    else if (en) count <= count + 1;
                end
            endmodule
        "#;

        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(
            r#"{
            "module": "cnt",
            "signals": [
                {"name": "count", "abstraction": "bounded_counter", "bound": 3}
            ],
            "inputs": [
                {"name": "en", "abstraction": "boolean"}
            ],
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)"}
            ]
        }"#,
        )
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let (automaton, properties, _) =
            kripke::build_kripke_with_config(&module, &config, &mut warnings).unwrap();

        assert_eq!(properties.len(), 1);
        // count: 0..3 = 4 in-domain states, plus up to 1 OOB sink for overflow.
        assert!(automaton.states.len() <= 5);
        assert!(!automaton.states.is_empty());
        assert!(automaton.states.iter().any(|s| s.is_initial));
    }

    #[test]
    fn sidecar_discover_uses_discovered_values() {
        // A sidecar with discover + pre-populated discovered_values
        let sv = r#"
            module alu2(
                input logic clk, input logic rst,
                input logic start,
                input logic [7:0] op,
                output logic [3:0] result
            );
                logic [3:0] acc;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) acc <= 0;
                    else if (start) begin
                        case (op)
                            0: ;
                            1: acc <= 5;
                            default: ;
                        endcase
                    end
                end
                assign result = acc;
            endmodule
        "#;

        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(
            r#"{
            "module": "alu2",
            "signals": [
                {"name": "acc", "abstraction": "bounded_counter", "bound": 7}
            ],
            "inputs": [
                {"name": "start", "abstraction": "boolean"},
                {"name": "op", "abstraction": "discover"}
            ],
            "discovered_values": {
                "op": {
                    "values": [
                        {"value": 0, "name": "NOP"},
                        {"value": 1, "name": "SET5"}
                    ],
                    "catch_all": "OTHER"
                }
            },
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)"}
            ]
        }"#,
        )
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let (automaton, properties, _) =
            kripke::build_kripke_with_config(&module, &config, &mut warnings).unwrap();

        // acc: 0..7 (8 states), but reachability prunes — at least acc_0 and acc_5
        assert!(
            automaton.states.len() >= 2,
            "Should have at least acc_0 and acc_5 (from SET5)"
        );
        assert_eq!(properties.len(), 1);

        // Check that acc_5 is reachable (from op=SET5)
        assert!(
            automaton.states.iter().any(|s| s.name.contains("5")),
            "acc_5 should be reachable via SET5 opcode"
        );
    }

    #[test]
    fn sidecar_excluded_signals_not_in_state_space() {
        let sv = r#"
            module proc(input logic clk, input logic rst);
                logic flag;
                logic [31:0] data;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) begin flag <= 0; data <= 0; end
                    else flag <= 1;
                end
            endmodule
        "#;

        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(
            r#"{
            "module": "proc",
            "signals": [
                {"name": "flag", "abstraction": "boolean"},
                {"name": "data", "preserve": false}
            ],
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)"}
            ]
        }"#,
        )
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let (automaton, _, _) =
            kripke::build_kripke_with_config(&module, &config, &mut warnings).unwrap();

        // Only flag (boolean, 2 states), data is excluded
        assert_eq!(automaton.states.len(), 2);
    }

    #[test]
    fn sidecar_properties_override_inline() {
        let sv = r#"
            // @mununu ltl inline_prop: nu X. ([] X)
            // @mununu mode kripke
            module test(input logic clk, input logic rst);
                logic flag;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) flag <= 0;
                    else flag <= 1;
                end
            endmodule
        "#;

        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [
                {"name": "flag", "abstraction": "boolean"}
            ],
            "properties": [
                {"id": "sidecar_prop", "formula": "nu X. ([] X)"}
            ]
        }"#,
        )
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);

        // Sidecar properties should be used, not inline
        assert_eq!(config.properties.len(), 1);
        assert_eq!(config.properties[0].id, "sidecar_prop");
    }

    #[test]
    fn sidecar_parameters_override_module() {
        let sv = r#"
            module test(input logic clk, input logic rst);
                localparam DEPTH = 4;
                logic [2:0] fill;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) fill <= 0;
                    else if (fill < DEPTH) fill <= fill + 1;
                end
            endmodule
        "#;

        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [
                {"name": "fill", "abstraction": "bounded_counter", "bound": 2}
            ],
            "parameters": {"DEPTH": 2},
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)"}
            ]
        }"#,
        )
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let (automaton, _, _) =
            kripke::build_kripke_with_config(&module, &config, &mut warnings).unwrap();

        // fill bounded to 0..2 (3 values), DEPTH overridden to 2
        // fill goes 0 → 1 → 2 → 2 (clamped)
        assert!(automaton.states.len() <= 3);
        assert!(!automaton.states.is_empty());
    }

    #[test]
    fn inline_still_works_without_sidecar() {
        // Inline annotations only — no sidecar
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            // @mununu domain count: bounded_counter 0..3
            module cnt(input logic clk, input logic rst, input logic en);
                logic [3:0] count;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) count <= 0;
                    else if (en) count <= count + 1;
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("cnt").expect("cnt automaton");
        // Default counter bound (3) → 4 in-domain states, plus up to 1 OOB sink
        // for overflow on the unguarded `count <= count + 1`.
        assert!(clts.state_count() <= 5);
        assert!(clts.state_count() > 0);
    }

    // ---------------------------------------------------------------
    // Bug detection: buggy version fails, fixed version passes
    // ---------------------------------------------------------------

    #[test]
    fn fifo_overflow_bug_detected() {
        // Buggy FIFO: no guard on write → fill can exceed DEPTH
        let sv = include_str!("../../../../../examples/systemverilog/fifo_overflow_bug.sv");
        let module = parser::parse(sv).unwrap();

        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/fifo_overflow_bug.mununu.json"
        ))
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);
        let options = AdapterOptions::default();
        let mut warnings = Vec::new();
        let ir = to_ir_with_config(&module, &options, &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();

        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();

        // no_overflow should be unrealizable (the bug is real)
        let formula = realized
            .formulas
            .get("no_overflow")
            .expect("no_overflow formula");
        let env = realized.environment_for("fifo_overflow_bug");
        let synth = realized
            .context
            .synthesise_controller("fifo_overflow_bug", &formula.formula, &env, None)
            .expect("synthesis should succeed");

        assert!(
            !synth.realizable,
            "Buggy FIFO should be UNREALIZABLE for no_overflow (overflow is reachable)"
        );
    }

    #[test]
    fn fifo_overflow_fix_verified() {
        // Fixed FIFO: guard prevents write when full
        let sv = include_str!("../../../../../examples/systemverilog/fifo_overflow_fixed.sv");
        let module = parser::parse(sv).unwrap();

        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/fifo_overflow_fixed.mununu.json"
        ))
        .unwrap();

        let config = annotation::merge_config(Some(&ann), &module);
        let options = AdapterOptions::default();
        let mut warnings = Vec::new();
        let ir = to_ir_with_config(&module, &options, &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();

        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();

        // no_overflow should be realizable (the fix works)
        let formula = realized
            .formulas
            .get("no_overflow")
            .expect("no_overflow formula");
        let env = realized.environment_for("fifo_overflow_fixed");
        let synth = realized
            .context
            .synthesise_controller("fifo_overflow_fixed", &formula.formula, &env, None)
            .expect("synthesis should succeed");

        assert!(
            synth.realizable,
            "Fixed FIFO should be REALIZABLE for no_overflow (guard prevents overflow)"
        );
    }

    // ---------------------------------------------------------------
    // AXI-Lite deadlock (Xilinx bug)
    // ---------------------------------------------------------------

    #[test]
    fn axilite_overlap_bug_detected() {
        let sv = include_str!("../../../../../examples/systemverilog/axilite_deadlock_bug.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/axilite_deadlock_bug.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let formula = realized.formulas.get("no_overlap").unwrap();
        let env = realized.environment_for("axilite_deadlock_bug");
        let synth = realized
            .context
            .synthesise_controller("axilite_deadlock_bug", &formula.formula, &env, None)
            .unwrap();
        assert!(
            !synth.realizable,
            "Buggy AXI-lite should be UNREALIZABLE for no_overlap"
        );
    }

    #[test]
    fn axilite_overlap_fix_verified() {
        let sv = include_str!("../../../../../examples/systemverilog/axilite_deadlock_fixed.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/axilite_deadlock_fixed.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let formula = realized.formulas.get("no_overlap").unwrap();
        let env = realized.environment_for("axilite_deadlock_fixed");
        let synth = realized
            .context
            .synthesise_controller("axilite_deadlock_fixed", &formula.formula, &env, None)
            .unwrap();
        assert!(
            synth.realizable,
            "Fixed AXI-lite should be REALIZABLE for no_overlap"
        );
    }

    // ---------------------------------------------------------------
    // CWE-1245: FSM with undefined states
    // ---------------------------------------------------------------

    #[test]
    fn cwe1245_undefined_state_bug() {
        let sv = include_str!("../../../../../examples/systemverilog/cwe1245_fsm_bug.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/cwe1245_fsm_bug.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();

        // recoverable: UNDEF state cannot reach IDLE (bug)
        let formula = realized.formulas.get("recoverable").unwrap();
        let clts = realized.context.clts("cwe1245_fsm_bug").unwrap();
        let env = realized.environment_for("cwe1245_fsm_bug");
        let sat = realized
            .context
            .evaluate_mu("cwe1245_fsm_bug", &formula.formula, &env, None)
            .unwrap();
        // Not all states satisfy recoverable
        assert!(
            sat.count_ones() < clts.state_count(),
            "Buggy CWE-1245: UNDEF should NOT satisfy recoverable (stuck state)"
        );
    }

    #[test]
    fn cwe1245_undefined_state_fixed() {
        let sv = include_str!("../../../../../examples/systemverilog/cwe1245_fsm_fixed.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/cwe1245_fsm_fixed.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();

        // recoverable: ALL states (including UNDEF) can reach IDLE (fixed)
        let formula = realized.formulas.get("recoverable").unwrap();
        let clts = realized.context.clts("cwe1245_fsm_fixed").unwrap();
        let env = realized.environment_for("cwe1245_fsm_fixed");
        let sat = realized
            .context
            .evaluate_mu("cwe1245_fsm_fixed", &formula.formula, &env, None)
            .unwrap();
        assert_eq!(
            sat.count_ones(),
            clts.state_count(),
            "Fixed CWE-1245: ALL states should satisfy recoverable"
        );
    }

    // ---------------------------------------------------------------
    // CWE-1260: Memory region address overlap
    // ---------------------------------------------------------------

    #[test]
    fn cwe1260_overlap_bug_detected() {
        let sv = include_str!("../../../../../examples/systemverilog/cwe1260_addr_overlap_bug.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/cwe1260_addr_overlap_bug.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let formula = realized.formulas.get("no_overlap").unwrap();
        let env = realized.environment_for("cwe1260_addr_overlap_bug");
        let synth = realized
            .context
            .synthesise_controller("cwe1260_addr_overlap_bug", &formula.formula, &env, None)
            .unwrap();
        assert!(
            !synth.realizable,
            "CWE-1260 buggy: UNREALIZABLE (overlap reachable)"
        );
    }

    #[test]
    fn cwe1260_overlap_fix_verified() {
        let sv =
            include_str!("../../../../../examples/systemverilog/cwe1260_addr_overlap_fixed.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/cwe1260_addr_overlap_fixed.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let formula = realized.formulas.get("no_overlap").unwrap();
        let env = realized.environment_for("cwe1260_addr_overlap_fixed");
        let synth = realized
            .context
            .synthesise_controller("cwe1260_addr_overlap_fixed", &formula.formula, &env, None)
            .unwrap();
        assert!(synth.realizable, "CWE-1260 fixed: REALIZABLE (no overlap)");
    }

    // ---------------------------------------------------------------
    // CWE-1262: CSR privilege bypass
    // ---------------------------------------------------------------

    #[test]
    fn cwe1262_bypass_bug_detected() {
        let sv = include_str!("../../../../../examples/systemverilog/cwe1262_csr_bypass_bug.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/cwe1262_csr_bypass_bug.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let formula = realized.formulas.get("no_bypass").unwrap();
        let env = realized.environment_for("cwe1262_csr_bypass_bug");
        let synth = realized
            .context
            .synthesise_controller("cwe1262_csr_bypass_bug", &formula.formula, &env, None)
            .unwrap();
        assert!(
            !synth.realizable,
            "CWE-1262 buggy: UNREALIZABLE (MEPC bypass)"
        );
    }

    #[test]
    fn cwe1262_bypass_fix_verified() {
        let sv = include_str!("../../../../../examples/systemverilog/cwe1262_csr_bypass_fixed.sv");
        let module = parser::parse(sv).unwrap();
        let ann: annotation::SvAnnotation = serde_json::from_str(include_str!(
            "../../../../../examples/systemverilog/cwe1262_csr_bypass_fixed.mununu.json"
        ))
        .unwrap();
        let config = annotation::merge_config(Some(&ann), &module);
        let mut warnings = Vec::new();
        let ir =
            to_ir_with_config(&module, &AdapterOptions::default(), &config, &mut warnings).unwrap();
        let result = crate::adapter::emit::emit(&ir).unwrap();
        let doc = crate::context_dsl::parse(&result.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let formula = realized.formulas.get("no_bypass").unwrap();
        let env = realized.environment_for("cwe1262_csr_bypass_fixed");
        let synth = realized
            .context
            .synthesise_controller("cwe1262_csr_bypass_fixed", &formula.formula, &env, None)
            .unwrap();
        assert!(
            synth.realizable,
            "CWE-1262 fixed: REALIZABLE (uniform check)"
        );
    }

    // ---------------------------------------------------------------
    // Existing FSM path tests
    // ---------------------------------------------------------------

    #[test]
    fn detect_systemverilog() {
        assert!(SystemVerilogAdapter::detect(
            "module test(); always_ff @(posedge clk) begin end endmodule"
        ));
        assert!(!SystemVerilogAdapter::detect(
            r#"{"id": "test", "states": {}}"#
        ));
        assert!(!SystemVerilogAdapter::detect("aag 0 0 0 0 0"));
    }

    #[test]
    fn translate_simple_fsm() {
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            module handshake(
                input logic clk, input logic rst,
                input logic req,
                output logic ack
            );
                typedef enum logic [1:0] {IDLE, WAIT, ACTIVE, DONE} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                    else case (state)
                        IDLE: if (req) state <= WAIT;
                        WAIT: state <= ACTIVE;
                        ACTIVE: if (!req) state <= DONE;
                        DONE: state <= IDLE;
                    endcase
                end
                assign ack = (state == ACTIVE);
            endmodule
        "#;

        let output = translate_sv(sv);
        assert_eq!(output.source_info.format, SourceFormat::SystemVerilog);
        assert_eq!(output.source_info.property_count, 1);

        // Verify CTXDSL can be parsed and realized
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized
            .context
            .clts("handshake")
            .expect("handshake automaton");
        assert_eq!(clts.state_count(), 4);
    }

    #[test]
    fn translate_and_synthesize() {
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            module arbiter(
                input logic clk, input logic rst,
                input logic req_a, input logic req_b,
                output logic grant_a, output logic grant_b
            );
                typedef enum logic [1:0] {IDLE, GRANT_A, GRANT_B} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                    else case (state)
                        IDLE: begin
                            if (req_a) state <= GRANT_A;
                            else if (req_b) state <= GRANT_B;
                        end
                        GRANT_A: if (!req_a) state <= IDLE;
                        GRANT_B: if (!req_b) state <= IDLE;
                    endcase
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl).unwrap();
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();

        let formula = realized.formulas.get("safety").expect("safety formula");
        let env = realized.environment_for("arbiter");
        let synth = realized
            .context
            .synthesise_controller("arbiter", &formula.formula, &env, None)
            .expect("synthesis should succeed");
        assert!(synth.realizable, "Arbiter safety should be realizable");
    }

    // ---------------------------------------------------------------
    // Tier 1: Package + struct integration tests
    // ---------------------------------------------------------------

    #[test]
    fn package_enum_cva6_style() {
        // CVA6-style: package with privilege level enum, imported into module
        let sv = r#"
            package riscv;
                typedef enum logic [1:0] {USER, SUPERVISOR, MACHINE} priv_lvl_t;
            endpackage

            // @mununu ltl no_bypass: nu X. ((!illegal_access || [] false) && [] X)
            // @mununu mode kripke
            module csr_check(
                input logic clk, input logic rst,
                input logic write_req
            );
                import riscv::*;
                priv_lvl_t priv_lvl;
                logic illegal_access;

                always_ff @(posedge clk or posedge rst) begin
                    if (rst) begin
                        priv_lvl <= USER;
                        illegal_access <= 0;
                    end else begin
                        if (write_req) begin
                            if (priv_lvl == MACHINE)
                                illegal_access <= 0;
                            else
                                illegal_access <= 1;
                        end
                    end
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl)
            .unwrap_or_else(|e| panic!("Parse failed:\n{}\n{e}", output.ctxdsl));
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("csr_check").expect("csr_check");

        // 3 priv levels × 2 illegal_access = 6 max states
        assert!(clts.state_count() > 0 && clts.state_count() <= 6);
    }

    #[test]
    fn struct_packed_field_access_kripke() {
        // Struct with field writes and reads through the full Kripke pipeline
        let sv = r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            // @mununu domain pkt: bounded_counter 0..31
            module pkt_proc(
                input logic clk, input logic rst,
                input logic send
            );
                typedef struct packed {
                    logic [2:0] tag;
                    logic [1:0] len;
                } pkt_t;
                pkt_t pkt;

                always_ff @(posedge clk or posedge rst) begin
                    if (rst) pkt <= 0;
                    else if (send) begin
                        pkt.tag <= pkt.tag + 1;
                    end
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl)
            .unwrap_or_else(|e| panic!("Parse failed:\n{}\n{e}", output.ctxdsl));
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("pkt_proc").expect("pkt_proc");

        // pkt is a 5-bit counter (tag:3 + len:2), bounded to 0..31 = 32 states max
        assert!(
            clts.state_count() > 0,
            "pkt_proc should have reachable states"
        );

        // Safety (no deadlock) should be realizable
        let formula = realized.formulas.get("safety").expect("safety formula");
        let env = realized.environment_for("pkt_proc");
        let synth = realized
            .context
            .synthesise_controller("pkt_proc", &formula.formula, &env, None)
            .expect("synthesis should succeed");
        assert!(synth.realizable, "pkt_proc safety should be realizable");
    }

    #[test]
    fn struct_from_package_kripke() {
        // Struct defined in package, imported, field access in expressions
        let sv = r#"
            package bus_pkg;
                typedef struct packed {
                    logic [3:0] addr;
                    logic       valid;
                } req_t;
            endpackage

            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            // @mununu domain req: bounded_counter 0..31
            module bus_ctrl(
                input logic clk, input logic rst,
                input logic start
            );
                import bus_pkg::*;
                req_t req;

                always_ff @(posedge clk or posedge rst) begin
                    if (rst) req <= 0;
                    else if (start) begin
                        req.valid <= 1;
                        req.addr <= 5;
                    end
                end
            endmodule
        "#;

        let output = translate_sv(sv);
        let doc = crate::context_dsl::parse(&output.ctxdsl)
            .unwrap_or_else(|e| panic!("Parse failed:\n{}\n{e}", output.ctxdsl));
        let realized = crate::context_dsl::realize_context(&doc, &[]).unwrap();
        let clts = realized.context.clts("bus_ctrl").expect("bus_ctrl");

        assert!(
            clts.state_count() > 0,
            "bus_ctrl should have reachable states"
        );

        // After start: req.valid=1, req.addr=5 → req = (5 << 1) | 1 = 11
        // Initial: req = 0
        // Should have at least 2 reachable states
        assert!(
            clts.state_count() >= 2,
            "Should have at least initial and post-start states"
        );

        let formula = realized.formulas.get("safety").expect("safety formula");
        let env = realized.environment_for("bus_ctrl");
        let synth = realized
            .context
            .synthesise_controller("bus_ctrl", &formula.formula, &env, None)
            .expect("synthesis should succeed");
        assert!(synth.realizable, "bus_ctrl safety should be realizable");
    }

    /// Tier A2 — substring guard matching fix (priority_roadmap §2.8).
    ///
    /// `token_appears` must respect identifier boundaries: a port named `req`
    /// should not match `request_count` or `xreq_y`, but should match `req`,
    /// `req == 1`, `(req)`, etc.
    #[test]
    fn guard_token_match_respects_identifier_boundaries() {
        // exact match
        assert!(token_appears("req", "req"));
        // word in expression
        assert!(token_appears("req == 1", "req"));
        assert!(token_appears("(req)", "req"));
        assert!(token_appears("!req && rdy", "req"));
        // not a substring of another identifier
        assert!(!token_appears("request_count", "req"));
        assert!(!token_appears("xreq", "req"));
        assert!(!token_appears("xreq_y", "req"));
        assert!(!token_appears("$reqister", "req"));
        // empty needle
        assert!(!token_appears("anything", ""));
    }

    #[test]
    fn guard_references_input_uses_token_boundaries() {
        let mut inputs = HashSet::new();
        inputs.insert("req".to_string());
        // Real port reference
        assert!(guard_references_input("req == 1", &inputs));
        // Substring should NOT match
        assert!(!guard_references_input("request_count > 0", &inputs));
        // Multi-port: only one needs to match
        inputs.insert("ack".to_string());
        assert!(guard_references_input("ack && full", &inputs));
        // No match at all
        assert!(!guard_references_input("counter < 3", &inputs));
    }

    // ---------------------------------------------------------------
    // Document B task B3 (custom-SV half) — blackbox sidecar emission
    // ---------------------------------------------------------------

    #[test]
    fn translate_multi_module_emits_sidecars_for_blackbox_entries() {
        use crate::adapter::SidecarOrigin;
        use std::collections::HashMap;

        // Real module with a Kripke. Empty `always_ff` keeps the
        // parser happy; we only care that translation succeeds.
        let real_sv = r#"
            module real_thing(input clk, input req, output reg ack);
                always_ff @(posedge clk) begin
                    if (req) ack <= 1;
                    else ack <= 0;
                end
            endmodule
        "#;
        // Blackbox module — body could be anything, only the port
        // list matters for the sidecar.
        let bb_sv = r#"
            module vendor_ip(
                input clk,
                input start,
                output ready,
                output done
            );
            endmodule
        "#;

        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_multi_v1",
            "modules": [
                { "name": "real_thing", "source": "real.sv" }
            ],
            "blackbox_modules": [
                { "name": "vendor_ip", "source": "vendor.sv" }
            ]
        });
        let mut sources = HashMap::new();
        sources.insert("real.sv".to_string(), real_sv.to_string());
        sources.insert("vendor.sv".to_string(), bb_sv.to_string());

        let out = SystemVerilogAdapter::translate_multi_module_content(
            &sidecar.to_string(),
            &sources,
            &AdapterOptions::default(),
        )
        .expect("multi-module translate should succeed");

        let iface = out
            .sidecars
            .iter()
            .find(|s| s.origin == SidecarOrigin::BlackBoxInterface)
            .expect("expected an interface sidecar");
        assert_eq!(iface.filename, "vendor_ip.interface.json");
        assert!(iface.content.contains("\"name\": \"vendor_ip\""));
        assert!(iface.content.contains("\"direction\": \"Input\""));
        assert!(iface.content.contains("\"direction\": \"Output\""));
        // The source_file should be the path the user supplied, not the
        // module name.
        assert!(iface.content.contains("\"source_file\": \"vendor.sv\""));

        let gap = out
            .sidecars
            .iter()
            .find(|s| s.origin == SidecarOrigin::BlackBoxGapReport)
            .expect("expected a gap-report sidecar");
        assert_eq!(gap.filename, "vendor_ip.gap_report.json");
        assert!(gap.content.contains("output_sequencing"));
    }

    #[test]
    fn translate_multi_module_warns_on_unreadable_blackbox_source() {
        use std::collections::HashMap;

        let real_sv = r#"
            module real_thing(input clk, output reg ack);
                always_ff @(posedge clk) ack <= 1;
            endmodule
        "#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_multi_v1",
            "modules": [
                { "name": "real_thing", "source": "real.sv" }
            ],
            "blackbox_modules": [
                // Source is intentionally missing from the sources map.
                { "name": "vendor_ip", "source": "does_not_exist.sv" }
            ]
        });
        let mut sources = HashMap::new();
        sources.insert("real.sv".to_string(), real_sv.to_string());

        let out = SystemVerilogAdapter::translate_multi_module_content(
            &sidecar.to_string(),
            &sources,
            &AdapterOptions::default(),
        )
        .expect("multi-module translate should not fail on missing bb source");
        assert!(out.sidecars.is_empty());
        let has_warning = out.warnings.iter().any(|w| {
            w.message.contains("blackbox module 'vendor_ip'")
                && w.message.contains("could not be read")
        });
        assert!(has_warning, "expected a warning about the missing source");
    }
}
