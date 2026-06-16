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

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::mu_calculus::{Formula, Node};

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

/// R.4 — Identifier for a single property in a clustering input.
///
/// Property identifiers are caller-chosen (typically the property's
/// CTXDSL name); the clustering helpers only use them to key the
/// returned cluster assignment, so they need to be cloneable and
/// hashable but carry no other semantics.
pub type PropertyId = String;

/// R.4 — Jaccard similarity of two sets: `|A ∩ B| / |A ∪ B|`.
///
/// Returns `1.0` when both sets are empty (vacuously identical), and
/// `0.0` when the union is non-empty but the intersection is empty.
/// Range `[0.0, 1.0]`.
///
/// Used by [`cluster_properties_by_jaccard`] to decide whether two
/// properties' cone signal sets overlap enough to share a partition.
/// Reference: `docs/design/native-sv-abstraction.md` §5 "Property
/// clustering" — the cheap-syntactic-first / refine-by-cone-overlap
/// two-pass discipline.
pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection: usize = a.iter().filter(|s| b.contains(*s)).count();
    let union: usize = a.len() + b.len() - intersection;
    if union == 0 {
        // Defensive — `a` and `b` both empty was handled above; this
        // branch is unreachable in practice but the explicit guard
        // keeps the divisor non-zero.
        return 1.0;
    }
    intersection as f64 / union as f64
}

/// R.4 — Result of clustering N properties by cone-overlap similarity.
///
/// Each entry pairs one cluster's member properties with the **union**
/// of their cone signal sets — the seed set the COI walk should consume
/// for that cluster's partition.
///
/// Invariant: every property in the input appears in exactly one
/// cluster; clusters are non-overlapping and exhaustive.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyCluster {
    /// Properties merged into this cluster.
    pub members: Vec<PropertyId>,
    /// Union of every member's cone signal set — the seeds the COI
    /// walk consumes for this cluster.
    pub seed_union: HashSet<String>,
}

/// R.4 — Group properties into clusters by Jaccard similarity on their
/// cone signal sets (the *cone* of each property, not its raw atom
/// set — that's the load-bearing distinction; two properties whose
/// raw atoms differ but whose transitive cones overlap heavily should
/// cluster together).
///
/// Algorithm — single-pass greedy clustering with a similarity floor:
///
/// 1. For each input property, compute its cone via [`cone_of_influence`]
///    from the property's seed atoms.
/// 2. Process properties in input order. For each property:
///    - Compare its cone against every existing cluster's `seed_union`
///      via [`jaccard`].
///    - If the best-matching cluster scores **≥ `similarity_floor`**,
///      merge this property in (cluster's `seed_union` becomes the
///      union of the old union and the new cone).
///    - Otherwise, create a new singleton cluster.
///
/// This is the cheap syntactic / single-pass half of the plan's §5
/// "two-pass" recipe. The refine-by-actual-fanin-overlap pass (the
/// second half) is structurally identical — it just re-runs this
/// algorithm with cones already in hand — so callers that want the
/// two-pass shape can pipe the output back in with a tighter floor.
///
/// **Choosing `similarity_floor`.** The plan recommends `0.5` as a
/// default (two properties with ≥ 50% cone overlap cluster together).
/// Tighter floors (closer to `1.0`) approach per-property COI;
/// looser floors (closer to `0.0`) collapse toward joint COI.
///
/// Returns clusters in deterministic input-order — the first property
/// processed seeds cluster 0, and so on. Ties in the best-match scan
/// resolve to the lowest-index existing cluster.
pub fn cluster_properties_by_jaccard(
    properties: &[(PropertyId, HashSet<String>)],
    deps: &HashMap<String, HashSet<String>>,
    similarity_floor: f64,
) -> Vec<PropertyCluster> {
    // Per-property cone — the COI walk's reach set, which is what we
    // actually want to compare (not the bare seed atoms).
    let cones: Vec<HashSet<String>> = properties
        .iter()
        .map(|(_, seeds)| cone_of_influence(seeds, deps))
        .collect();

    let mut clusters: Vec<PropertyCluster> = Vec::new();

    for (idx, (pid, _)) in properties.iter().enumerate() {
        let cone = &cones[idx];

        // Find the existing cluster with the highest Jaccard similarity
        // to this property's cone. Empty cluster list ⇒ start a new one.
        let mut best_cluster: Option<usize> = None;
        let mut best_score = -1.0_f64;
        for (cidx, cluster) in clusters.iter().enumerate() {
            let score = jaccard(cone, &cluster.seed_union);
            if score > best_score {
                best_score = score;
                best_cluster = Some(cidx);
            }
        }

        match best_cluster {
            Some(cidx) if best_score >= similarity_floor => {
                clusters[cidx].members.push(pid.clone());
                clusters[cidx].seed_union.extend(cone.iter().cloned());
            }
            _ => {
                clusters.push(PropertyCluster {
                    members: vec![pid.clone()],
                    seed_union: cone.clone(),
                });
            }
        }
    }

    clusters
}

