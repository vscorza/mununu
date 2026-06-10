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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
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

// ─────────────────────────────────────────────────────────────────────
// R-S2b.2 (2026-06-10) — Verilator compile invocation
// ─────────────────────────────────────────────────────────────────────

/// Caller-facing options for a Verilator compile invocation.
///
/// Mirrors [`crate::adapter::yosys::YosysOptions`] in spirit — an
/// inert options bag the caller fills before invoking
/// [`compile_verilator`].
#[derive(Debug, Clone, Default)]
pub struct VerilatorOptions {
    /// Optional `--top-module <NAME>` override. When `None`,
    /// Verilator infers the top from the source file. The
    /// reset-simulation harness (R-S2b.3) normally passes
    /// `Some(top_name)` for determinism.
    pub top: Option<String>,
    /// Pass `-O3` to Verilator-generated C++. Defaults to `false`
    /// (use `-O0` for faster compile during the short
    /// reset-simulation runs). R-S2b's reset simulation is bounded
    /// to ~100 cycles per call, so simulation throughput is not the
    /// bottleneck.
    pub optimize: bool,
    /// Pass `-Wno-fatal` to demote warnings to non-fatal. Useful
    /// when the input SV emits widening / sign-mismatch warnings
    /// the user has triaged. Defaults to `false`.
    pub silence_warnings: bool,
}

/// R-S2b.2 (2026-06-10) — pure helper that constructs the
/// `verilator` command-line argument vector for a compile
/// invocation, given an options bag + SV source + testbench C++
/// path + Verilator output directory (`--Mdir`).
///
/// **Pure**: does not invoke any subprocess. Testable in isolation
/// without a Verilator binary present.
///
/// **Output shape** (illustrative — order matters because the
/// non-flag positional arguments — the testbench `.cpp` and the
/// SV source — must come last):
/// ```text
/// --cc --exe --Mdir <mdir> [--top-module <top>] [-O3]
///   [-Wno-fatal] <tb_cpp> <sv_path>
/// ```
///
/// The caller passes the result to `std::process::Command::new(verilator).args(...)`.
pub fn build_verilator_compile_args(
    opts: &VerilatorOptions,
    sv_path: &Path,
    tb_cpp_path: &Path,
    mdir: &Path,
) -> Vec<OsString> {
    // `--cc` emits the C++ model; `--exe` tells Verilator to also
    // build an executable (using the supplied testbench .cpp +
    // generated `Mdir`). `--Mdir <mdir>` directs the per-call
    // work directory.
    let mut args: Vec<OsString> = vec![
        OsString::from("--cc"),
        OsString::from("--exe"),
        OsString::from("--Mdir"),
        OsString::from(mdir),
    ];
    // Optional `--top-module <NAME>`.
    if let Some(top) = &opts.top {
        args.push(OsString::from("--top-module"));
        args.push(OsString::from(top));
    }
    // Optional `-O3` (C++ optimisation level).
    if opts.optimize {
        args.push(OsString::from("-O3"));
    }
    // Optional `-Wno-fatal`.
    if opts.silence_warnings {
        args.push(OsString::from("-Wno-fatal"));
    }
    // Positional: testbench C++ first, then the SV source. Verilator
    // accepts these in any order, but a stable ordering eases
    // debugging.
    args.push(OsString::from(tb_cpp_path));
    args.push(OsString::from(sv_path));
    args
}

/// R-S2b.2 (2026-06-10) — derive the top-module name Verilator
/// will use for the generated header (`V<top>.h`) and Makefile
/// (`V<top>.mk`).
///
/// When the caller supplied `opts.top`, use it. Otherwise, fall
/// back to the SV file stem (Verilator's default behaviour when
/// `--top-module` is omitted is to use the filename without
/// extension as the top).
pub fn derive_top_name(opts: &VerilatorOptions, sv_path: &Path) -> String {
    if let Some(top) = &opts.top {
        return top.clone();
    }
    sv_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("top")
        .to_string()
}

