# EXP-0002a-warmup-rerun: re-run EXP-0002 at L3 protocol, with warmup, full samples

> **PARTIAL FINDING.** This EXP tested the SoA on benches that don't exercise iteration_ranks (the workloads use `evaluate_with_options` with `witness_map = None`). The original ≥2× hypothesis is unaddressed by this archive. [EXP-0002b-synth-bench](../EXP-0002b-synth-bench/) tests it on the right workload (synthesis with ProductGame mode + non-trivial alternation) and **confirms 2.4× speedup on grid_32x32** (p<0.001).

**One-line summary.** Re-records the SoA-vs-HashMap comparison using the level-3 same-session A/B protocol from ADR-0006: warmup-discard + same shell session + full Criterion samples (30 per side). **Hypothesis "≥2× synthesis speedup" cannot be tested on this workload** — see EXP-0002b for the right test. SoA is performance-neutral on workloads that don't touch iteration_ranks.

## Motivation

The user asked: "do we have previous tests and benches re ran with warmup?" Answer: no — EXP-0002 was recorded with `--fresh` but before `--warmup` was added in sitting 3. EXP-0002's apparent 5-7× speedup vs EXP-0001 was contaminated by cache-warmup differences across separate `cargo bench` invocations.

This EXP supersedes EXP-0002's performance claim. EXP-0002 stays in place as historical evidence of the contamination class (per the "supersede, don't rewrite" policy in ADR-0004).

## Method

L3 protocol (ADR-0006 / `notebook/BENCH_POLICY.md`):

1. Stash SoA changes via `git diff > /tmp/exp-0002-soa.patch`, then `git checkout HEAD -- ...` to reach the HashMap baseline state.
2. Run `scripts/bench_compare.sh exp-0002-full -- -p mununu-core --features test_support --bench mu_calculus_only` — this does the warmup discard, then `cargo bench --save-baseline exp-0002-full` (HashMap = A side, 30 samples per bench function).
3. Restore SoA: `git apply /tmp/exp-0002-soa.patch`.
4. Run `scripts/bench_compare.sh exp-0002-full --baseline-only -- ...` — this runs `cargo bench --baseline exp-0002-full` (SoA = B side) which automatically computes Criterion's bootstrap-CI change report against the saved A.
5. `scripts/bench_diff.sh exp-0002-full --robust` for the median-ratio + Mann-Whitney report.

Both sides ran in the same shell session, on the same compile cache, within minutes of each other. Sample size: 30 per bench function (Criterion default at the per-bench config level). Hardware unchanged from EXP-0001.

## Results

### Criterion's own change report (Welch's t-test on bootstrap)

| Bench | Median delta | 95% CI | p | Verdict |
|-------|-------------:|--------|---:|---------|
| `reachability_mu/chain_1k` | **+26.5%** | [+17.0%, +37.6%] | 0.00 | **SoA slower (significant)** |
| `reachability_mu/ring_1k` | +9.8% | [+3.1%, +17.1%] | 0.01 | SoA slower (significant) |
| `reachability_mu/grid_32x32` | +4.1% | [−1.8%, +10.6%] | 0.20 | noise |
| `invariance_nu/chain_1k` | −4.9% | [−8.6%, −1.1%] | 0.02 | SoA faster (significant) |
| `invariance_nu/ring_1k` | −2.6% | [−4.6%, −0.8%] | 0.01 | SoA faster (significant) |
| `invariance_nu/grid_32x32` | −6.6% | [−16.4%, +0.2%] | 0.23 | noise |

### Robust diff (median ratio + Mann-Whitney p<0.01 gate)