/// R4W-1 (R.4 clustered-COI wiring, 2026-06-16) — harvest the
/// cone-of-influence **seed atoms** a μ-calculus formula references.
///
/// This is the glue the verify path was missing: [`cone_of_influence`]
/// and [`cluster_properties_by_jaccard`] consume per-property seed sets,
/// but nothing extracted those seeds from a parsed [`Formula`]. The
/// harvest walks the formula's node arena and collects:
///
/// - every atomic proposition ([`Node::Predicate`]);
/// - every modal guard's current-state and next-state signal
///   references (`req_cur` / `forb_cur` / `req_next` / `forb_next`);
/// - every modal guard's label set.
///
/// The returned names are *seeds*, not a cone — feed them to
/// [`cone_of_influence`] (or [`cluster_coi_report`]) to walk the
/// dependency graph. Names that do not appear in the dep graph are
/// harmless: the COI walk treats them as zero-out-degree seeds.
pub fn property_seed_atoms(formula: &Formula) -> HashSet<String> {
    let mut seeds = HashSet::new();
    for node in formula.nodes() {
        match node {
            Node::Predicate(name) => {
                seeds.insert(name.clone());
            }
            Node::Modal { guard, .. } => {
                seeds.extend(guard.labels.iter().cloned());
                seeds.extend(guard.current.required.iter().cloned());
                seeds.extend(guard.current.forbidden.iter().cloned());
                seeds.extend(guard.next.required.iter().cloned());
                seeds.extend(guard.next.forbidden.iter().cloned());
            }
            Node::True
            | Node::False
            | Node::Variable(_)
            | Node::Not(_)
            | Node::And(_, _)
            | Node::Or(_, _)
            | Node::Mu { .. }
            | Node::Nu { .. } => {}
        }
    }
    seeds
}

/// R4W-1 — One cluster's entry in a [`ClusterCoiReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterCoiEntry {
    /// Property identifiers merged into this cluster.
    pub members: Vec<PropertyId>,
    /// Size of this cluster's cone (the union of its members' cones) —
    /// the signal count a per-cluster bit-blast must carry.
    pub cone_size: usize,
}

/// R4W-1 — Joint-vs-clustered cone comparison, the core M.3 metric.
///
/// `joint_cone_size` is what a single naive COI over **all** properties
/// would keep (the union of every property's cone). `max_cluster_cone_size`
/// is the largest per-cluster cone — the binding constraint when each
/// cluster is analysed independently. Clustering provides value exactly
/// when `max_cluster_cone_size < joint_cone_size` (and, at the bit-blast
/// layer, when the joint cone busts the state-bit cap while each cluster
/// fits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterCoiReport {
    /// Signals a naive joint COI over all properties would keep.
    pub joint_cone_size: usize,
    /// Per-cluster entries (deterministic input order).
    pub clusters: Vec<ClusterCoiEntry>,
    /// Largest per-cluster cone — bounds independent per-cluster analysis.
    pub max_cluster_cone_size: usize,
}