/// R-S2b.2 (2026-06-10) — end-to-end Verilator compile + link.
///
/// 1. Writes `tb_cpp_content` to `<workdir>/tb.cpp`.
/// 2. Invokes `verilator --cc --exe ... tb.cpp sv_path` with the
///    `--Mdir <workdir>/obj_dir` argument so the generated C++
///    + Makefile land in a deterministic, per-call subdirectory.
/// 3. Invokes `make -C <obj_dir> -f V<top>.mk V<top>` to compile
///    + link the simulation binary.
/// 4. Returns the absolute path to the produced binary
///    (`<obj_dir>/V<top>`).
///
/// **Workdir lifetime is the caller's responsibility.** R-S2b.3
/// owns the [`VerilatorTempDir`] handle and drops it after
/// extracting the steady-state register valuation.
///
/// **Error handling**: failures spawn / non-zero exit at either
/// step return `AdapterError { kind: ParseError, ... }` (matching
/// the convention `run_yosys` + `run_sv2v` use). The Verilator
/// stderr is embedded in the error message so the user can
/// diagnose SV-parse failures + missing-construct gaps.
pub fn compile_verilator(
    verilator: &Path,
    opts: &VerilatorOptions,
    sv_path: &Path,
    tb_cpp_content: &str,
    workdir: &Path,
) -> Result<PathBuf, AdapterError> {
    let tb_cpp_path = workdir.join("tb.cpp");
    std::fs::write(&tb_cpp_path, tb_cpp_content).map_err(|e| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: format!(
            "adapter/verilator: failed to write testbench to {}: {e}",
            tb_cpp_path.display()
        ),
        location: None,
    })?;

    let mdir = workdir.join("obj_dir");
    let args = build_verilator_compile_args(opts, sv_path, &tb_cpp_path, &mdir);

    let v_out = Command::new(verilator)
        .args(&args)
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!(
                "adapter/verilator: failed to spawn `{}`: {e}",
                verilator.display()
            ),
            location: None,
        })?;
    if !v_out.status.success() {
        let stderr = String::from_utf8_lossy(&v_out.stderr);
        let stdout = String::from_utf8_lossy(&v_out.stdout);
        return Err(AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!(
                "adapter/verilator: verilator exited with status {} for {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                v_out.status,
                sv_path.display()
            ),
            location: None,
        });
    }

    let top = derive_top_name(opts, sv_path);
    let mk_name = format!("V{top}.mk");
    let bin_name = format!("V{top}");
    let m_out = Command::new("make")
        .arg("-C")
        .arg(&mdir)
        .arg("-f")
        .arg(&mk_name)
        .arg(&bin_name)
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!(
                "adapter/verilator: failed to spawn `make` for {}: {e}",
                mdir.display()
            ),
            location: None,
        })?;
    if !m_out.status.success() {
        let stderr = String::from_utf8_lossy(&m_out.stderr);
        let stdout = String::from_utf8_lossy(&m_out.stdout);
        return Err(AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!(
                "adapter/verilator: make -f {mk_name} {bin_name} exited with status {} in {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                m_out.status,
                mdir.display()
            ),
            location: None,
        });
    }

    Ok(mdir.join(bin_name))
}

/// R-S2b.2 (2026-06-10) — per-call work directory for Verilator
/// compile + simulation. Mirrors `yosys::TempDir` (private to that
/// module per the "avoid pulling in another dep" convention).
///
/// Set `MUNUNU_KEEP_VERILATOR_TMP=1` to preserve the directory
/// after drop (useful when diagnosing a Verilator compile failure).
pub struct VerilatorTempDir {
    path: PathBuf,
    keep: bool,
}

