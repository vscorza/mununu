//! Label hiding (τ-abstraction) for compositional verification.
//!
//! Hiding converts visible actions to internal (τ) actions. In CCS notation:
//! `P \ L` hides all actions in set `L`.
//!
//! Hiding is a prerequisite for effective bisimulation minimization — internal
//! actions can be absorbed by branching bisimulation, dramatically reducing
//! state count before composition.
//!
//! # References
//!
//! - Milner, R. (1989). *Communication and Concurrency*. Prentice Hall. Chapter 5.
//! - Garavel, H. et al. (2013). "CADP 2011: A Toolbox for the Construction and
//!   Analysis of Distributed Processes." *STTT*, 15(2):89-107.

use crate::clts::{Clts, CltsResult, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
use std::collections::HashSet;

/// Hide the specified labels in a CLTS by reclassifying them as internal.
///
/// Returns a new CLTS with the same states and transitions, but where
/// the specified labels are marked as [`LabelControllability::Internal`].
/// Hidden labels no longer synchronize during parallel composition —
/// they interleave independently.
///
/// # Parameters
///
/// * `clts` — the source CLTS
/// * `labels_to_hide` — label name strings to reclassify as internal
///
/// # Example
///
/// ```text
/// Before: ev_log (Uncontrollable), ev_request (Uncontrollable), ev_close (Controllable)
/// hide({"ev_log"})
/// After:  ev_log (Internal), ev_request (Uncontrollable), ev_close (Controllable)
/// ```
pub fn hide_labels(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    labels_to_hide: &HashSet<String>,
) -> CltsResult<Clts<DefaultStateIdx, DefaultLabelIdx>> {
    let mut builder = Clts::builder();

    // 1. Copy all states, preserving initial flags and variables
    let mut state_mapping = std::collections::HashMap::new();
    for state in clts.states() {
        let name = clts.state_name(state).unwrap_or("state").to_owned();
        if let Some(new_id) = builder.state_with_name(name) {
            if clts.initial_states().contains(&state) {
                builder.initial_state_id(new_id);
            }
            let vars = clts.state_variables(state);
            if !vars.is_empty() {
                builder.with_variables_for_state(new_id, vars.iter().map(|s| s.as_str()));
            }
            state_mapping.insert(state, new_id);
        }
    }

    // 2. Copy transitions, reclassifying hidden labels as Internal
    for state in clts.states() {
        let &source_new = match state_mapping.get(&state) {
            Some(id) => id,
            None => continue,
        };

        for transition in clts.outgoing(state) {
            let &target_new = match state_mapping.get(&transition.target()) {
                Some(id) => id,
                None => continue,
            };

            // Intern each label set from the original transition
            for &label_id in transition.labels() {
                let payload = match clts.label_payload(label_id) {
                    Some(p) => p,
                    None => continue,
                };

                let new_label_id = builder
                    .labels()
                    .intern(payload.iter().map(|s| s.as_str()))?;

                // Determine controllability: hide if ANY symbol in the label is in the hide set
                let should_hide = payload.iter().any(|s| labels_to_hide.contains(s));
                let controllability = if should_hide {
                    LabelControllability::Internal
                } else {
                    clts.label_controllability(label_id)
                        .unwrap_or(LabelControllability::Uncontrollable)
                };

                builder.set_label_controllability(new_label_id, controllability);
                builder.transition_ids(source_new, &[new_label_id], target_new);
            }
        }
    }

    builder.build()
}

/// Statistics from a hiding operation.
#[derive(Debug, Clone)]
pub struct HideResult {
    /// Number of labels hidden (reclassified to Internal).
    pub labels_hidden: usize,
    /// Total labels in the CLTS.
    pub total_labels: usize,
}

/// Hide labels and return statistics.
pub fn hide_labels_with_stats(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    labels_to_hide: &HashSet<String>,
) -> CltsResult<(Clts<DefaultStateIdx, DefaultLabelIdx>, HideResult)> {
    let total_before = clts.alphabet().len();
    let internal_before = clts.internal_alphabet().len();

    let result = hide_labels(clts, labels_to_hide)?;

    let internal_after = result.internal_alphabet().len();
    let labels_hidden = internal_after.saturating_sub(internal_before);

    Ok((
        result,
        HideResult {
            labels_hidden,
            total_labels: total_before,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_clts() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
        let mut builder = Clts::builder();

        let s0 = builder.state_with_name("S0".to_string()).unwrap();
        let s1 = builder.state_with_name("S1".to_string()).unwrap();
        let s2 = builder.state_with_name("S2".to_string()).unwrap();
        builder.initial_state_id(s0);

        let ev_a = builder.labels().intern(["ev_a"]).unwrap();
        let ev_b = builder.labels().intern(["ev_b"]).unwrap();
        let ev_c = builder.labels().intern(["ev_c"]).unwrap();
        let noop = builder.labels().intern(["noop"]).unwrap();

        builder.set_label_controllability(ev_a, LabelControllability::Controllable);
        builder.set_label_controllability(ev_b, LabelControllability::Uncontrollable);
        builder.set_label_controllability(ev_c, LabelControllability::Controllable);
        builder.set_label_controllability(noop, LabelControllability::Uncontrollable);

        builder.transition_ids(s0, &[ev_a], s1);
        builder.transition_ids(s1, &[ev_b], s2);
        builder.transition_ids(s2, &[ev_c], s0);
        builder.transition_ids(s0, &[noop], s0);
        builder.transition_ids(s1, &[noop], s1);
        builder.transition_ids(s2, &[noop], s2);

        builder.build().unwrap()
    }

    #[test]
    fn hide_single_label() {
        let clts = build_test_clts();
        let internal_before = clts.internal_alphabet().len();
        assert_eq!(internal_before, 0);

        let mut to_hide = HashSet::new();
        to_hide.insert("ev_b".to_string());

        let hidden = hide_labels(&clts, &to_hide).unwrap();

        assert_eq!(hidden.state_count(), 3);
        // ev_b should now be internal
        assert!(!hidden.internal_alphabet().is_empty());
    }

    #[test]
    fn hide_preserves_state_count() {
        let clts = build_test_clts();
        let mut to_hide = HashSet::new();
        to_hide.insert("ev_a".to_string());
        to_hide.insert("ev_c".to_string());

        let hidden = hide_labels(&clts, &to_hide).unwrap();
        assert_eq!(hidden.state_count(), clts.state_count());
    }

    #[test]
    fn hide_empty_set_preserves_classification() {
        let clts = build_test_clts();
        let empty: HashSet<String> = HashSet::new();
        let hidden = hide_labels(&clts, &empty).unwrap();

        assert_eq!(hidden.state_count(), clts.state_count());
        assert_eq!(hidden.internal_alphabet().len(), 0);
    }

    #[test]
    fn hide_with_stats() {
        let clts = build_test_clts();
        let mut to_hide = HashSet::new();
        to_hide.insert("ev_b".to_string());

        let (hidden, stats) = hide_labels_with_stats(&clts, &to_hide).unwrap();
        assert_eq!(hidden.state_count(), 3);
        assert!(stats.labels_hidden > 0);
        assert!(stats.total_labels > 0);
    }

    #[test]
    fn hide_preserves_initial_states() {
        let clts = build_test_clts();
        let mut to_hide = HashSet::new();
        to_hide.insert("ev_a".to_string());

        let hidden = hide_labels(&clts, &to_hide).unwrap();
        assert!(!hidden.initial_states().is_empty());
    }
}
