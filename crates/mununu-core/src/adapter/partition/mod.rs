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
