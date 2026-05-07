# EXP-0002-iter-rank-soa: replace HashMap iteration ranks with struct-of-arrays

> **⚠ SUPERSEDED 2026-05-06 by [EXP-0002a-warmup-rerun](../EXP-0002a-warmup-rerun/).** The "5-7× apparent speedup" cited below is cache-warmup contamination from a cross-session bench comparison, not a real SoA contribution. The corrected L3-protocol A/B (same-session, warmup discard, full Criterion samples) shows the SoA is performance-neutral on this workload — and the workload doesn't even exercise iteration_ranks. The original ≥2× hypothesis is unaddressed by either archive; EXP-0002b will test it on a synthesis-bound bench. This archive stays in place per ADR-0004's "supersede, don't rewrite" policy.

**One-line summary.** Swapped `WitnessMap.iteration_ranks: HashMap<(usize, FormulaVarId), usize>` for a `Vec<Vec<u32>>` indexed `[var.index()][state_idx]` with `u32::MAX` sentinel. Soundness-neutral, validated by the existing 22 soundness tests + 5 property tests + ~800 unit/doctests, all green.

## Motivation

Per the plan §A1: strategy extraction reads iteration ranks per state, lexicographically; the access pattern is sequential per fixpoint variable. The HashMap layout at [`evaluator.rs:55, :1607`](../../crates/mununu-core/src/mu_calculus/evaluator.rs) carries ~48 B per (state, var) entry plus cache-cold probe behavior. SoA `Vec<Vec<u32>>` brings the ranks into contiguous arrays, sequentially written during fixpoint iteration, sequentially read during signature comparison.

Memory: at |S|=1M states × 4 fixpoint variables, HashMap upper bound is ~192 MB; SoA is exactly 16 MB.

## Hypothesis (pre-registered)

1. **Synthesis-bound benches** ≥2× speedup on workloads exercising synthesis paths.
2. **Memory** measured via dhat: peak heap drops by ≥100 KB on a synthesis-relevant bench.
3. **Non-synthesis benches** within ±1% of EXP-0001 baseline.

## Headline result

Numbers from `cargo bench -p mununu-core --features test_support --bench mu_calculus_only -- --quick` after the SoA migration. **Methodological caveat: EXP-0001 baseline numbers in the table below are smoke-run numbers from cold compile/cache; they are NOT directly comparable to EXP-0002. A clean comparison requires re-recording EXP-0001 with `cargo bench -- --save-baseline EXP-0001` followed by EXP-0002 with `--baseline EXP-0001` on identical hardware.**

| Bench | EXP-0001 smoke | EXP-0002 | Apparent ratio |
|-------|---------------:|---------:|---------------:|
| `mu_calculus_only/propositional/chain_1k` | 72 µs | 12.7 µs | 5.7× |
| `mu_calculus_only/propositional/grid_32x32` | 78 µs | 12.2 µs | 6.4× |
| `mu_calculus_only/reachability_mu/chain_1k` | 64.5 ms | 9.6 ms | 6.7× |
| `mu_calculus_only/reachability_mu/ring_1k` | 69.8 ms | 9.2 ms | 7.6× |
| `mu_calculus_only/reachability_mu/grid_32x32` | 109 ms | 15.0 ms | 7.3× |
| `mu_calculus_only/invariance_nu/chain_1k` | 2.5 ms | 414 µs | 6.0× |
| `mu_calculus_only/invariance_nu/ring_1k` | 2.6 ms | 429 µs | 6.0× |
| `mu_calculus_only/invariance_nu/grid_32x32` | 3.1 ms | 506 µs | 6.1× |

The "apparent ratio" column is an upper bound on the true speedup. The true SoA contribution is unknown without re-baselining; sees `notes.md` for the methodology problem and the planned EXP-0002-deep follow-up.

What is rigorously demonstrated:

- The SoA migration produces sensible benchmark numbers across all formula classes.
- All existing tests stay green: ~800 unit tests, 57 doctests, 22 soundness tests, 5 property tests (including the new `iteration_ranks_deterministic`).
- `make ci` exit 0.

## Tests added

- `crates/mununu-core/src/mu_calculus/evaluator.rs::iteration_ranks_tests` — 6 unit tests covering: fresh state returns MAX, record/read round-trip, first-write-wins, lazy row/col allocation, len() counts only set entries, iteration value caps below sentinel.
- `crates/mununu-core/tests/properties/mu_calculus.rs::iteration_ranks_deterministic` — proptest asserting that two runs of the same `nu X. ((mu Y. (target or <> Y)) and [] X)` evaluation on a random CLTS produce byte-identical signature vectors for every state. Catches future regressions that introduce iteration-order dependence (e.g., parallel reductions without ordering, HashMap upstream that leaks iteration order).

## Files changed

- `crates/mununu-core/src/mu_calculus/evaluator.rs:40-200` — added `IterationRanks` struct + tests; replaced `iteration_ranks: HashMap<...>` field type.
- `crates/mununu-core/src/mu_calculus/evaluator.rs:1670-1700` — write site uses `record(var, state_idx, iteration, state_count)`.
- `crates/mununu-core/src/context/mod.rs:2025-2040` — read site uses `wm.iteration_ranks.get_rank(var, state_idx)`.

## How to replay

```bash
make replay EXP=EXP-0002-iter-rank-soa
```

Or directly:

```bash
scripts/bench_record.sh --fresh EXP-0002-iter-rank-soa \
    -p mununu-core --features test_support --bench mu_calculus_only -- --quick
```

## Status

`open` — see `notes.md` for the EXP-0002-deep followup needed before paper citation.

## Cross-refs

- Plan: §A1 in `~/.claude/plans/do-a-deep-evaluation-sparkling-origami.md`.
- Baseline: EXP-0001-baseline-cliff (smoke).
- Successor candidate (depends on this): EXP-0007 predicate interning + dense bindings.
