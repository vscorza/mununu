//! H.O.1a (2026-06-29) — external BTOR2 model-checker wrapper (`btormc`).
//!
//! # Why this module exists
//!
//! H.O.0 ([`crate::adapter::btor2::concrete_oracle`]) is the *internal* exact
//! oracle: it computes a safety property's concrete truth by **bounded
//! reachability enumeration** (mununu's own one-step semantics, no abstraction).
//! That is sound for finding violations always, but it can only conclude
//! `AG`-true when it enumerated the full input space — so on a real OpenTitan
//! design with wide config inputs (sysrst's `cfg_detect_timer_i`, …) it is forced
//! to [`AgOracle::Inconclusive`](crate::adapter::btor2::concrete_oracle::AgOracle::Inconclusive):
//! it can REFUTE a spurious cube-`HOLDS` but cannot CONFIRM a real-RTL `HOLDS`
//! (H.O.0.3b established exactly this boundary).
//!
//! H.O.1 closes that gap with an **external, symbolic** oracle: emit the
//! safety-fragment property as a BTOR2 `bad` monitor (H.O.1b) and let a real
//! BTOR2 model checker decide it. `btormc` works over SMT (no enumeration), so it
//! returns a *definite* verdict on the same wide-input designs the internal
//! oracle cannot.
//!
//! # Tool choice — `btormc --kind`
//!
//! The `mununu-sva` image's oss-cad-suite bundles `btormc`, `pono`, `bitwuzla`,
//! `boolector`. `pono`'s strongest engine (`ic3ia`) needs a MathSAT build that is
//! absent; `btormc` reads BTOR2 `bad` natively and proves unbounded safety via
//! k-induction (`--kind`). Probed contract (2026-06-29, in `mununu-sva`):
//!
//! | btormc stdout (first token) | meaning | [`McVerdict`] |
//! |---|---|---|
//! | `sat` | a reachable `bad` (with a witness trace) | [`McVerdict::Violated`] |
//! | `unsat` | k-induction CLOSED — `bad` unreachable (a real proof) | [`McVerdict::Safe`] |
//! | *(neither / empty)* | bounded: no CEX and no proof within `-kmax` | [`McVerdict::Unknown`] |
//!
//! **Soundness of the contract.** `unsat` is printed ONLY under `--kind` when
//! k-induction actually closes — plain BMC never prints it. The soundness-critical
//! case (a CEX at depth 5 run under `--kind -kmax 2`) printed NOTHING, not a false
//! `unsat`. So `unsat` ⇒ a genuine unbounded safety proof, and a too-shallow
//! `-kmax` degrades to [`McVerdict::Unknown`], never to a wrong [`McVerdict::Safe`].
//!
//! # `btormc` is optional at runtime
//!
//! Like the CVC5 wrapper ([`crate::adapter::cvc5`]) — discovery returns a
//! structured [`AdapterError`] when the binary is absent; callers (the H.O.1c
//! differential / e2e) treat that as "oracle unavailable", never a hard failure.
//! Per the subprocess-tools-not-bundled policy, `btormc` is contributor-installed
//! (oss-cad-suite / Homebrew `yosys` formula) — it is NOT in `Dockerfile.dev`; the
//! `mununu-sva` image carries it for the `#[ignore]`-gated e2e validation.
//!
//! # H.O.1a scope
//!
//! THIS increment ships the wrapper only: binary discovery + version probe
//! ([`locate_btormc`]), the verdict enum ([`McVerdict`]), output parsing
//! ([`parse_btormc_output`], make-ci unit-tested against canned stdout), and the
//! subprocess invocation ([`run_btormc`], docker-validated). The `bad`-monitor
//! EMISSION (property → augmented BTOR2) is H.O.1b; the `McOracle` differential +
//! real-RTL e2e is H.O.1c.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::adapter::{AdapterError, AdapterErrorKind};

/// Default k-induction / BMC bound for [`run_btormc`]. Picked generous enough to
/// reach a CEX or close k-induction on the small FSM fragments H.O targets, while
/// bounding wall-clock on the e2e. Callers can override per-invocation.
pub const DEFAULT_KMAX: u32 = 40;

/// A discovered `btormc` binary + its parsed version (diagnostic only — mununu
/// does not gate on a minimum version).
#[derive(Debug, Clone)]
pub struct BtormcBin {
    /// Resolved path: a bare `btormc` (found on `$PATH`) or the absolute path
    /// from `MUNUNU_BTORMC_PATH`.
    pub path: PathBuf,
    /// Version string from `btormc --version` (e.g. `3.2.4`), or
    /// `"<unparseable>"` when the output does not match the expected shape.
    pub version: String,
}

