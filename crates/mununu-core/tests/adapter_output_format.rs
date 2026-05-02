//! Integration tests for controller output in native formats.
//!
//! Tests the full round-trip: source format → translate → realize → synthesize
//! → emit controller in native format (XState JSON, SystemVerilog module).

use mununu_core::adapter::xstate::XStateAdapter;
use mununu_core::adapter::xstate::emit_controller::controller_to_xstate_json;
use mununu_core::adapter::{AdapterOptions, FormatAdapter};
use mununu_core::context_dsl;

use mununu_core::adapter::gdscript::emit_controller::{
    collect_controllable_labels, controller_to_gdscript,
};
use mununu_core::adapter::systemverilog::SystemVerilogAdapter;
use mununu_core::adapter::systemverilog::emit_controller::controller_to_systemverilog;

// ---------------------------------------------------------------------------
// XState round-trip: JSON → synthesize → XState JSON controller
// ---------------------------------------------------------------------------

const XSTATE_MACHINE: &str = r#"{
    "id": "light",
    "initial": "green",
    "states": {
        "green": { "on": { "TIMER": "yellow" } },
        "yellow": { "on": { "TIMER": "red" } },
        "red": { "on": { "TIMER": "green" } }
    },
    "__mununu": {
        "controllable": ["TIMER"],
        "properties": [
            { "name": "safety", "formula": "nu X. ([] X)", "role": "guarantee" }
        ]
    }
}"#;

