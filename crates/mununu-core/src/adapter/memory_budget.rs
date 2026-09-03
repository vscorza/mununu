//! mununu#490 — self-imposed process-memory ceiling.
//!
//! **Motivation.** The default Rust allocator calls `abort()` (process exit 134)
//! on a failed allocation. That is invisible to the model checker: the process
//! dies BEFORE any BDD-level or SMT-level budget can fire, taking every property
//! in the same invocation down with it. A verify-lane consumer (monono's `sv
//! verify-auto` gate on 25 checks) then sees a crash instead of an `unknown`
//! verdict, so a run that would have decided N-1 properties reports none.
//!
//! **What this module provides.** A caller-configurable ceiling (`MUNUNU_MAX_PROCESS_MEMORY_BYTES`)
//! that mununu polls itself at coarse chokepoints — between properties on the
//! SV verify-auto per-property loop, at each `escalate_bottom` step, and at
//! entry to the heavier engine surfaces. When the process RSS exceeds the
//! ceiling, the current + remaining properties abstain (`Unknown`) with a
//! `memory-budget-exceeded` reason, and prior verdicts are preserved. This
//! trades a crash for a graceful degradation.
//!
//! **What this module does NOT provide.** It CANNOT catch an allocation that
//! fails BETWEEN checkpoints — a single BDD blowup can still crash the
//! process. The ceiling is a coarse-granularity graceful-degradation lever,
//! not an absolute crash guarantee. Recommended setting: 70-80% of the
//! process's actual memory limit (`ulimit -m`, container `--memory`), leaving
//! headroom for allocator overhead + non-mununu memory.
//!
//! **Mirrors the existing budget vocabulary.** `MUNUNU_BDD_MAX_BITS`,
//! `MUNUNU_BDD_ARENA_NODES`, `MUNUNU_BDD_ITER_BUDGET`, `MUNUNU_BDD_TIME_BUDGET_MS`
//! (see CLAUDE.md's Environment Variables table) all follow the same pattern:
//! default unset = disabled, explicit value = active ceiling, over-budget =
//! abstain (never over-approximate). A caller sizes the ceiling based on the
//! platform's real memory limit.

/// The env var that configures the process-memory ceiling in bytes. Default
/// unset ⇒ disabled (no ceiling); explicit non-numeric or zero ⇒ disabled with
/// a debug log; explicit positive value ⇒ active ceiling.
pub const MEMORY_BUDGET_ENV: &str = "MUNUNU_MAX_PROCESS_MEMORY_BYTES";

/// A memory-budget check failed: the current process RSS strictly exceeds the
/// caller-configured ceiling. Carries both numbers so the caller can render an
/// informative abstention note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudgetExceeded {
    /// The process's resident set size at the check point, in bytes.
    pub current_rss_bytes: u64,
    /// The caller-configured ceiling, in bytes.
    pub limit_bytes: u64,
}

impl std::fmt::Display for MemoryBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "memory budget exceeded: {} B in use, ceiling {} B ({}={})",
            self.current_rss_bytes, self.limit_bytes, MEMORY_BUDGET_ENV, self.limit_bytes
        )
    }
}

/// Pure check: does `current` strictly exceed `limit`? Testable without env
/// mutation or a real RSS read.
///
/// Boundary is STRICT (`current > limit` ⇒ Err; `current == limit` ⇒ Ok). A
/// caller that wants the equal case to abstain can pass `limit - 1` — this
/// matches the "still under the ceiling at the boundary" reading and avoids
/// spurious abstention on a ceiling that happens to be hit exactly during
/// idle steady-state.
pub fn check_memory_budget_bytes(current: u64, limit: u64) -> Result<(), MemoryBudgetExceeded> {
    if current > limit {
        Err(MemoryBudgetExceeded {
            current_rss_bytes: current,
            limit_bytes: limit,
        })
    } else {
        Ok(())
    }
}

/// Parse the env var into an optional ceiling. `None` when unset, non-numeric,
/// or zero (treated as "disabled"); `Some(bytes)` when a positive u64.
pub fn read_memory_budget_env() -> Option<u64> {
    let raw = std::env::var(MEMORY_BUDGET_ENV).ok()?;
    let n = raw.trim().parse::<u64>().ok()?;
    if n == 0 { None } else { Some(n) }
}