/// The external model checker's verdict on a BTOR2 `bad` property. See the module
/// docs for the `btormc --kind` output contract this maps from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McVerdict {
    /// k-induction proved the `bad` unreachable — the property is SAFE (`HOLDS`).
    Safe,
    /// A reachable `bad` (a sound counterexample) — the property is VIOLATED.
    Violated,
    /// Neither a CEX nor a proof within `-kmax` — bounded, inconclusive.
    Unknown,
}

/// H.O.1a — locate a usable `btormc` binary and probe its version.
///
/// Discovery order (mirrors [`crate::adapter::cvc5::locate_cvc5`] /
/// [`crate::adapter::verilator::locate_verilator`]):
/// 1. `MUNUNU_BTORMC_PATH` env var (explicit override).
/// 2. `btormc` on `$PATH` (oss-cad-suite, Homebrew `yosys`, …).
///
/// Returns `Err(AdapterError { kind: UnsupportedConstruct, .. })` when the binary
/// is absent or the version probe fails — callers fall back gracefully (the
/// oracle is simply unavailable).
pub fn locate_btormc() -> Result<BtormcBin, AdapterError> {
    let path = if let Ok(p) = std::env::var("MUNUNU_BTORMC_PATH") {
        PathBuf::from(p)
    } else {
        PathBuf::from("btormc")
    };

    let output = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/btormc: failed to invoke `{} --version`: {e}. \
                 Set MUNUNU_BTORMC_PATH or install btormc (oss-cad-suite, or the \
                 Homebrew `yosys` formula which bundles the btor2tools).",
                path.display()
            ),
            location: None,
        })?;

    if !output.status.success() {
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/btormc: `{} --version` exited with status {}",
                path.display(),
                output.status
            ),
            location: None,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = parse_btormc_version(&stdout).unwrap_or_else(|| "<unparseable>".to_string());

    Ok(BtormcBin { path, version })
}

