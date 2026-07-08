//! Reachability portfolio — decide BTOR2 `bad`-reachability with every available
//! engine and merge under the **differential-oracle discipline**.
//!
//! The P1 payoff made usable — five sound engines, each deciding a *different*
//! slice of designs:
//! - the **exact** BDD engine
//!   ([`exact_bad_reachable`](crate::adapter::btor2::symbolic_bitblast::exact_bad_reachable))
//!   within its 40-bit cone cap;
//! - the in-house **native** engine (BMC + k-induction on the Z3 seam,
//!   [`native_bmc`](crate::adapter::btor2::native_bmc)) — no bit cap, no
//!   subprocess: mununu's own scalable safety member;
//! - the in-house **SPACER** engine (IC3/PDR + interpolation via Z3's Fixedpoint,
//!   [`native_spacer`](crate::adapter::btor2::native_spacer)) — decides safe
//!   properties whose inductive invariant native k-induction cannot reach by simple
//!   induction, also in-process;
//! - **btormc** (BMC + k-induction) on deep counterexamples;
//! - **Pono** (IC3/PDR) on IC3-provable / violated instances.
//!
//! Running them together decides strictly more than any one, and — crucially —
//! every engine's verdict is **sound**, so:
//!
//! - **first definite wins** — any engine's `Reachable` / `Unreachable` decides it;
//! - **two DEFINITE verdicts that DISAGREE raise a soundness alarm** ([`ReachVerdict::Contradiction`])
//!   rather than a guess. Since all engines are sound, a disagreement can only mean
//!   a real bug — exactly what the differential oracle exists to catch.
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
use crate::adapter::btor2::native_bmc::{self, SafetyVerdict};
use crate::adapter::btor2::native_spacer;
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

/// The in-house **native engine** (BMC + k-induction on the Z3 seam) verdict:
/// `Some(true)` reachable (a counterexample), `Some(false)` unreachable (a
/// k-inductive safety proof), `None` abstained (Unknown / timeout / encode error).
/// Unlike the exact BDD engine it has no 40-bit cone cap, and unlike btormc/Pono
/// it needs no subprocess — mununu's own scalable safety member.
fn run_native(file: &Btor2File) -> Option<bool> {
    match native_bmc::decide_bad_safety(
        file,
        native_bmc::DEFAULT_MAX_K,
        Some(native_bmc::DEFAULT_TIMEOUT_MS),
    ) {
        Ok(SafetyVerdict::Violated { .. }) => Some(true),
        Ok(SafetyVerdict::Safe { .. }) => Some(false),
        Ok(SafetyVerdict::Unknown { .. }) | Err(_) => None,
    }
}

/// The in-house **SPACER engine** (IC3/PDR + interpolation via Z3's Fixedpoint)
/// verdict: `Some(true)` reachable, `Some(false)` unreachable (an inductive-invariant
/// safety proof), `None` abstained (Unknown / timeout / encode error). SPACER's
/// invariant discovery decides *safe* designs whose invariant native k-induction
/// cannot reach by simple induction below a large depth — also in-process, no
/// subprocess.
fn run_spacer(file: &Btor2File) -> Option<bool> {
    match native_spacer::decide_bad_safety_spacer(file, Some(native_spacer::DEFAULT_TIMEOUT_MS)) {
        Ok(SafetyVerdict::Violated { .. }) => Some(true),
        Ok(SafetyVerdict::Safe { .. }) => Some(false),
        Ok(SafetyVerdict::Unknown { .. }) | Err(_) => None,
    }
}

/// Merge the members' verdicts into a [`ReachOutcome`], in the fixed engine order
/// (`exact`, `native`, `spacer`, `btormc`, `pono`) so the outcome is deterministic
/// regardless of which driver (sequential / parallel) produced them.
fn collect(
    exact: Option<bool>,
    native: Option<bool>,
    spacer: Option<bool>,
    btormc_v: Option<McVerdict>,
    pono_v: Option<McVerdict>,
) -> ReachOutcome {
    let mut reachable_by: Vec<&'static str> = Vec::new();
    let mut unreachable_by: Vec<&'static str> = Vec::new();
    // In-house engines report reachability as a bool (`Some(true)` = reachable).
    for (name, v) in [("exact", exact), ("native", native), ("spacer", spacer)] {
        match v {
            Some(true) => reachable_by.push(name),
            Some(false) => unreachable_by.push(name),
            None => {}
        }
    }
    // Subprocess members report an `McVerdict`.
    for (name, v) in [("btormc", btormc_v), ("pono", pono_v)] {
        match v {
            Some(McVerdict::Violated) => reachable_by.push(name),
            Some(McVerdict::Safe) => unreachable_by.push(name),
            Some(McVerdict::Unknown) | None => {}
        }
    }
    ReachOutcome::from_sets(reachable_by, unreachable_by)
}

