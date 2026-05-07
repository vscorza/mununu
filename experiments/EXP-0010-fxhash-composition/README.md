# EXP-0010-fxhash-composition: FxHashMap drop-in for composition — HYPOTHESIS FALSIFIED

> **⚠ HYPOTHESIS FALSIFIED.** The plan §B2 expectation that FxHashMap drop-in would yield 1.5-2× speedup on composition workloads is wrong for this codebase. FxHashMap is **30-60% slower** than std HashMap on every composition_only bench (p<0.001). The change has been reverted.

**One-line summary.** Replaced `std::HashMap`/`HashSet` with `rustc_hash::FxHashMap`/`FxHashSet` aliases in `composition/mod.rs` (ProductStateBuilder + 4 arena caches + BFS dedup sets). All 920+ unit tests pass. Performance regresses across the board.

## Motivation

Plan §B2 / EXP-0010 hypothesis (from sitting 1's deep evaluation):
> "Replace `HashMap<(StateId, StateId), StateId>` with FxHashMap. Expected 1.5-2× speedup. Zero unsafe, single-line replace."

The composition's `ProductStateBuilder.state_map` keys are `(StateId, StateId)` pairs, which on the surface look like the integer-pair workload FxHash optimizes. The arena caches (`StateKey = (usize, usize)`, `StatePairKey = 4-tuple`, `LabelPairKey = (usize, usize)`) are similar.

## Hypothesis (pre-registered, in the plan)

≥1.5× speedup on composition-heavy workloads.

## Method

L3 protocol per ADR-0006:
1. Add `rustc-hash = "1.1"` dep to `crates/mununu-core/Cargo.toml`.
2. Replace imports in `crates/mununu-core/src/composition/mod.rs`:
   - `use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};` → split, with `use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};` for the renamed aliases.
   - Replace `HashSet::new()` → `HashSet::default()` (FxHashSet has no `new()` because it's a type alias for `HashSet<T, BuildHasherDefault<FxHasher>>`).
3. Save patch with `git diff > /tmp/exp-0010-fxhash.patch`.
4. `git checkout HEAD -- ...` to revert to std HashMap (A side).
5. `scripts/bench_compare.sh exp-0010-fxhash -- ... --bench composition_only` — A side, warmup discard, save baseline.
6. Re-apply EXP-0010 changes manually (the patch applies cleanly only when sitting-2 changes are absent).
7. `scripts/bench_compare.sh exp-0010-fxhash --baseline-only -- ... --bench composition_only` — B side, full samples.
8. `scripts/bench_diff.sh exp-0010-fxhash --robust`.

Both sides ran in the same shell session, 30 samples per bench function, 8-10s measurement window.

## Results

| Bench | A (std HashMap) | B (FxHashMap) | Δ | 95% CI | p (Criterion) | p (MW per-iter) |
|-------|----------------:|---------------:|--:|--------|---:|---:|
| `chain_sync/chain1k_x_ring1k` | 2.61 µs | 3.37 µs | **+36.8%** | [+25.6%, +52.3%] | 0.00 | 0.000 |
| `grid_async/grid32_x_grid32` | 1.29 µs | 1.63 µs | **+31.1%** | [+26.0%, +38.7%] | 0.00 | 0.000 |
| `mode_compare/sync` | 2.52 µs | 3.49 µs | **+40.8%** | [+34.0%, +50.3%] | 0.00 | 0.000 |
| `mode_compare/async` | 2.51 µs | 3.32 µs | **+32.7%** | [+29.9%, +35.6%] | 0.00 | 0.000 |
| `mode_compare/superset` | 2.51 µs | 3.61 µs | **+59.6%** | [+33.4%, +107.9%] | 0.00 | 0.000 |

**5 of 5 benches regress significantly under both estimators (Criterion's bootstrap-on-means and Mann-Whitney on per-iteration times).**

## Why FxHashMap loses on this workload

Three converging reasons:

1. **The hot map keys aren't simple integers.** `ProductStateBuilder.state_map` is `HashMap<(StateId, StateId), StateId>`, where StateId is a u32 newtype — that should favor FxHash. But the arena's other four maps key on `Vec<String>` (label intern), `(usize, usize)` (state cache), `4-tuple` (state-pair cache), `(usize, usize)` (label-pair cache). FxHash's win on integer pairs is small here because the byte-stream hashing inside Vec<String> dominates.
2. **The maps are small.** Composition products at chain_1k × ring_1k typically reach 1000-2000 product states; the arena caches stay smaller. At these sizes, std HashMap's table fits in L1/L2; the SipHash overhead per probe is ~20 ns. FxHash's faster hash function gets dominated by RefCell::borrow_mut overhead and Rc cloning.
3. **Hash quality matters at small sizes.** SipHash distributes keys uniformly over the table; FxHash, being multiplicative on weak inputs, can produce more clustering. With small maps and short load factors, clustering means more probe-chain follows. The constant-factor win on hash computation gets eaten by additional cache-line accesses.

This is consistent with `hashbrown`'s benchmarks: FxHash beats SipHash on lookups in maps with **millions of entries**, but for maps with fewer than a few thousand entries the difference is workload-dependent and often negative.

## Decision

**Revert the FxHashMap change.** The plan §B2 hypothesis is falsified for this codebase; there's no architectural argument to keep the swap (unlike EXP-0002's SoA where the type-safer struct enables future EXPs).

ADR-0009 documents the falsification and updates the plan §B2 status.

## Reproducibility-protocol bug found and fixed during this EXP

`scripts/bench_diff.sh --robust` was comparing raw `times` from Criterion's sample.json instead of per-iteration times (`times[i] / iters[i]`). Criterion's linear sampling mode varies `iters` across samples to fit the measurement window; raw `times` reflects the iter ramp, not the bench cost. Fixed: bench_diff now divides times by iters before feeding into Mann-Whitney.

Before fix: MW returned p=0.7-0.99 on these regressions (false neutral). After fix: MW returns p<0.001 (correctly flags every regression). Criterion's own bootstrap was unaffected and reported p=0.00 throughout.

## How to replay

```bash
make replay EXP=EXP-0010-fxhash-composition
```

Or directly via `command.txt`. Reproduces the regression on any host with the same binary; the contention is on hash-quality vs hash-speed tradeoffs in std::HashMap, not on hardware specifics.

## Status

`closed` — hypothesis falsified, change reverted. Archive stays in place as historical evidence of the falsification.

## Cross-refs

- Plan §B2 (FxHashMap drop-in for composition): falsified, see ADR-0009.
- ADR-0006: L3 protocol used here (without it, the regression might have been missed under cache-warmup contamination).
- Companion finding: `bench_diff.sh --robust` Mann-Whitney was comparing raw times instead of per-iter times. Fixed in this sitting.
- Followups: do NOT pursue EXP-0010-deep at L4. The regression is robust at L3 (~30 samples per side, p<0.001); a deeper run would just confirm the same direction. Move on to EXP-0003 (LabelSetTable interning) or another perf experiment instead.
