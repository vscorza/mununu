//! Reachability portfolio — decide BTOR2 `bad`-reachability with every available
//! engine and merge under the **differential-oracle discipline**.
//!
//! The P1 payoff made usable: the exact BDD engine
//! ([`exact_bad_reachable`](crate::adapter::btor2::symbolic_bitblast::exact_bad_reachable)),
//! btormc (BMC + k-induction), and Pono (IC3/PDR) each decide a *different*
//! slice of designs — the exact engine within its 40-bit cone cap, btormc on
//! deep counterexamples, Pono on IC3-provable/violated instances. Running them
//! together (the emit seam feeds the two subprocess members) decides strictly
//! more than any one, and — crucially — every engine's verdict is **sound**, so:
//!
//! - **first definite wins** — any engine's `Reachable` / `Unreachable` decides it;
//! - **two DEFINITE verdicts that DISAGREE raise a soundness alarm** ([`ReachVerdict::Contradiction`])
//!   rather than a guess. Since all three engines are sound, a disagreement can
//!   only mean a real bug — exactly what the differential oracle exists to catch.
//!
//! Both a **sequential** ([`decide_reach_portfolio`]) and a **parallel**
//! ([`decide_reach_portfolio_parallel`]) driver are provided. They merge
//! identically — the parallel variant only overlaps the two subprocess members
//! (and the in-process exact engine) in wall-clock, which matters once a
//! per-engine timeout is in play: a slow member no longer serialises in front of
//! a fast one. Every member carries a **wall-clock timeout** (Pono's IC3 has no
//! native bound and can run unbounded on a hard instance); a member that errors,
//! times out, or is undecided simply abstains — a timeout is a sound
//! [`ReachVerdict::Unknown`], never a wrong verdict.

use crate::adapter::btor2::ast::Btor2File;
use crate::adapter::btor2::emit::emit_btor2;
use crate::adapter::btor2::symbolic_bitblast::exact_bad_reachable;
use crate::adapter::btormc::{self, McVerdict};
use crate::adapter::pono;

/// The merged reachability verdict.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReachVerdict {
    /// At least one sound engine found `bad` reachable (a real counterexample).
    Reachable,
    /// At least one sound engine proved `bad` unreachable (a real safety proof).
    Unreachable,
    /// No engine decided (all abstained / timed out / were undecided).
    #[default]
    Unknown,
    /// Two DEFINITE verdicts disagree — a soundness alarm, never a silent guess.
    Contradiction,
}

/// The portfolio outcome: the merged verdict plus which engines reached each
/// definite conclusion (empty lists ⇒ that side was undecided by all).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReachOutcome {
    pub verdict: ReachVerdict,
    /// Engines that found `bad` reachable.
    pub reachable_by: Vec<&'static str>,
    /// Engines that proved `bad` unreachable.
    pub unreachable_by: Vec<&'static str>,
}

impl ReachOutcome {
    /// Merge from the two verdict sets under the first-definite +
    /// contradiction-alarm rule.
    fn from_sets(reachable_by: Vec<&'static str>, unreachable_by: Vec<&'static str>) -> Self {
        let verdict = match (!reachable_by.is_empty(), !unreachable_by.is_empty()) {
            (true, true) => ReachVerdict::Contradiction,
            (true, false) => ReachVerdict::Reachable,
            (false, true) => ReachVerdict::Unreachable,
            (false, false) => ReachVerdict::Unknown,
        };
        ReachOutcome {
            verdict,
            reachable_by,
            unreachable_by,
        }
    }
}

/// The exact engine's optional verdict: `Some(true)` reachable, `Some(false)`
/// unreachable, `None` abstained (over-cap / free-init / error).
fn run_exact(content: &str) -> Option<bool> {
    exact_bad_reachable(content).ok()
}

