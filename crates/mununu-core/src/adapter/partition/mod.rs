//! Automatic state-space partitioning for adapters.
//!
//! This module unifies two classical hardware-verification techniques
//! into a single per-adapter pass that runs **before** sidecar resolution:
//!
//! 1. **Cone-of-influence** (Kurshan, 1994). Signals outside the
//!    transitive data-flow ancestry of the property's atoms are
//!    classified [`PartitionClass::Dropped`] and become
//!    [`crate::adapter::domain::AbstractionType::Ignored`] unless the
//!    user explicitly overrides them in the sidecar.
//! 2. **Datapath UF substitution** (Andraus & Sakallah, Reveal, LPAR
//!    2008). Wide combinational arithmetic sub-trees whose only forward
//!    edges feed narrow boolean outputs are classified
//!    [`PartitionClass::Datapath`] and replaced with an uninterpreted
//!    function symbol. **Not implemented in Phase A.3** — the variant
//!    is reserved for the follow-up plan.
//!
//! The complete design is documented in
//! [`docs/design/auto-extraction-architecture.md`](../../../../docs/design/auto-extraction-architecture.md)
//! §2 Stage 2; the literature anchors live in
//! [`docs/design/abstraction-literature.md`](../../../../docs/design/abstraction-literature.md)
//! entries #1 (Kurshan) and #9 (Andraus–Sakallah).
//!
//! # Composition with the sidecar
//!
//! Auto-partition is **always advisory**. The sidecar
//! ([`crate::adapter::sidecar`]) consults the partition only for
//! signals that omit an explicit `preserve` flag. User declarations
//! always win on collision.
//!
//! # SOUNDNESS
//!
//! Cone-of-influence drops signals that have no syntactic path to any
//! property atom. This is **safe for safety properties under
//! over-approximation**: dropping signals can only add behaviours to
//! the model, never remove them, so a `True` verdict on the abstracted
//! model is sound for the concrete model. For liveness properties the
//! posture is also over-approximating but the soundness is weaker —
//! see [`docs/abstraction.md`](../../../../docs/abstraction.md). Each
//! per-adapter `DepGraphBuilder` impl is responsible for emitting a
//! `// SOUNDNESS:` annotation when its dep-graph construction
//! over-approximates (e.g. extraction adapter's indirect references —
//! see `phase-a3-followup-indirect-references.md`).

pub mod coi;
pub mod dep_graph;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use dep_graph::DepGraphBuilder;

/// Classification of a single signal under automatic partitioning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionClass {
    /// Kept — the signal is transitively referenced by some property
    /// atom (or the user explicitly preserved it). The sidecar may
    /// still narrow its abstraction further.
    Kept,
    /// Dropped — the signal lies outside the cone of influence. Becomes
    /// [`crate::adapter::domain::AbstractionType::Ignored`] unless a
    /// sidecar entry overrides this classification.
    Dropped { reason: &'static str },
    /// Datapath — the signal is the output of a wide combinational
    /// arithmetic block that has been collapsed to an uninterpreted
    /// function symbol. **Reserved for the follow-up plan**
    /// `phase-a3-followup-datapath-uf.md`; not produced by A.3.
    Datapath { uf_symbol: String },
}

/// UF stub introduced by datapath partitioning. Reserved for the
/// follow-up plan; not populated by A.3.
#[derive(Debug, Clone)]
pub struct DatapathUf {
    pub symbol: String,
    pub inputs: Vec<String>,
    pub output: String,
    pub width: u32,
}