/// Read the current process resident set size in bytes. `None` when the
/// platform's RSS reader is unavailable (fall back to "no check" rather than
/// pretending we're at 0).
pub fn read_process_rss_bytes() -> Option<u64> {
    memory_stats::memory_stats().map(|s| s.physical_mem as u64)
}

/// The full check: reads the env var + the current RSS and dispatches to
/// [`check_memory_budget_bytes`]. Returns `Ok(())` when the env var is unset
/// or the RSS reader is unavailable (fail-open — this is a caller-opt-in
/// ceiling, not a mandatory gate).
pub fn check_process_memory_budget() -> Result<(), MemoryBudgetExceeded> {
    let Some(limit) = read_memory_budget_env() else {
        return Ok(());
    };
    let Some(current) = read_process_rss_bytes() else {
        return Ok(());
    };
    check_memory_budget_bytes(current, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strictly_under_ceiling_is_ok() {
        assert!(check_memory_budget_bytes(99, 100).is_ok());
    }

    #[test]
    fn at_ceiling_is_ok_boundary_case() {
        // Documented behaviour: the boundary is STRICT `>`, so being exactly
        // at the ceiling does not trigger an abstention.
        assert!(check_memory_budget_bytes(100, 100).is_ok());
    }

    #[test]
    fn strictly_over_ceiling_is_err_with_expected_fields() {
        let err = check_memory_budget_bytes(101, 100).expect_err("should abstain");
        assert_eq!(err.current_rss_bytes, 101);
        assert_eq!(err.limit_bytes, 100);
    }

    #[test]
    fn err_display_names_env_var_and_both_numbers() {
        let err = MemoryBudgetExceeded {
            current_rss_bytes: 1_500_000_000,
            limit_bytes: 1_000_000_000,
        };
        let s = format!("{err}");
        assert!(
            s.contains("1500000000") && s.contains("1000000000"),
            "display should name both numbers: {s}"
        );
        assert!(
            s.contains(MEMORY_BUDGET_ENV),
            "display should name the env var for consumer diagnostics: {s}"
        );
    }

    /// Env-var parsing cases exercised as ONE sequential test — env is
    /// process-global and cargo runs tests in parallel by default, so a
    /// per-case test would race with a sibling test's set/remove and flake.
    /// The four cases must appear in a single test body so they serialize
    /// on the test's execution.
    #[test]
    fn env_var_parsing_sequences_the_four_cases() {
        // SAFETY: this is the only test in the module that mutates env; all
        // reads are self-contained after each set. `MUNUNU_MAX_PROCESS_MEMORY_BYTES`
        // is not otherwise consumed by test code in mununu-core.
        unsafe {
            std::env::remove_var(MEMORY_BUDGET_ENV);
        }
        assert_eq!(read_memory_budget_env(), None, "unset ⇒ None (fail-open)");

        unsafe {
            std::env::set_var(MEMORY_BUDGET_ENV, "0");
        }
        assert_eq!(
            read_memory_budget_env(),
            None,
            "explicit `=0` ⇒ None (disabled, matches stale shell state)"
        );

        unsafe {
            std::env::set_var(MEMORY_BUDGET_ENV, "not_a_number");
        }
        assert_eq!(
            read_memory_budget_env(),
            None,
            "malformed ⇒ None (fail-open, never crash)"
        );

        unsafe {
            std::env::set_var(MEMORY_BUDGET_ENV, "1073741824"); // 1 GiB
        }
        assert_eq!(
            read_memory_budget_env(),
            Some(1_073_741_824),
            "positive numeric ⇒ Some(bytes)"
        );

        // Restore to unset so any subsequent test (in-module or cross-module
        // running serially after this one) sees the default state.
        unsafe {
            std::env::remove_var(MEMORY_BUDGET_ENV);
        }
    }

    #[test]
    fn rss_reader_returns_a_positive_value_when_platform_supports_it() {
        // Sanity check on the platform layer: on the test hosts CI runs on
        // (linux + macOS), memory_stats returns Some(_) with a positive
        // physical_mem. If a future platform lacks support, this test will
        // fail loudly rather than silently pretending the ceiling works.
        let rss = read_process_rss_bytes().expect("memory_stats supported on this platform");
        assert!(rss > 0, "process RSS should be positive; got {rss}");
    }
}
