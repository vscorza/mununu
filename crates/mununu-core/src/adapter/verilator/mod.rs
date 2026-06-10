//! R-S2b (Phase 9 §9.1) sub-item R-S2b.1 (2026-06-10) — Verilator
//! subprocess wrapper skeleton + binary discovery + version probe.
//!
//! # What this module does (R-S2b.1 scope)
//!
//! Locates a usable `verilator` binary and probes its version. Mirrors
//! the [`crate::adapter::cvc5`] subprocess-wrapper conventions
//! established for CVC5 — same `XxxBin` handle struct, same env-var
//! override (`MUNUNU_VERILATOR_PATH`), same structured `AdapterError`
//! on missing binary, same `#[ignore]`-gated integration test for the
//! present-binary path.
//!
//! Verilator is **optional at runtime**. R-S2b's parent purpose is to
//! seed predicate sets from observed steady-state register valuations
//! after a short concrete reset simulation (§Phase 9 R-S2b strategy).
//! When Verilator is absent, the caller is expected to log an
//! `AdapterWarning` and fall back to other Phase 9 strategies (R-S5
//! type-driven, R-S7 property-syntactic, etc.) — exactly the same
//! "optional subprocess, structured fallback" discipline R.5 Item 3
//! sub-item 3.4 established for CVC5 / Craig interpolation.
//!
//! # What this module does NOT do yet
//!
//! R-S2b is a multi-session arc; this sub-item ships ONLY the
//! discovery layer. Subsequent sub-items will add:
//!
//! - R-S2b.2 — Verilator compile invocation: given an SV file + top +
//!   reset signal name, produce a Verilator-compiled simulation
//!   binary in a temp dir. Mirror of `run_sv2v` / `run_yosys`.
//! - R-S2b.3 — reset-simulation harness: C++ testbench that drives
//!   the reset for K cycles + dumps register valuations; compile +
//!   run; capture steady-state.
//! - R-S2b.4 — steady-state → predicate-set integration. New
//!   `PredicateSource::ResetSimulation` variant feeding the existing
//!   seeding pipeline.
//! - R-S2b.5 — sidecar `simulate_reset: Option<SimulateReset>` field.
//! - R-S2b.6 — CLI flag + end-to-end integration test.
//!
//! # Why `MUNUNU_VERILATOR_PATH`
//!
//! Same rationale as `MUNUNU_CVC5_PATH` (CVC5 wrapper §"Pattern B
//! deployment"): Verilator is contributor-installed, not bundled in
//! `Dockerfile.dev`. Common install paths:
//!
//! - macOS Homebrew: `brew install verilator` → `/usr/local/bin/verilator`
//!   (Intel) or `/opt/homebrew/bin/verilator` (Apple Silicon).
//! - Debian / Ubuntu: `apt install verilator` → `/usr/bin/verilator`.
//! - Build-from-source: typically `/usr/local/bin/verilator`.

use std::path::PathBuf;
use std::process::Command;

use crate::adapter::{AdapterError, AdapterErrorKind};

/// Resolved Verilator binary + parsed version. Returned by
/// [`locate_verilator`].
#[derive(Debug, Clone)]
pub struct VerilatorBin {
    /// Resolved path to the Verilator binary. May be a bare name
    /// (`verilator`) if discovered on `$PATH`, or an absolute path
    /// if `MUNUNU_VERILATOR_PATH` is set.
    pub path: PathBuf,
    /// Parsed version string (e.g. `5.018`). `"<unparseable>"` when
    /// the `verilator --version` output doesn't match the expected
    /// format — discovery still succeeds; the version is
    /// diagnostic-only and does not gate functionality.
    pub version: String,
}

/// R-S2b.1 (2026-06-10) — locate a usable Verilator binary + probe
/// its version. Returns `Ok(VerilatorBin)` on success.
///
/// Returns `Err(AdapterError { kind: UnsupportedConstruct, ... })`
/// when the binary is absent or the version probe fails — callers
/// are expected to fall back gracefully (see R-S2b.4 + R-S2b.6 for
/// the predicate-seeding-pipeline wiring; the pattern is the same
/// optional-subprocess fallback CVC5 / Craig interpolation uses).
pub fn locate_verilator() -> Result<VerilatorBin, AdapterError> {
    let path = if let Ok(p) = std::env::var("MUNUNU_VERILATOR_PATH") {
        PathBuf::from(p)
    } else {
        PathBuf::from("verilator")
    };

    let output = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/verilator: failed to invoke `{} --version`: {e}. \
                 Set MUNUNU_VERILATOR_PATH or install verilator ≥ 4.0 \
                 (Homebrew: `brew install verilator`; Debian: \
                 `apt install verilator`).",
                path.display()
            ),
            location: None,
        })?;

    if !output.status.success() {
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/verilator: `{} --version` exited with status {}",
                path.display(),
                output.status
            ),
            location: None,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = parse_verilator_version(&stdout).unwrap_or_else(|| "<unparseable>".to_string());

    Ok(VerilatorBin { path, version })
}

