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

use super::ir::*;
use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo, WarningKind,
};
use ast::{Module, MununuPropertyKind, PortDirection};
use std::collections::HashSet;

/// SystemVerilog adapter implementing [`FormatAdapter`].
pub struct SystemVerilogAdapter;

impl SystemVerilogAdapter {
    /// Translate with an explicit file path, enabling `.mununu.json` sidecar loading.
    pub fn translate_with_path(
        content: &str,
        options: &AdapterOptions,
        sv_path: &std::path::Path,
    ) -> Result<AdapterOutput, AdapterError> {
        let module = parser::parse(content)?;

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
        let ir = to_ir_with_config(&module, options, &config, &mut warnings)?;

        let result = crate::adapter::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let property_count = ir.properties.len();

        Ok(AdapterOutput {
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::SystemVerilog,
                title: Some(module.name.clone()),
                signal_count: module.ports.len(),
                state_count: result.state_count,
                property_count,
            },
        })
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
        let module = parser::parse(content)?;

        let mut warnings = Vec::new();
        let ir = to_ir(&module, options, &mut warnings)?;

        let result = crate::adapter::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let property_count = ir.properties.len();

        Ok(AdapterOutput {
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::SystemVerilog,
                title: Some(module.name.clone()),
                signal_count: module.ports.len(),
                state_count: result.state_count,
                property_count,
            },
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

    // Decide path: Kripke if forced, or if no enum FSM is found
    let use_kripke = module.force_kripke
        || !module.domain_annotations.is_empty()
        || fsm::extract_fsm(module).is_none();

    if use_kripke {
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

/// Check if a guard string references any input port name.
fn guard_references_input(guard: &str, input_ports: &HashSet<String>) -> bool {
    input_ports.iter().any(|port| guard.contains(port.as_str()))
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
        assert!(clts.state_count() <= 4); // 2-bit counter = max 4 reachable states
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
        // 3 enum states × 3 counter values = 9 max, but pruned by reachability
        assert!(clts.state_count() <= 9);
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
        assert!(
            synth.realizable,
            "ALU safety should be realizable (states: {})",
            clts.state_count()
        );
    }

    #[test]
    fn kripke_fifo_example() {
        let sv = include_str!("../../../../../examples/systemverilog/fifo.sv");
        let output = translate_sv(sv);

        let doc = crate::context_dsl::parse(&output.ctxdsl)
            .unwrap_or_else(|e| panic!("CTXDSL parse failed:\n{}\n\nError: {e}", output.ctxdsl));
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
        assert!(automaton.states.len() <= 4); // count: 0..3
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
        assert!(clts.state_count() <= 4);
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
}