/// Extract the version token from `btormc --version` output (observed: a single
/// line, e.g. `3.2.4`). Returns the first non-empty trimmed line, or `None` when
/// the output is empty — the caller treats that as `"<unparseable>"` (diagnostic
/// only; the binary may still run queries).
pub fn parse_btormc_version(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Map `btormc` stdout to an [`McVerdict`]. The first line that is exactly `sat`
/// or `unsat` decides; absence of both is [`McVerdict::Unknown`] (bounded run, no
/// CEX and no proof). See the module docs for why `unsat` is a sound `Safe`.
pub fn parse_btormc_output(stdout: &str) -> McVerdict {
    for line in stdout.lines() {
        match line.trim() {
            "sat" => return McVerdict::Violated,
            "unsat" => return McVerdict::Safe,
            _ => {}
        }
    }
    McVerdict::Unknown
}

/// H.O.1a — run `btormc --kind -kmax <kmax>` on a BTOR2 description (piped via
/// stdin) and parse the verdict.
///
/// `btor2` is the full BTOR2 text **including a `bad` line** (the safety monitor;
/// H.O.1b builds it). The `--kind` flag enables the k-induction proof that lets
/// the run conclude [`McVerdict::Safe`]; without it `btormc` only ever finds CEXs.
///
/// Returns `Err` only when the subprocess could not run or `btormc` reported a
/// hard error (e.g. a malformed BTOR2 — a bug in the emission, not an
/// inconclusive verdict) with no parseable verdict on stdout. A clean run with
/// neither `sat` nor `unsat` is [`McVerdict::Unknown`], not an error.
pub fn run_btormc(bin: &BtormcBin, btor2: &str, kmax: u32) -> Result<McVerdict, AdapterError> {
    let mut child = Command::new(&bin.path)
        .arg("--kind")
        .arg("-kmax")
        .arg(kmax.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/btormc: failed to spawn `{}`: {e}",
                bin.path.display()
            ),
            location: None,
        })?;

    // Write the BTOR2 to stdin, then drop the handle to signal EOF (the fragment
    // is small, so the pipe buffer never fills before `wait_with_output` drains
    // stdout — no deadlock).
    {
        let mut stdin = child.stdin.take().ok_or_else(|| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: "adapter/btormc: child stdin unavailable".to_string(),
            location: None,
        })?;
        stdin
            .write_all(btor2.as_bytes())
            .map_err(|e| AdapterError {
                kind: AdapterErrorKind::UnsupportedConstruct,
                message: format!("adapter/btormc: failed writing BTOR2 to stdin: {e}"),
                location: None,
            })?;
    }

    let output = child.wait_with_output().map_err(|e| AdapterError {
        kind: AdapterErrorKind::UnsupportedConstruct,
        message: format!(
            "adapter/btormc: failed waiting for `{}`: {e}",
            bin.path.display()
        ),
        location: None,
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict = parse_btormc_output(&stdout);

    // A clean Unknown (exit 0, empty stdout) is valid. But if btormc reported a
    // hard error (non-zero exit, e.g. a BTOR2 parse error) AND produced no
    // verdict, surface it — that signals a malformed monitor, not inconclusiveness.
    if verdict == McVerdict::Unknown && !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/btormc: `{}` exited with status {} and no verdict; stderr: {}",
                bin.path.display(),
                output.status,
                stderr.trim()
            ),
            location: None,
        });
    }

    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_output_sat_is_violated() {
        // btormc prints the bad index + a witness trace after `sat`.
        let out = "sat\nb0\n@0\n@1\n.\n";
        assert_eq!(parse_btormc_output(out), McVerdict::Violated);
    }

    #[test]
    fn parse_output_unsat_is_safe() {
        let out = "unsat\nb0\n";
        assert_eq!(parse_btormc_output(out), McVerdict::Safe);
    }

    #[test]
    fn parse_output_empty_is_unknown() {
        // The probed bounded case: a CEX deeper than -kmax + k-induction not yet
        // closed prints nothing → Unknown (NOT a false Safe).
        assert_eq!(parse_btormc_output(""), McVerdict::Unknown);
        assert_eq!(parse_btormc_output("\n  \n"), McVerdict::Unknown);
    }

    #[test]
    fn parse_output_first_verdict_wins() {
        // Defensive: a stray later token never overrides the first real verdict.
        assert_eq!(parse_btormc_output("sat\nunsat\n"), McVerdict::Violated);
        assert_eq!(parse_btormc_output("unsat\nsat\n"), McVerdict::Safe);
    }

    #[test]
    fn parse_version_single_line() {
        assert_eq!(parse_btormc_version("3.2.4\n").as_deref(), Some("3.2.4"));
        assert_eq!(parse_btormc_version("  3.2.4  ").as_deref(), Some("3.2.4"));
        assert_eq!(parse_btormc_version(""), None);
    }

    #[test]
    fn locate_absent_btormc_is_graceful_error() {
        // A bogus override path must yield a structured error, never a panic —
        // callers fall back to "oracle unavailable".
        // SAFETY: single-threaded test; restored before returning.
        unsafe { std::env::set_var("MUNUNU_BTORMC_PATH", "/nonexistent/btormc-xyz") };
        let r = locate_btormc();
        unsafe { std::env::remove_var("MUNUNU_BTORMC_PATH") };
        let err = r.expect_err("absent binary must error");
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
        assert!(err.message.contains("btormc"));
    }

    // ---- docker-validated (`mununu-sva`): exercise the real binary -----------
    // These need `btormc` on PATH (oss-cad-suite); run with `--ignored`.

    /// A reachable `bad`: `q` init 0, `next q = 1` → `bad = q` true at k=1.
    /// (`init sid state val` needs `state_nid > val_nid`, so the const is first.)
    const REACH_BTOR2: &str = "\
1 sort bitvec 1
2 zero 1
3 one 1
4 state 1 q
5 init 1 4 2
6 next 1 4 3
7 bad 4
";

    /// An inductively-safe `bad`: `q` init 0, `next q = 0` → `bad = q` never
    /// reachable; `--kind` proves it (the property invariant `q == 0` is
    /// 1-inductive).
    const SAFE_BTOR2: &str = "\
1 sort bitvec 1
2 zero 1
3 state 1 q
4 init 1 3 2
5 next 1 3 2
6 bad 3
";

    #[test]
    #[ignore = "requires btormc (MUNUNU_BTORMC_PATH or $PATH); run with --ignored in mununu-sva"]
    fn run_btormc_reach_is_violated() {
        let bin = locate_btormc().expect("btormc present");
        assert_eq!(
            run_btormc(&bin, REACH_BTOR2, DEFAULT_KMAX).unwrap(),
            McVerdict::Violated
        );
    }

    #[test]
    #[ignore = "requires btormc (MUNUNU_BTORMC_PATH or $PATH); run with --ignored in mununu-sva"]
    fn run_btormc_safe_is_safe() {
        let bin = locate_btormc().expect("btormc present");
        // `--kind` (set by run_btormc) is what lets this conclude Safe.
        assert_eq!(
            run_btormc(&bin, SAFE_BTOR2, DEFAULT_KMAX).unwrap(),
            McVerdict::Safe
        );
    }
}