/// Per-adapter partition telemetry surfaced in `AdapterOutput` and
/// reachable from the CLI's JSON summaries / the verify orchestrator's
/// report. Adapters populate this **after** their partition step runs
/// and the sidecar resolver has had a chance to override individual
/// signals via explicit listings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartitionSummary {
    /// Total number of signals (state cells + input ports) the
    /// partition classified — equals `kept + dropped_coi + datapath_uf`
    /// when the partition ran, or `0` when the partition was skipped
    /// (e.g. seeds empty, adapter not yet wired).
    pub total_signals: usize,
    /// Signals kept either by the COI walk or by an explicit sidecar
    /// override.
    pub kept: usize,
    /// Signals dropped as outside the cone of influence.
    pub dropped_coi: usize,
    /// Signals replaced by datapath UF stubs. Always `0` in A.3 — the
    /// follow-up plan `phase-a3-followup-datapath-uf.md` populates it.
    pub datapath_uf: usize,
    /// Total raw bit-width across kept state cells before the
    /// partition ran. `None` when the adapter does not track widths
    /// (extraction). Surfaced for adapters with bit-precise state
    /// (SV register widths, BTOR2 cell widths) so the
    /// `state_bits_after` reduction is observable.
    pub state_bits_before: Option<usize>,
    /// Total raw bit-width across kept state cells after the
    /// partition's `Dropped` signals collapsed to `Ignored`.
    pub state_bits_after: Option<usize>,
    /// R4W-2 (R.4 clustered-COI wiring) — joint-vs-clustered cone
    /// comparison over the manifest's per-property COI seeds. `Some`
    /// only when the caller threaded `AdapterOptions::property_seeds`
    /// (i.e. the verify orchestrator resolved `[[properties]]` and
    /// harvested their seed atoms); `None` for intrinsic-seed-only
    /// runs (the legacy default — no behaviour change). Pure telemetry:
    /// the report does not drive which signals the partition keeps; it
    /// reports what a per-cluster bit-blast *could* save vs the naive
    /// joint COI (the M.3 reduction metric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_coi: Option<coi::ClusterCoiReport>,
}

impl PartitionSummary {
    /// Compute a summary from a `Partition` over `(name, width)` pairs.
    /// `widths` is `None` when the adapter does not track per-cell
    /// bit widths; in that case `state_bits_before` /
    /// `state_bits_after` come back as `None`.
    pub fn from_partition(
        partition: &Partition,
        widths: Option<&std::collections::HashMap<String, usize>>,
    ) -> Self {
        let total_signals = partition.classes.len();
        let mut kept = 0usize;
        let mut dropped_coi = 0usize;
        let mut datapath_uf = 0usize;
        let (mut sb_before, mut sb_after) = (0usize, 0usize);
        let mut tracking_widths = widths.is_some();

        for (name, cls) in &partition.classes {
            match cls {
                PartitionClass::Kept => {
                    kept += 1;
                    if tracking_widths && let Some(w) = widths.unwrap().get(name) {
                        sb_before += w;
                        sb_after += w;
                    } else if tracking_widths {
                        // Width map exists but doesn't list this
                        // signal — surface as "tracking incomplete"
                        // by giving up the bit accounting entirely.
                        tracking_widths = false;
                    }
                }
                PartitionClass::Dropped { .. } => {
                    dropped_coi += 1;
                    if tracking_widths && let Some(w) = widths.unwrap().get(name) {
                        sb_before += w;
                        // Dropped → not counted in `state_bits_after`.
                    } else if tracking_widths {
                        tracking_widths = false;
                    }
                }
                PartitionClass::Datapath { .. } => {
                    datapath_uf += 1;
                    if tracking_widths && let Some(w) = widths.unwrap().get(name) {
                        sb_before += w;
                        // UF replaces wide arithmetic; the abstracted
                        // output's width remains in `_after` because
                        // we still observe it, but the internals do
                        // not. Conservative: count it.
                        sb_after += w;
                    } else if tracking_widths {
                        tracking_widths = false;
                    }
                }
            }
        }

        Self {
            total_signals,
            kept,
            dropped_coi,
            datapath_uf,
            state_bits_before: tracking_widths.then_some(sb_before),
            state_bits_after: tracking_widths.then_some(sb_after),
            // R4W-2 — populated by the caller (the BTOR2 bit-blaster)
            // when `AdapterOptions::property_seeds` is non-empty; this
            // constructor has no access to the dep graph or the seeds,
            // so it defaults to `None` and the caller fills it in.
            cluster_coi: None,
        }
    }
}

