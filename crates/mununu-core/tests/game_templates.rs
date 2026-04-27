//! Integration tests for game engine examples with property templates.
//!
//! These tests verify end-to-end:
//! 1. Game `.espec.json` files parse and translate correctly via ExtractionAdapter
//! 2. Template-based properties resolve and produce valid mu-calculus
//! 3. Known-verdict regressions: softlocked FSM fails, fixed FSM passes

use mununu_core::adapter::extraction::ExtractionAdapter;
use mununu_core::adapter::{AdapterOptions, FormatAdapter};
use mununu_core::context_dsl::{parse as parse_context_doc, realize_context};

fn eval_template_property(
    espec_json: &str,
    formula_name: &str,
    automaton_name: &str,
) -> (bool, usize, usize) {
    let options = AdapterOptions {
        mode: Some("vulnerable".to_string()),
        ..Default::default()
    };
    let output =
        ExtractionAdapter::translate(espec_json, &options).expect("translation should succeed");

    let doc = parse_context_doc(&output.ctxdsl).expect("CTXDSL should parse");
    let realized = realize_context(&doc, &[]).expect("realization should succeed");

    let clts = realized
        .context
        .clts(automaton_name)
        .unwrap_or_else(|| panic!("automaton '{}' should exist", automaton_name));

    let rf = realized
        .formulas
        .get(formula_name)
        .unwrap_or_else(|| panic!("formula '{}' should exist", formula_name));

    let env = realized.environment_for(automaton_name);

    let result = realized
        .context
        .evaluate_mu(automaton_name, &rf.formula, &env, None)
        .expect("evaluation should succeed");

    let total = clts.state_count();
    let satisfying = result.count_ones();
    let initial_satisfy = clts
        .initial_states()
        .iter()
        .all(|s| result.get(s.index()).is_some_and(|b| *b));

    (initial_satisfy, satisfying, total)
}

// ---------------------------------------------------------------------------
// Player FSM: softlock detection
// ---------------------------------------------------------------------------

#[test]
fn player_fsm_softlock_detected() {
    let json = include_str!("../../../examples/game/player_fsm.espec.json");
    let (initial_satisfy, satisfying, total) =
        eval_template_property(json, "no_softlock", "PlayerState");

    // Dead state has no outgoing transitions → no_deadlock fails
    assert!(
        !initial_satisfy,
        "initial should NOT satisfy (Dead is reachable and deadlocked)"
    );
    assert!(
        satisfying < total,
        "not all states should satisfy (Dead doesn't): {satisfying}/{total}"
    );
}

#[test]
fn player_fsm_fixed_passes() {
    let json = include_str!("../../../examples/game/player_fsm_fixed.espec.json");
    let (initial_satisfy, satisfying, total) =
        eval_template_property(json, "no_softlock", "PlayerState");

    // Fixed version: Dead → Idle via ev_respawn
    assert!(
        initial_satisfy,
        "initial should satisfy (all states have exits)"
    );
    assert_eq!(
        satisfying, total,
        "all states should satisfy: {satisfying}/{total}"
    );
}

#[test]
fn player_fsm_dead_reachable() {
    let json = include_str!("../../../examples/game/player_fsm.espec.json");
    let (initial_satisfy, _, _) = eval_template_property(json, "dead_reachable", "PlayerState");

    // Dead is reachable from Idle
    assert!(initial_satisfy, "Dead should be reachable from initial");
}

// ---------------------------------------------------------------------------
// Quest system: unreachable goal
// ---------------------------------------------------------------------------

#[test]
fn quest_deadlock_goal_unreachable() {
    let json = include_str!("../../../examples/game/quest_deadlock.espec.json");
    let (initial_satisfy, _, _) =
        eval_template_property(json, "all_quests_completable", "QuestProgress");

    // AllComplete is not reachable due to circular dependency
    assert!(
        !initial_satisfy,
        "AllComplete should NOT be reachable (circular dependency)"
    );
}

// ---------------------------------------------------------------------------
// NPC AI: deadlock freedom
// ---------------------------------------------------------------------------

#[test]
fn npc_ai_no_deadlock() {
    let json = include_str!("../../../examples/game/npc_ai_loop.espec.json");
    let (initial_satisfy, satisfying, total) =
        eval_template_property(json, "no_ai_deadlock", "AIState");

    // All NPC states have outgoing transitions
    assert!(initial_satisfy, "initial should satisfy (no AI deadlocks)");
    assert_eq!(
        satisfying, total,
        "all AI states should satisfy: {satisfying}/{total}"
    );
}

#[test]
fn npc_ai_attack_reachable() {
    let json = include_str!("../../../examples/game/npc_ai_loop.espec.json");
    let (initial_satisfy, _, _) = eval_template_property(json, "attack_reachable", "AIState");

    assert!(initial_satisfy, "Attack should be reachable from Patrol");
}

// ---------------------------------------------------------------------------
// Template catalog: basic sanity
// ---------------------------------------------------------------------------

#[test]
fn template_catalog_loads_and_all_templates_instantiate() {
    use mununu_core::adapter::templates::{TemplateRef, TemplateRegistry};

    let registry = TemplateRegistry::builtin();

    // no_deadlock (no params)
    let r = registry
        .instantiate(&TemplateRef {
            template: "no_deadlock".into(),
            args: Default::default(),
        })
        .unwrap();
    assert!(!r.formula.contains("${"), "no placeholders should remain");

    // reachable (one param)
    let r = registry
        .instantiate(&TemplateRef {
            template: "reachable".into(),
            args: [("TARGET".into(), "Idle".into())].into(),
        })
        .unwrap();
    assert!(r.formula.contains("Idle"));

    // bounded (one required, one optional with default)
    let r = registry
        .instantiate(&TemplateRef {
            template: "bounded".into(),
            args: [("OVERFLOW".into(), "max_hp".into())].into(),
        })
        .unwrap();
    assert!(r.formula.contains("max_hp"));
    assert!(r.formula.contains("false")); // UNDERFLOW default

    // mutual_exclusion (two required)
    let r = registry
        .instantiate(&TemplateRef {
            template: "mutual_exclusion".into(),
            args: [("A".into(), "P1".into()), ("B".into(), "P2".into())].into(),
        })
        .unwrap();
    assert!(r.formula.contains("P1"));
    assert!(r.formula.contains("P2"));
}
