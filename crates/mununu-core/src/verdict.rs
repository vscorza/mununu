//! Canonical property-verdict vocabulary — the single source of truth for the
//! verdict string every verify surface reports.
//!
//! Model checking answers a property with one of four outcomes. Historically each
//! surface spelled them differently — `holds`/`violated`/`unknown` on the flagship
//! `sv verify-auto` and extraction, but `reachable`/`unreachable`/`contradiction` on
//! `btor2 verify` and `inconclusive` on `btor2 verify-liveness`. [`PropertyVerdict`]
//! fixes ONE spelling and every surface maps its engine-specific verdict into it via
//! the `From` impls here, so the surfaces cannot drift.
//!
//! The vocabulary is the one the flagship already uses: `holds` / `violated` /
//! `unknown` / `skipped`. Surface-specific *detail* (which engines decided, a
//! soundness-contradiction alarm, cube-cell counts) stays in the surface's own
//! response fields alongside this canonical verdict.

use crate::adapter::btor2::symbolic_bitblast::ExactVerdict;
use crate::adapter::liveness_rescue::LivenessVerdict;
use crate::adapter::reach_portfolio::ReachVerdict;

/// The canonical answer to "did the property hold?", shared by every verify surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyVerdict {
    /// The property holds on every reachable state / path — a sound proof.
    Holds,
    /// The property is violated — a real counterexample exists.
    Violated,
    /// No engine decided: abstention, over-cap, timeout, or a soundness alarm. The
    /// caller surfaces the *reason* (e.g. a `contradiction` flag) separately.
    Unknown,
    /// The property was not evaluated — out of the supported fragment, or filtered
    /// out before the run.
    Skipped,
}

impl PropertyVerdict {
    /// The stable lowercase label — identical across CLI, HTTP API, and UI.
    pub fn as_str(self) -> &'static str {
        match self {
            PropertyVerdict::Holds => "holds",
            PropertyVerdict::Violated => "violated",
            PropertyVerdict::Unknown => "unknown",
            PropertyVerdict::Skipped => "skipped",
        }
    }
}

impl From<ReachVerdict> for PropertyVerdict {
    /// The safety reading of `bad`-reachability: `bad` **unreachable** = the
    /// assertion HOLDS; **reachable** = VIOLATED; undecided or a contradiction alarm
    /// = UNKNOWN (the alarm is surfaced separately by the caller's `contradiction`
    /// flag + the per-engine `reachable_by` / `unreachable_by` breakdown).
    fn from(v: ReachVerdict) -> Self {
        match v {
            ReachVerdict::Unreachable => PropertyVerdict::Holds,
            ReachVerdict::Reachable => PropertyVerdict::Violated,
            ReachVerdict::Unknown | ReachVerdict::Contradiction => PropertyVerdict::Unknown,
        }
    }
}

impl From<LivenessVerdict> for PropertyVerdict {
    fn from(v: LivenessVerdict) -> Self {
        match v {
            LivenessVerdict::Holds => PropertyVerdict::Holds,
            LivenessVerdict::Violated => PropertyVerdict::Violated,
            LivenessVerdict::Inconclusive => PropertyVerdict::Unknown,
        }
    }
}

impl From<ExactVerdict> for PropertyVerdict {
    /// The exact 3-valued engine gives a *definite* verdict within its cap (the
    /// over-cap / unsupported case is an `Err` the caller maps to `Unknown`).
    fn from(v: ExactVerdict) -> Self {
        match v {
            ExactVerdict::Holds => PropertyVerdict::Holds,
            ExactVerdict::Violated => PropertyVerdict::Violated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_the_canonical_vocabulary() {
        assert_eq!(PropertyVerdict::Holds.as_str(), "holds");
        assert_eq!(PropertyVerdict::Violated.as_str(), "violated");
        assert_eq!(PropertyVerdict::Unknown.as_str(), "unknown");
        assert_eq!(PropertyVerdict::Skipped.as_str(), "skipped");
    }

    #[test]
    fn reach_verdict_maps_to_the_safety_reading() {
        assert_eq!(
            PropertyVerdict::from(ReachVerdict::Unreachable),
            PropertyVerdict::Holds
        );
        assert_eq!(
            PropertyVerdict::from(ReachVerdict::Reachable),
            PropertyVerdict::Violated
        );
        assert_eq!(
            PropertyVerdict::from(ReachVerdict::Unknown),
            PropertyVerdict::Unknown
        );
        // A contradiction is undecided at the property level; the alarm is a flag.
        assert_eq!(
            PropertyVerdict::from(ReachVerdict::Contradiction),
            PropertyVerdict::Unknown
        );
    }

    #[test]
    fn liveness_inconclusive_folds_to_unknown() {
        assert_eq!(
            PropertyVerdict::from(LivenessVerdict::Inconclusive),
            PropertyVerdict::Unknown
        );
        assert_eq!(
            PropertyVerdict::from(LivenessVerdict::Holds),
            PropertyVerdict::Holds
        );
    }
}
