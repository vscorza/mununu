# EXP-0012-changed-flag-fixpoint: in-place merge with changed-flag — HYPOTHESIS FALSIFIED

> **⚠ HYPOTHESIS FALSIFIED.** Plan §B4 predicted ≥1.2× speedup on long-iteration fixpoints by replacing `next_set == current_set` + `clone(next_set)` with an in-place `or_assign_track` / `and_assign_track` that returns whether any bit changed. Empirically: 7 of 10 benches regress at p<0.001, with `synthesis_product_game/ring_1k` **+93.8%** slower. Reverted.

**One-line summary.** Added two helpers (`or_assign_track`, `and_assign_track`) that merge BitVecs word-by-word and return whether any bit changed; refactored `eval_fixpoint` to use them in place of clone+compare. All 830+ unit tests + soundness suite + property tests pass; performance regresses sharply.

## Motivation

Plan §B4 reasoning: the fixpoint loop does (a) `next_set == current_set` (full BitVec compare, O(|S|/w)) and (b) `current_set = clone_bitvec(&next_set)` (full BitVec clone) per iteration. Replacing both with a single in-place merge that tracks change should save the clone and the redundant compare.

For monotone fixpoints, `current_set ⊆ next_set` (mu) or `next_set ⊆ current_set` (nu) always holds, so `current |= next` (mu) and `current &= next` (nu) are semantically equivalent to `current := next`.

## Hypothesis (pre-registered, plan §B4)

≥1.2× on long-iteration formulas; allocator pressure drops by one BitVec per iteration.

## Method

L3 protocol per ADR-0006:

1. Add `or_assign_track`, `and_assign_track` to `EvalContext` impl in `mu_calculus/evaluator.rs` (~25 lines each, word-loop with per-word branch).
2. Refactor `eval_fixpoint` (line 1726) to call the appropriate tracker by `FixpointKind`. Eliminate the `next_set == current_set` check and the `current_set = clone_bitvec(&next_set)?` clone.
3. L3 A/B on the full `mu_calculus_only` bench suite (10 benches across propositional, reachability_mu, invariance_nu, synthesis_product_game). 30-40 samples per side, full Criterion config.

## Results

### Criterion bootstrap, 30+ samples per side, p<0.001 unless noted

| Bench | A (clone+compare) | B (track-during-merge) | Δ | 95% CI | p |
|-------|------------------:|-----------------------:|--:|--------|---:|
| `synthesis_product_game/ring_1k` | 14.0 ms | 27.5 ms | **+93.8%** | [+89.8%, +97.3%] | 0.000 |
| `reachability_mu/chain_1k` | 9.39 ms | 9.96 ms | +37.9% | [+9.0%, +88.3%] | 0.003 |
| `propositional/chain_1k` | 12.3 µs | 15.4 µs | **+25.4%** | [+23.3%, +27.9%] | 0.000 |
| `invariance_nu/ring_1k` | 410 µs | 452 µs | +18.8% | [+9.8%, +26.6%] | 0.001 |
| `invariance_nu/grid_32x32` | 495 µs | 583 µs | +17.9% | [+14.8%, +20.8%] | 0.000 |
| `invariance_nu/chain_1k` | 409 µs | 444 µs | +15.6% | [+10.6%, +22.2%] | 0.000 |
| `propositional/grid_32x32` | 11.9 µs | 13.1 µs | +10.3% | [+7.9%, +13.0%] | 0.000 |
| `synthesis_product_game/grid_32x32` | 1.23 s | 1.11 s | -8.7% | [-13.4%, -3.6%] | 0.351 (noise) |
| `reachability_mu/grid_32x32` | 15.1 ms | 15.4 ms | +3.4% | [+1.7%, +5.2%] | 0.000 |
| `reachability_mu/ring_1k` | 9.17 ms | 9.35 ms | +1.4% | [-0.2%, +2.8%] | 0.003 |

7/10 benches regress significantly. The lone "improvement" (`synthesis_product_game/grid_32x32` −8.7%) fails Mann-Whitney significance at p=0.351.