/// R4W-1 — Compute the joint-vs-clustered COI comparison for a set of
/// properties over a dependency graph.
///
/// Each input is `(property_id, seed_atoms)` — typically
/// `(name, property_seed_atoms(&formula))`. `deps` is the design's
/// signal dependency graph; `similarity_floor` is the Jaccard threshold
/// passed to [`cluster_properties_by_jaccard`] (see its docs for the
/// `0.5` default and the floor→{joint, per-property} limits).
pub fn cluster_coi_report(
    properties: &[(PropertyId, HashSet<String>)],
    deps: &HashMap<String, HashSet<String>>,
    similarity_floor: f64,
) -> ClusterCoiReport {
    // Joint COI = cone over the union of every property's seed atoms.
    let joint_seeds: HashSet<String> = properties
        .iter()
        .flat_map(|(_, s)| s.iter().cloned())
        .collect();
    let joint_cone_size = cone_of_influence(&joint_seeds, deps).len();

    // Per-cluster cones. `PropertyCluster::seed_union` is already the
    // union of its members' cones (see `cluster_properties_by_jaccard`),
    // so its length is the cluster's cone size directly.
    let clusters: Vec<ClusterCoiEntry> =
        cluster_properties_by_jaccard(properties, deps, similarity_floor)
            .into_iter()
            .map(|c| ClusterCoiEntry {
                members: c.members,
                cone_size: c.seed_union.len(),
            })
            .collect();

    let max_cluster_cone_size = clusters.iter().map(|c| c.cone_size).max().unwrap_or(0);

    ClusterCoiReport {
        joint_cone_size,
        clusters,
        max_cluster_cone_size,
    }
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
    fn jaccard_empty_sets_are_identical() {
        let empty: HashSet<String> = HashSet::new();
        assert_eq!(jaccard(&empty, &empty), 1.0);
    }

    #[test]
    fn jaccard_disjoint_sets_score_zero() {
        let a = seeds(&["x", "y"]);
        let b = seeds(&["p", "q"]);
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_identical_sets_score_one() {
        let a = seeds(&["x", "y", "z"]);
        let b = seeds(&["x", "y", "z"]);
        assert_eq!(jaccard(&a, &b), 1.0);
    }

    #[test]
    fn jaccard_overlapping_sets_score_proportionally() {
        // {a, b, c} vs {b, c, d}: intersection={b,c}=2, union={a,b,c,d}=4 → 0.5
        let a = seeds(&["a", "b", "c"]);
        let b = seeds(&["b", "c", "d"]);
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn cluster_two_overlapping_properties_into_one_cluster() {
        // Properties P1 and P2 share the cone {fsm_state} via different
        // seed atoms; floor 0.4 should cluster them.
        let deps = deps_of(&[
            ("fsm_state", &["clk", "rst"]),
            ("counter", &["clk", "tick"]),
        ]);
        let p1 = ("P1".to_string(), seeds(&["fsm_state"]));
        let p2 = ("P2".to_string(), seeds(&["fsm_state"]));
        let clusters = cluster_properties_by_jaccard(&[p1, p2], &deps, 0.5);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].members, vec!["P1", "P2"]);
        // Cone of fsm_state is {fsm_state, clk, rst}.
        assert!(clusters[0].seed_union.contains("fsm_state"));
        assert!(clusters[0].seed_union.contains("clk"));
        assert!(clusters[0].seed_union.contains("rst"));
    }

    #[test]
    fn cluster_disjoint_properties_into_two_clusters() {
        // Properties P1 (cone {fsm_state, clk, rst}) and P2 (cone
        // {counter, clk, tick}) overlap on {clk} only. Jaccard:
        // 1/5 = 0.2 < floor 0.5 → two clusters.
        let deps = deps_of(&[
            ("fsm_state", &["clk", "rst"]),
            ("counter", &["clk", "tick"]),
        ]);
        let p1 = ("P1".to_string(), seeds(&["fsm_state"]));
        let p2 = ("P2".to_string(), seeds(&["counter"]));
        let clusters = cluster_properties_by_jaccard(&[p1, p2], &deps, 0.5);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].members, vec!["P1"]);
        assert_eq!(clusters[1].members, vec!["P2"]);
    }

    #[test]
    fn cluster_loose_floor_collapses_to_joint_coi() {
        // floor 0.0 means every property joins the first cluster (any
        // overlap, including zero, counts as "≥0").
        let deps = deps_of(&[
            ("fsm_state", &["clk"]),
            ("counter", &["tick"]),
            ("data", &["payload"]),
        ]);
        let p1 = ("P1".to_string(), seeds(&["fsm_state"]));
        let p2 = ("P2".to_string(), seeds(&["counter"]));
        let p3 = ("P3".to_string(), seeds(&["data"]));
        let clusters = cluster_properties_by_jaccard(&[p1, p2, p3], &deps, 0.0);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].members, vec!["P1", "P2", "P3"]);
    }

    #[test]
    fn cluster_tight_floor_approaches_per_property_coi() {
        // floor 1.0 means only identical cones cluster. The two
        // overlapping-but-not-identical properties stay separate.
        let deps = deps_of(&[
            ("fsm_state", &["clk", "rst"]),
            ("counter", &["clk", "tick"]),
        ]);
        let p1 = ("P1".to_string(), seeds(&["fsm_state"]));
        let p2 = ("P2".to_string(), seeds(&["counter"]));
        let p3 = ("P3".to_string(), seeds(&["counter"])); // identical cone to P2
        let clusters = cluster_properties_by_jaccard(&[p1, p2, p3], &deps, 1.0);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].members, vec!["P1"]);
        assert_eq!(clusters[1].members, vec!["P2", "P3"]);
    }

    #[test]
    fn cluster_empty_input_returns_no_clusters() {
        let deps = deps_of(&[]);
        let clusters = cluster_properties_by_jaccard(&[], &deps, 0.5);
        assert!(clusters.is_empty());
    }

    #[test]
    fn cycles_terminate() {
        let deps = deps_of(&[("a", &["b"]), ("b", &["a"])]);
        let r = cone_of_influence(&seeds(&["a"]), &deps);
        assert!(r.contains("a") && r.contains("b"));
        assert_eq!(r.len(), 2);
    }

    // ---- R4W-1: Formula seed harvester + cluster_coi_report ----

    #[test]
    fn property_seed_atoms_collects_predicates() {
        use crate::mu_calculus::parser::parse;
        let f = parse("nu X. (active && [] X)").expect("parse");
        let s = property_seed_atoms(&f);
        assert!(s.contains("active"), "predicate `active` harvested: {s:?}");
    }

    #[test]
    fn property_seed_atoms_collects_guard_current_and_next() {
        use crate::mu_calculus::parser::parse;
        // A modal guard referencing current- and next-state signals.
        let f = parse("[(req_cur = {flag}, req_next = {done})] busy").expect("parse");
        let s = property_seed_atoms(&f);
        assert!(s.contains("flag"), "req_cur signal harvested: {s:?}");
        assert!(s.contains("done"), "req_next signal harvested: {s:?}");
        assert!(s.contains("busy"), "predicate harvested: {s:?}");
    }

    #[test]
    fn property_seed_atoms_ignores_fixpoint_vars() {
        use crate::mu_calculus::parser::parse;
        // `X` is a fixpoint variable, not an atomic proposition — it
        // must not appear as a seed.
        let f = parse("mu X. (p || <> X)").expect("parse");
        let s = property_seed_atoms(&f);
        assert!(s.contains("p"));
        assert!(!s.contains("X"), "fixpoint var must not be a seed: {s:?}");
    }

    #[test]
    fn cluster_coi_report_disjoint_properties_reduce_max_cone() {
        // Two properties over two disjoint sub-graphs: P1 over {a→b→c},
        // P2 over {x→y→z}. Joint COI keeps all 6; each cluster keeps 3.
        let deps = deps_of(&[
            ("a", &["b"]),
            ("b", &["c"]),
            ("c", &[]),
            ("x", &["y"]),
            ("y", &["z"]),
            ("z", &[]),
        ]);
        let props = vec![
            ("P1".to_string(), seeds(&["a"])),
            ("P2".to_string(), seeds(&["x"])),
        ];
        let report = cluster_coi_report(&props, &deps, 0.5);
        assert_eq!(report.joint_cone_size, 6, "joint keeps both sub-graphs");
        assert_eq!(report.clusters.len(), 2, "disjoint cones → 2 clusters");
        assert_eq!(
            report.max_cluster_cone_size, 3,
            "each cluster keeps one chain"
        );
        assert!(
            report.max_cluster_cone_size < report.joint_cone_size,
            "clustering reduces the per-analysis cone (the M.3 claim)"
        );
    }

    #[test]
    fn cluster_coi_report_floor_zero_collapses_to_joint() {
        // similarity_floor = 0.0 merges everything into one cluster, so
        // the max cluster cone equals the joint cone (no clustering value).
        let deps = deps_of(&[("a", &["b"]), ("b", &[]), ("x", &["y"]), ("y", &[])]);
        let props = vec![
            ("P1".to_string(), seeds(&["a"])),
            ("P2".to_string(), seeds(&["x"])),
        ];
        let report = cluster_coi_report(&props, &deps, 0.0);
        assert_eq!(report.clusters.len(), 1, "floor 0 → single joint cluster");
        assert_eq!(report.max_cluster_cone_size, report.joint_cone_size);
    }
}
