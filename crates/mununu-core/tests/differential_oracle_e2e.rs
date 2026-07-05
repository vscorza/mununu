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
use mununu_core::adapter::slang::verify_auto::{VerifyAutoOptions, VerifyOutcome, verify_auto};
use mununu_core::adapter::yosys::YosysOptions;
use mununu_core::mu_calculus::parser;
use std::path::PathBuf;

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

// ============================================================================
// P2 — corpus differential over real open designs, with the MONOTONE VERDICT
// LEDGER invariant.
//
// The corpus (plan: `.claude/plans/differential-oracle-e2e-suite.md`) runs each
// real design's OWN SVA (extracted untouched from source) plus a mununu-exclusive
// liveness/recoverability annotation through `sv verify-auto`, and checks the
// per-property verdict against a recorded LEDGER under the refinement lattice:
//
//     ⊥ (Bottom / Skipped)  ⊑  { True, False }
//
//   - A recorded DEFINITE verdict (True/False) MUST be preserved exactly. A
//     True↔False flip is a SOUNDNESS bug; a definite→⊥ is a precision REGRESSION.
//   - A recorded ⊥ MAY stay ⊥ or FLIP UP to a definite verdict — that is an
//     IMPROVEMENT as our abstraction / verification refines (update the ledger to
//     lock the new definite value in). Verdicts only ever move UP the lattice.
//
// The corpus designs + their SVA are kept BYTE-FOR-BYTE from upstream (models-from-
// source, claims-integrity); only the `@mununu_guarantee` liveness annotation is
// added, and it is a distinct property, never a rewrite of the design's SVA.
// ============================================================================

/// A recorded verdict in the refinement lattice. `Indefinite` covers both `⊥`
/// (extracted, undecided) and `Skipped` (not yet extractable) — both may flip up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LedgerVerdict {
    True,
    False,
    Indefinite,
}

fn outcome_verdict(o: &VerifyOutcome) -> LedgerVerdict {
    match o {
        VerifyOutcome::Holds => LedgerVerdict::True,
        VerifyOutcome::Violated { .. } => LedgerVerdict::False,
        VerifyOutcome::Unknown { .. } | VerifyOutcome::Skipped { .. } => LedgerVerdict::Indefinite,
    }
}

/// The monotone-refinement check. Returns `Some(new_definite)` when a recorded `⊥`
/// flipped UP to a definite verdict (an improvement to record in the ledger); panics
/// on any lattice violation (a definite verdict that flipped or regressed).
#[must_use]
fn assert_monotone(
    design: &str,
    property: &str,
    recorded: LedgerVerdict,
    current: LedgerVerdict,
) -> Option<LedgerVerdict> {
    match recorded {
        LedgerVerdict::True | LedgerVerdict::False => {
            assert_eq!(
                current, recorded,
                "MONOTONE VIOLATION: {design}.{property} recorded definite {recorded:?} but is now \
                 {current:?} — a definite verdict must be preserved exactly (a True↔False flip is a \
                 soundness bug; a definite→⊥ is a precision regression)."
            );
            None
        }
        LedgerVerdict::Indefinite => {
            if current != LedgerVerdict::Indefinite {
                // ⊥ → definite: an allowed improvement. Surface it so the ledger is updated.
                eprintln!(
                    "IMPROVEMENT: {design}.{property} ⊥ → {current:?} — the pipeline refined; \
                     update the ledger to lock this definite verdict in."
                );
                Some(current)
            } else {
                None
            }
        }
    }
}

/// One corpus design: its untouched sources (primary + deps), top module, any config
/// concretization, and the recorded ledger (property-name-substring → verdict). The
/// design's own SVA is verified automatically by `verify-auto`; `annotations` carries
/// the mununu-exclusive liveness properties prepended as `@mununu_guarantee` comments.
struct CorpusDesign {
    name: &'static str,
    /// (filename, path-relative-to `examples/verify/`) — read untouched from disk.
    sources: &'static [(&'static str, &'static str)],
    top: &'static str,
    /// `@mununu_guarantee <mu-formula>` lines prepended to the PRIMARY source.
    annotations: &'static [&'static str],
    config: &'static [(&'static str, u64)],
    /// (property-name-substring, recorded verdict) — the monotone ledger.
    ledger: &'static [(&'static str, LedgerVerdict)],
}

