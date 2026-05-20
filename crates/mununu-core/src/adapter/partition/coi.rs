//! Cone-of-influence computation over a [`super::DepGraphBuilder`].
//!
//! The algorithm is BFS from the property seeds through the dependency
//! graph; a signal is `Kept` iff it is reachable. This is the same
//! algorithm previously embedded in
//! `crate::adapter::systemverilog::kripke::compute_cone_of_influence`,
//! lifted to operate against the adapter-agnostic
//! [`super::DepGraphBuilder`] trait.
//!
//! # SOUNDNESS
//!
//! Over-approximation: if a signal is reachable in the (possibly
//! over-approximated) dep graph from any property atom, it is `Kept`.
//! `Dropped` signals are pinned to a single value via
//! [`crate::adapter::domain::AbstractionType::Ignored`], which adds
//! behaviours to the model (any value the signal could have taken in
//! the concrete system is collapsed into the pinned value's
//! equivalence class). For safety + over-approximation this is sound;
//! for liveness see [`crate::adapter::partition`] module docs.

use std::collections::{HashSet, VecDeque};

/// BFS reachability from `seeds` through `deps`.
///
/// `deps` is the adjacency returned by `DepGraphBuilder::build()`.
/// Returns the set of signal names transitively reachable from any
/// seed, **including** the seeds themselves (so a seed with no
/// outgoing edges is correctly classified `Kept`).
pub fn cone_of_influence(
    seeds: &HashSet<String>,
    deps: &std::collections::HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let mut reached = HashSet::with_capacity(deps.len());
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();

    while let Some(signal) = queue.pop_front() {
        if reached.insert(signal.clone())
            && let Some(signal_deps) = deps.get(&signal)
        {
            for dep in signal_deps {
                if !reached.contains(dep) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    reached
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn deps_of(pairs: &[(&str, &[&str])]) -> HashMap<String, HashSet<String>> {
        pairs
            .iter()
            .map(|(src, tgts)| {
                (
                    src.to_string(),
                    tgts.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect()
    }

    fn seeds(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn seed_with_no_edges_is_reached() {
        let deps = deps_of(&[]);
        let r = cone_of_influence(&seeds(&["a"]), &deps);
        assert!(r.contains("a"));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn transitive_dependencies_are_collected() {
        let deps = deps_of(&[("a", &["b"]), ("b", &["c"]), ("c", &[])]);
        let r = cone_of_influence(&seeds(&["a"]), &deps);
        assert!(r.contains("a") && r.contains("b") && r.contains("c"));
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn unrelated_signals_are_not_collected() {
        let deps = deps_of(&[("a", &["b"]), ("x", &["y"])]);
        let r = cone_of_influence(&seeds(&["a"]), &deps);
        assert!(r.contains("a") && r.contains("b"));
        assert!(!r.contains("x") && !r.contains("y"));
    }

    #[test]
    fn cycles_terminate() {
        let deps = deps_of(&[("a", &["b"]), ("b", &["a"])]);
        let r = cone_of_influence(&seeds(&["a"]), &deps);
        assert!(r.contains("a") && r.contains("b"));
        assert_eq!(r.len(), 2);
    }
}
