//! R.5 Item 3 sub-item 3.1 (2026-06-03) — CVC5 binary discovery
//! + version probe.
//!
//! CVC5 (https://cvc5.github.io/) is an OPTIONAL runtime
//! dependency invoked via subprocess for Craig interpolation
//! queries on the R.5 CEGAR loop's `PredicateSource::CraigInterpolation`
//! path. The subprocess pattern mirrors the existing
//! `locate_yosys` / `locate_sv2v` helpers in `adapter/yosys/mod.rs`
//! (the Rust-side `cvc5` / `cvc5-sys` crates are deliberately NOT
//! used — they're younger than `z3-rs` and bring a heavy build
//! dep chain: GMP, CLN, antlr3-c, several C++ deps).
//!
//! When CVC5 is not found at runtime, callers (e.g.
//! `cegar_refine_loop`) fall back to the WP heuristic and emit
//! an [`crate::adapter::AdapterWarning`] documenting the missing
//! dep. CVC5 is never a hard requirement; the rest of mununu
//! functions identically without it.
//!
//! **Discovery order** (mirrors `adapter/yosys/mod.rs` pattern):
//! 1. `MUNUNU_CVC5_PATH` env var (explicit override).
//! 2. `cvc5` on `$PATH` (the standard install location for
//!    Homebrew `brew install cvc5`, Debian `apt install cvc5`,
//!    or the upstream pre-built binaries).
//!
//! **Sub-item 3.1 scope**: discovery + version probe ONLY. The
//! SMT-LIB query construction (sub-item 3.2), subprocess
//! invocation + interpolant parsing (sub-item 3.3), CEGAR wiring
//! (sub-item 3.4), and CLI / sidecar surface (sub-item 3.5)
//! ship in subsequent commits.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use crate::adapter::btor2::PredicateSpec;
use crate::adapter::{AdapterError, AdapterErrorKind};

/// R.5 Item 3 sub-item 3.2 (2026-06-03) — default bit-width
/// assumed for register-equality predicates when constructing
/// the SMT-LIB interpolation query. Picked to match the
/// `u64`-typed `PredicateSpec::value` and the typical BTOR2
/// register widths we lift (8/16/32-bit).
///
/// Sub-item 3.4 will plumb the actual per-register bit-width
/// from the BTOR2 file into the query so wider registers (64-
/// bit timestamps, 128-bit AES state) get the right
/// declarations.
pub const DEFAULT_BV_WIDTH: u32 = 32;

/// R.5 Item 3 sub-item 3.1 (2026-06-03) — handle to a discovered
/// CVC5 binary + its parsed version string.
///
/// The version is captured at discovery time; it's diagnostic
/// only (mununu does not gate on version ≥ X). Returned in
/// adapter warnings + tracing logs so users can confirm which
/// binary is being invoked.
#[derive(Debug, Clone)]
pub struct Cvc5Bin {
    /// Resolved path to the CVC5 binary. May be a bare name
    /// (`cvc5`) if discovered on `$PATH`, or an absolute path if
    /// `MUNUNU_CVC5_PATH` is set.
    pub path: PathBuf,
    /// Parsed version string (e.g. `1.0.5`). `"<unparseable>"`
    /// when the `cvc5 --version` output doesn't match the
    /// expected format — discovery still succeeds.
    pub version: String,
}

/// R.5 Item 3 sub-item 3.1 (2026-06-03) — locate a usable CVC5
/// binary + probe its version. Returns `Ok(Cvc5Bin)` on success.
/// Returns `Err(AdapterError { kind: UnsupportedConstruct, ... })`
/// when the binary is absent or the version probe fails — callers
/// are expected to fall back gracefully (see sub-item 3.4 for the
/// CEGAR-loop wiring).
pub fn locate_cvc5() -> Result<Cvc5Bin, AdapterError> {
    let path = if let Ok(p) = std::env::var("MUNUNU_CVC5_PATH") {
        PathBuf::from(p)
    } else {
        PathBuf::from("cvc5")
    };

    let output = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/cvc5: failed to invoke `{} --version`: {e}. \
                 Set MUNUNU_CVC5_PATH or install cvc5 ≥ 1.0 (Homebrew: \
                 `brew install cvc5`; Debian: `apt install cvc5`).",
                path.display()
            ),
            location: None,
        })?;

    if !output.status.success() {
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/cvc5: `{} --version` exited with status {}",
                path.display(),
                output.status
            ),
            location: None,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = parse_cvc5_version(&stdout).unwrap_or_else(|| "<unparseable>".to_string());

    Ok(Cvc5Bin { path, version })
}

