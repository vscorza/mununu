//! SCC-based discharge check for assume/guarantee contracts.
//!
//! Task A2 of `docs/design/black-box-modules.md`. Given a `ContractSet`,
//! builds the directed graph where each clause is a node and each
//! `(discharger, dischargee)` edge means "this guarantee discharges that
//! assumption," then runs Tarjan SCC to detect cycles. The verdict tells
//! the user whether the standard non-circular Pnueli (1985) rule suffices,
//! whether circular reasoning is required (McMillan 1999), or whether the
//! graph is incomplete and the analysis must be conservative.
//!
//! The lightweight mu-rank McMillan-style check from task A8 (see §8.8 of
//! the design doc) is a future extension on top of this module — it will
//! attempt to discharge non-trivial SCCs before falling back to HITL.

use super::{ContractSet, DischargeEdge};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Outcome of running the discharge check on a `ContractSet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DischargeVerdict {
    /// Every SCC is a singleton: the discharge order is a linear topological
    /// sort. The Pnueli 1985 rule applies — non-circular A/G suffices.
    Acyclic {
        /// One ordering of clauses that respects discharge dependencies.
        topological: Vec<String>,
        /// Assumptions that have no in-graph discharger but are declared as
        /// top-level environment assumptions (consistent with the user's
        /// declaration in `ContractSet::environment_assumptions`).
        unmet_environment: Vec<String>,
    },
    /// At least one non-trivial SCC: circular reasoning is required.
    /// Mununu refuses to silently accept and surfaces the cycles for HITL
    /// review (per §3.x of the design doc).
    Circular {
        /// Every non-trivial SCC (each as the list of clause ids forming
        /// the cycle).
        cycles: Vec<Vec<String>>,
        /// Singletons (clauses outside any cycle) in topological order.
        acyclic_remainder: Vec<String>,
    },
    /// One or more clauses referenced in `discharges` or
    /// `environment_assumptions` are not present in the clause list. The
    /// graph is incomplete; treat every unresolved id as potentially in a
    /// cycle until it can be loaded.
    PotentiallyCircular {
        /// The ids that did not resolve.
        unresolved: Vec<String>,
        /// Best-effort verdict over the resolved portion.
        partial: Box<DischargeVerdict>,
    },
    /// Some assumptions in the set have neither an in-graph discharger nor
    /// an entry in `environment_assumptions`. Treated as an unmet obligation
    /// — distinct from a circular cycle because it is structurally fixable.
    Unmet {
        /// Assumption ids without any guarantor or top-level environment
        /// declaration.
        missing_dischargers: Vec<String>,
        /// Best-effort verdict over the rest.
        partial: Box<DischargeVerdict>,
    },
}

impl DischargeVerdict {
    /// Whether mununu is willing to proceed to verification under this
    /// verdict without explicit user override.
    ///
    /// - `Acyclic` → always green to proceed (subject to unmet env list
    ///   being acceptable; that is a separate user decision).
    /// - `Circular` → blocks; require HITL.
    /// - `PotentiallyCircular` → blocks; the user needs to refresh the
    ///   corpus or fill the gap.
    /// - `Unmet` → blocks; structural fix required.
    pub fn auto_proceed(&self) -> bool {
        matches!(self, DischargeVerdict::Acyclic { .. })
    }
}

/// Run the discharge check on a contract set.
pub fn validate(set: &ContractSet) -> DischargeVerdict {
    // 1. Resolve unknown ids first. If any edge or env assumption refers
    //    to a missing clause, return PotentiallyCircular with the partial
    //    over the resolved portion.
    let unresolved = set.unknown_ids();
    if !unresolved.is_empty() {
        let resolved_set = strip_unresolved(set, &unresolved);
        let partial = validate(&resolved_set);
        return DischargeVerdict::PotentiallyCircular {
            unresolved,
            partial: Box::new(partial),
        };
    }

    // 2. Build the index: clause id → node index.
    let id_to_idx: HashMap<&str, usize> = set
        .clauses
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let n = set.clauses.len();

    // 3. Build adjacency list. Edge from discharger to dischargee — we use
    //    that direction so the SCC reflects "guarantor feeds consumer."
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for edge in &set.discharges {
        let (Some(&u), Some(&v)) = (
            id_to_idx.get(edge.discharger.as_str()),
            id_to_idx.get(edge.dischargee.as_str()),
        ) else {
            // Already excluded by unresolved check above; defensive.
            continue;
        };
        adj[u].push(v);
    }

    // 4. Tarjan SCC.
    let sccs = tarjan(&adj);

    // 5. Classify each SCC and collect cycles vs singletons.
    let mut cycles: Vec<Vec<String>> = Vec::new();
    let mut singletons: Vec<String> = Vec::new();
    for scc in &sccs {
        if scc.len() == 1 {
            // Singleton — but check for self-loop (still a cycle).
            let v = scc[0];
            if adj[v].contains(&v) {
                cycles.push(vec![set.clauses[v].id.clone()]);
            } else {
                singletons.push(set.clauses[v].id.clone());
            }
        } else {
            // Non-trivial SCC — by definition all nodes mutually reach.
            cycles.push(scc.iter().map(|&i| set.clauses[i].id.clone()).collect());
        }
    }

    // 6. Detect unmet assumptions: every Assumption clause must be either
    //    discharged by some edge or listed in environment_assumptions.
    let dischargee_ids: std::collections::HashSet<&str> = set
        .discharges
        .iter()
        .map(|e| e.dischargee.as_str())
        .collect();
    let env_ids: std::collections::HashSet<&str> = set
        .environment_assumptions
        .iter()
        .map(|s| s.as_str())
        .collect();
    let mut missing_dischargers: Vec<String> = set
        .clauses
        .iter()
        .filter(|c| c.kind.requires_discharge())
        .filter(|c| !dischargee_ids.contains(c.id.as_str()))
        .filter(|c| !env_ids.contains(c.id.as_str()))
        .map(|c| c.id.clone())
        .collect();
    missing_dischargers.sort();

    // 7. Build the underlying verdict from cycles vs acyclic remainder.
    let core = if cycles.is_empty() {
        // Walk SCC list in *reverse* order — Tarjan emits in reverse
        // topological order.
        let topological: Vec<String> = sccs
            .iter()
            .rev()
            .flat_map(|scc| scc.iter().map(|&i| set.clauses[i].id.clone()))
            .collect();
        let unmet_environment = set
            .environment_assumptions
            .iter()
            .filter(|id| id_to_idx.contains_key(id.as_str()))
            .filter(|id| !dischargee_ids.contains(id.as_str()))
            .cloned()
            .collect();
        DischargeVerdict::Acyclic {
            topological,
            unmet_environment,
        }
    } else {
        let mut acyclic_remainder = singletons;
        acyclic_remainder.sort();
        DischargeVerdict::Circular {
            cycles,
            acyclic_remainder,
        }
    };

    if missing_dischargers.is_empty() {
        core
    } else {
        DischargeVerdict::Unmet {
            missing_dischargers,
            partial: Box::new(core),
        }
    }
}

/// Build a copy of the set with edges/env-assumptions that reference
/// unresolved ids stripped out, so we can compute a partial verdict over
/// the resolved portion.
fn strip_unresolved(set: &ContractSet, unresolved: &[String]) -> ContractSet {
    let unresolved_set: std::collections::HashSet<&str> =
        unresolved.iter().map(|s| s.as_str()).collect();
    let discharges: Vec<DischargeEdge> = set
        .discharges
        .iter()
        .filter(|e| {
            !unresolved_set.contains(e.discharger.as_str())
                && !unresolved_set.contains(e.dischargee.as_str())
        })
        .cloned()
        .collect();
    let environment_assumptions: Vec<String> = set
        .environment_assumptions
        .iter()
        .filter(|s| !unresolved_set.contains(s.as_str()))
        .cloned()
        .collect();
    ContractSet {
        clauses: set.clauses.clone(),
        discharges,
        environment_assumptions,
    }
}

/// Tarjan strongly-connected-components. Returns SCCs in reverse
/// topological order (the first SCC has no in-edges from later SCCs).
fn tarjan(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adj.len();
    let mut index_counter: usize = 0;
    let mut stack: Vec<usize> = Vec::new();
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut indices: Vec<Option<usize>> = vec![None; n];
    let mut lowlink: Vec<usize> = vec![0; n];
    let mut result: Vec<Vec<usize>> = Vec::new();

    // Iterative Tarjan to avoid recursion-depth issues on pathological
    // contract sets.
    for start in 0..n {
        if indices[start].is_some() {
            continue;
        }
        // Each frame on `call_stack` is (node, iter_position).
        let mut call_stack: Vec<(usize, usize)> = Vec::new();
        indices[start] = Some(index_counter);
        lowlink[start] = index_counter;
        index_counter += 1;
        stack.push(start);
        on_stack[start] = true;
        call_stack.push((start, 0));

        while let Some(&(v, i)) = call_stack.last() {
            if i < adj[v].len() {
                let w = adj[v][i];
                // Advance the iterator at this frame before descending.
                call_stack.last_mut().unwrap().1 += 1;
                if indices[w].is_none() {
                    indices[w] = Some(index_counter);
                    lowlink[w] = index_counter;
                    index_counter += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call_stack.push((w, 0));
                } else if on_stack[w] {
                    let w_idx = indices[w].unwrap();
                    if w_idx < lowlink[v] {
                        lowlink[v] = w_idx;
                    }
                }
            } else {
                // Done with this node — pop and propagate lowlink.
                call_stack.pop();
                let v_idx = indices[v].unwrap();
                if lowlink[v] == v_idx {
                    let mut scc = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        scc.push(w);
                        if w == v {
                            break;
                        }
                    }
                    scc.reverse();
                    result.push(scc);
                }
                if let Some(&(parent, _)) = call_stack.last()
                    && lowlink[v] < lowlink[parent]
                {
                    lowlink[parent] = lowlink[v];
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::super::{ClauseKind, ClauseProvenance, ContractClause};
    use super::*;

    fn clause(id: &str, kind: ClauseKind, owner: &str) -> ContractClause {
        ContractClause {
            id: id.to_string(),
            kind,
            owner: owner.to_string(),
            description: None,
            provenance: ClauseProvenance::UserAuthored,
        }
    }

    fn edge(discharger: &str, dischargee: &str) -> DischargeEdge {
        DischargeEdge {
            discharger: discharger.to_string(),
            dischargee: dischargee.to_string(),
        }
    }

    #[test]
    fn acyclic_chain_reports_topological_order() {
        // G_a discharges A_b; G_b discharges A_c. Linear chain.
        let set = ContractSet {
            clauses: vec![
                clause("G_a", ClauseKind::Guarantee, "a"),
                clause("A_b", ClauseKind::Assumption, "b"),
                clause("G_b", ClauseKind::Guarantee, "b"),
                clause("A_c", ClauseKind::Assumption, "c"),
            ],
            discharges: vec![edge("G_a", "A_b"), edge("G_b", "A_c")],
            environment_assumptions: vec![],
        };
        match validate(&set) {
            DischargeVerdict::Acyclic { topological, .. } => {
                assert_eq!(topological.len(), 4);
                let pos = |id: &str| topological.iter().position(|s| s == id).unwrap();
                assert!(pos("G_a") < pos("A_b"));
                assert!(pos("G_b") < pos("A_c"));
            }
            other => panic!("expected Acyclic, got {:?}", other),
        }
    }

    #[test]
    fn self_loop_is_a_cycle() {
        let set = ContractSet {
            clauses: vec![clause("X", ClauseKind::Guarantee, "x")],
            discharges: vec![edge("X", "X")],
            environment_assumptions: vec![],
        };
        match validate(&set) {
            DischargeVerdict::Circular { cycles, .. } => {
                assert_eq!(cycles, vec![vec!["X".to_string()]]);
            }
            other => panic!("expected Circular, got {:?}", other),
        }
    }

    #[test]
    fn two_node_cycle_is_circular() {
        // A's guarantee discharges B's assumption; B's guarantee discharges A's.
        let set = ContractSet {
            clauses: vec![
                clause("G_a", ClauseKind::Guarantee, "a"),
                clause("A_a", ClauseKind::Assumption, "a"),
                clause("G_b", ClauseKind::Guarantee, "b"),
                clause("A_b", ClauseKind::Assumption, "b"),
            ],
            discharges: vec![
                edge("G_a", "A_b"),
                edge("G_b", "A_a"),
                edge("A_b", "G_b"),
                edge("A_a", "G_a"),
            ],
            environment_assumptions: vec![],
        };
        match validate(&set) {
            DischargeVerdict::Circular { cycles, .. } => {
                assert_eq!(cycles.len(), 1);
                let cycle = &cycles[0];
                assert_eq!(cycle.len(), 4);
                for expected in ["G_a", "A_a", "G_b", "A_b"] {
                    assert!(
                        cycle.iter().any(|id| id == expected),
                        "cycle should contain {}, got {:?}",
                        expected,
                        cycle
                    );
                }
            }
            other => panic!("expected Circular, got {:?}", other),
        }
    }

    #[test]
    fn unresolved_id_yields_potentially_circular() {
        let set = ContractSet {
            clauses: vec![clause("A_x", ClauseKind::Assumption, "x")],
            // Discharger does not exist in clauses.
            discharges: vec![edge("G_missing", "A_x")],
            environment_assumptions: vec![],
        };
        match validate(&set) {
            DischargeVerdict::PotentiallyCircular { unresolved, .. } => {
                assert_eq!(unresolved, vec!["G_missing".to_string()]);
            }
            other => panic!("expected PotentiallyCircular, got {:?}", other),
        }
    }

    #[test]
    fn unmet_assumption_without_env_is_flagged() {
        // A_b has no discharger and is not declared as env.
        let set = ContractSet {
            clauses: vec![
                clause("G_a", ClauseKind::Guarantee, "a"),
                clause("A_b", ClauseKind::Assumption, "b"),
            ],
            discharges: vec![],
            environment_assumptions: vec![],
        };
        match validate(&set) {
            DischargeVerdict::Unmet {
                missing_dischargers,
                partial,
            } => {
                assert_eq!(missing_dischargers, vec!["A_b".to_string()]);
                // Partial verdict over the rest should be Acyclic
                // (since the graph has no edges).
                assert!(matches!(*partial, DischargeVerdict::Acyclic { .. }));
            }
            other => panic!("expected Unmet, got {:?}", other),
        }
    }

    #[test]
    fn env_assumption_satisfies_obligation() {
        let set = ContractSet {
            clauses: vec![
                clause("G_a", ClauseKind::Guarantee, "a"),
                clause("A_b", ClauseKind::Assumption, "b"),
            ],
            discharges: vec![],
            environment_assumptions: vec!["A_b".to_string()],
        };
        match validate(&set) {
            DischargeVerdict::Acyclic { .. } => {}
            other => panic!("expected Acyclic, got {:?}", other),
        }
    }

    #[test]
    fn invariant_can_discharge_assumption() {
        let set = ContractSet {
            clauses: vec![
                clause("Inv_clock", ClauseKind::Invariant, "clock"),
                clause("A_module", ClauseKind::Assumption, "module"),
            ],
            discharges: vec![edge("Inv_clock", "A_module")],
            environment_assumptions: vec![],
        };
        match validate(&set) {
            DischargeVerdict::Acyclic { topological, .. } => {
                assert_eq!(topological.len(), 2);
                let pos = |id: &str| topological.iter().position(|s| s == id).unwrap();
                assert!(pos("Inv_clock") < pos("A_module"));
            }
            other => panic!("expected Acyclic, got {:?}", other),
        }
    }

    #[test]
    fn auto_proceed_only_on_acyclic() {
        assert!(
            DischargeVerdict::Acyclic {
                topological: vec![],
                unmet_environment: vec![]
            }
            .auto_proceed()
        );
        assert!(
            !DischargeVerdict::Circular {
                cycles: vec![],
                acyclic_remainder: vec![]
            }
            .auto_proceed()
        );
        assert!(
            !DischargeVerdict::Unmet {
                missing_dischargers: vec![],
                partial: Box::new(DischargeVerdict::Acyclic {
                    topological: vec![],
                    unmet_environment: vec![]
                })
            }
            .auto_proceed()
        );
    }
}
