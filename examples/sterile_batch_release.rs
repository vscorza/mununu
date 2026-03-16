use mununu::context::{ControllerSynthesisOptions, DiagnosticsOptions};
use mununu::examples::sterile_batch_release::{SterileScenario, sterile_scenario};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let SterileScenario {
        context,
        pipeline,
        ship_requires_release,
        completion_leads_to_disposition,
        environment,
    } = sterile_scenario().map_err(|e| -> Box<dyn Error> { Box::new(e) })?;

    let safety = context.evaluate_mu(pipeline, &ship_requires_release, &environment, None)?;
    let liveness = context.evaluate_mu(
        pipeline,
        &completion_leads_to_disposition,
        &environment,
        None,
    )?;

    println!(
        "Sterile batch release safety: {} / {} states satisfy ship_requires_release",
        safety.iter().filter(|bit| **bit).count(),
        safety.len()
    );
    println!(
        "Sterile batch release liveness: {} / {} states satisfy completion_leads_to_disposition",
        liveness.iter().filter(|bit| **bit).count(),
        liveness.len()
    );

    let diagnostics_options = DiagnosticsOptions {
        counterexample: true,
        deadlock_traces: true,
        max_counter_traces: None,
        proof_obligations: true,
    };
    let controller = context.synthesise_controller_with_options(
        pipeline,
        &ship_requires_release,
        &environment,
        ControllerSynthesisOptions {
            diagnostics: Some(&diagnostics_options),
            minimize: true,
            ..Default::default()
        },
    )?;
    println!(
        "Controller realisable: {}, states kept: {}",
        controller.realizable,
        controller.controller.state_count()
    );
    println!("Diagnostics summary: {}", controller.diagnostics.summary());
    if !controller.diagnostics.messages.is_empty() {
        println!(
            "Diagnostics: {}",
            controller.diagnostics.messages.join(" | ")
        );
    }
    if let Some(trace) = &controller.diagnostics.counterexample_trace {
        println!("Counterexample trace: {}", trace.join(" -> "));
    }
    if !controller.diagnostics.counterstrategy_traces.is_empty() {
        for (idx, trace) in controller
            .diagnostics
            .counterstrategy_traces
            .iter()
            .enumerate()
        {
            println!("Counterstrategy trace {}: {}", idx + 1, trace.join(" -> "));
        }
    }
    if !controller.diagnostics.deadlock_traces.is_empty() {
        for (idx, trace) in controller.diagnostics.deadlock_traces.iter().enumerate() {
            println!("Deadlock trace {}: {}", idx + 1, trace.join(" -> "));
        }
    }
    if !controller.diagnostics.proof_obligations.is_empty() {
        for obligation in &controller.diagnostics.proof_obligations {
            match &obligation.detail {
                Some(detail) => println!("Proof obligation – state {}: {detail}", obligation.state),
                None => println!("Proof obligation – state {}", obligation.state),
            }
        }
    }
    if let Some(strategy) = &controller.diagnostics.counterstrategy {
        println!("Counterstrategy states: {}", strategy.states.join(", "));
        if !strategy.transitions.is_empty() {
            println!("Counterstrategy transitions:");
            for edge in &strategy.transitions {
                if edge.labels.is_empty() {
                    println!("  {} -> {}", edge.from, edge.to);
                } else {
                    println!(
                        "  {} -[{labels}]→ {}",
                        edge.from,
                        edge.to,
                        labels = edge.labels.join(", ")
                    );
                }
            }
        }
    }
    if let Some(min) = &controller.diagnostics.minimization {
        println!(
            "Minimization removed {} state(s) and {} transition(s).",
            min.removed_states, min.removed_transitions
        );
        if !min.merged_states.is_empty() {
            println!("Merged states: {}", min.merged_states.join(", "));
        }
    }
    if let Ok(json) = controller.diagnostics.to_json_string_pretty() {
        println!("Diagnostics JSON:\n{json}");
    }

    Ok(())
}