/// Merge the three members' verdicts into a [`ReachOutcome`], in the fixed
/// engine order (`exact`, `btormc`, `pono`) so the outcome is deterministic
/// regardless of which driver (sequential / parallel) produced them.
fn collect(
    exact: Option<bool>,
    btormc_v: Option<McVerdict>,
    pono_v: Option<McVerdict>,
) -> ReachOutcome {
    let mut reachable_by: Vec<&'static str> = Vec::new();
    let mut unreachable_by: Vec<&'static str> = Vec::new();
    match exact {
        Some(true) => reachable_by.push("exact"),
        Some(false) => unreachable_by.push("exact"),
        None => {}
    }
    for (name, v) in [("btormc", btormc_v), ("pono", pono_v)] {
        match v {
            Some(McVerdict::Violated) => reachable_by.push(name),
            Some(McVerdict::Safe) => unreachable_by.push(name),
            Some(McVerdict::Unknown) | None => {}
        }
    }
    ReachOutcome::from_sets(reachable_by, unreachable_by)
}

/// Decide `bad`-reachability of `file` across the exact BDD engine + the btormc
/// and Pono subprocess members, merged under the differential-oracle discipline.
///
/// Each member abstains gracefully: the exact engine on an over-cap / free-init
/// design (`Err`), a subprocess member when its binary is absent (`Err`), it is
/// inconclusive (`Unknown`), or it hits its wall-clock timeout. The emitted BTOR2
/// (from [`emit_btor2`]) is the shared input, so a *reduced / transformed* model
/// is decided consistently by all three.
///
/// This is the **sequential** driver — members run one after another. Use
/// [`decide_reach_portfolio_parallel`] to overlap them in wall-clock.
pub fn decide_reach_portfolio(file: &Btor2File) -> ReachOutcome {
    let content = emit_btor2(file);
    // Exact BDD engine — sound both ways (REACHABLE is always sound; an
    // UNREACHABLE verdict is refused on free-init state), within the bit cap.
    let exact = run_exact(&content);
    // btormc — BMC (CEX) + k-induction (proof).
    let btormc_v =
        btormc::decide_via_btormc(file, btormc::DEFAULT_KMAX, btormc::DEFAULT_TIMEOUT).ok();
    // Pono — IC3/PDR (proof + shallow CEX).
    let pono_v = pono::decide_via_pono(file, pono::DEFAULT_ENGINE, pono::DEFAULT_TIMEOUT).ok();
    collect(exact, btormc_v, pono_v)
}

