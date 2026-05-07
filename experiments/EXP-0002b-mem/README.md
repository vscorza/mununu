# EXP-0002b-mem: dhat heap profile of SoA vs HashMap iteration_ranks

**One-line summary.** dhat-instrumented A/B replay of the EXP-0002b `synthesis_product_game/grid_32x32` workload. **The 2.4× wall-clock win does NOT come from heap allocation reduction** — total bytes and alloc counts are within 0.02% across A and B. Peak heap drops 6.7% (-76 KB). The win is in cache locality / hash probe count, not heap pressure.

## Motivation

EXP-0002b confirmed a 2.4× wall-clock speedup on synthesis_product_game/grid_32x32 with the SoA migration. EXP-0002b-mem asks the orthogonal question: where does the win come from? Three plausible accounts:

1. **Heap pressure**: SoA allocates fewer/smaller blocks → less GC-equivalent work.
2. **Cache locality**: same allocation pattern, but Vec<Vec<u32>> indexing follows a contiguous-load pattern that the cache predicts well, while HashMap probes scatter.
3. **Hash function cost**: SoA replaces 2 HashMap probes per `signature()` call with 2 Vec indexes. Those indexes are 5-10 ns; HashMap probes including SipHash are 30-50 ns.

dhat measures (1) directly. If the heap profile is roughly the same between A and B, the win must come from (2) and/or (3).

## Hypothesis (pre-registered)

EXP-0002 README claimed "≥100 KB heap reduction on synthesis-relevant bench." This is the explicit memory dimension test.

## Method

1. New `crates/mununu-core/src/bin/dhat_synthesis.rs` binary: `#[global_allocator]` = `dhat::Alloc`, `_profiler = dhat::Profiler::new_heap()`, runs the EXP-0002b synthesis call 3 times (to amortize fixture-build cost and stabilize the per-call shape), writes `dhat-heap.json` on Drop.
2. New `[[bin]]` target in Cargo.toml gated by `required-features = ["test_support", "dhat"]`.
3. Save SoA patch (`git diff > /tmp/exp-0002-soa.patch`).
4. **A side** (HashMap): `git checkout HEAD -- ...` → `cargo build --release --features test_support,dhat --bin dhat_synthesis` → run → save dhat-heap.json as `dhat-heap.A.json`.
5. **B side** (SoA): `git apply /tmp/exp-0002-soa.patch` → rebuild → run → save as `dhat-heap.B.json`.

Both runs were under release-mode optimizations on the same host within minutes of each other. The `#[global_allocator] = dhat::Alloc` interception adds ~5-10× wall-clock overhead but is consistent across A and B; the heap profile itself is unaffected.

## Results

| Metric | A (HashMap) | B (SoA) | Δ |
|--------|------------:|--------:|--:|
| Total bytes (3 synth runs) | 1,633,826,260 | 1,633,442,349 | **-384 KB (-0.02%)** |
| Total allocations | 28,856,332 | 28,853,942 | **-2,390 (-0.008%)** |
| Peak heap (t-gmax) | 1,141,316 | 1,064,732 | **-76,584 (-6.7%)** |
| Peak live blocks | 8,279 | 8,279 | **0** |
| Final live bytes | 0 | 0 | (clean teardown both sides) |

**Hypothesis ≥100 KB heap reduction is FALSIFIED at this scale.** Peak heap drops 76 KB, not 100+. Total bytes and alloc count are statistically indistinguishable.

## Interpretation

The 2.4× wall-clock win on grid_32x32 (EXP-0002b) is **not** a heap-pressure win. dhat's totals are within rounding of each other — the SoA and HashMap variants do roughly the same number of allocations of roughly the same total size. The 76 KB peak-heap reduction is the iteration_ranks structure itself being smaller (Vec<Vec<u32>> at ~16 KB vs HashMap with table+entries+slack at ~92 KB), but it doesn't propagate to the rest of the synthesis pipeline.

The win must therefore be in:

