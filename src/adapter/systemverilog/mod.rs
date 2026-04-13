//! SystemVerilog adapter.
//!
//! Translates behavioral SystemVerilog RTL into CTXDSL via FSM extraction
//! from `always_ff` blocks. Uses the explicit-automaton encoding path
//! (like Promela), producing named states and sparse transitions.
//!
//! Supported subset: module declarations, typedef enum, always_ff with
//! case/if-else, always_comb, assign statements, and `// @mununu` properties.

pub mod ast;
pub mod emit_controller;
pub mod fsm;
mod parser;

use super::ir::*;
use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo, WarningKind,
};
use ast::{Module, MununuPropertyKind, PortDirection};
use std::collections::HashSet;

/// SystemVerilog adapter implementing [`FormatAdapter`].
pub struct SystemVerilogAdapter;

impl FormatAdapter for SystemVerilogAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        // Must have `module` keyword and at least one always block or assign
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

        let result = super::emit::emit(&ir).map_err(|e| AdapterError {
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

    // Extract FSM from always_ff + enum
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

#[cfg(test)]
mod tests {
    use super::*;

    fn translate_sv(sv: &str) -> AdapterOutput {
        let options = AdapterOptions::default();
        SystemVerilogAdapter::translate(sv, &options).expect("translation should succeed")
    }

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