#[test]
fn xstate_round_trip_json_output() {
    let options = AdapterOptions::default();
    let output = XStateAdapter::translate(XSTATE_MACHINE, &options).unwrap();
    let doc = context_dsl::parse(&output.ctxdsl).unwrap();
    let realized = context_dsl::realize_context(&doc, &[]).unwrap();

    let formula = realized.formulas.get("safety").unwrap();
    let env = realized.environment_for("light");
    let synth = realized
        .context
        .synthesise_controller("light", &formula.formula, &env, None)
        .unwrap();

    assert!(synth.realizable);

    // Emit as XState JSON
    let json_output = controller_to_xstate_json(&synth.controller, "light", true);

    // Verify valid JSON structure
    let parsed: serde_json::Value = serde_json::from_str(&json_output)
        .unwrap_or_else(|e| panic!("Invalid XState JSON output: {e}\n\n{json_output}"));

    assert_eq!(parsed["id"], "light_controller");
    assert_eq!(parsed["__mununu"]["synthesis_result"], "realizable");
    assert!(!parsed["states"].as_object().unwrap().is_empty());
    assert!(!parsed["initial"].as_str().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// SystemVerilog round-trip: SV → synthesize → SV module controller
// ---------------------------------------------------------------------------

const SV_HANDSHAKE: &str = r#"
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
    endmodule
"#;

#[test]
fn systemverilog_round_trip_sv_output() {
    let options = AdapterOptions::default();
    let output = SystemVerilogAdapter::translate(SV_HANDSHAKE, &options).unwrap();
    let doc = context_dsl::parse(&output.ctxdsl).unwrap();
    let realized = context_dsl::realize_context(&doc, &[]).unwrap();

    let formula = realized.formulas.get("safety").unwrap();
    let env = realized.environment_for("handshake");
    let synth = realized
        .context
        .synthesise_controller("handshake", &formula.formula, &env, None)
        .unwrap();

    assert!(synth.realizable);

    // Emit as SystemVerilog module
    let sv_output = controller_to_systemverilog(&synth.controller, "handshake", true);

    // Verify valid SV module structure
    assert!(
        sv_output.contains("module handshake_controller"),
        "Should contain module declaration"
    );
    assert!(
        sv_output.contains("typedef enum"),
        "Should contain enum typedef"
    );
    assert!(
        sv_output.contains("always_ff"),
        "Should contain always_ff block"
    );
    assert!(
        sv_output.contains("case (state)"),
        "Should contain case statement"
    );
    assert!(sv_output.contains("endmodule"), "Should end with endmodule");
    assert!(sv_output.contains("if (rst)"), "Should contain reset logic");
}

// ---------------------------------------------------------------------------
// Cross-format round-trip: same system, different output formats
// ---------------------------------------------------------------------------

#[test]
fn cross_format_same_system_different_outputs() {
    // Use a simple CTXDSL system and export to both formats
    let ctxdsl = r#"
    context test {
        alphabet { label go; label stop; }
        automata {
            automaton FSM {
                controllable { label go; label stop; }
                states { state A initial; state B; }
                transitions {
                    transition A -> B on label go;
                    transition B -> A on label stop;
                }
            }
        }
        mu_formulas {
            formula safety { over FSM; body = nu X. ([] X); }
        }
        controllers { controller c { source FSM; satisfying safety; } }
    }
    "#;

    let doc = context_dsl::parse(ctxdsl).unwrap();
    let realized = context_dsl::realize_context(&doc, &[]).unwrap();
    let formula = realized.formulas.get("safety").unwrap();
    let env = realized.environment_for("FSM");
    let synth = realized
        .context
        .synthesise_controller("FSM", &formula.formula, &env, None)
        .unwrap();

    assert!(synth.realizable);

    // XState JSON output
    let xstate_json = controller_to_xstate_json(&synth.controller, "FSM", true);
    let parsed: serde_json::Value = serde_json::from_str(&xstate_json).unwrap();
    assert_eq!(parsed["__mununu"]["synthesis_result"], "realizable");
    let xstate_states = parsed["states"].as_object().unwrap().len();

    // SystemVerilog output
    let sv = controller_to_systemverilog(&synth.controller, "FSM", true);
    assert!(sv.contains("module FSM_controller"));
    assert!(sv.contains("endmodule"));

    // Both outputs should represent the same number of states
    assert!(xstate_states > 0, "XState output should have states");
    assert!(
        sv.contains("typedef enum"),
        "SV output should have states via enum"
    );
}

// ---------------------------------------------------------------------------
// Unrealizable: both output formats should handle gracefully
// ---------------------------------------------------------------------------

#[test]
fn unrealizable_output_xstate() {
    let json = controller_to_xstate_json(
        &mununu_core::clts::Clts::builder().build().unwrap(),
        "empty",
        false,
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["__mununu"]["synthesis_result"], "unrealizable");
}

#[test]
fn unrealizable_output_systemverilog() {
    let sv = controller_to_systemverilog(
        &mununu_core::clts::Clts::builder().build().unwrap(),
        "empty",
        false,
    );
    assert!(sv.contains("unrealizable"));
    assert!(!sv.contains("module empty_controller"));
}

// ---------------------------------------------------------------------------
// GDScript round-trip: dialogue tree → synthesize → GDScript controller
// ---------------------------------------------------------------------------

#[test]
fn gdscript_dialogue_tree_export() {
    use mununu_core::adapter::extraction::ExtractionAdapter;

    let json = include_str!("../../../examples/game/dialogue_tree.espec.json");
    let options = AdapterOptions {
        mode: Some("vulnerable".to_string()),
        ..Default::default()
    };
    let output = ExtractionAdapter::translate(json, &options).unwrap();
    let doc = context_dsl::parse(&output.ctxdsl).unwrap();
    let realized = context_dsl::realize_context(&doc, &[]).unwrap();

    let formula = realized.formulas.get("farewell_reachable").unwrap();
    let env = realized.environment_for("Dialogue");
    let synth = realized
        .context
        .synthesise_controller("Dialogue", &formula.formula, &env, None)
        .unwrap();

    assert!(synth.realizable);

    let controllable = collect_controllable_labels(&synth.controller);
    let gd = controller_to_gdscript(&synth.controller, "Dialogue", true, &controllable);

    assert!(
        gd.contains("class_name DialogueController"),
        "missing class_name"
    );
    assert!(gd.contains("enum State"), "missing enum");
    assert!(gd.contains("match current_state"), "missing match");
    assert!(gd.contains("realizable"), "missing synthesis result");
    // Locked and Threatened should be removed (not in winning region)
    assert!(
        !gd.contains("LOCKED"),
        "Locked should be excluded from controller"
    );
    assert!(
        !gd.contains("THREATENED"),
        "Threatened should be excluded from controller"
    );
    // Should have action methods (controllable → bool, or uncontrollable → on_ prefix)
    assert!(
        gd.contains("-> bool") || gd.contains("func on_"),
        "should have action methods"
    );
    // Must have at least some transition-producing functions
    assert!(
        gd.contains("func ev_") || gd.contains("func on_ev_"),
        "should have event functions"
    );
}

#[test]
fn gdscript_cross_format() {
    let ctxdsl = r#"
    context test {
        alphabet { label go; label stop; }
        automata {
            automaton FSM {
                controllable { label go; label stop; }
                states { state A initial; state B; }
                transitions {
                    transition A -> B on label go;
                    transition B -> A on label stop;
                }
            }
        }
        mu_formulas {
            formula safety { over FSM; body = nu X. ([] X); }
        }
        controllers { controller c { source FSM; satisfying safety; } }
    }
    "#;

    let doc = context_dsl::parse(ctxdsl).unwrap();
    let realized = context_dsl::realize_context(&doc, &[]).unwrap();
    let formula = realized.formulas.get("safety").unwrap();
    let env = realized.environment_for("FSM");
    let synth = realized
        .context
        .synthesise_controller("FSM", &formula.formula, &env, None)
        .unwrap();

    assert!(synth.realizable);

    // All three formats should produce valid output for the same system
    let xstate_json = controller_to_xstate_json(&synth.controller, "FSM", true);
    let sv = controller_to_systemverilog(&synth.controller, "FSM", true);
    let controllable = collect_controllable_labels(&synth.controller);
    let gd = controller_to_gdscript(&synth.controller, "FSM", true, &controllable);

    // All should contain state declarations
    let xstate_states = serde_json::from_str::<serde_json::Value>(&xstate_json).unwrap()["states"]
        .as_object()
        .unwrap()
        .len();
    assert!(xstate_states > 0);
    assert!(sv.contains("typedef enum"));
    assert!(gd.contains("enum State"));

    // GDScript-specific checks
    assert!(gd.contains("class_name FSMController"));
    assert!(gd.contains("extends Node"));
    assert!(gd.contains("get_state_name"));
}

#[test]
fn unrealizable_output_gdscript() {
    let empty = mununu_core::clts::Clts::builder().build().unwrap();
    let gd = controller_to_gdscript(&empty, "empty", false, &std::collections::HashSet::new());
    assert!(gd.contains("UNREALIZABLE"));
    assert!(!gd.contains("class_name"));
    assert!(!gd.contains("enum State"));
}
