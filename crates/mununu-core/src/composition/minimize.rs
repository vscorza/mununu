//! Strong bisimulation minimization via partition refinement.
//!
//! Computes the coarsest bisimulation equivalence on a CLTS. Two states are
//! strongly bisimilar iff they have identical outgoing transition signatures
//! (same labels to equivalent target states, same variable assignments).
//! The quotient CLTS has one state per equivalence class.
//!
//! This is the key scalability technique for compositional verification:
//! minimize each component independently before composing.
//!
//! # Algorithm
//!
//! Partition refinement (Kanellakis–Smolka):
//! 1. Start with all states in one partition block
//! 2. Compute each state's signature: (variables, sorted transitions to partition blocks)
//! 3. Split blocks where states have different signatures
//! 4. Repeat until stable
//!
//! Complexity: O(m·n) where m = transitions, n = states (naïve).
//! Paige–Tarjan (1987) achieves O(m·log n) but is not needed for our scale.
//!
//! # References
//!
//! - Kanellakis, P.C. & Smolka, S.A. (1990). "CCS expressions, finite state
//!   processes, and three problems of equivalence." *Information and Computation*,
//!   86(1):43-68.
//! - Paige, R. & Tarjan, R.E. (1987). "Three partition refinement algorithms."
//!   *SIAM J. Computing*, 16(6):973-989.
//! - van Glabbeek, R.J. & Weijland, W.P. (1996). "Branching time and abstraction
//!   in bisimulation semantics." *JACM*, 43(3):555-600.

use crate::clts::{
    Clts, CltsBuilder, CltsError, CltsResult, DefaultLabelIdx, DefaultStateIdx, LabelId, StateId,
};
use bitvec::prelude::*;
use std::collections::{HashMap, HashSet};

/// Result of minimization with statistics.
#[derive(Debug, Clone)]
pub struct MinimizationReport {
    /// Number of states before minimization.
    pub states_before: usize,
    /// Number of states after minimization.
    pub states_after: usize,
    /// Number of transitions before minimization.
    pub transitions_before: usize,
    /// Number of transitions after minimization.
    pub transitions_after: usize,
    /// Names of states that were merged into representatives.
    pub merged_states: Vec<String>,
}