/// The **parallel** driver: run the exact engine (in-process) and the two
/// subprocess members concurrently, then merge identically to
/// [`decide_reach_portfolio`]. Scoped threads borrow `file` directly — the scope
/// guarantees the borrows end before this returns, so no clone / `Arc` is needed.
///
/// The merge is unchanged, so the verdict is identical to the sequential driver;
/// only the wall-clock differs (≈ the slowest single member instead of the sum).
/// This matters once per-engine timeouts are in play — a member that burns its
/// full budget no longer serialises in front of a fast one.
pub fn decide_reach_portfolio_parallel(file: &Btor2File) -> ReachOutcome {
    let content = emit_btor2(file);
    std::thread::scope(|scope| {
        let btormc_h = scope.spawn(|| {
            btormc::decide_via_btormc(file, btormc::DEFAULT_KMAX, btormc::DEFAULT_TIMEOUT).ok()
        });
        let pono_h = scope.spawn(|| {
            pono::decide_via_pono(file, pono::DEFAULT_ENGINE, pono::DEFAULT_TIMEOUT).ok()
        });
        // The exact engine is in-process BDD work — run it on this thread while the
        // two subprocess members run on theirs.
        let exact = run_exact(&content);
        // A panicked member thread abstains (None) rather than poisoning the merge.
        let btormc_v = btormc_h.join().unwrap_or(None);
        let pono_v = pono_h.join().unwrap_or(None);
        collect(exact, btormc_v, pono_v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser;

    // Merge-logic unit tests (no subprocess): the first-definite + contradiction
    // rule is pure and testable without any external binary.
    #[test]
    fn merge_first_definite_and_contradiction() {
        assert_eq!(
            ReachOutcome::from_sets(vec!["exact"], vec![]).verdict,
            ReachVerdict::Reachable
        );
        assert_eq!(
            ReachOutcome::from_sets(vec![], vec!["btormc"]).verdict,
            ReachVerdict::Unreachable
        );
        assert_eq!(
            ReachOutcome::from_sets(vec![], vec![]).verdict,
            ReachVerdict::Unknown
        );
        // Two sound engines disagreeing ⇒ a soundness alarm, never a silent pick.
        let c = ReachOutcome::from_sets(vec!["btormc"], vec!["exact"]);
        assert_eq!(c.verdict, ReachVerdict::Contradiction);
        assert_eq!(c.reachable_by, vec!["btormc"]);
        assert_eq!(c.unreachable_by, vec!["exact"]);
    }

    #[test]
    fn portfolio_decides_reachable_without_subprocess() {
        // With no btormc/pono on PATH the two subprocess members abstain (Err),
        // but the exact engine alone decides the small in-cap counter — so the
        // portfolio still returns a definite verdict. (In mununu-sva all three
        // run; here we prove the exact-only fallback + the merge shape.)
        const COUNTER: &str = "1 sort bitvec 3\n2 zero 1\n3 state 1\n4 init 1 3 2\n5 one 1\n\
                               6 add 1 3 5\n7 next 1 3 6\n8 ones 1\n9 sort bitvec 1\n\
                               10 eq 9 3 8\n11 bad 10\n";
        let file = parser::parse(COUNTER).expect("parse");
        let out = decide_reach_portfolio(&file);
        assert_eq!(out.verdict, ReachVerdict::Reachable);
        assert!(out.reachable_by.contains(&"exact"));
        assert!(
            out.unreachable_by.is_empty(),
            "no engine should call the reachable counter unreachable"
        );
    }

    #[test]
    fn parallel_driver_agrees_with_sequential() {
        // The parallel driver must return the *identical* outcome to the
        // sequential one — it only overlaps members in wall-clock, never changes
        // the merge. With no subprocess members on PATH both reduce to the
        // exact-only verdict; the point is they agree byte-for-byte.
        const COUNTER: &str = "1 sort bitvec 3\n2 zero 1\n3 state 1\n4 init 1 3 2\n5 one 1\n\
                               6 add 1 3 5\n7 next 1 3 6\n8 ones 1\n9 sort bitvec 1\n\
                               10 eq 9 3 8\n11 bad 10\n";
        let file = parser::parse(COUNTER).expect("parse");
        let seq = decide_reach_portfolio(&file);
        let par = decide_reach_portfolio_parallel(&file);
        assert_eq!(seq, par, "parallel and sequential drivers must agree");
        assert_eq!(par.verdict, ReachVerdict::Reachable);
    }

    #[test]
    #[ignore = "requires btormc + pono (mununu-sva); run with --ignored"]
    fn portfolio_agrees_across_engines_in_sva() {
        // In mununu-sva all three engines run. On a small reachable counter they
        // must AGREE (all Reachable) — no contradiction alarm — demonstrating the
        // cross-check with the subprocess members live.
        const COUNTER: &str = "1 sort bitvec 3\n2 zero 1\n3 state 1\n4 init 1 3 2\n5 one 1\n\
                               6 add 1 3 5\n7 next 1 3 6\n8 ones 1\n9 sort bitvec 1\n\
                               10 eq 9 3 8\n11 bad 10\n";
        let file = parser::parse(COUNTER).expect("parse");
        let out = decide_reach_portfolio(&file);
        assert_eq!(out.verdict, ReachVerdict::Reachable);
        assert!(
            out.reachable_by.len() >= 2,
            "at least the exact engine + one subprocess member should agree; got {:?}",
            out
        );
    }
}