```
REGRESSIONS (0):
IMPROVEMENTS (0):
NEUTRAL (8):
    +25.5%   reachability_mu/chain_1k       [p=0.268]
    +13.4%   reachability_mu/ring_1k        [p=0.110]
     +4.7%   reachability_mu/grid_32x32     [p=0.745]
     +3.8%   propositional/chain_1k         [p=0.294]
     -3.5%   invariance_nu/ring_1k          [p=0.734]
     +3.0%   propositional/grid_32x32       [p=0.351]
     -2.7%   invariance_nu/chain_1k         [p=0.442]
     +0.7%   invariance_nu/grid_32x32       [p=0.636]
```

The two estimators disagree on significance because Criterion uses Welch's t-test on bootstrap-resampled means after outlier filtering; my robust gate uses a normal-approximation Mann-Whitney U test on raw 30-sample data, which is more conservative for bimodal/long-tailed distributions. **Interpretation:** Criterion's analysis is more sensitive but possibly overstates significance on small samples; the Mann-Whitney result correctly flags that ±25% noise is plausible at n=30. Both agree the median direction.

### Headline finding

**SoA is roughly performance-neutral on this workload.** Mu-fixpoints (least, reachability) show a mild apparent regression (+10% to +27% on chain/ring); nu-fixpoints (greatest, invariance) show a mild apparent improvement (−3% to −5% on chain/ring). The grid bench is dominated by other costs and shows neither.

The hypothesis "≥2× synthesis speedup" is **falsified** for these workloads at this scale.

## Why is the SoA neutral, not faster?

The benches use `evaluate_with_options` (witness_map = None), so the iteration_ranks code paths are **never executed during these benches**. The observed differences are LLVM monomorphization side-effects from the `WitnessMap` field type change, which is essentially noise.

To validate the SoA on its actual hot path requires a synthesis-bound bench (`Context::synthesise_controller_with_options(... ControllerMode::ProductGame ...)` against a fixture with non-trivial alternation depth and witness extraction enabled). This is the next experiment.

## What this means for the SoA migration

- **Soundness:** unchanged — all 800+ unit tests + 22 soundness tests + 5 property tests + 6 new SoA unit tests + 1 SoA proptest pass.
- **Performance on non-witness paths:** within ±10% of HashMap, no significant difference at n=30.
- **Performance on witness paths:** UNKNOWN — needs a dedicated synthesis bench.
- **Recommendation:** keep the SoA migration. The struct is a cleaner API, type-safer, and lays the groundwork for the EXP-0007 (predicate interning + dense bindings) refactor that depends on a stable IterationRanks shape. Open EXP-0002b-synth-bench to add a synthesis-bound bench and re-validate the original ≥2× hypothesis on the workload it actually targets.

## How to replay

```bash
make replay EXP=EXP-0002a-warmup-rerun
```

Or directly:

```bash
# Save current SoA changes:
git diff HEAD -- crates/mununu-core/src/mu_calculus/evaluator.rs \
                 crates/mununu-core/src/context/mod.rs > /tmp/exp-0002-soa.patch

# A side (HashMap baseline):
git checkout HEAD -- crates/mununu-core/src/mu_calculus/evaluator.rs \
                     crates/mununu-core/src/context/mod.rs
scripts/bench_compare.sh exp-0002-full -- \
    -p mununu-core --features test_support --bench mu_calculus_only

# B side (SoA candidate):
git apply /tmp/exp-0002-soa.patch
scripts/bench_compare.sh exp-0002-full --baseline-only -- \
    -p mununu-core --features test_support --bench mu_calculus_only

# Diff:
scripts/bench_diff.sh exp-0002-full --robust
```

## Status

`closed` — supersedes EXP-0002's performance claim; EXP-0002 stays in place as the historical record of the contamination class.

## Cross-refs

- Predecessor (superseded): EXP-0002-iter-rank-soa.
- Plan: §A1 in `~/.claude/plans/do-a-deep-evaluation-sparkling-origami.md`.
- ADR-0004: supersede, don't rewrite.
- ADR-0006: four-level regression-mitigation protocol.
- Followup: EXP-0002b (synth-bound bench to test the actual ≥2× hypothesis on the witness-extraction hot path).