/// R-S2b.1 (2026-06-10) — extract the version token from
/// `verilator --version` output.
///
/// Verilator's `--version` output format (observed across 4.x and
/// 5.x releases):
/// ```text
/// Verilator 5.018 2024-01-07 rev (Homebrew v5.018)
/// ```
/// or, for older / from-source builds:
/// ```text
/// Verilator 4.228 2022-07-04 rev v4.228
/// ```
///
/// Returns `Some("5.018")` / `Some("4.228")` on success or `None`
/// when the format doesn't match — the caller treats this as
/// `"<unparseable>"` rather than a hard failure (the binary may
/// still be usable for actual queries; the version string is
/// diagnostic-only).
pub fn parse_verilator_version(output: &str) -> Option<String> {
    let first_line = output.lines().next()?;
    let trimmed = first_line.trim();
    let after_prefix = trimmed.strip_prefix("Verilator ")?;
    let end = after_prefix
        .find(char::is_whitespace)
        .unwrap_or(after_prefix.len());
    Some(after_prefix[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_verilator_returns_structured_error_when_binary_absent() {
        // Force a definitely-missing path via the env var. The
        // function MUST return a structured AdapterError (not panic)
        // so callers can fall back gracefully.
        // SAFETY: env vars are process-global; this test sets +
        // restores to avoid leaking the bogus path to other tests
        // that may discover via PATH.
        let original = std::env::var("MUNUNU_VERILATOR_PATH").ok();
        // SAFETY: required for env var manipulation in tests; the
        // value is restored before the test returns.
        unsafe {
            std::env::set_var(
                "MUNUNU_VERILATOR_PATH",
                "/nonexistent/path/to/verilator/binary/definitely/not/here",
            );
        }
        let result = locate_verilator();
        unsafe {
            match original {
                Some(v) => std::env::set_var("MUNUNU_VERILATOR_PATH", v),
                None => std::env::remove_var("MUNUNU_VERILATOR_PATH"),
            }
        }
        assert!(
            result.is_err(),
            "locate_verilator MUST return Err when the binary is absent; got {result:?}"
        );
        let err = result.unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
        assert!(
            err.message.contains("verilator"),
            "error message MUST mention verilator for diagnosability; got: {}",
            err.message
        );
    }

    #[test]
    #[ignore = "requires verilator binary installed; run with --ignored when available"]
    fn locate_verilator_succeeds_when_binary_available() {
        // R-S2b.1 integration test that's ignored by default. Run
        // with `cargo test -- --ignored` when Verilator is
        // installed locally (e.g. `brew install verilator` first).
        let result = locate_verilator();
        match result {
            Ok(bin) => {
                assert!(
                    bin.version != "<unparseable>",
                    "expected a parseable version from `verilator --version`; got {}",
                    bin.version
                );
            }
            Err(e) => panic!(
                "locate_verilator failed when Verilator should be installed (run \
                 `brew install verilator` or set MUNUNU_VERILATOR_PATH first): {}",
                e.message
            ),
        }
    }

    #[test]
    fn parse_verilator_version_matches_homebrew_5x_format() {
        let output = "Verilator 5.018 2024-01-07 rev (Homebrew v5.018)\n";
        assert_eq!(parse_verilator_version(output), Some("5.018".to_string()));
    }

    #[test]
    fn parse_verilator_version_matches_legacy_4x_format() {
        let output = "Verilator 4.228 2022-07-04 rev v4.228\n";
        assert_eq!(parse_verilator_version(output), Some("4.228".to_string()));
    }

    #[test]
    fn parse_verilator_version_returns_none_on_unknown_format() {
        let output = "some other tool that is not verilator\n";
        assert_eq!(parse_verilator_version(output), None);
    }

    #[test]
    fn parse_verilator_version_returns_none_on_empty_output() {
        assert_eq!(parse_verilator_version(""), None);
    }
}
