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

use std::path::PathBuf;
use std::process::Command;

use crate::adapter::{AdapterError, AdapterErrorKind};

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
}