/// R.5 Item 3 sub-item 3.1 (2026-06-03) — extract the version
/// token from `cvc5 --version` output.
///
/// CVC5's `--version` output format (observed across 1.0.x and
/// 1.1.x releases):
/// ```text
/// This is cvc5 version 1.0.5 [git tag v1.0.5 branch HEAD]
/// Copyright (C) 2009-2023 by the authors and their institutional affiliations
/// ...
/// ```
///
/// Some packaged builds (e.g. older Homebrew bottles) drop the
/// leading "This is " and start directly with "cvc5 version".
/// Both forms are accepted.
///
/// Returns `Some("1.0.5")` on success or `None` when the format
/// doesn't match — the caller treats this as `"<unparseable>"`
/// rather than a hard failure (the binary may still be usable
/// for actual queries; the version string is diagnostic-only).
pub fn parse_cvc5_version(output: &str) -> Option<String> {
    let first_line = output.lines().next()?;
    let trimmed = first_line.trim();
    // Accept either "This is cvc5 version X.Y.Z..." or
    // "cvc5 version X.Y.Z...".
    let after_version = trimmed
        .strip_prefix("This is cvc5 version ")
        .or_else(|| trimmed.strip_prefix("cvc5 version "))?;
    // The version token is the first whitespace- or
    // bracket-delimited word.
    let end = after_version
        .find(|c: char| c.is_whitespace() || c == '[')
        .unwrap_or(after_version.len());
    Some(after_version[..end].to_string())
}

/// R.5 Item 3 sub-item 3.2 (2026-06-03) — render a CVC5-
/// compatible SMT-LIB interpolation query that asks for a
/// predicate separating `source_cube` from `target_cube` under
/// the given `predicates` set.
///
/// **Semantics**: each cube index encodes a bit pattern over
/// the predicate set (predicate at position `i` is true at cube
/// `c` iff `(c >> i) & 1 == 1`, mirroring the lifter's
/// convention in `kmts_lift.rs`). The query asserts the source
/// cube's predicate conjunction as the "A" side, then asks CVC5
/// for an interpolant separating it from the target cube's
/// conjunction.
///
/// **Output shape** (illustrative, not literal):
/// ```text
/// (set-logic QF_BV)
/// (set-option :produce-interpolants true)
/// (declare-fun reg_a () (_ BitVec 32))
/// (assert (= reg_a (_ bv0 32)))
/// (get-interpolant I (not (= reg_a (_ bv1 32))))
/// (exit)
/// ```
///
/// **Sub-item 3.2 scope**: pure rendering. The function does
/// not invoke CVC5, parse the interpolant response, or feed it
/// back into mununu's predicate set — those are sub-items 3.3 +
/// 3.4. This function is golden-output testable via byte-equal
/// comparison against an expected string.
///
/// **Register bit-width**: uses [`DEFAULT_BV_WIDTH`] (32-bit)
/// for every register. Sub-item 3.4 will plumb the actual
/// per-register width from BTOR2.
///
/// **Cube-index interpretation**: matches the lifter's bit
/// convention. For `predicates = [p_a, p_b, p_c]` (in that
/// order):
/// - cube 0 = (¬p_a ∧ ¬p_b ∧ ¬p_c)
/// - cube 1 = (p_a ∧ ¬p_b ∧ ¬p_c)
/// - cube 2 = (¬p_a ∧ p_b ∧ ¬p_c)
/// - ...
/// - cube 7 = (p_a ∧ p_b ∧ p_c)
///
/// Each predicate is rendered as `(= <register> (_ bv<value>
/// <width>))` for the positive case or
/// `(not (= <register> (_ bv<value> <width>)))` for the negated
/// case.
pub fn build_interpolation_query(
    predicates: &[PredicateSpec],
    source_cube: usize,
    target_cube: usize,
) -> String {
    let mut out = String::new();
    out.push_str("(set-logic QF_BV)\n");
    out.push_str("(set-option :produce-interpolants true)\n");

    // Declare each distinct register. Sorted for deterministic
    // output (BTreeSet gives the iteration order; same order
    // means byte-stable golden tests across runs).
    let registers: BTreeSet<&str> = predicates.iter().map(|p| p.register.as_str()).collect();
    for reg in &registers {
        out.push_str(&format!(
            "(declare-fun {} () (_ BitVec {}))\n",
            reg, DEFAULT_BV_WIDTH
        ));
    }

    // Source cube facts (A side). Emit as a single (assert (and
    // ...)) when |predicates| > 1, or a single (assert ...) when
    // == 1, or skip entirely when == 0 (the empty conjunction
    // is trivially true).
    let source_conj = render_cube_conjunction(predicates, source_cube);
    if let Some(conj) = source_conj {
        out.push_str(&format!("(assert {conj})\n"));
    }

    // Target cube facts (B side) — passed as the get-interpolant
    // formula. CVC5's contract: returns I such that A ⊨ I and
    // I ⊨ ¬B. We pass the target cube's conjunction as B; the
    // returned interpolant separates source from target.
    let target_conj =
        render_cube_conjunction(predicates, target_cube).unwrap_or_else(|| "true".to_string());
    out.push_str(&format!("(get-interpolant I {target_conj})\n"));
    out.push_str("(exit)\n");
    out
}

