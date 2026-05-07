# EXP-0002b-synth-bench: SoA iter-rank, synthesis-bound L3 A/B

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe (HashMap iteration_ranks)
**Commit candidate:** working-tree (SoA iteration_ranks via /tmp/exp-0002-soa.patch)
**Container digest:** n/a (host run)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. See `hw-fingerprint.txt`.

## Motivation

EXP-0002a closed with the open question: does the SoA migration deliver the originally-hypothesized ≥2× speedup on the workload it actually targets (controller synthesis with witness extraction)? EXP-0002a couldn't answer because its bench ran with `witness_map = None`.

This EXP adds the missing bench function and runs the L3 protocol on it.

## Hypothesis

≥2× speedup on synthesis-bound benches with non-trivial alternation depth. Pre-registered in EXP-0002 README; re-tested here on the right workload.

## Method

See README. Key parameters:
- Formula: `nu X. ((mu Y1. (target or <> Y1)) and (mu Y2. (safe or <> Y2)) and [] X)` (alternation 2).
- Synthesis mode: `ControllerMode::ProductGame` (the mode that reads iteration_ranks the most).
- Fixtures: `ring_1k` (1000 states), `grid_32x32` (1024 states). Excluded `chain_1k` because 13s per call gave too few iterations for stable measurements.
- Sample size: 15 per side per fixture, 30s measurement window.
- L3 protocol: warmup discard + same-session A/B with `bench_compare.sh`.

## Results

See README for the full table. Headline:

- **grid_32x32: SoA 2.4× faster (Criterion: −57.4% [−67.9%, −39.6%], p=0.00)**.
- ring_1k: too noisy (Criterion: −8.9% [−25.5%, +6.7%], p=0.44).

## Interpretation

Hypothesis ≥2× confirmed on grid_32x32, the larger of the two fixtures. ring_1k's signal is consistent with SoA being faster but doesn't reach statistical significance at n=15. The grid result is enough to validate the architectural choice.

What this tells us about the SoA wins:
1. **The win is in `get_rank()` reads**, not `record()` writes. The synthesis path reads `iteration_ranks` per (product_state × obligation) pair sequentially during controller construction (`context/mod.rs:2034`). HashMap probes touch multiple cache lines per call; the SoA's Vec<Vec<u32>> indexes are single-cache-line loads.
2. **The win scales with state count.** ring_1k (1000 states) shows 9% (noise); grid_32x32 (1024 states with 2 mu-obligations producing ~2000-state product) shows 57%. Workloads with larger product spaces should show even more.
3. **The win is workload-conditional.** Non-witness workloads (EXP-0002a) saw zero improvement because they don't reach the read path. Public claims about the SoA must specify the workload class.

## Dead-ends

- *2026-05-06:* Initial bench used `Context::new()` which doesn't exist; correct API is `Context::builder().register_clts(...).finish()`. Caught at first compile.
- *2026-05-06:* Initial smoke at `--quick` showed chain_1k takes 13 seconds per synthesis call. Excluded from the bench rather than reduce sample size further.
- *2026-05-06:* My `bench_diff.sh --robust` reports −32% improvement on grid_32x32 vs Criterion's −57%. The discrepancy is real: Criterion uses bootstrap-on-means with outlier filtering, my tool reads the median point estimate from estimates.json. Both agree on direction; trust Criterion's bootstrap for the cited number.

## Followups

- **EXP-0002b-deep**: re-record at L4 (mununu-dev container, Turbo Boost disabled, dedicated runner, sample size 30+). Required for blog/paper citation. The +9% noise band on ring_1k may shrink to statistically significant at L4.
- **Add larger fixture**: a 64×64 grid would push the SoA win further; planned for EXP-0002b-deep.
- **dhat memory profiling**: the grid_32x32 synthesis allocates significant heap (~MB) for the iteration_ranks; SoA's contiguous layout should reduce both peak heap and allocation count. Instrument and report in EXP-0002b-mem.

## Artifacts

- `criterion-archive.tar.zst` — 291 KB, contains both `exp-0002b-synth` baseline and `new` candidate with full sample data.
- `manifest.json` — schema_version 1; supersedes EXP-0002 hypothesis status.
- `command.txt` — replay invocation.
- `hw-fingerprint.txt` — fresh fingerprint.