/// Output of `classify` — a complete per-signal classification plus any
/// UF stubs introduced by datapath substitution.
#[derive(Debug, Clone, Default)]
pub struct Partition {
    /// Signal name → classification. Includes every state cell and
    /// input port the dep-graph builder exposed.
    pub classes: BTreeMap<String, PartitionClass>,
    /// UF symbols indexed by abstracted output signal name. Empty in
    /// A.3.
    pub datapath_uf: BTreeMap<String, DatapathUf>,
}

/// Tunables for `classify`. Defaults are set so that A.3's COI runs
/// (datapath UF stays off).
#[derive(Debug, Clone)]
pub struct PartitionOptions {
    /// Set true to disable auto-partitioning entirely. Regression
    /// escape hatch — equivalent to behaviour before A.3.
    pub disabled: bool,
    /// Minimum width (bits) at which a combinational arithmetic block
    /// is eligible for UF substitution. `u32::MAX` (the default)
    /// disables the heuristic entirely; the follow-up plan will pick a
    /// real value.
    pub datapath_min_width: u32,
}

impl Default for PartitionOptions {
    fn default() -> Self {
        Self {
            disabled: false,
            datapath_min_width: u32::MAX,
        }
    }
}

/// Compute a `Partition` for an adapter.
///
/// Inputs:
///
/// - `builder` — adapter-specific dep-graph view of the frontend IR.
/// - `property_atoms` — names of signals that must be kept (typically
///   the union of atom names extracted from every formula in the
///   verify project).
/// - `opts` — tunables; pass `PartitionOptions::default()` for the
///   A.3-compatible behaviour.
///
/// The output `Partition::classes` includes every signal the
/// `DepGraphBuilder` exposed via `state_cells()` ∪ `input_ports()`;
/// signals not reached by the COI walk are classified
/// `Dropped { reason: "outside-cone-of-influence" }`.
///
/// When `opts.disabled` is set, every signal the builder exposes is
/// classified `Kept` — equivalent to running with no auto-partition.
pub fn classify<B: DepGraphBuilder>(
    builder: &B,
    property_atoms: &std::collections::HashSet<String>,
    opts: &PartitionOptions,
) -> Partition {
    let mut classes: BTreeMap<String, PartitionClass> = BTreeMap::new();
    let state_cells = builder.state_cells();
    let input_ports = builder.input_ports();

    // Two cases short-circuit to "keep everything":
    //
    // 1. `opts.disabled` — caller explicitly opted out of auto-partition.
    // 2. `property_atoms.is_empty()` — adapter could not extract any
    //    seed symbols (e.g. BTOR2 file whose `bad` line traces only
    //    through anonymous state cells, or SV module without
    //    @mununu-anchored properties). Without seeds the COI walk
    //    would drop every signal, which is never the right answer at
    //    the adapter level. Defer the decision to the sidecar / user.
    //
    // SOUNDNESS: both cases keep the abstract model at least as
    // precise as the un-partitioned baseline. No precision lost.
    if opts.disabled || property_atoms.is_empty() {
        for name in state_cells.iter().chain(input_ports.iter()) {
            classes.insert(name.clone(), PartitionClass::Kept);
        }
        return Partition {
            classes,
            datapath_uf: BTreeMap::new(),
        };
    }

    // SOUNDNESS: auto-COI is an over-approximation of the dep-graph's
    // signal set. Every signal transitively reachable from a property
    // atom is `Kept`; everything else is `Dropped { … }` and will be
    // pinned to a single value via AbstractionType::Ignored. Pinning
    // adds behaviours to the abstract model, so a `True` verdict on
    // the abstracted model is sound for the concrete model under
    // safety + over-approximation. See module docs for the liveness
    // posture.
    let deps = builder.build();
    let reached = coi::cone_of_influence(property_atoms, &deps);

    for name in state_cells.iter().chain(input_ports.iter()) {
        if reached.contains(name) {
            classes.insert(name.clone(), PartitionClass::Kept);
        } else {
            classes.insert(
                name.clone(),
                PartitionClass::Dropped {
                    reason: "outside-cone-of-influence",
                },
            );
        }
    }

    // Datapath UF substitution is reserved for the follow-up plan
    // `phase-a3-followup-datapath-uf.md`. The default `datapath_min_width
    // = u32::MAX` keeps it off; the follow-up will replace this no-op.
    let _ = opts.datapath_min_width;

    Partition {
        classes,
        datapath_uf: BTreeMap::new(),
    }
}