impl VerilatorTempDir {
    /// Create a new per-call work directory under `$TMPDIR`. The
    /// directory name embeds pid + thread id + sequence + nanos so
    /// parallel tests in the same process do not collide.
    pub fn new() -> Result<Self, AdapterError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let tid = format!("{:?}", std::thread::current().id());
        let tid_digits: String = tid.chars().filter(|c| c.is_ascii_digit()).collect();
        let mut base = std::env::temp_dir();
        base.push(format!(
            "mununu-verilator-{pid}-{tid_digits}-{seq}-{nanos}"
        ));
        std::fs::create_dir_all(&base).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!(
                "adapter/verilator: failed to create tempdir at {}: {e}",
                base.display()
            ),
            location: None,
        })?;
        let keep = std::env::var("MUNUNU_KEEP_VERILATOR_TMP")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Ok(VerilatorTempDir { path: base, keep })
    }

    /// Path to the work directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for VerilatorTempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
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

    // ─────────────────────────────────────────────────────────────
    // R-S2b.2 tests — compile-invocation args + tempdir
    // ─────────────────────────────────────────────────────────────

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn build_args_includes_cc_and_exe() {
        let opts = VerilatorOptions::default();
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        assert!(args.contains(&os("--cc")), "args must include --cc");
        assert!(args.contains(&os("--exe")), "args must include --exe");
    }

    #[test]
    fn build_args_includes_mdir_flag_and_path() {
        let opts = VerilatorOptions::default();
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        // --Mdir followed by the actual directory.
        let i = args
            .iter()
            .position(|a| a == &os("--Mdir"))
            .expect("--Mdir must be present");
        assert_eq!(
            args.get(i + 1),
            Some(&os("/work/obj_dir")),
            "--Mdir must be followed by the obj_dir path"
        );
    }

    #[test]
    fn build_args_includes_top_module_when_set() {
        let opts = VerilatorOptions {
            top: Some("amba_arbiter".to_string()),
            ..VerilatorOptions::default()
        };
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        let i = args
            .iter()
            .position(|a| a == &os("--top-module"))
            .expect("--top-module must be present when opts.top is set");
        assert_eq!(args.get(i + 1), Some(&os("amba_arbiter")));
    }

    #[test]
    fn build_args_omits_top_module_when_none() {
        let opts = VerilatorOptions::default();
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        assert!(
            !args.contains(&os("--top-module")),
            "--top-module must be omitted when opts.top is None; got {args:?}"
        );
    }

    #[test]
    fn build_args_includes_optimize_when_set() {
        let opts = VerilatorOptions {
            optimize: true,
            ..VerilatorOptions::default()
        };
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        assert!(args.contains(&os("-O3")));
    }

    #[test]
    fn build_args_includes_wno_fatal_when_set() {
        let opts = VerilatorOptions {
            silence_warnings: true,
            ..VerilatorOptions::default()
        };
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        assert!(args.contains(&os("-Wno-fatal")));
    }

    #[test]
    fn build_args_omits_optimize_and_warnings_by_default() {
        let opts = VerilatorOptions::default();
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        assert!(!args.contains(&os("-O3")));
        assert!(!args.contains(&os("-Wno-fatal")));
    }

    #[test]
    fn build_args_places_tb_and_sv_at_end_in_order() {
        // Verilator accepts these in any order, but a stable
        // ordering eases reproduction + debugging. The contract is:
        // tb.cpp comes before the .sv source.
        let opts = VerilatorOptions::default();
        let args = build_verilator_compile_args(
            &opts,
            Path::new("/work/design.sv"),
            Path::new("/work/tb.cpp"),
            Path::new("/work/obj_dir"),
        );
        let tb_i = args.iter().position(|a| a == &os("/work/tb.cpp"));
        let sv_i = args.iter().position(|a| a == &os("/work/design.sv"));
        assert!(tb_i.is_some() && sv_i.is_some());
        assert!(
            tb_i.unwrap() < sv_i.unwrap(),
            "tb.cpp must come before design.sv"
        );
    }

    #[test]
    fn derive_top_name_uses_opts_when_set() {
        let opts = VerilatorOptions {
            top: Some("explicit_top".to_string()),
            ..VerilatorOptions::default()
        };
        assert_eq!(
            derive_top_name(&opts, Path::new("/work/design.sv")),
            "explicit_top"
        );
    }

    #[test]
    fn derive_top_name_falls_back_to_file_stem() {
        let opts = VerilatorOptions::default();
        assert_eq!(
            derive_top_name(&opts, Path::new("/work/amba_arbiter.sv")),
            "amba_arbiter"
        );
    }

    #[test]
    fn verilator_tempdir_creates_unique_directories() {
        // Two tempdirs in the same process must not collide. The
        // pid + thread + seq + nanos suffix guarantees this.
        let d1 = VerilatorTempDir::new().expect("tempdir 1");
        let d2 = VerilatorTempDir::new().expect("tempdir 2");
        assert_ne!(d1.path(), d2.path());
        assert!(d1.path().exists());
        assert!(d2.path().exists());
    }

    #[test]
    fn verilator_tempdir_removes_directory_on_drop() {
        let path = {
            let d = VerilatorTempDir::new().expect("tempdir");
            assert!(d.path().exists());
            d.path().to_path_buf()
            // drop here
        };
        assert!(
            !path.exists(),
            "VerilatorTempDir must remove its directory on drop (unless MUNUNU_KEEP_VERILATOR_TMP=1)"
        );
    }

    #[test]
    #[ignore = "requires verilator + make installed; run with --ignored when available"]
    fn compile_verilator_produces_runnable_binary_on_trivial_module() {
        // End-to-end integration test: a minimal SV module + a
        // no-op testbench compile cleanly + produce a binary that
        // exits 0 when run. Gated because Verilator is contributor-
        // installed (matches the locate_verilator integration-test
        // gating).
        let bin = locate_verilator().expect("verilator must be installed for this test");
        let tmp = VerilatorTempDir::new().expect("tempdir");
        let sv_path = tmp.path().join("trivial.sv");
        std::fs::write(
            &sv_path,
            "module trivial(input wire clk); endmodule\n",
        )
        .expect("write sv");
        let tb = "#include \"Vtrivial.h\"\nint main(int,char**){ Vtrivial m; m.eval(); return 0; }\n";
        let opts = VerilatorOptions {
            top: Some("trivial".to_string()),
            ..VerilatorOptions::default()
        };
        let bin_path = compile_verilator(&bin.path, &opts, &sv_path, tb, tmp.path())
            .expect("compile_verilator must succeed on the trivial module");
        assert!(bin_path.exists(), "binary path {} must exist", bin_path.display());
        let status = Command::new(&bin_path)
            .status()
            .expect("spawn compiled binary");
        assert!(status.success(), "compiled binary must exit 0; got {status}");
    }
}