/// Minimize a CLTS by strong bisimulation quotient.
///
/// Returns `None` if the CLTS is already minimal (no states can be merged).
/// Returns `Some((minimized_clts, report))` if reduction occurred.
///
/// When `label_store` is provided, the builder reuses the shared label store
/// (preserving label IDs across contexts). When `None`, a fresh store is used.
pub fn minimize_bisimulation(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    label_store: Option<crate::clts::LabelStoreBuilder<DefaultLabelIdx>>,
) -> CltsResult<Option<(Clts<DefaultStateIdx, DefaultLabelIdx>, MinimizationReport)>> {
    let state_count = clts.state_count();
    if state_count <= 1 {
        return Ok(None);
    }

    // Phase 1: Partition refinement
    let mut partition = vec![0usize; state_count];
    let mut changed = true;

    while changed {
        changed = false;
        let mut sig_map: HashMap<StateSignature, usize> = HashMap::new();
        let mut next_partition = vec![0usize; state_count];
        let mut next_id = 0usize;

        for state in clts.states() {
            let signature = StateSignature::compute(clts, state, &partition);
            let entry = sig_map.entry(signature).or_insert_with(|| {
                let id = next_id;
                next_id += 1;
                id
            });
            next_partition[state.index()] = *entry;
        }

        if next_partition != partition {
            partition = next_partition;
            changed = true;
        }
    }

    // Check if any merging occurred
    let class_count = partition.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    if class_count == state_count {
        return Ok(None); // Already minimal
    }

    // Phase 2: Build quotient CLTS
    let mut class_members: Vec<Vec<StateId<DefaultStateIdx>>> = vec![Vec::new(); class_count];
    for state in clts.states() {
        class_members[partition[state.index()]].push(state);
    }

    let mut class_info: Vec<Vec<StateId<DefaultStateIdx>>> = class_members
        .into_iter()
        .filter(|members| !members.is_empty())
        .collect();

    for members in &mut class_info {
        members.sort_by_key(|s| s.index());
    }
    class_info.sort_by_key(|members| members[0].index());

    if class_info.is_empty() {
        return Ok(None);
    }

    // Build the minimized CLTS
    let mut builder = if let Some(store) = label_store {
        CltsBuilder::with_label_store(store)
    } else {
        Clts::builder()
    };
    builder.reserve_states(class_info.len());

    let mut mapping: HashMap<StateId<DefaultStateIdx>, StateId<DefaultStateIdx>> = HashMap::new();
    let mut representatives = Vec::with_capacity(class_info.len());
    let mut report = MinimizationReport {
        states_before: state_count,
        states_after: 0,
        transitions_before: 0,
        transitions_after: 0,
        merged_states: Vec::new(),
    };

    report.transitions_before = clts.states().map(|s| clts.outgoing(s).len()).sum();

    for members in &class_info {
        let representative = members[0];
        let name = clts
            .state_name(representative)
            .unwrap_or("state")
            .to_owned();
        let new_state = builder.state_with_name(name).ok_or(CltsError::IdOverflow {
            kind: "state",
            value: usize::MAX,
        })?;

        // Initial if ANY member is initial
        if members.iter().any(|s| clts.initial_states().contains(s)) {
            builder.initial_state_id(new_state);
        }

        // Copy variables from representative
        let vars = clts.state_variables(representative);
        if !vars.is_empty() {
            builder.with_variables_for_state(new_state, vars.iter().map(|s| s.as_str()));
        }

        // Map all members to the representative
        for (idx, &state) in members.iter().enumerate() {
            mapping.insert(state, new_state);
            if idx > 0
                && let Some(name) = clts.state_name(state)
            {
                report.merged_states.push(name.to_owned());
            }
        }

        representatives.push((representative, new_state));
    }

    report.states_after = representatives.len();

    // Add deduplicated transitions
    let mut new_transition_count = 0usize;
    for &(source, new_source) in &representatives {
        let mut seen: HashSet<TransitionKey> = HashSet::new();
        for transition in clts.outgoing(source) {
            if let Some(&target_new) = mapping.get(&transition.target()) {
                let key = TransitionKey::from_transition(target_new, transition.labels());
                if seen.insert(key) {
                    builder.transition_ids(new_source, transition.labels(), target_new);
                    new_transition_count += 1;
                }
            }
        }
    }

    report.transitions_after = new_transition_count;

    let minimized = builder.build()?;
    Ok(Some((minimized, report)))
}

// ---------------------------------------------------------------------------
// Internal types for partition refinement
// ---------------------------------------------------------------------------

/// Signature of a state: variables + sorted transition fingerprints.
///
/// Two states with identical signatures are bisimilar and can be merged.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StateSignature {
    variables: BitVec<usize, Lsb0>,
    transitions: Vec<TransitionSignature>,
}

impl StateSignature {
    fn compute(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        state: StateId<DefaultStateIdx>,
        partition: &[usize],
    ) -> Self {
        let variables = clts.state_variable_bitset(state).bits().to_bitvec();
        let mut transitions: Vec<TransitionSignature> = clts
            .outgoing(state)
            .iter()
            .map(|t| {
                let mut labels: Vec<usize> = t.labels().iter().map(|l| l.index()).collect();
                labels.sort_unstable();
                TransitionSignature {
                    target_block: partition[t.target().index()],
                    labels,
                }
            })
            .collect();
        transitions.sort();
        // Use set-semantics on the outgoing-transition fingerprint, matching
        // standard strong bisimulation (Kanellakis-Smolka 1990, van
        // Glabbeek-Weijland 1996): two transitions with the same target block
        // and the same label set are the same edge. Without this dedup, the
        // partition refinement uses multiset semantics while the quotient
        // construction (`seen: HashSet<TransitionKey>` below) uses set
        // semantics — the inconsistency breaks idempotence: states with
        // different transition multiplicity can become bisimilar after the
        // quotient flattens duplicates, so a second pass finds further
        // merges.
        transitions.dedup();
        Self {
            variables,
            transitions,
        }
    }
}

