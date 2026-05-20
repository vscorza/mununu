//! SMT predicate-image discovery (Phase A.4).
//!
//! Hoder–Bjørner–de Moura CAV 2006 all-SMT enumeration of abstract
//! transitions under a per-theory transition relation. Produces a
//! refined `discovered_values` set the existing sidecar resolver can
//! consume without changes.
//!
//! # Roles
//!
//! - **`Theory`** — selects the SMT theory family used to encode the
//!   transition relation. `BvOnly` for SV / BTOR2; `BvUfArray` for the
//!   C-extraction path.
//! - **`PredicateImage`** — the live SMT context. Wraps a Z3 solver
//!   plus the predicate set plus the encoded transition relation.
//!   Computes the abstract image as a set of `(from, to)` bit-mask pairs.
//! - **`Predicate`** — name + witness expression. Names round-trip into
//!   `SvAnnotation.discovered_values` as `DiscoveredValue.name`.
//! - **`AbstractTransition`** — one edge in the all-SMT enumeration's
//!   output. The recall harness reads these to score
//!   `discovered ∩ expected`.
//!
//! # Phase A.4 status
//!
//! - **Step 4.1 (this module skeleton)**: types + module structure;
//!   no algorithm yet.
//! - **Step 4.2**: `btor2_encode.rs` implements BTOR2 → SMT transition
//!   relation.
//! - **Step 4.3**: `all_smt.rs` implements the Hoder–Bjørner–de Moura
//!   enumeration; ships `Theory::BvOnly` against the benchmark from
//!   `examples/verify/bench_predicate_image_a4/`.
//! - **Step 4.4**: CLI wiring.
//! - **Step 4.5**: `Theory::BvUfArray` variant + extraction adapter
//!   unblock.
//!
//! # SOUNDNESS
//!
//! The predicate image is an **over-approximation** of the abstract
//! transition relation by construction — every concrete edge has a
//! corresponding abstract edge, and the enumeration only adds edges
//! that the SMT solver proves are satisfiable. Missing edges are
//! impossible under sound encoding; spurious edges are only possible
//! if the BTOR2 / SV → SMT encoder over-approximates an operator
//! (e.g., the under-approximation fallback in `theory.rs` when Z3
//! saturates). Each over-approximation site is annotated with a
//! `// SOUNDNESS:` comment.

pub mod all_smt;
pub mod btor2_encode;
pub mod seed;
pub mod theory;

pub use theory::Theory;

use std::collections::HashSet;

/// One predicate in the partition's witness set. The `name` round-trips
/// into [`crate::adapter::systemverilog::annotation::DiscoveredValue::name`];
/// the `witness` is the SMT-side expression the predicate stands for
/// (a single equality, range comparison, or arbitrary boolean formula
/// over signals — the seed extractor in `seed.rs` decides the shape).
#[derive(Debug, Clone)]
pub struct Predicate {
    /// Human-readable name, used as the value-map entry in the
    /// resolved `discovered_values`.
    pub name: String,
    /// The signal this predicate is anchored on. Multiple predicates
    /// may share a signal — they become alternatives in the resolved
    /// `EnumValues` domain.
    pub signal: String,
    /// The integer constant the predicate witnesses, when the
    /// predicate has the shape `signal == k`. `None` for richer
    /// predicates (range, parity, etc.) that don't reduce to a single
    /// integer in the sidecar.
    pub witness_constant: Option<i64>,
}

/// One edge in the all-SMT enumeration's output. The bit-masks index
/// into [`PredicateImage::predicates`] — `from[i] = true` iff the
/// `i`-th predicate holds in the `from`-state of the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbstractTransition {
    pub from: Vec<bool>,
    pub to: Vec<bool>,
}

/// Tunables for the predicate-image enumeration.
#[derive(Debug, Clone)]
pub struct ImageOptions {
    /// Maximum number of abstract edges the enumeration will emit per
    /// `(predicate_set, transition)` query. Bounded to keep the
    /// enumeration tractable; default 4 096.
    pub cap_edges: usize,
    /// Per-query SMT timeout in milliseconds. The Bryant–Kroening
    /// under-approximation fallback kicks in on timeout. Default 5 000.
    pub per_query_timeout_ms: u32,
    /// Maximum bit-width for under-approximation shrinks. Bryant–Kroening
    /// halves widths on each retry; this is the floor. Default 4.
    pub under_approx_min_width: u32,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            cap_edges: 4_096,
            per_query_timeout_ms: 5_000,
            under_approx_min_width: 4,
        }
    }
}

/// Per-fixture recall metric — `|discovered ∩ expected| / |expected|`.
/// Computed by the recall harness in step 4.3 against the fixtures
/// declared in
/// [`examples/verify/bench_predicate_image_a4/fixtures.toml`](../../../../../examples/verify/bench_predicate_image_a4/fixtures.toml).
#[derive(Debug, Clone)]
pub struct RecallScore {
    pub fixture_id: String,
    pub signal: String,
    pub expected: HashSet<i64>,
    pub discovered: HashSet<i64>,
    pub recall: f64,
}

impl RecallScore {
    pub fn compute(
        fixture_id: impl Into<String>,
        signal: impl Into<String>,
        expected: HashSet<i64>,
        discovered: HashSet<i64>,
    ) -> Self {
        let intersection = expected.intersection(&discovered).count() as f64;
        let denom = expected.len() as f64;
        let recall = if denom > 0.0 {
            intersection / denom
        } else {
            // Empty expected set — recall is degenerate. The harness
            // skips such fixtures (Pono variant before download, for
            // instance) rather than fail them.
            1.0
        };
        Self {
            fixture_id: fixture_id.into(),
            signal: signal.into(),
            expected,
            discovered,
            recall,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(xs: &[i64]) -> HashSet<i64> {
        xs.iter().copied().collect()
    }

    #[test]
    fn image_options_default_is_capped() {
        let opts = ImageOptions::default();
        assert_eq!(opts.cap_edges, 4_096);
        assert_eq!(opts.per_query_timeout_ms, 5_000);
        assert!(opts.under_approx_min_width > 0);
    }

    #[test]
    fn recall_score_perfect_match() {
        let score =
            RecallScore::compute("test_fix", "cnt", ints(&[0, 1, 2, 3]), ints(&[0, 1, 2, 3]));
        assert_eq!(score.recall, 1.0);
    }

    #[test]
    fn recall_score_partial_match() {
        let score =
            RecallScore::compute("test_fix", "cnt", ints(&[0, 1, 2, 3, 4]), ints(&[0, 1, 2]));
        // 3 of 5 expected discovered → 0.6
        assert!((score.recall - 0.6).abs() < 1e-9);
    }

    #[test]
    fn recall_score_empty_expected_is_one() {
        // Degenerate case (e.g. Pono fixture with no curated baseline)
        // returns 1.0 so the harness can skip-without-failing.
        let score = RecallScore::compute("pono", "x", HashSet::new(), ints(&[0, 5, 10]));
        assert_eq!(score.recall, 1.0);
    }

    #[test]
    fn recall_score_extra_discovered_does_not_penalise() {
        // Recall is intersection / expected — extra discovered values
        // (precision loss) don't reduce the score. Precision is a
        // separate metric we don't track in Phase A.4.
        let score = RecallScore::compute("test", "cnt", ints(&[0, 1]), ints(&[0, 1, 99, 100]));
        assert_eq!(score.recall, 1.0);
    }

    #[test]
    fn abstract_transition_equality() {
        let a = AbstractTransition {
            from: vec![true, false, true],
            to: vec![false, true, false],
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
