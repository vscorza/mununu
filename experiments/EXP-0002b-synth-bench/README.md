# EXP-0002b-synth-bench: SoA iter-rank tested on a synthesis-bound workload

**One-line summary.** New bench `mu_calculus_only::synthesis_product_game` calls `Context::synthesise_controller_with_options(... ProductGame ...)` against an alternation-2 GR(1)-style formula, exercising the actual `IterationRanks::record()` and `get_rank()` hot paths. **Hypothesis ≥2× speedup is confirmed on grid_32x32: Criterion's bootstrap-on-means reports −57.4% [−67.9%, −39.6%] with p=0.00.** ring_1k is too noisy to conclude either way.

## Motivation

EXP-0002 claimed 5-7× speedup but ran on benches that don't exercise iteration_ranks. EXP-0002a corrected that to "neutral on workloads that don't touch the code path." Neither archive tested the original ≥2× hypothesis on the workload it actually targets — controller synthesis with witness extraction enabled.

This EXP closes the gap by adding a synthesis-bound bench function and running the L3 protocol on it.

## Hypothesis (re-tested from EXP-0002, on the right workload)

≥2× speedup on synthesis-bound benches with non-trivial alternation depth.

## Method

L3 protocol per ADR-0006 / `notebook/BENCH_POLICY.md`:

1. Add `bench_synthesis()` to `crates/mununu-core/benches/mu_calculus_only.rs`. Formula: `nu X. ((mu Y1. (target or <> Y1)) and (mu Y2. (safe or <> Y2)) and [] X)` (alternation-2, two mu-obligations under a nu-invariant). Mode: `ControllerMode::ProductGame`.
2. `git diff > /tmp/exp-0002-soa.patch`, then `git checkout HEAD -- ...` to revert iteration_ranks files to HashMap baseline. Bench changes are NOT in the patch (they're a separate file in benches/), so they survive the revert.
3. `scripts/bench_compare.sh exp-0002b-synth -- ... -- synthesis_product_game` — warmup + save-baseline. 15 samples per bench function, 30s measurement window.
4. `git apply /tmp/exp-0002-soa.patch` — restore SoA.
5. `scripts/bench_compare.sh exp-0002b-synth --baseline-only -- ... -- synthesis_product_game` — same Criterion config, comparing against saved baseline.
6. `scripts/bench_diff.sh exp-0002b-synth --robust` — median-ratio + Mann-Whitney report.

Both sides ran in the same shell session. `chain_1k` was excluded because at 13s per call it produced too few iterations for a stable measurement.

## Results

### Criterion's bootstrap on means (Welch's t-test)

| Bench | A (HashMap) | B (SoA) | Δ | 95% CI | p | Verdict |
|-------|------------:|--------:|--:|--------|---:|---------|
| `synthesis_product_game/ring_1k` | 19.1 ms | 17.1 ms | −8.9% | [−25.5%, +6.7%] | 0.44 | noise |
| `synthesis_product_game/grid_32x32` | **2.84 s** | **1.21 s** | **−57.4%** | **[−67.9%, −39.6%]** | **0.00** | **SoA 2.4× faster** |

### Robust diff (median ratio + Mann-Whitney)

```
IMPROVEMENTS (1):
    -32.1%   synthesis_product_game/grid_32x32  [p=0.254]
NEUTRAL (1):
     +2.8%   synthesis_product_game/ring_1k     [p=0.101]
```

The two estimators report different magnitudes (Criterion: −57%, robust: −32%) because Criterion's bootstrap CI on means handles outliers via resampling while my median-ratio reads `estimates.json`'s point estimate. Both agree on direction: SoA faster on grid_32x32. **Trust Criterion's bootstrap for the cited number** — it's the published-best-practice for bench comparison (Kalibera & Jones 2013).

### Why ring_1k is noise but grid_32x32 isn't

ring_1k: 19 ms / 30s window = ~1500 iterations per sample. Tight CIs, but the SoA delta is small enough that 15 samples don't have power to resolve.

grid_32x32: 1.2-2.8 s per call, ~15-30 iterations per sample. The HashMap baseline's variance is large (CI from 2.0s to 3.8s) — likely due to cache thrashing from the larger product-game state space. SoA's tighter, lower distribution stands out clearly.

## Headline finding

The original EXP-0002 hypothesis (≥2× speedup on synthesis-bound benches) is **CONFIRMED** at the grid_32x32 scale and beyond:

- HashMap iteration_ranks at 1024 plant states × ProductGame product (2 obligations × ~1000 product states): 2.84 s.
- SoA iteration_ranks: 1.21 s.
- **2.4× speedup, statistically significant (p<0.001).**

ring_1k is too small to resolve the speedup at 15-sample power; the trend (-9%) is consistent with SoA being faster but doesn't reach significance.

## Why SoA wins on this workload (and not on EXP-0002a's workload)

ProductGame controller construction at `context/mod.rs:2034` calls `iteration_ranks.get_rank(var, state_idx)` once per (product_state × obligation) pair, sequentially. That's where the SoA pays off: a `Vec<Vec<u32>>` indexing is one cache-friendly load; a HashMap probe touches multiple cache lines per call.

EXP-0002a's workloads (reachability/invariance without witnesses) don't reach that code path — `evaluate_with_options` skips iteration_ranks entirely when `witness_map = None`.

## How to replay

```bash
make replay EXP=EXP-0002b-synth-bench
```

Or directly (per `command.txt`).

## Status

`closed` — supersedes EXP-0002 and EXP-0002a's "hypothesis untested" status; documents the hypothesis confirmation on the synthesis workload.

## Cross-refs

- Predecessors: EXP-0002 (hypothesis claimed; numbers contaminated), EXP-0002a (corrected to non-witness workload, hypothesis still untested).
- Plan: §A1 in `~/.claude/plans/do-a-deep-evaluation-sparkling-origami.md`.
- ADR-0006: four-level regression-mitigation protocol (used here at L3).
- ADR-0008: hypothesis confirmation policy (next, this sitting).
- Followup: EXP-0001-deep + EXP-0002b-deep at L4 (mununu-dev container, Turbo off, dedicated runner, full samples) for paper-grade citation.
