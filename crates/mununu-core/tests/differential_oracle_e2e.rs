//! Differential-oracle e2e suite (P1 seed) — plan: `.claude/plans/differential-oracle-e2e-suite.md`.
//!
//! **Principle.** No single-engine definite verdict is trusted; every DEFINITE verdict is
//! cross-checked against an INDEPENDENT oracle, and a disagreement is a test failure. This
//! inverts the "pin an expected verdict literal" pattern that let the #242 frozen-register
//! bug hide for weeks (the e2e tests asserted the buggy HOLDS instead of cross-checking).
//!
//! P1 is the **reachability differential**: the exact-symbolic engine's `EF(atom)` verdict
//! (reachable?) must agree with **btormc**'s bad-reachability of the same atom — btormc is an
//! independent external model checker on the same BTOR2, so a disagreement is a genuine
//! engine bug. This is exactly the differential that #242 would have failed: the frozen
//! register made the exact engine report a reachable state as UNREACHABLE, while btormc
//! (which never touches the BddBitBlaster) reports it reachable.
//!
//! Docker-gated (`mununu-sva`): needs btormc. Run with `--ignored`.

use mununu_core::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
use mununu_core::adapter::btormc::{DEFAULT_KMAX, McVerdict, locate_btormc, run_btormc};
use mununu_core::mu_calculus::parser;

/// A register that LOADS a nonzero value (10) from a free input `ld`, whose user-visible
/// name `reg` survives only on a `uext` alias of an UNNAMED state — the flattened-yosys shape
/// that triggered #242. `init reg = 0`; `bad = (reg == 10)`. From the reset state, asserting
/// `ld` reaches `reg = 10` in one step, so `reg == 10` IS reachable — both oracles must agree.
// Note: btormc's parser requires the `init` value's id to be BELOW the state's id, so the
// constants are declared before the state (unlike a typical yosys dump where the state is
// early). mununu's own parser is order-agnostic; this ordering keeps BOTH oracles happy.
const UEXT_ALIASED_LOAD_WITH_BAD: &str = r#"
1 sort bitvec 1
2 sort bitvec 4
3 const 2 0000
4 const 2 1010
5 input 1 ld
6 state 2
7 ite 2 5 4 6
8 uext 2 6 0 reg
9 eq 1 8 4
10 next 2 6 7
11 init 2 6 3
12 bad 9
"#;

/// The reachability differential, returned as a structured result so a failure is actionable.
struct ReachDifferential {
    exact_reachable: bool,
    btormc_reachable: bool,
}

impl ReachDifferential {
    fn agrees(&self) -> bool {
        self.exact_reachable == self.btormc_reachable
    }
}

/// Run `EF(atom)` through the exact engine and btormc's bad-reachability on the same BTOR2
/// (the `bad` line must encode `atom`), and report whether they agree that `atom` is reachable.
fn reachability_differential(btor2: &str, ef_formula: &str) -> ReachDifferential {
    let formula = parser::parse(ef_formula).expect("EF formula parses");
    let exact_reachable = matches!(
        exact_symbolic_verdict(btor2, &formula).expect("exact verdict"),
        ExactVerdict::Holds, // EF true ⇒ the atom is reachable from the init state
    );
    let bin = locate_btormc().expect("btormc present (mununu-sva)");
    let btormc_reachable = matches!(
        run_btormc(&bin, btor2, DEFAULT_KMAX).expect("btormc runs"),
        McVerdict::Violated, // a reachable `bad` ⇒ the atom is reachable
    );
    ReachDifferential {
        exact_reachable,
        btormc_reachable,
    }
}

#[test]
#[ignore = "requires btormc (mununu-sva image); run with --ignored"]
fn diff_reachability_exact_vs_btormc_agree_on_uext_aliased_register() {
    // The #242-catcher. `reg == 10` is reachable (assert `ld` once from init `reg == 0`).
    // Post-fix both oracles say reachable; PRE-#242-fix the exact engine froze the register
    // and reported it UNREACHABLE while btormc reported it reachable — a divergence this
    // differential fails on. `reg` binds through the `uext` alias, the exact #242 shape.
    let diff = reachability_differential(UEXT_ALIASED_LOAD_WITH_BAD, "mu Y. ((reg == 10) or <> Y)");
    assert!(
        diff.btormc_reachable,
        "sanity: btormc must find `reg == 10` reachable (assert `ld` from init 0)"
    );
    assert!(
        diff.agrees(),
        "REACHABILITY DIFFERENTIAL FAILED: exact_reachable={} but btormc_reachable={} for \
         `reg == 10` on the uext-aliased register — the exact engine disagrees with the \
         independent btormc oracle (the #242 frozen-register signature).",
        diff.exact_reachable,
        diff.btormc_reachable,
    );
}
