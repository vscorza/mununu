//! Phase A0 viability spike: hand-construct AdapterIR structures that mimic
//! XState machines, push through emit → parse → realize → eval/synth.
//!
//! These tests validate that the explicit-automaton emission path works for
//! XState-like structures BEFORE building a parser.

use mununu::adapter::SourceFormat;
use mununu::adapter::emit;
use mununu::adapter::ir::*;
use mununu::context_dsl;

/// Helper: emit IR, parse the resulting CTXDSL, realize it.
fn emit_and_realize(ir: &AdapterIR) -> context_dsl::realize::RealizedContext {
    let result = emit::emit(ir).expect("emit should succeed");
    let doc = context_dsl::parse(&result.ctxdsl).unwrap_or_else(|e| {
        panic!(
            "Generated CTXDSL failed to parse: {e}\n\nCTXDSL:\n{}",
            result.ctxdsl
        )
    });
    context_dsl::realize_context(&doc, &[])
        .unwrap_or_else(|e| panic!("Generated CTXDSL failed to realize: {e}"))
}

// -----------------------------------------------------------------------
// Spike 1: Simple 3-state traffic light — no hierarchy, no composition
// -----------------------------------------------------------------------

#[test]
fn viability_simple_automaton_eval_synth() {
    let ir = AdapterIR {
        metadata: Metadata {
            title: "traffic_light".to_string(),
            source_format: SourceFormat::Promela, // reuse for now
            description: Some("Viability spike: simple 3-state traffic light".to_string()),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata: vec![AutomatonSpec {
            name: "TrafficLight".to_string(),
            states: vec![
                StateSpec {
                    name: "Green".to_string(),
                    is_initial: true,
                },
                StateSpec {
                    name: "Yellow".to_string(),
                    is_initial: false,
                },
                StateSpec {
                    name: "Red".to_string(),
                    is_initial: false,
                },
            ],
            transitions: vec![
                TransitionSpec {
                    source: "Green".to_string(),
                    target: "Yellow".to_string(),
                    labels: vec!["timer".to_string()],
                },
                TransitionSpec {
                    source: "Yellow".to_string(),
                    target: "Red".to_string(),
                    labels: vec!["timer".to_string()],
                },
                TransitionSpec {
                    source: "Red".to_string(),
                    target: "Green".to_string(),
                    labels: vec!["timer".to_string()],
                },
            ],
            controllable_labels: vec!["timer".to_string()],
            internal_labels: vec![],
        }],
        compositions: vec![],
        properties: vec![PropertySpec {
            name: "safety_invariant".to_string(),
            kind: PropertyKind::Safety,
            formula: PropertyFormula::MuCalculus("nu X. ([] X)".to_string()),
            role: PropertyRole::Guarantee,
            over: None,
        }],
        controller: Some(ControllerSpec {
            name: "safe_light".to_string(),
            source_automaton: "TrafficLight".to_string(),
            formula_name: "safety_invariant".to_string(),
        }),
    };

    let realized = emit_and_realize(&ir);

    // Structural: automaton exists with 3 states
    let clts = realized
        .context
        .clts("TrafficLight")
        .expect("TrafficLight automaton should exist");
    assert_eq!(clts.state_count(), 3, "Should have 3 states");

    // Formula exists
    let formula = realized
        .formulas
        .get("safety_invariant")
        .expect("safety_invariant formula should exist");

    // Evaluate
    let env = realized.environment_for("TrafficLight");
    let eval_result = realized
        .context
        .evaluate_mu("TrafficLight", &formula.formula, &env, None)
        .expect("Formula evaluation should succeed");
    assert_eq!(
        eval_result.len(),
        3,
        "Eval result should cover all 3 states"
    );
    assert_eq!(
        eval_result.count_ones(),
        3,
        "All 3 states should satisfy the safety invariant"
    );

    // Synthesize
    let synth = realized
        .context
        .synthesise_controller("TrafficLight", &formula.formula, &env, None)
        .expect("Synthesis should succeed");
    assert!(synth.realizable, "Traffic light should be realizable");
    assert!(
        synth.controller.state_count() > 0,
        "Controller should have states"
    );
}

// -----------------------------------------------------------------------
// Spike 2: Two automata with synchronous composition (parallel regions)
// -----------------------------------------------------------------------

#[test]
fn viability_synchronous_composition() {
    let ir = AdapterIR {
        metadata: Metadata {
            title: "parallel_regions".to_string(),
            source_format: SourceFormat::Promela,
            description: Some(
                "Viability spike: two regions in synchronous composition".to_string(),
            ),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata: vec![
            // Region A: 2 states (on/off), toggles via "toggle"
            AutomatonSpec {
                name: "RegionA".to_string(),
                states: vec![
                    StateSpec {
                        name: "Off".to_string(),
                        is_initial: true,
                    },
                    StateSpec {
                        name: "On".to_string(),
                        is_initial: false,
                    },
                ],
                transitions: vec![
                    TransitionSpec {
                        source: "Off".to_string(),
                        target: "On".to_string(),
                        labels: vec!["toggle".to_string()],
                    },
                    TransitionSpec {
                        source: "On".to_string(),
                        target: "Off".to_string(),
                        labels: vec!["toggle".to_string()],
                    },
                ],
                controllable_labels: vec!["toggle".to_string()],
                internal_labels: vec![],
            },
            // Region B: 3 states (idle/active/done), shared "toggle" event
            AutomatonSpec {
                name: "RegionB".to_string(),
                states: vec![
                    StateSpec {
                        name: "Idle".to_string(),
                        is_initial: true,
                    },
                    StateSpec {
                        name: "Active".to_string(),
                        is_initial: false,
                    },
                    StateSpec {
                        name: "Done".to_string(),
                        is_initial: false,
                    },
                ],
                transitions: vec![
                    TransitionSpec {
                        source: "Idle".to_string(),
                        target: "Active".to_string(),
                        labels: vec!["toggle".to_string()],
                    },
                    TransitionSpec {
                        source: "Active".to_string(),
                        target: "Done".to_string(),
                        labels: vec!["step".to_string()],
                    },
                    TransitionSpec {
                        source: "Done".to_string(),
                        target: "Idle".to_string(),
                        labels: vec!["toggle".to_string()],
                    },
                ],
                controllable_labels: vec!["step".to_string()],
                internal_labels: vec![],
            },
        ],
        compositions: vec![CompositionSpec::Synchronous {
            name: "System".to_string(),
            members: vec!["RegionA".to_string(), "RegionB".to_string()],
        }],
        properties: vec![PropertySpec {
            name: "all_reachable".to_string(),
            kind: PropertyKind::Safety,
            formula: PropertyFormula::MuCalculus("nu X. ([] X)".to_string()),
            role: PropertyRole::Guarantee,
            over: None,
        }],
        controller: None,
    };

    let realized = emit_and_realize(&ir);

    // Both component automata should exist
    let clts_a = realized
        .context
        .clts("RegionA")
        .expect("RegionA should exist");
    assert_eq!(clts_a.state_count(), 2);

    let clts_b = realized
        .context
        .clts("RegionB")
        .expect("RegionB should exist");
    assert_eq!(clts_b.state_count(), 3);

    // Composed system should exist with product state count
    let clts_system = realized
        .context
        .clts("System")
        .expect("System composition should exist");
    // Synchronous product: not all 2×3=6 states may be reachable,
    // but the composition should exist and have states.
    assert!(
        clts_system.state_count() > 0,
        "Composed system should have states"
    );
    // "toggle" synchronizes, so from (Off, Idle) only toggle fires together → (On, Active)
    // From (On, Active) "step" is independent in B (not shared with A, so A self-loops or blocks)
    // The exact product depends on sync semantics, but the composition must work.

    // Evaluate formula on composition
    let formula = realized
        .formulas
        .get("all_reachable")
        .expect("all_reachable should exist");
    let env = realized.environment_for("System");
    let eval_result = realized
        .context
        .evaluate_mu("System", &formula.formula, &env, None)
        .expect("Eval on composition should succeed");
    assert!(
        !eval_result.is_empty(),
        "Eval result should cover composed states"
    );
}

// -----------------------------------------------------------------------
// Spike 3: Variable automaton pattern (simulating XState context)
// -----------------------------------------------------------------------

#[test]
fn viability_variable_automaton_guard_sync() {
    // Model: a machine with a "counter" context variable (0..2)
    // Transitions are guarded by the counter value.
    // This follows the Promela adapter's create_variable_automaton pattern:
    //   - Variable automaton has states for each value
    //   - "set_counter_N" labels transition the var automaton
    //   - "test_counter_N" labels are self-loops for guard synchronization
    let ir = AdapterIR {
        metadata: Metadata {
            title: "context_variable".to_string(),
            source_format: SourceFormat::Promela,
            description: Some("Viability spike: variable automaton for context".to_string()),
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata: vec![
            // Main FSM
            AutomatonSpec {
                name: "Machine".to_string(),
                states: vec![
                    StateSpec {
                        name: "Idle".to_string(),
                        is_initial: true,
                    },
                    StateSpec {
                        name: "Working".to_string(),
                        is_initial: false,
                    },
                    StateSpec {
                        name: "Done".to_string(),
                        is_initial: false,
                    },
                ],
                transitions: vec![
                    // Idle -> Working: set counter to 0
                    TransitionSpec {
                        source: "Idle".to_string(),
                        target: "Working".to_string(),
                        labels: vec!["start".to_string(), "set_counter_0".to_string()],
                    },
                    // Working -> Working: increment (test that counter is 0, set to 1)
                    TransitionSpec {
                        source: "Working".to_string(),
                        target: "Working".to_string(),
                        labels: vec![
                            "tick".to_string(),
                            "test_counter_0".to_string(),
                            "set_counter_1".to_string(),
                        ],
                    },
                    // Working -> Working: increment (test that counter is 1, set to 2)
                    TransitionSpec {
                        source: "Working".to_string(),
                        target: "Working".to_string(),
                        labels: vec![
                            "tick".to_string(),
                            "test_counter_1".to_string(),
                            "set_counter_2".to_string(),
                        ],
                    },
                    // Working -> Done: finish when counter is 2
                    TransitionSpec {
                        source: "Working".to_string(),
                        target: "Done".to_string(),
                        labels: vec!["finish".to_string(), "test_counter_2".to_string()],
                    },
                    // Done -> Idle: reset
                    TransitionSpec {
                        source: "Done".to_string(),
                        target: "Idle".to_string(),
                        labels: vec!["reset".to_string()],
                    },
                ],
                controllable_labels: vec![
                    "start".to_string(),
                    "tick".to_string(),
                    "finish".to_string(),
                    "reset".to_string(),
                ],
                internal_labels: vec![],
            },
            // Variable automaton for "counter" (0, 1, 2)
            AutomatonSpec {
                name: "Var_counter".to_string(),
                states: vec![
                    StateSpec {
                        name: "counter_0".to_string(),
                        is_initial: true,
                    },
                    StateSpec {
                        name: "counter_1".to_string(),
                        is_initial: false,
                    },
                    StateSpec {
                        name: "counter_2".to_string(),
                        is_initial: false,
                    },
                ],
                transitions: vec![
                    // set transitions: from any state to target value
                    TransitionSpec {
                        source: "counter_0".to_string(),
                        target: "counter_0".to_string(),
                        labels: vec!["set_counter_0".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_1".to_string(),
                        target: "counter_0".to_string(),
                        labels: vec!["set_counter_0".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_2".to_string(),
                        target: "counter_0".to_string(),
                        labels: vec!["set_counter_0".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_0".to_string(),
                        target: "counter_1".to_string(),
                        labels: vec!["set_counter_1".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_1".to_string(),
                        target: "counter_1".to_string(),
                        labels: vec!["set_counter_1".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_2".to_string(),
                        target: "counter_1".to_string(),
                        labels: vec!["set_counter_1".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_0".to_string(),
                        target: "counter_2".to_string(),
                        labels: vec!["set_counter_2".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_1".to_string(),
                        target: "counter_2".to_string(),
                        labels: vec!["set_counter_2".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_2".to_string(),
                        target: "counter_2".to_string(),
                        labels: vec!["set_counter_2".to_string()],
                    },
                    // test transitions: self-loops (guard checks)
                    TransitionSpec {
                        source: "counter_0".to_string(),
                        target: "counter_0".to_string(),
                        labels: vec!["test_counter_0".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_1".to_string(),
                        target: "counter_1".to_string(),
                        labels: vec!["test_counter_1".to_string()],
                    },
                    TransitionSpec {
                        source: "counter_2".to_string(),
                        target: "counter_2".to_string(),
                        labels: vec!["test_counter_2".to_string()],
                    },
                ],
                controllable_labels: vec![
                    "set_counter_0".to_string(),
                    "set_counter_1".to_string(),
                    "set_counter_2".to_string(),
                    "test_counter_0".to_string(),
                    "test_counter_1".to_string(),
                    "test_counter_2".to_string(),
                ],
                internal_labels: vec![],
            },
        ],
        compositions: vec![CompositionSpec::Synchronous {
            name: "WithCounter".to_string(),
            members: vec!["Machine".to_string(), "Var_counter".to_string()],
        }],
        properties: vec![
            PropertySpec {
                name: "safety".to_string(),
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus("nu X. ([] X)".to_string()),
                role: PropertyRole::Guarantee,
                over: None,
            },
            // Done is reachable: the counter mechanism works
            PropertySpec {
                name: "done_reachable".to_string(),
                kind: PropertyKind::Liveness,
                formula: PropertyFormula::MuCalculus("mu X. (Done || <> X)".to_string()),
                role: PropertyRole::Guarantee,
                over: None,
            },
        ],
        controller: None,
    };

    let realized = emit_and_realize(&ir);

    // Component automata exist
    let machine = realized
        .context
        .clts("Machine")
        .expect("Machine should exist");
    assert_eq!(machine.state_count(), 3);

    let var = realized
        .context
        .clts("Var_counter")
        .expect("Var_counter should exist");
    assert_eq!(
        var.state_count(),
        3,
        "Variable automaton should have 3 states (0,1,2)"
    );

    // Composition exists
    let composed = realized
        .context
        .clts("WithCounter")
        .expect("WithCounter composition should exist");
    // Product: 3 machine states × 3 counter values = 9 max, but
    // reachable states depend on sync semantics
    assert!(
        composed.state_count() > 0,
        "Composed system should have states"
    );

    // Evaluate safety on composition
    let formula = realized
        .formulas
        .get("safety")
        .expect("safety formula should exist");
    let env = realized.environment_for("WithCounter");
    let eval_result = realized
        .context
        .evaluate_mu("WithCounter", &formula.formula, &env, None)
        .expect("Safety eval should succeed");
    assert!(
        !eval_result.is_empty(),
        "Eval result should cover composed states"
    );

    // Evaluate done_reachable on the Machine (not composition, since
    // "Done" is a state name in Machine, not in the product)
    let done_formula = realized
        .formulas
        .get("done_reachable")
        .expect("done_reachable should exist");
    let machine_env = realized.environment_for("Machine");
    let done_eval = realized
        .context
        .evaluate_mu("Machine", &done_formula.formula, &machine_env, None)
        .expect("Done reachable eval should succeed");
    // All states in Machine should satisfy "Done is reachable" since the
    // machine can reach Done from any state via the controllable transitions
    assert_eq!(
        done_eval.count_ones(),
        3,
        "Done should be reachable from all 3 Machine states"
    );
}
