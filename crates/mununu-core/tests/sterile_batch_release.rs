//! Industrial BPM Example: Sterile Pharmaceutical Batch Release
//!
//! This test instantiates a regulated fill-and-finish workflow—mirroring Pfizer/Roche style
//! pipelines under FDA 21 CFR Part 11 & EU GMP Annex 1—in four CLTS components: production,
//! quality control, qualified-person release, and cold-chain logistics. It documents the
//! compliance requirement that nothing ships before QC pass + QP sign-off, and every completed
//! batch eventually gets released or quarantined. We reuse the μ-calculus formulas to confirm the
//! safety/liveness behaviour and show that the current process violates the safety policy (hence
//! the controller synthesis yields the empty controller).
use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::context_dsl::parser;
use mununu_core::examples::sterile_batch_release::{SterileScenario, sterile_scenario};
use std::collections::VecDeque;
use std::error::Error;

fn reachable_states(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> Vec<bool> {
    let mut visited = vec![false; clts.state_count()];
    let mut queue = VecDeque::new();

    for initial in clts.initial_states() {
        visited[initial.index()] = true;
        queue.push_back(*initial);
    }

    while let Some(state) = queue.pop_front() {
        for transition in clts.outgoing(state) {
            let target = transition.target();
            if !visited[target.index()] {
                visited[target.index()] = true;
                queue.push_back(target);
            }
        }
    }

    visited
}

#[test]
fn sterile_batch_release_properties_hold() -> Result<(), Box<dyn Error>> {
    let SterileScenario {
        context,
        pipeline,
        ship_requires_release,
        completion_leads_to_disposition,
        environment,
    } = sterile_scenario().map_err(|e| -> Box<dyn Error> { Box::new(e) })?;

    let pipeline_clts = context
        .clts(pipeline)
        .expect("pipeline registered in scenario context");
    let reachable = reachable_states(pipeline_clts);

    let safety = context.evaluate_mu(pipeline, &ship_requires_release, &environment, None)?;
    assert!(
        reachable.iter().enumerate().any(|(idx, reachable_state)| {
            *reachable_state && !safety.get(idx).is_some_and(|bit| *bit)
        }),
        "expected at least one reachable state to violate ship_requires_release"
    );

    let liveness = context.evaluate_mu(
        pipeline,
        &completion_leads_to_disposition,
        &environment,
        None,
    )?;
    assert!(
        reachable.iter().enumerate().any(|(idx, reachable_state)| {
            *reachable_state && !liveness.get(idx).is_some_and(|bit| *bit)
        }),
        "expected some reachable states to require disposition handling"
    );

    let controller =
        context.synthesise_controller(pipeline, &ship_requires_release, &environment, None)?;
    assert!(!controller.realizable);
    assert_eq!(controller.controller.state_count(), 0);
    assert_eq!(controller.diagnostics.violating_initials.len(), 1);
    assert!(
        controller
            .diagnostics
            .messages
            .iter()
            .any(|msg| msg.contains("Controller unrealizable"))
    );
    assert!(
        controller.diagnostics.counterexample_trace.is_some(),
        "expected a minimal counterexample trace"
    );
    assert!(controller.diagnostics.deadlock_traces.is_empty());
    assert!(controller.diagnostics.counterstrategy_traces.is_empty());

    Ok(())
}

#[test]
fn sterile_batch_release_dsl_example_parses() -> Result<(), Box<dyn Error>> {
    let doc = parser::parse(include_str!(
        "../../../examples/sterile_batch_release.ctxdsl"
    ))
    .map_err(|e| -> Box<dyn Error> { Box::new(e) })?;
    assert_eq!(doc.alphabet.len(), 8);
    assert_eq!(doc.automata.len(), 4);
    assert_eq!(doc.compositions.len(), 1);
    assert_eq!(doc.controllers.len(), 1);
    assert_eq!(doc.mu_formulas.len(), 2);
    Ok(())
}