## Why the optimization fails

Two converging reasons:

1. **`BitVec::==` is already early-exit.** `bitvec` implements equality as a word-level scan that bails on the first differing word. Most fixpoint iterations *don't* converge (they're computing the next iterate); the equality check returns false after the first mismatching word, in O(1) average for non-converged cases.

2. **The clone is small and the alternative is serial.** For state counts ≤ 1024 (our largest bench), a BitVec is 16 u64 words = 128 bytes ≈ 2 cache lines. `clone_bitvec` is a Vec allocation + memcpy; the allocator handles small allocations from a thread-local cache, and memcpy is hardware-vectorized. The new word loop with per-word branch (`if new != dst[i] { changed = true; ... }`) is a serial dependency chain — each iteration's `changed` depends on previous. For tiny BitVecs, the branchful loop is slower than the branchless memcpy + early-exit compare.

For very long-iteration fixpoints on much larger state spaces (10k+), the change might amortize. But at the workload sizes in our benches, the equality+clone pattern is faster than the change-tracking pattern. **The "obvious bottleneck" of compare+clone wasn't actually the bottleneck.**

## Why `synthesis_product_game/ring_1k` regresses worst (+93.8%)

The ring_1k bench has 1000 plant states × 2 mu obligations producing many fixpoint iterations per controller synthesis call. Each iteration now does a full word-loop (16 u64 words × per-word branch) where it previously did a fast equality scan. With ~thousands of iterations per synthesis call, the per-iteration overhead compounds.

`grid_32x32` synthesis has fewer but longer iterations (the larger product space converges slower); the change tracker amortizes better there, hence the trend toward improvement (-8.7%). But still not statistically significant at n=15.

## Decision

**Revert the EXP-0012 changes.** Plan §B4 is marked falsified-at-this-scale.

A future EXP-0012-redesigned might:

1. **Special-case the BitVec word size.** For BitVecs with ≤ 4 u64 words (≤ 256 states), use clone+compare; for larger, use track-during-merge. Hybrid.
2. **Pre-allocate `next_set` outside the loop.** Avoid the per-iteration BitVec allocation by reusing a buffer. This is more work than the simple drop-in but matches the structural-change pattern that EXP-0002b and EXP-0004 confirmed.

Neither is a "drop-in." The simple substitution doesn't deliver.

## Soundness

All tests pass — 830 lib tests, 33 test groups, 22 soundness tests, 5 proptests including `iteration_ranks_deterministic`. The change is semantically equivalent under monotone fixpoint iteration; only performance differs.

## Cross-refs

- **EXP-0010** (FxHashMap drop-in for composition): falsified, +30-60% regression.
- **EXP-0003** (LabelSetTable drop-in): falsified, +18-85% on composition, +3-5% on synth.
- **EXP-0012** (this archive): falsified, +10-94% on mu_calculus_only.
- All three share the failure mode: the targeted "bottleneck" (hash speed, key shape, clone+compare) wasn't actually the dominant cost on workloads at this scale. RefCell+Rc churn, BitVec memcpy, hashbrown's word-level early-exit all already optimize the alleged hot paths.

The win pattern (now 4 EXPs across two confirmations + one heap falsification + four wall-clock falsifications): structural changes that change the *access pattern* (EXP-0002b SoA, EXP-0004 Vec doubling) win; drop-in substitutions that change *what runs on the same access pattern* lose.

## How to replay

```bash
make replay EXP=EXP-0012-changed-flag-fixpoint
```

## Status

`closed` — hypothesis falsified, change reverted. Archive stays in place as historical evidence.

## Cross-refs

- Plan §B4: falsified-at-this-scale. ADR-0013 records the decision.
- Companion: EXP-0010 + EXP-0003 (the two prior drop-in falsifications). Together: 3 drop-in fails, 2 structural wins.
- Followup: a future EXP-0012-hybrid could special-case by BitVec word size; not scheduled.
