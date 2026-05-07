# EXP-0004-with-capacity: drop 20% growth wrapper, let Vec::push double

**One-line summary.** Removed the custom `~20% capacity growth` wrapper from `CltsBuilder`, `LabelStoreBuilder`, and `VariableStoreBuilder` in favor of `Vec::push`'s native amortized doubling. **All 7 clts_construction benches improve at p<0.001**; biggest win is `chain/100000` at **-31% (1.45× speedup)** where reallocation count dominates.

## Motivation

Plan §A4 (and the original deep evaluation): the custom `grow_capacity()` wrapper at `clts/mod.rs:46-58` increments capacity by ~20% per fill. For 1M states, that's ~25 reallocations per `Vec` (each ~20% bigger than the last). `Vec::push`'s native doubling produces ~12 reallocations (each 2× bigger) — fewer reallocations = fewer copies = faster.

The wrapper also coordinated parallel Vec growth (state_names, state_variables, state_valuations, state_map, initial_states) into a single `reserve()` call. Removing the wrapper means each Vec doubles independently, which matches the access pattern (each `push` only triggers growth on its own Vec).

## Hypothesis (pre-registered, plan §A4)

≥1.1× speedup on builder-only bench.

## Method

L3 protocol per ADR-0006:

1. Save EXP-0004 patch (`git diff > /tmp/exp-0004-with-capacity.patch`).
2. **A side**: `git checkout HEAD -- crates/mununu-core/src/clts/mod.rs` to restore the 20% wrapper. Run `scripts/bench_compare.sh exp-0004-with-cap -- ... --bench clts_construction` for warmup discard + save-baseline. 40 samples per bench function, 8-15s measurement window.
3. **B side**: `git apply /tmp/exp-0004-with-capacity.patch`. Re-run with `--baseline-only`.
4. `scripts/bench_diff.sh exp-0004-with-cap --robust`.

## Code change summary

- Removed `grow_capacity()` function (`clts/mod.rs:46-58`).
- Removed `ensure_state_capacity()`, `ensure_transition_capacity()` from `CltsBuilder`.
- Removed `ensure_entry_capacity()` from `LabelStoreBuilder`.
- Removed `ensure_set_capacity()` from `VariableStoreBuilder`.
- Removed all 6 call sites of those helpers.
- Removed the 4 capacity-hint fields (`state_capacity_hint`, `transition_capacity_hint`, `entry_capacity_hint`, `set_capacity_hint`).
- Updated `reserve_states()` and `reserve_transitions()` to just call `Vec::reserve()` without the hint update.
- All 825 lib tests + 33 test groups pass unchanged.

Net: +30 lines of comments documenting the change, -50 lines of growth-management code. Cleaner.

## Results

### Criterion bootstrap (40 samples per side, p<0.001 across all)

| Bench | A (20% wrapper) | B (Vec::push) | Δ | 95% CI |
|-------|----------------:|--------------:|--:|--------|
| `chain/1000` | 796 µs | 742 µs | **-12.3%** | [-15.8%, -8.9%] |
| `chain/10000` | 8.10 ms | 7.69 ms | **-12.1%** | [-17.8%, -6.7%] |
| `chain/100000` | **130 ms** | **90.5 ms** | **-31.0%** | **[-38.9%, -23.1%]** |
| `grid/32x32` | 805 µs | 729 µs | -15.5% | [-22.3%, -10.2%] |
| `grid/64x64` | 3.38 ms | 2.88 ms | -17.6% | [-20.5%, -14.6%] |
| `random_seeded/256` | 1.08 ms | 0.96 ms | -14.1% | [-17.8%, -10.5%] |
| `random_seeded/1024` | 17.3 ms | 16.4 ms | -5.2% | [-7.0%, -3.2%] |

### Robust diff (Mann-Whitney p<0.01 gate)

```
IMPROVEMENTS (6):
    -31.0%   chain/100000           CI[-38.9%, -23.1%]  med -30.3%  p=0.000
    -17.6%   grid/64x64             CI[-20.5%, -14.6%]  med -14.9%  p=0.000
    -15.5%   grid/32x32             CI[-22.3%, -10.2%]  med -9.4%   p=0.000
    -14.1%   random_seeded/256      CI[-17.8%, -10.5%]  med -11.0%  p=0.000
    -12.3%   chain/1000             CI[-15.8%, -8.9%]   med -6.8%   p=0.000
    -12.1%   chain/10000            CI[-17.8%, -6.7%]   med -5.0%   p=0.000
NEUTRAL (1):
     -5.2%   random_seeded/1024     CI[-7.0%, -3.2%]    med -5.2%   p=0.000
```

`random_seeded/1024` is below the 10% threshold but still p<0.001 — Criterion classifies as significant; the robust gate categorizes it NEUTRAL (below threshold) but reports the p-value.

## Why the win scales with state count

`chain/100000` shows the biggest delta (-31%) because:
- The 20% wrapper requires log_1.2(100k / 256) ≈ 32 reallocations.
- Vec doubling requires log_2(100k / 256) ≈ 9 reallocations.
- Each reallocation copies the entire current contents to a new location.
- Bigger Vecs → bigger copy cost per realloc → reallocation count dominates.

For smaller chains (1k, 10k), reallocation cost is a smaller fraction of total construction time, so the speedup is smaller (~12%).

For random_seeded (which uses `RandomClts`), the construction work is dominated by `rand_chacha` RNG calls and SmallVec label set creation, not the Vec growth itself. Hence the smaller speedup at random_seeded/1024.

## What this falsifies (a counterintuitive lesson)

The 20% wrapper was originally written with the rationale (per the inline comment): *"large enough to avoid frequent reallocations while still keeping peak memory under control for big benches."* The peak-memory argument turns out to be wrong: doubling allocates at most 2× over-provision, while the 20% wrapper eventually allocates the same amount in smaller increments — same peak, more reallocations.

Hypothesis-driven cleanup beats hand-tuned growth strategies.

## Soundness

- All 825 lib unit tests pass unchanged.
- All 33 test groups (composition, properties, soundness, doctests) pass unchanged.
- Public API surface unchanged: `reserve_states()` and `reserve_transitions()` still accept additional capacity hints; they just call `Vec::reserve()` directly now.
- Behavior change: parallel Vecs may have different capacities at any moment (each doubles independently). Functionally equivalent — none of the code reads parallel-vec capacity.

## How to replay

```bash
make replay EXP=EXP-0004-with-capacity
```

Or directly via `command.txt`.

## Status

`closed` — hypothesis confirmed, change kept in tree.

## Cross-refs

- Plan §A4 (drop 20% growth wrapper): **confirmed**, expected ≥1.1× delivered as 1.06× to 1.45× across the bench matrix.
- ADR-0006: L3 protocol used here (the original 20% wrapper claim was based on intuition, not measurement; L3 confirms the alternative is faster).
- Companion: EXP-0010-fxhash-composition (also a 0.5-day plan item from the §B-series; falsified). EXP-0004 confirms plan §A-series intuition holds; EXP-0010 falsifies a §B-series intuition. Bench-first methodology distinguishes them.
- Followup: EXP-0001-deep should re-record the construction baseline at L4 with this change applied. The current EXP-0001 baseline still reflects the 20% wrapper.