/// R.4 — Per-cluster partition record produced by
/// [`classify_clustered`]. Each cluster carries its constituent
/// property names (so the verify orchestrator can attribute verdicts
/// back to the originating property) and the `Partition` computed
/// from the cluster's union seed set.
#[derive(Debug, Clone)]
pub struct ClusterPartition {
    /// Property names merged into this cluster (cf. the input
    /// `properties` vector to [`classify_clustered`]). One-element
    /// when the property is singleton-clustered.
    pub members: Vec<String>,
    /// COI-based partition computed from the union of the cluster
    /// members' cone signal sets. Shape matches the non-clustered
    /// [`classify`] output.
    pub partition: Partition,
}

/// R.4 — Property-clustered COI per
/// `docs/design/native-sv-abstraction.md` §5 and the KMTS roadmap
/// (§10.1 R.4, `.claude/plans/you-are-a-formal-vast-lake.md`).
///
/// Given N properties (each with its own seed atom set), this
/// function:
///
/// 1. Clusters them by Jaccard similarity on their cone signal sets
///    via [`coi::cluster_properties_by_jaccard`] with the supplied
///    `similarity_floor`.
/// 2. Runs [`classify`] **once per cluster** with the cluster's
///    `seed_union` as the seed set.
/// 3. Returns one [`ClusterPartition`] per cluster, preserving the
///    cluster order produced by the Jaccard pass.
///
/// **Why per-cluster?** A single joint COI over all properties' atoms
/// keeps every signal any property mentions, even when one
/// pathologically-wide-fanin property pulls in signals every other
/// property could safely drop. Per-property COI maximises reduction
/// but loses fixpoint-cache reuse across related properties. Clustering
/// is the middle ground — properties that *do* share most of their
/// cone share one partition (and thus the same verified abstraction);
/// properties whose cones are largely disjoint get their own. Wins
/// the Caliptra-fixture ≥10× reduction goal called out in §10.1 R.4.
///
/// **Soundness.** Each cluster's [`Partition`] is independently sound
/// for the properties in *that* cluster (cone-of-influence is exact
/// on the property's atomic-proposition set under
/// over-approximation). Verifying a property against its cluster's
/// abstraction is the same correctness contract as verifying it
/// against the joint COI; the *only* difference is that signals
/// outside this cluster's cone get pinned to a single value, which
/// adds behaviours rather than removing them. Safety verdicts on the
/// cluster-abstracted model transfer to the concrete; liveness picks
/// up the same caveat as joint COI (see module docs).
///
/// **Singleton-cluster fallback.** When `properties.is_empty()` the
/// function returns an empty vector. When every property
/// scores below the floor against every existing cluster, every
/// property gets its own singleton cluster — equivalent to per-property
/// COI. When the floor is `0.0` the result is one cluster with all
/// properties — equivalent to joint COI. The middle range is where
/// R.4 earns its keep.
pub fn classify_clustered<B: DepGraphBuilder>(
    builder: &B,
    properties: &[(String, std::collections::HashSet<String>)],
    opts: &PartitionOptions,
    similarity_floor: f64,
) -> Vec<ClusterPartition> {
    if properties.is_empty() {
        return Vec::new();
    }

    // Build the dep-graph once and reuse for both the clustering pass
    // (which calls into `cone_of_influence`) and the per-cluster
    // `classify` walks.
    let deps = builder.build();
    let clusters = coi::cluster_properties_by_jaccard(properties, &deps, similarity_floor);

    let mut out = Vec::with_capacity(clusters.len());
    for cluster in clusters {
        // The cluster's seed_union *is* the cone (already a fixpoint
        // of the COI walk under these deps), so reusing it as the
        // input atom set to `classify` is idempotent — the inner COI
        // walk returns the same set, just keyed back through the
        // `Partition` API for the standard `Kept` / `Dropped`
        // accounting that `PartitionSummary` consumes downstream.
        let partition = classify(builder, &cluster.seed_union, opts);
        out.push(ClusterPartition {
            members: cluster.members,
            partition,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn partition_default_is_empty() {
        let p = Partition::default();
        assert!(p.classes.is_empty());
        assert!(p.datapath_uf.is_empty());
    }

    #[test]
    fn default_options_disable_datapath_uf() {
        let opts = PartitionOptions::default();
        assert!(!opts.disabled);
        assert_eq!(opts.datapath_min_width, u32::MAX);
    }

    struct StubBuilder {
        deps: HashMap<String, HashSet<String>>,
        states: HashSet<String>,
        inputs: HashSet<String>,
    }

    impl DepGraphBuilder for StubBuilder {
        fn build(&self) -> HashMap<String, HashSet<String>> {
            self.deps.clone()
        }
        fn state_cells(&self) -> HashSet<String> {
            self.states.clone()
        }
        fn input_ports(&self) -> HashSet<String> {
            self.inputs.clone()
        }
    }

    fn names(xs: &[&str]) -> HashSet<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn classify_keeps_reachable_drops_orphans() {
        let mut deps = HashMap::new();
        deps.insert("state".to_string(), names(&["req"]));
        deps.insert("data".to_string(), names(&["payload"]));

        let builder = StubBuilder {
            deps,
            states: names(&["state", "data"]),
            inputs: names(&["req", "payload"]),
        };
        let atoms = names(&["state"]);
        let p = classify(&builder, &atoms, &PartitionOptions::default());

        assert!(matches!(p.classes.get("state"), Some(PartitionClass::Kept)));
        assert!(matches!(p.classes.get("req"), Some(PartitionClass::Kept)));
        assert!(matches!(
            p.classes.get("data"),
            Some(PartitionClass::Dropped { .. })
        ));
        assert!(matches!(
            p.classes.get("payload"),
            Some(PartitionClass::Dropped { .. })
        ));
    }

    #[test]
    fn classify_empty_seeds_keeps_everything() {
        // Adapter could not extract any seed symbols (e.g. BTOR2 file
        // whose `bad` line only traces through anonymous state cells).
        // The partition must short-circuit to Kept rather than drop
        // every signal.
        let mut deps = HashMap::new();
        deps.insert("a".to_string(), names(&["b"]));
        let builder = StubBuilder {
            deps,
            states: names(&["a", "b"]),
            inputs: names(&["c"]),
        };
        let p = classify(&builder, &HashSet::new(), &PartitionOptions::default());
        assert_eq!(p.classes.len(), 3);
        for cls in p.classes.values() {
            assert!(matches!(cls, PartitionClass::Kept));
        }
    }

    #[test]
    fn classify_clustered_empty_input_returns_empty() {
        let builder = StubBuilder {
            deps: HashMap::new(),
            states: names(&["a"]),
            inputs: names(&[]),
        };
        let result = classify_clustered(&builder, &[], &PartitionOptions::default(), 0.5);
        assert!(result.is_empty());
    }

    #[test]
    fn classify_clustered_merges_overlapping_properties() {
        // P1 and P2 both cone to the same fsm_state subgraph; with
        // floor 0.5 they cluster together and get one partition.
        let mut deps = HashMap::new();
        deps.insert("fsm_state".to_string(), names(&["clk", "rst"]));
        deps.insert("counter".to_string(), names(&["tick"]));
        let builder = StubBuilder {
            deps,
            states: names(&["fsm_state", "counter"]),
            inputs: names(&["clk", "rst", "tick"]),
        };
        let properties = vec![
            ("P1".to_string(), names(&["fsm_state"])),
            ("P2".to_string(), names(&["fsm_state"])),
        ];
        let result = classify_clustered(&builder, &properties, &PartitionOptions::default(), 0.5);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].members, vec!["P1", "P2"]);
        // The cluster's partition keeps fsm_state's cone, drops counter / tick.
        assert!(matches!(
            result[0].partition.classes.get("fsm_state"),
            Some(PartitionClass::Kept)
        ));
        assert!(matches!(
            result[0].partition.classes.get("counter"),
            Some(PartitionClass::Dropped { .. })
        ));
        assert!(matches!(
            result[0].partition.classes.get("tick"),
            Some(PartitionClass::Dropped { .. })
        ));
    }

    #[test]
    fn classify_clustered_separates_disjoint_properties() {
        let mut deps = HashMap::new();
        deps.insert("fsm_state".to_string(), names(&["clk", "rst"]));
        deps.insert("counter".to_string(), names(&["tick"]));
        let builder = StubBuilder {
            deps,
            states: names(&["fsm_state", "counter"]),
            inputs: names(&["clk", "rst", "tick"]),
        };
        let properties = vec![
            ("P1".to_string(), names(&["fsm_state"])),
            ("P2".to_string(), names(&["counter"])),
        ];
        let result = classify_clustered(&builder, &properties, &PartitionOptions::default(), 0.5);
        assert_eq!(result.len(), 2);
        // Cluster 0: P1; keeps fsm_state, drops counter.
        assert_eq!(result[0].members, vec!["P1"]);
        assert!(matches!(
            result[0].partition.classes.get("fsm_state"),
            Some(PartitionClass::Kept)
        ));
        assert!(matches!(
            result[0].partition.classes.get("counter"),
            Some(PartitionClass::Dropped { .. })
        ));
        // Cluster 1: P2; keeps counter, drops fsm_state.
        assert_eq!(result[1].members, vec!["P2"]);
        assert!(matches!(
            result[1].partition.classes.get("counter"),
            Some(PartitionClass::Kept)
        ));
        assert!(matches!(
            result[1].partition.classes.get("fsm_state"),
            Some(PartitionClass::Dropped { .. })
        ));
    }

    #[test]
    fn classify_clustered_floor_zero_collapses_to_joint_coi() {
        let mut deps = HashMap::new();
        deps.insert("fsm_state".to_string(), names(&["clk"]));
        deps.insert("counter".to_string(), names(&["tick"]));
        let builder = StubBuilder {
            deps,
            states: names(&["fsm_state", "counter"]),
            inputs: names(&["clk", "tick"]),
        };
        let properties = vec![
            ("P1".to_string(), names(&["fsm_state"])),
            ("P2".to_string(), names(&["counter"])),
        ];
        // floor 0.0 ⇒ both properties join the first cluster.
        let result = classify_clustered(&builder, &properties, &PartitionOptions::default(), 0.0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].members, vec!["P1", "P2"]);
        // Joint COI keeps both subgraphs.
        assert!(matches!(
            result[0].partition.classes.get("fsm_state"),
            Some(PartitionClass::Kept)
        ));
        assert!(matches!(
            result[0].partition.classes.get("counter"),
            Some(PartitionClass::Kept)
        ));
    }

    #[test]
    fn classify_disabled_keeps_everything() {
        let builder = StubBuilder {
            deps: HashMap::new(),
            states: names(&["a", "b"]),
            inputs: names(&["c"]),
        };
        let opts = PartitionOptions {
            disabled: true,
            datapath_min_width: u32::MAX,
        };
        let p = classify(&builder, &HashSet::new(), &opts);
        assert_eq!(p.classes.len(), 3);
        for cls in p.classes.values() {
            assert!(matches!(cls, PartitionClass::Kept));
        }
    }
}