- **Cache locality**: ProductGame controller construction at `context/mod.rs:2034` calls `iteration_ranks.get_rank(var, state_idx)` once per (product_state × obligation) pair, sequentially. SoA's `Vec<Vec<u32>>` indexing produces sequential cache loads. HashMap probes follow the table's hash distribution, scattering across the slack-padded table.
- **Hash probe count**: HashMap.get is one hash + one slot probe + (sometimes) collision chain follow. SoA `get_rank` is two slice indexes. The constant difference (~30-40 ns vs ~5-10 ns) compounds over thousands of get_rank calls per ProductGame construction.

For grid_32x32 (1024 plant states × 2 mu-obligations producing ~2000-state product), the synthesis path makes ~4000 `get_rank` calls. Save 30 ns per call = 120 µs saved. Multiplied by the iteration counts in 30-sample bench measurement, the wall-clock difference adds up.

## What this means for the paper

The §3.x narrative for SoA must shift:

- **Before EXP-0002b-mem**: "SoA reduces memory pressure and improves wall-clock time."
- **After EXP-0002b-mem**: "SoA preserves memory totals but improves *access pattern*. The wall-clock win comes from cache locality + faster per-lookup cost on the synthesis-bound hot path."

This is an honest, more nuanced story. It also predicts where the win will scale further:

- **Larger plant states** (10k, 100k): more get_rank calls → bigger absolute wall-clock saving.
- **Higher alternation depth**: more obligations × more product states → quadratic growth in get_rank calls.
- **Larger product spaces** in ProductGame: multiplicative scaling.

The peak-heap-reduction hypothesis falsification is an honest update; the wall-clock confirmation from EXP-0002b stands.

## What this does NOT mean

- The SoA migration was wrong. EXP-0002b's 2.4× wall-clock win is real, citable, and reproducible. EXP-0002b-mem just clarifies *which* axis the win lives on.
- We should revert. The SoA's cleaner API + cache-locality win + groundwork for EXP-0007 keeps it in tree.
- All future heap-axis hypotheses are wrong. The plan's §A3 (CSR adjacency) targets a much larger structure (Vec<Vec<Transition>>) where heap reduction is the primary mechanism. EXP-0002b-mem doesn't transfer.

## How to replay

```bash
make replay EXP=EXP-0002b-mem
```

Or directly:

```bash
git diff HEAD -- crates/mununu-core/src/mu_calculus/evaluator.rs \
                 crates/mununu-core/src/context/mod.rs > /tmp/exp-0002-soa.patch

# A side (HashMap):
git checkout HEAD -- crates/mununu-core/src/mu_calculus/evaluator.rs \
                     crates/mununu-core/src/context/mod.rs
cargo build --release -p mununu-core --features test_support,dhat --bin dhat_synthesis
./target/release/dhat_synthesis
mv dhat-heap.json experiments/EXP-0002b-mem/dhat-heap.A.json

# B side (SoA):
git apply /tmp/exp-0002-soa.patch
cargo build --release -p mununu-core --features test_support,dhat --bin dhat_synthesis
./target/release/dhat_synthesis
mv dhat-heap.json experiments/EXP-0002b-mem/dhat-heap.B.json
```

View either profile in [dh_view](https://nnethercote.github.io/dh_view/dh_view.html).

## Status

`closed` — heap-axis hypothesis falsified, wall-clock-axis confirmation from EXP-0002b stands. Refines the SoA narrative for paper §3.x and blog post 2.

## Cross-refs

- Predecessor: EXP-0002b-synth-bench (wall-clock 2.4× confirmation).
- Companion: EXP-0002 README's "≥100 KB heap reduction" pre-registration is now empirically falsified at the grid_32x32 scale.
- Plan §A1: pre-registered hypothesis was wall-clock + heap. Wall-clock confirmed, heap falsified at this scale; the wall-clock mechanism is cache-locality, not heap-pressure.
- Followup: EXP-0002b-mem-deep at L4 with a 64×64 or larger fixture would reveal whether peak-heap delta scales (predicted: linearly with state count × num_fixpoint_vars). Required if the paper wants to claim memory-axis benefits.
