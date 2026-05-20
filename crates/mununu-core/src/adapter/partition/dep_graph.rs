//! Adapter-specific dependency-graph builder.
//!
//! Each format adapter implements [`DepGraphBuilder`] against its
//! frontend IR. The trait is intentionally narrow: it exposes only what
//! `coi::cone_of_influence` needs (an adjacency map plus the
//! signal-class scoping required to distinguish state cells from
//! inputs).
//!
//! # SOUNDNESS
//!
//! `build()` must be a sound over-approximation of the design's
//! signal-to-signal data-flow: every concrete dependency must be
//! represented by an edge. Spurious extra edges are allowed (they
//! reduce precision but preserve soundness for safety properties).
//! Missing edges are **unsound** — they let `cone_of_influence`
//! silently drop signals the property genuinely needs. Implementors
//! that cannot represent some dependency exactly (e.g. the
//! extraction adapter's indirect pointer writes) must add a
//! `// SOUNDNESS:` annotation describing the over-approximation and
//! issue an `AdapterWarning` at translate time.

use std::collections::{HashMap, HashSet};

/// Adapter-side view of a frontend IR that exposes the data needed to
/// compute a cone-of-influence partition.
pub trait DepGraphBuilder {
    /// Adjacency: signal → set of signals it transitively depends on.
    ///
    /// For a register `r := f(a, b)` the map must contain
    /// `r → {a, b}`. Combinational nets fold their right-hand side
    /// into their dependency set. The result must be a sound
    /// over-approximation (see module docs).
    fn build(&self) -> HashMap<String, HashSet<String>>;

    /// Names of state cells (registers / latches). Used to scope the
    /// partition's `Kept` / `Dropped` decisions to sequential
    /// elements; combinational signals are handled implicitly through
    /// the dep graph.
    fn state_cells(&self) -> HashSet<String>;

    /// Names of input ports. Inputs receive the same COI treatment as
    /// state cells but are tracked separately so the partition summary
    /// can distinguish "dropped state" from "dropped input."
    fn input_ports(&self) -> HashSet<String>;
}