/// Fingerprint of a single transition: target partition block + sorted labels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct TransitionSignature {
    target_block: usize,
    labels: Vec<usize>,
}

/// Key for deduplicating transitions in the quotient CLTS.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TransitionKey {
    target: usize,
    labels: Vec<usize>,
}

impl TransitionKey {
    fn from_transition(
        target: StateId<DefaultStateIdx>,
        labels: &[LabelId<DefaultLabelIdx>],
    ) -> Self {
        let mut label_ids: Vec<usize> = labels.iter().map(|l| l.index()).collect();
        label_ids.sort_unstable();
        Self {
            target: target.index(),
            labels: label_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::LabelControllability;

    fn build_diamond_clts() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
        // A → B (ev_left), A → C (ev_right), B → D (ev_join), C → D (ev_join)
        // B and C are bisimilar (same outgoing: ev_join → D)
        let mut builder = Clts::builder();
        let a = builder.state_with_name("A".to_string()).unwrap();
        let b = builder.state_with_name("B".to_string()).unwrap();
        let c = builder.state_with_name("C".to_string()).unwrap();
        let d = builder.state_with_name("D".to_string()).unwrap();
        builder.initial_state_id(a);

        let ev_left = builder.labels().intern(["ev_left"]).unwrap();
        let ev_right = builder.labels().intern(["ev_right"]).unwrap();
        let ev_join = builder.labels().intern(["ev_join"]).unwrap();
        let noop = builder.labels().intern(["noop"]).unwrap();

        builder.set_label_controllability(ev_left, LabelControllability::Controllable);
        builder.set_label_controllability(ev_right, LabelControllability::Controllable);
        builder.set_label_controllability(ev_join, LabelControllability::Controllable);
        builder.set_label_controllability(noop, LabelControllability::Uncontrollable);

        builder.transition_ids(a, &[ev_left], b);
        builder.transition_ids(a, &[ev_right], c);
        builder.transition_ids(b, &[ev_join], d);
        builder.transition_ids(c, &[ev_join], d);
        builder.transition_ids(d, &[noop], d);
        builder.transition_ids(a, &[noop], a);
        builder.transition_ids(b, &[noop], b);
        builder.transition_ids(c, &[noop], c);

        builder.build().unwrap()
    }

    #[test]
    fn diamond_merges_bisimilar_states() {
        let clts = build_diamond_clts();
        assert_eq!(clts.state_count(), 4);

        let result = minimize_bisimulation(&clts, None).unwrap();
        assert!(
            result.is_some(),
            "B and C should be bisimilar → reduction expected"
        );

        let (minimized, report) = result.unwrap();
        // B and C merge → 3 states remain (A, B≡C, D)
        assert_eq!(report.states_before, 4);
        assert_eq!(report.states_after, 3);
        assert_eq!(minimized.state_count(), 3);
        assert_eq!(report.merged_states.len(), 1); // one of B/C merged into the other
    }

    #[test]
    fn already_minimal_returns_none() {
        // A → B → C (all different signatures)
        let mut builder = Clts::builder();
        let a = builder.state_with_name("A".to_string()).unwrap();
        let b = builder.state_with_name("B".to_string()).unwrap();
        let c = builder.state_with_name("C".to_string()).unwrap();
        builder.initial_state_id(a);

        let ev_x = builder.labels().intern(["ev_x"]).unwrap();
        let ev_y = builder.labels().intern(["ev_y"]).unwrap();
        let ev_z = builder.labels().intern(["ev_z"]).unwrap();

        builder.transition_ids(a, &[ev_x], b);
        builder.transition_ids(b, &[ev_y], c);
        builder.transition_ids(c, &[ev_z], c);

        let clts = builder.build().unwrap();
        let result = minimize_bisimulation(&clts, None).unwrap();
        assert!(result.is_none(), "All states are distinct → no reduction");
    }

    #[test]
    fn single_state_returns_none() {
        let mut builder = Clts::builder();
        let s = builder.state_with_name("S".to_string()).unwrap();
        builder.initial_state_id(s);
        let noop = builder.labels().intern(["noop"]).unwrap();
        builder.transition_ids(s, &[noop], s);

        let clts = builder.build().unwrap();
        let result = minimize_bisimulation(&clts, None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn preserves_initial_states() {
        let clts = build_diamond_clts();
        let (minimized, _) = minimize_bisimulation(&clts, None).unwrap().unwrap();
        assert!(!minimized.initial_states().is_empty());
        assert_eq!(minimized.initial_states().len(), 1);
    }

    #[test]
    fn all_same_transitions_full_collapse() {
        // 4 states, all with identical noop self-loops → should collapse to 1
        let mut builder = Clts::builder();
        let s0 = builder.state_with_name("S0".to_string()).unwrap();
        let s1 = builder.state_with_name("S1".to_string()).unwrap();
        let s2 = builder.state_with_name("S2".to_string()).unwrap();
        let s3 = builder.state_with_name("S3".to_string()).unwrap();
        builder.initial_state_id(s0);

        let noop = builder.labels().intern(["noop"]).unwrap();
        builder.set_label_controllability(noop, LabelControllability::Uncontrollable);

        builder.transition_ids(s0, &[noop], s0);
        builder.transition_ids(s1, &[noop], s1);
        builder.transition_ids(s2, &[noop], s2);
        builder.transition_ids(s3, &[noop], s3);

        let clts = builder.build().unwrap();
        assert_eq!(clts.state_count(), 4);

        let (minimized, report) = minimize_bisimulation(&clts, None).unwrap().unwrap();
        assert_eq!(minimized.state_count(), 1);
        assert_eq!(report.merged_states.len(), 3);
    }

    #[test]
    fn transition_deduplication() {
        // After merging B≡C, transitions A→B and A→C become the same → only one in quotient
        let clts = build_diamond_clts();
        let (_minimized, report) = minimize_bisimulation(&clts, None).unwrap().unwrap();
        assert!(report.transitions_after < report.transitions_before);
    }

    #[test]
    fn idempotence_under_set_semantics() {
        // Regression test for the multiset-vs-set inconsistency: state X has
        // two transitions [c]→Y, [c]→Z while state W has one [c]→V. After
        // bisimulation Y, Z, V land in the same equivalence class. Without
        // set-semantics on the partition signature, X's signature is
        // {(class, [c]), (class, [c])} and W's is {(class, [c])} — different
        // multisets — so they don't merge on the first pass. The quotient
        // construction then dedupes X's two transitions to one, and a
        // second pass would find X and W bisimilar. With set-semantics on
        // the partition signature (`transitions.dedup()` in
        // StateSignature::compute), the first pass already sees them as
        // bisimilar and the second pass returns None.
        let mut builder = Clts::builder();
        let x = builder.state_with_name("X".to_string()).unwrap();
        let y = builder.state_with_name("Y".to_string()).unwrap();
        let z = builder.state_with_name("Z".to_string()).unwrap();
        let w = builder.state_with_name("W".to_string()).unwrap();
        let v = builder.state_with_name("V".to_string()).unwrap();
        builder.initial_state_id(x);

        let c = builder.labels().intern(["c"]).unwrap();
        builder.set_label_controllability(c, LabelControllability::Controllable);

        builder.transition_ids(x, &[c], y);
        builder.transition_ids(x, &[c], z);
        builder.transition_ids(w, &[c], v);

        let clts = builder.build().unwrap();
        assert_eq!(clts.state_count(), 5);

        let (m1, _) = minimize_bisimulation(&clts, None).unwrap().unwrap();
        let second = minimize_bisimulation(&m1, None).unwrap();
        assert!(
            second.is_none(),
            "minimize must be idempotent: second pass got {} states from {}-state input",
            second.as_ref().map(|(c, _)| c.state_count()).unwrap_or(0),
            m1.state_count(),
        );
    }
}