/// R.5 Item 3 sub-item 3.2 (2026-06-03) — render the conjunction
/// of predicate facts at a given cube index. Returns:
/// - `None` if `predicates` is empty (the empty conjunction is
///   trivially true; caller decides whether to emit `true` or
///   skip the assert entirely).
/// - `Some(<single-fact>)` for a 1-element predicate set.
/// - `Some((and <facts>))` for multi-element sets.
///
/// Each fact is `(= <register> (_ bv<value> <width>))` for the
/// positive bit and `(not (= <register> (_ bv<value> <width>)))`
/// for the negated bit.
fn render_cube_conjunction(predicates: &[PredicateSpec], cube: usize) -> Option<String> {
    if predicates.is_empty() {
        return None;
    }
    let facts: Vec<String> = predicates
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let positive = (cube >> i) & 1 == 1;
            let eq = format!("(= {} (_ bv{} {}))", p.register, p.value, DEFAULT_BV_WIDTH);
            if positive { eq } else { format!("(not {eq})") }
        })
        .collect();
    if facts.len() == 1 {
        Some(facts.into_iter().next().unwrap())
    } else {
        Some(format!("(and {})", facts.join(" ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cvc5_version_extracts_from_standard_format() {
        let out = "This is cvc5 version 1.0.5 [git tag v1.0.5 branch HEAD]\n\
                   Copyright (C) 2009-2023 by the authors\n";
        assert_eq!(parse_cvc5_version(out).as_deref(), Some("1.0.5"));
    }

    #[test]
    fn parse_cvc5_version_extracts_from_short_format() {
        let out = "cvc5 version 1.1.2\n";
        assert_eq!(parse_cvc5_version(out).as_deref(), Some("1.1.2"));
    }

    #[test]
    fn parse_cvc5_version_handles_trailing_whitespace_only() {
        let out = "This is cvc5 version 1.0.0\n";
        assert_eq!(parse_cvc5_version(out).as_deref(), Some("1.0.0"));
    }

    #[test]
    fn parse_cvc5_version_returns_none_on_unknown_format() {
        let out = "Something else entirely\n";
        assert_eq!(parse_cvc5_version(out), None);
    }

    #[test]
    fn parse_cvc5_version_returns_none_on_empty_output() {
        assert_eq!(parse_cvc5_version(""), None);
    }

    #[test]
    fn locate_cvc5_returns_structured_error_when_binary_absent() {
        // Force a definitely-missing path via the env var. The
        // function MUST return a structured AdapterError (not
        // panic) so callers can fall back gracefully.
        // SAFETY: env vars are process-global; this test sets +
        // unsets to avoid leaking the bogus path to other tests
        // that may discover via PATH. Tests in the same crate
        // run in parallel by default but `--test-threads=1`
        // serializes if collisions show up.
        // Use a path that's guaranteed not to exist + not
        // executable.
        let original = std::env::var("MUNUNU_CVC5_PATH").ok();
        // SAFETY: required for env var manipulation in tests; the
        // value is restored before the test returns.
        unsafe {
            std::env::set_var(
                "MUNUNU_CVC5_PATH",
                "/nonexistent/path/to/cvc5/binary/definitely/not/here",
            );
        }
        let result = locate_cvc5();
        unsafe {
            match original {
                Some(v) => std::env::set_var("MUNUNU_CVC5_PATH", v),
                None => std::env::remove_var("MUNUNU_CVC5_PATH"),
            }
        }
        assert!(
            result.is_err(),
            "locate_cvc5 MUST return Err when the binary is absent; got {result:?}"
        );
        let err = result.unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
        assert!(
            err.message.contains("cvc5"),
            "error message MUST mention cvc5 for diagnosability; got: {}",
            err.message
        );
    }

    #[test]
    #[ignore = "requires cvc5 binary installed; run with --ignored when available"]
    fn locate_cvc5_succeeds_when_binary_available() {
        // R.5 Item 3 sub-item 3.1 — integration test that's
        // ignored by default. Run with `cargo test -- --ignored`
        // when CVC5 is installed locally (e.g. `brew install
        // cvc5` first).
        let result = locate_cvc5();
        match result {
            Ok(bin) => {
                assert!(
                    bin.version != "<unparseable>",
                    "expected a parseable version from `cvc5 --version`; got {}",
                    bin.version
                );
            }
            Err(e) => panic!(
                "locate_cvc5 failed when CVC5 should be installed (run \
                 `brew install cvc5` or set MUNUNU_CVC5_PATH first): {}",
                e.message
            ),
        }
    }

    // ─────────────────────────────────────────────────────────────
    // R.5 Item 3 sub-item 3.2 tests — SMT-LIB interpolation query
    // construction. Golden-output tests; byte-equal comparison
    // against expected strings.
    // ─────────────────────────────────────────────────────────────

    fn pred(name: &str, register: &str, value: u64) -> PredicateSpec {
        PredicateSpec {
            name: name.to_string(),
            register: register.to_string(),
            value,
        }
    }

    #[test]
    fn r5_subitem_32_render_empty_predicates_yields_trivial_query() {
        // Edge case: no predicates ⇒ empty conjunction ⇒ no
        // assert; the get-interpolant target is `true`.
        let got = build_interpolation_query(&[], 0, 0);
        let expected = "\
(set-logic QF_BV)
(set-option :produce-interpolants true)
(get-interpolant I true)
(exit)
";
        assert_eq!(got, expected);
    }

    #[test]
    fn r5_subitem_32_render_single_predicate_positive_source_negative_target() {
        // |P| = 1, source cube 1 (predicate holds), target cube
        // 0 (predicate doesn't hold). Source assertion is the
        // positive equality; target is the negation.
        let preds = vec![pred("p_zero", "reg_a", 0)];
        let got = build_interpolation_query(&preds, 1, 0);
        let expected = "\
(set-logic QF_BV)
(set-option :produce-interpolants true)
(declare-fun reg_a () (_ BitVec 32))
(assert (= reg_a (_ bv0 32)))
(get-interpolant I (not (= reg_a (_ bv0 32))))
(exit)
";
        assert_eq!(got, expected);
    }

    #[test]
    fn r5_subitem_32_render_single_predicate_negative_source_positive_target() {
        // |P| = 1, source cube 0 (predicate doesn't hold),
        // target cube 1 (predicate holds). Source assertion is
        // the negation; target is the positive equality.
        let preds = vec![pred("p_zero", "reg_a", 0)];
        let got = build_interpolation_query(&preds, 0, 1);
        let expected = "\
(set-logic QF_BV)
(set-option :produce-interpolants true)
(declare-fun reg_a () (_ BitVec 32))
(assert (not (= reg_a (_ bv0 32))))
(get-interpolant I (= reg_a (_ bv0 32)))
(exit)
";
        assert_eq!(got, expected);
    }

    #[test]
    fn r5_subitem_32_render_multi_predicate_both_registers() {
        // |P| = 2, two different registers. Cube 3 (binary 11)
        // = both predicates hold; cube 0 = neither holds.
        // Conjunction is rendered as (and <facts>).
        let preds = vec![pred("p_a_zero", "reg_a", 0), pred("p_b_one", "reg_b", 1)];
        let got = build_interpolation_query(&preds, 3, 0);
        let expected = "\
(set-logic QF_BV)
(set-option :produce-interpolants true)
(declare-fun reg_a () (_ BitVec 32))
(declare-fun reg_b () (_ BitVec 32))
(assert (and (= reg_a (_ bv0 32)) (= reg_b (_ bv1 32))))
(get-interpolant I (and (not (= reg_a (_ bv0 32))) (not (= reg_b (_ bv1 32)))))
(exit)
";
        assert_eq!(got, expected);
    }

    #[test]
    fn r5_subitem_32_render_predicates_sharing_same_register_dedupes_decl() {
        // Two predicates over the same register: should declare
        // the register only once (BTreeSet dedupes).
        let preds = vec![pred("p_zero", "reg_a", 0), pred("p_one", "reg_a", 1)];
        let got = build_interpolation_query(&preds, 1, 2);
        // Source cube 1 = (p_zero holds, p_one doesn't) =
        //   reg_a == 0 AND reg_a != 1 (consistent: reg_a == 0).
        // Target cube 2 = (p_zero doesn't hold, p_one holds) =
        //   reg_a != 0 AND reg_a == 1 (consistent: reg_a == 1).
        // The declare-fun for reg_a appears EXACTLY ONCE.
        let expected = "\
(set-logic QF_BV)
(set-option :produce-interpolants true)
(declare-fun reg_a () (_ BitVec 32))
(assert (and (= reg_a (_ bv0 32)) (not (= reg_a (_ bv1 32)))))
(get-interpolant I (and (not (= reg_a (_ bv0 32))) (= reg_a (_ bv1 32))))
(exit)
";
        assert_eq!(got, expected);
    }

    #[test]
    fn r5_subitem_32_render_is_deterministic_across_calls() {
        // The function MUST be pure + deterministic. Calling it
        // twice with the same input must produce byte-identical
        // output (BTreeSet iteration order is sorted/stable).
        let preds = vec![
            pred("p_c", "reg_c", 5),
            pred("p_a", "reg_a", 1),
            pred("p_b", "reg_b", 3),
        ];
        let q1 = build_interpolation_query(&preds, 5, 2);
        let q2 = build_interpolation_query(&preds, 5, 2);
        assert_eq!(q1, q2, "build_interpolation_query MUST be deterministic");
        // And the declare-fun order MUST be alphabetical by
        // register name (BTreeSet sort), even though predicates
        // were given in c, a, b order.
        let lines: Vec<&str> = q1.lines().collect();
        let declare_lines: Vec<&str> = lines
            .iter()
            .filter(|l| l.starts_with("(declare-fun"))
            .copied()
            .collect();
        assert_eq!(
            declare_lines,
            vec![
                "(declare-fun reg_a () (_ BitVec 32))",
                "(declare-fun reg_b () (_ BitVec 32))",
                "(declare-fun reg_c () (_ BitVec 32))",
            ],
            "declare-fun lines MUST be sorted alphabetically by register name"
        );
    }

    #[test]
    fn r5_subitem_32_render_cube_index_bit_convention_matches_lifter() {
        // The cube-index bit convention MUST match the lifter's
        // (kmts_lift.rs ~line 587):
        //   predicate at position `i` holds at cube `c` iff
        //   `(c >> i) & 1 == 1`.
        // For |P| = 3, cube 5 (binary 101) = predicates at
        // positions 0 and 2 hold, position 1 doesn't.
        let preds = vec![
            pred("p0", "reg_x", 10),
            pred("p1", "reg_y", 20),
            pred("p2", "reg_z", 30),
        ];
        let got = build_interpolation_query(&preds, 5, 0);
        // Source = cube 5 = (p0 holds, p1 doesn't, p2 holds)
        //        = (reg_x == 10) AND (reg_y != 20) AND (reg_z == 30)
        assert!(
            got.contains(
                "(assert (and (= reg_x (_ bv10 32)) (not (= reg_y (_ bv20 32))) (= reg_z (_ bv30 32))))"
            ),
            "source-cube assertion MUST encode cube index 5 = binary 101 correctly; got:\n{got}"
        );
    }
}