/// Decide `bad`-reachability of `file` across all five engines — the exact BDD
/// engine, the in-house native (BMC + k-induction) and SPACER engines, and the
/// btormc and Pono subprocess members — merged under the differential-oracle
/// discipline.
///
/// Each member abstains gracefully: the exact engine on an over-cap / free-init
/// design (`Err`), the in-house engines on `Unknown` / timeout / encode error, a
/// subprocess member when its binary is absent (`Err`), it is inconclusive
/// (`Unknown`), or it hits its wall-clock timeout. The emitted BTOR2 (from
/// [`emit_btor2`]) is the shared input, so a *reduced / transformed* model is decided
/// consistently across all members.
///
/// This is the **sequential** driver — members run one after another. Use
/// [`decide_reach_portfolio_parallel`] to overlap them in wall-clock.
pub fn decide_reach_portfolio(file: &Btor2File) -> ReachOutcome {
    let content = emit_btor2(file);
    // Exact BDD engine — sound both ways (REACHABLE is always sound; an
    // UNREACHABLE verdict is refused on free-init state), within the bit cap.
    let exact = run_exact(&content);
    // Native engine — in-house BMC + k-induction on the Z3 seam, no bit cap.
    let native = run_native(file);
    // SPACER engine — in-house IC3/PDR + interpolation (invariant discovery).
    let spacer = run_spacer(file);
    // btormc — BMC (CEX) + k-induction (proof).
    let btormc_v =
        btormc::decide_via_btormc(file, btormc::DEFAULT_KMAX, btormc::DEFAULT_TIMEOUT).ok();
    // Pono — IC3/PDR (proof + shallow CEX).
    let pono_v = pono::decide_via_pono(file, pono::DEFAULT_ENGINE, pono::DEFAULT_TIMEOUT).ok();
    collect(exact, native, spacer, btormc_v, pono_v)
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
        // The native and SPACER engines are in-process Z3 work — each on its own
        // thread so it overlaps the exact engine + the subprocess members rather than
        // serialising.
        let native_h = scope.spawn(|| run_native(file));
        let spacer_h = scope.spawn(|| run_spacer(file));
        // The exact engine is in-process BDD work — run it on this thread while the
        // other members run on theirs.
        let exact = run_exact(&content);
        // A panicked member thread abstains (None) rather than poisoning the merge.
        let native = native_h.join().unwrap_or(None);
        let spacer = spacer_h.join().unwrap_or(None);
        let btormc_v = btormc_h.join().unwrap_or(None);
        let pono_v = pono_h.join().unwrap_or(None);
        collect(exact, native, spacer, btormc_v, pono_v)
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
    fn native_engine_decides_beyond_the_exact_cap_in_portfolio() {
        // A 64-bit register that stays 0, `bad = (big != 0)`: SAFE, but the 64-bit
        // cone is over the exact engine's 40-bit cap. With no btormc/pono on PATH
        // and the exact engine abstaining, the portfolio would previously be
        // Unknown; the in-house native engine (k-induction) now proves it
        // Unreachable — the scale win, in-process, no subprocess.
        const WIDE_SAFE: &str = "1 sort bitvec 64\n2 zero 1\n3 state 1 big\n4 init 1 3 2\n\
                                 5 next 1 3 3\n6 sort bitvec 1\n7 neq 6 3 2\n8 bad 7\n";
        let file = parser::parse(WIDE_SAFE).expect("parse");
        let out = decide_reach_portfolio(&file);
        assert_eq!(out.verdict, ReachVerdict::Unreachable, "outcome: {out:?}");
        assert!(
            out.unreachable_by.contains(&"native"),
            "the native engine must carry the beyond-cap verdict: {out:?}"
        );
        assert!(
            !out.unreachable_by.contains(&"exact"),
            "the exact engine must abstain on the over-cap 64-bit design: {out:?}"
        );
    }

    #[test]
    fn spacer_decides_beyond_native_k_induction_in_portfolio() {
        // A 64-bit counter that STALLS at 0 (init 0, next = (c==0)?0:c+1), bad = (c==MAX).
        // Reachable set is {0} ⇒ SAFE, but:
        //   - the exact BDD engine abstains (64-bit > its 40-bit cone cap);
        //   - native k-induction abstains — the property is not provable by *simple*
        //     k-induction: an unreachable free path MAX-k → … → MAX stays ¬bad until
        //     the last step at every depth, so it never closes below depth 2^64-1.
        // Only SPACER's invariant discovery (`c == 0`) decides it, so the portfolio
        // verdict is carried uniquely by "spacer". (btormc/pono absent in make-ci.)
        const WIDE_STALL: &str = "1 sort bitvec 64\n2 zero 1\n3 one 1\n4 state 1 c\n5 init 1 4 2\n\
             6 sort bitvec 1\n7 eq 6 4 2\n8 add 1 4 3\n9 ite 1 7 2 8\n\
             10 next 1 4 9\n11 ones 1\n12 eq 6 4 11\n13 bad 12\n";
        let file = parser::parse(WIDE_STALL).expect("parse");
        let out = decide_reach_portfolio(&file);
        assert_eq!(out.verdict, ReachVerdict::Unreachable, "outcome: {out:?}");
        assert!(
            out.unreachable_by.contains(&"spacer"),
            "SPACER must carry the beyond-k-induction verdict: {out:?}"
        );
        assert!(
            !out.unreachable_by.contains(&"native"),
            "native k-induction must abstain on the non-simple-inductive design: {out:?}"
        );
        assert!(
            !out.unreachable_by.contains(&"exact"),
            "the exact engine must abstain on the over-cap 64-bit design: {out:?}"
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
