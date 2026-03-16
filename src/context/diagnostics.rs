//! Diagnostics and reachability helpers for controller synthesis.
//!
//! This submodule centralises common logic for:
//! - computing the set of reachable satisfying states (BFS),
//! - building diagnostics for unrealizable controllers, and
//! - enriching diagnostics when some initial states are excluded.

use std::collections::{HashMap, VecDeque};

use bitvec::prelude::{BitVec, Lsb0};

use crate::clts::{Clts, CltsBuilder, DefaultLabelIdx, DefaultStateIdx, StateId};

use super::{
    Context, ContextError, ControllerDiagnostics, ControllerSynthesis, ControllerSynthesisOptions,
    ProofObligation,
};

use super::CounterExampleExplorer;

/// Result of a breadth-first search over the satisfying region of a CLTS.
#[derive(Debug)]
pub(crate) struct BfsReachability {
    /// Bitset of reachable states that satisfy the requested specification.
    pub visited: BitVec<usize, Lsb0>,
    /// Predecessor relation used to reconstruct counterexample traces.
    pub parent: HashMap<StateId<DefaultStateIdx>, StateId<DefaultStateIdx>>,
    /// Initial states that do *not* satisfy the specification.
    pub violating_initials: Vec<StateId<DefaultStateIdx>>,
}

/// Runs a breadth-first search from all satisfying initial states of `clts`,
/// restricted to states whose bit in `keep_bits` is set.
///
/// Returns:
/// - `visited`: bitset of reachable satisfying states,
/// - `parent`: predecessor relation for reconstructing paths,
/// - `violating_initials`: initial states that do not satisfy the specification.
pub(crate) fn bfs_reachable_states(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    keep_bits: &BitVec<usize, Lsb0>,
) -> BfsReachability {
    let mut visited = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
    let mut queue = VecDeque::new();
    let mut parent: HashMap<StateId<DefaultStateIdx>, StateId<DefaultStateIdx>> = HashMap::new();
    let mut violating_initials = Vec::new();

    // Seed BFS with satisfying initial states and collect violating initials.
    for initial in clts.initial_states() {
        if keep_bits.get(initial.index()).is_some_and(|bit| *bit) {
            visited.set(initial.index(), true);
            queue.push_back(*initial);
        } else {
            violating_initials.push(*initial);
        }
    }

    // Standard BFS restricted to states satisfying `keep_bits`.
    while let Some(state) = queue.pop_front() {
        for transition in clts.outgoing(state) {
            let target = transition.target();
            if keep_bits.get(target.index()).is_some_and(|bit| *bit)
                && !visited.get(target.index()).is_some_and(|bit| *bit)
            {
                parent.insert(target, state);
                visited.set(target.index(), true);
                queue.push_back(target);
            }
        }
    }

    BfsReachability {
        visited,
        parent,
        violating_initials,
    }
}

/// Builds a `ControllerSynthesis` for the case where no initial state satisfies
/// the specification (i.e., the controller is unrealizable).
pub(crate) fn build_unrealizable_initials_synthesis(
    ctx: &Context,
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    keep_bits: &BitVec<usize, Lsb0>,
    violating_initials: &[StateId<DefaultStateIdx>],
    options: &ControllerSynthesisOptions<'_>,
) -> Result<ControllerSynthesis, ContextError> {
    let mut diagnostics = ControllerDiagnostics::default();
    let proof_obligations_enabled = options
        .diagnostics
        .is_none_or(|diag| diag.proof_obligations);

    let builder = CltsBuilder::with_label_store(ctx.label_store.clone());
    let controller = builder.build().map_err(ContextError::Controller)?;

    let violating_names: Vec<String> = violating_initials
        .iter()
        .filter_map(|state| clts.state_name(*state))
        .map(|name| name.to_owned())
        .collect();

    diagnostics.violating_initials = violating_names.clone();
    if !violating_names.is_empty() {
        diagnostics.messages.push(format!(
            "Controller unrealizable: initial state(s) {} do not satisfy the specification.",
            violating_names.join(", ")
        ));
        if proof_obligations_enabled {
            diagnostics.proof_obligations = violating_names
                .iter()
                .map(|state| ProofObligation {
                    state: state.clone(),
                    detail: Some(
                        "Initial state violates the specification and cannot be controlled."
                            .to_owned(),
                    ),
                })
                .collect();
        }
        if options.diagnostics.is_some_and(|diag| diag.counterexample) {
            if let Some(first) = violating_names.first() {
                diagnostics.counterexample_trace = Some(vec![first.clone()]);
            }
            diagnostics.counterstrategy_traces = violating_names
                .iter()
                .map(|name| vec![name.clone()])
                .collect();
            let explorer = CounterExampleExplorer::build(violating_initials, clts, keep_bits);
            diagnostics.counterstrategy = Some(explorer.strategy(clts));
        } else if let Some(first) = violating_names.first() {
            diagnostics.counterexample_trace = Some(vec![first.clone()]);
        }
    } else {
        diagnostics.messages.push(
            "Controller unrealizable: no initial state satisfies the specification.".to_owned(),
        );
        if proof_obligations_enabled {
            diagnostics.proof_obligations.push(ProofObligation {
                state: "initial_states".to_owned(),
                detail: Some("No initial state satisfies the requested specification.".to_owned()),
            });
        }
    }

    Ok(ControllerSynthesis {
        controller,
        realizable: false,
        diagnostics,
    })
}

/// Enriches diagnostics for the case where some initial states are excluded from
/// the controller even though the specification is realizable.
pub(crate) fn enrich_diagnostics_for_excluded_initials(
    diagnostics: &mut ControllerDiagnostics,
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    keep_bits: &BitVec<usize, Lsb0>,
    violating_initials: &[StateId<DefaultStateIdx>],
    options: &ControllerSynthesisOptions<'_>,
    proof_obligations_enabled: bool,
) {
    if violating_initials.is_empty() {
        return;
    }

    let counterexample_enabled = options.diagnostics.is_some_and(|diag| diag.counterexample);
    let explorer = if counterexample_enabled {
        Some(CounterExampleExplorer::build(
            violating_initials,
            clts,
            keep_bits,
        ))
    } else {
        None
    };

    let names: Vec<String> = violating_initials
        .iter()
        .map(|state| clts.state_name(*state).unwrap_or("state").to_owned())
        .collect();
    diagnostics.violating_initials = names.clone();
    if proof_obligations_enabled {
        diagnostics
            .proof_obligations
            .extend(names.iter().map(|name| ProofObligation {
                state: name.clone(),
                detail: Some("Initial state excluded from controller; provide environment strategy or relax specification.".to_owned()),
            }));
    }
    diagnostics.messages.push(format!(
        "Initial state(s) excluded from controller: {}.",
        names.join(", ")
    ));

    if let Some(explorer) = explorer {
        let (trace, mut counter_paths) = explorer.minimal_traces(clts, keep_bits);
        if let Some(limit) = options.diagnostics.and_then(|diag| diag.max_counter_traces) {
            counter_paths.truncate(limit);
        }
        diagnostics.counterexample_trace = Some(trace);
        diagnostics.counterstrategy_traces = counter_paths;
        diagnostics.counterstrategy = Some(explorer.strategy(clts));
    } else if let Some(first) = names.first() {
        diagnostics
            .counterexample_trace
            .get_or_insert_with(|| vec![first.clone()]);
    }
}
