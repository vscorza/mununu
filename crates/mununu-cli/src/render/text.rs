// Text rendering helpers — moved from main.rs in the render-layer split.
// All functions here are pure formatters that write to stdout; no side effects
// other than terminal output.

pub(crate) fn render_proposal_provenance(
    p: &mununu_core::contract::review::ProposalProvenance,
) -> String {
    use mununu_core::contract::review::ProposalProvenance;
    match p {
        ProposalProvenance::SourceComment { tag, source_line } => match source_line {
            Some(line) => format!("@mununu_{tag} (line {line})"),
            None => format!("@mununu_{tag}"),
        },
        ProposalProvenance::Corpus {
            entry_id,
            alternative,
        } => match alternative {
            Some(a) => format!("corpus:{entry_id} alt={a}"),
            None => format!("corpus:{entry_id}"),
        },
    }
}

pub(crate) fn render_discharge_verdict_text(
    verdict: &mununu_core::contract::discharge::DischargeVerdict,
) {
    use mununu_core::contract::discharge::DischargeVerdict;
    match verdict {
        DischargeVerdict::Acyclic {
            topological,
            unmet_environment,
        } => {
            println!("discharge: acyclic");
            if !topological.is_empty() {
                println!("  topological order:");
                for id in topological {
                    println!("    - {id}");
                }
            }
            if !unmet_environment.is_empty() {
                println!("  declared env assumptions with no in-graph guarantor:");
                for id in unmet_environment {
                    println!("    - {id}  (expected — accepted as top-level env)");
                }
            }
        }
        DischargeVerdict::CircularWithRankWitness {
            cycles,
            acyclic_remainder,
        } => {
            println!("discharge: circular with mu-rank witness (auto-accepted, McMillan-style)");
            for cycle in cycles {
                println!(
                    "  - cycle [{}], base edge: {} -> {}",
                    cycle.cycle.join(" -> "),
                    cycle.base_edge.0,
                    cycle.base_edge.1,
                );
            }
            if !acyclic_remainder.is_empty() {
                println!("  acyclic remainder:");
                for id in acyclic_remainder {
                    println!("    - {id}");
                }
            }
        }
        DischargeVerdict::Circular {
            cycles,
            acyclic_remainder,
        } => {
            println!("discharge: circular reasoning required (no mu-rank witness)");
            println!("  cycles:");
            for cycle in cycles {
                println!("    - [{}]", cycle.join(" -> "));
            }
            if !acyclic_remainder.is_empty() {
                println!("  acyclic remainder (singletons):");
                for id in acyclic_remainder {
                    println!("    - {id}");
                }
            }
            println!("  → mununu refuses to silently accept circular discharge.");
            println!("    HITL must approve, or one cycle clause must be rewritten");
            println!("    to be unconditional.");
            println!("  Tip: assign `mu_rank` to each clause for the lightweight");
            println!("    McMillan-style automatic discharge (task A8).");
        }
        DischargeVerdict::PotentiallyCircular {
            unresolved,
            partial,
        } => {
            println!("discharge: potentially circular (unresolved clauses)");
            println!("  unresolved ids (refresh corpus or fill in clauses):");
            for id in unresolved {
                println!("    - {id}");
            }
            println!("  partial verdict over the resolved portion:");
            render_discharge_verdict_text_indented(partial, 4);
        }
        DischargeVerdict::Unmet {
            missing_dischargers,
            partial,
        } => {
            println!("discharge: unmet obligations");
            println!("  assumptions without any guarantor or env declaration:");
            for id in missing_dischargers {
                println!("    - {id}");
            }
            println!("  partial verdict over the rest:");
            render_discharge_verdict_text_indented(partial, 4);
        }
    }
}

fn render_discharge_verdict_text_indented(
    verdict: &mununu_core::contract::discharge::DischargeVerdict,
    indent: usize,
) {
    use mununu_core::contract::discharge::DischargeVerdict;
    let pad = " ".repeat(indent);
    match verdict {
        DischargeVerdict::Acyclic { topological, .. } => {
            println!("{pad}acyclic ({} clauses)", topological.len());
        }
        DischargeVerdict::Circular { cycles, .. } => {
            println!("{pad}circular ({} cycle(s))", cycles.len());
        }
        DischargeVerdict::CircularWithRankWitness { cycles, .. } => {
            println!(
                "{pad}circular with mu-rank witness ({} cycle(s))",
                cycles.len()
            );
        }
        DischargeVerdict::PotentiallyCircular { unresolved, .. } => {
            println!(
                "{pad}potentially circular ({} unresolved id(s))",
                unresolved.len()
            );
        }
        DischargeVerdict::Unmet {
            missing_dischargers,
            ..
        } => {
            println!(
                "{pad}unmet ({} missing discharger(s))",
                missing_dischargers.len()
            );
        }
    }
}

pub(crate) fn render_controller_diagnostics(
    diagnostics: &mununu_core::context::ControllerDiagnostics,
) {
    let has_details = !diagnostics.messages.is_empty()
        || !diagnostics.violating_initials.is_empty()
        || diagnostics.counterexample_trace.is_some()
        || !diagnostics.deadlock_traces.is_empty()
        || !diagnostics.counterstrategy_traces.is_empty()
        || diagnostics.minimization.is_some()
        || !diagnostics.proof_obligations.is_empty()
        || diagnostics.counterstrategy.is_some();

    if !has_details {
        println!("  Diagnostics: no additional notes recorded.");
        return;
    }

    println!("  Diagnostics:");
    for message in &diagnostics.messages {
        println!("    note: {message}");
    }
    if !diagnostics.violating_initials.is_empty() {
        println!(
            "    violating initials: {}",
            diagnostics.violating_initials.join(", ")
        );
    }
    if let Some(trace) = &diagnostics.counterexample_trace {
        println!("    counterexample trace: {}", trace.join(" -> "));
    }
    if !diagnostics.deadlock_traces.is_empty() {
        for (idx, trace) in diagnostics.deadlock_traces.iter().enumerate() {
            println!("    deadlock trace #{idx}: {}", trace.join(" -> "));
        }
    }
    if !diagnostics.counterstrategy_traces.is_empty() {
        for (idx, trace) in diagnostics.counterstrategy_traces.iter().enumerate() {
            println!("    counterstrategy trace #{idx}: {}", trace.join(" -> "));
        }
    }
    if !diagnostics.lasso_traces.is_empty() {
        for (idx, lasso) in diagnostics.lasso_traces.iter().enumerate() {
            if lasso.cycle.is_empty() {
                println!("    lasso trace #{idx}: {}", lasso.prefix.join(" -> "));
            } else {
                println!(
                    "    lasso trace #{idx}: {} -> ({})^ω",
                    lasso.prefix.join(" -> "),
                    lasso.cycle.join(" -> ")
                );
            }
        }
    }
    if let Some(report) = &diagnostics.minimization {
        println!(
            "    minimisation removed {} states and {} transitions",
            report.removed_states, report.removed_transitions
        );
        if !report.merged_states.is_empty() {
            println!("    merged states: {}", report.merged_states.join(", "));
        }
    }
    if !diagnostics.proof_obligations.is_empty() {
        println!(
            "    proof obligations: {}",
            diagnostics.proof_obligations.len()
        );
    }
    if let Some(strategy) = &diagnostics.counterstrategy {
        println!("    counterstrategy states: {}", strategy.states.join(", "));
    }
}