/// Run one corpus design through `verify-auto` and enforce the monotone ledger on every
/// recorded property. Returns the list of ⊥→definite improvements (empty in steady state).
fn check_corpus_design(d: &CorpusDesign) -> Vec<(String, LedgerVerdict)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
    let read = |rel: &str| {
        let p = root.join(rel);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    };
    // Prepend the mununu-exclusive annotations to the primary source (source stays
    // untouched on disk; only this in-memory copy carries the added properties).
    let (primary_name, primary_rel) = d.sources[0];
    let mut primary = String::new();
    for ann in d.annotations {
        primary.push_str("// @mununu_guarantee ");
        primary.push_str(ann);
        primary.push('\n');
    }
    primary.push_str(&read(primary_rel));

    let mut sources = vec![(primary_name.to_string(), primary)];
    for (name, rel) in &d.sources[1..] {
        sources.push((name.to_string(), read(rel)));
    }
    let yopts = YosysOptions {
        top: Some(d.top.to_string()),
        use_sv2v: true,
        additional_sources: sources[1..].to_vec(),
        ..Default::default()
    };
    let config_values = d.config.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    let report = verify_auto(
        &sources,
        &yopts,
        &VerifyAutoOptions {
            exact_symbolic: true,
            config_values,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{}: verify_auto failed: {}", d.name, e.message));

    let mut improvements = Vec::new();
    for (needle, recorded) in d.ledger {
        let prop = report
            .properties
            .iter()
            .find(|p| p.name.contains(needle) || p.formula.contains(needle))
            .unwrap_or_else(|| {
                panic!(
                    "{}: ledger property `{needle}` not in the report; got {:?}",
                    d.name,
                    report
                        .properties
                        .iter()
                        .map(|p| &p.name)
                        .collect::<Vec<_>>()
                )
            });
        let current = outcome_verdict(&prop.outcome);
        if let Some(up) = assert_monotone(d.name, needle, *recorded, current) {
            improvements.push((format!("{}.{needle}", d.name), up));
        }
    }
    improvements
}

/// The corpus. Grows by adding a `CorpusDesign` entry (untouched source + a ledger).
/// Seeded with uart_tx; the verification-prospector backlog (≥15 designs) feeds the
/// expansion. csrng's recoverability flip lives in the v8 example + its own e2e test.
const CORPUS: &[CorpusDesign] = &[CorpusDesign {
    name: "uart_tx",
    sources: &[("uart_tx.sv", "m1_opentitan_uart_tx/source/uart_tx.sv")],
    top: "uart_tx",
    annotations: &[
        // AG AF (a transmission always completes) — VIOLATED (a persistent write / stalled
        // tick holds the counter non-zero forever); AG EF (recoverability) — HOLDS.
        "nu X. ((mu Y. ((bit_cnt_q == 0) or [] Y)) and [] X)",
        "nu Y. ((mu X. ((bit_cnt_q == 0) or <> X)) and [] Y)",
    ],
    config: &[],
    ledger: &[
        // Recorded definite verdicts — must be preserved (a flip = a soundness/precision bug).
        ("(mu Y. ((bit_cnt_q == 0) or [] Y))", LedgerVerdict::False), // AG AF: VIOLATED
        ("(mu X. ((bit_cnt_q == 0) or <> X))", LedgerVerdict::True),  // AG EF: HOLDS
    ],
}];

#[test]
#[ignore = "requires slang + sv2v + yosys (mununu-sva image); run with --ignored"]
fn diff_corpus_monotone_verdict_ledger() {
    let mut all_improvements = Vec::new();
    for d in CORPUS {
        all_improvements.extend(check_corpus_design(d));
    }
    // Improvements (⊥→definite) are NOT failures — they are the signal to update the
    // ledger. Print them; the suite stays green. A definite flip/regress already panicked.
    if !all_improvements.is_empty() {
        eprintln!("\n=== ledger improvements (⊥→definite) — update the ledger ===");
        for (prop, v) in &all_improvements {
            eprintln!("  {prop}: {v:?}");
        }
    }
}
