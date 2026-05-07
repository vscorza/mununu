# EXP-0030-csr-adjacency: flat CSR adjacency via staging-flatten — HYPOTHESIS FALSIFIED

> **⚠ HYPOTHESIS FALSIFIED.** Plan §A3 (partial) predicted memory + locality wins from replacing `Vec<Vec<Transition>>` with `(Vec<Transition>, Vec<u32>)`. Empirically: 6 of 7 `clts_construction` benches regress at p<0.001 (range +12% to +65%); composition and synth are ±10% noise. The CSR-via-staging-flatten implementation adds an O(|E|) extra copy at build time that absorbs the predicted savings. Reverted.

**One-line summary.** Added `AdjacencyCsr<S, L>` (flat transitions Vec + offsets Vec); replaced `Clts.outgoing` and `Clts.incoming` field types; routed the build through `AdjacencyCsr::from_rows(staging)`. All tests pass; performance regresses on construction.

## Motivation

Plan §A3: predicted ≥3× memory reduction at 1M states / 4M edges; ≥1.5× on modal-pre-image hot loops. The §A3 design pairs CSR with §A2 (LabelSetTable) for per-edge size reduction (~24 → 8 bytes); without §A2, the win is just the per-row Vec header overhead (~24 bytes × N states).

This EXP shipped only the row-overhead win (kept Transition struct unchanged) since §A2 was already falsified (EXP-0003).

## Hypothesis (pre-registered, plan §A3 partial)

Memory: ~24 MB savings at 1M states from eliminating per-row Vec headers. Wall-clock: neutral on construction (the flatten is amortized), neutral-to-positive on `outgoing(state)` consumers (cache locality from contiguous transition storage).

## Method

L3 protocol on three benches:
- `clts_construction` (the build path that constructs the staging Vec<Vec<>> and now also flattens it).
- `composition_only` (which calls compose() that builds new CLTSs).
- `mu_calculus_only::synthesis_product_game` (which calls outgoing(state) on the CSR).

Both sides 30+ samples per bench function in same-shell A/B comparison.

## Results

### `clts_construction` — 6 regressions, 1 improvement

| Bench | Δ | 95% CI | p |
|-------|--:|--------|---:|
| `random_seeded/1024` | **+64.5%** | [+51.1%, +79.8%] | 0.000 |
| `grid/32x32` | +33.6% | [+30.6%, +36.4%] | 0.000 |
| `grid/64x64` | +28.6% | [+17.5%, +41.8%] | 0.000 |
| `chain/100000` | +23.6% | [+19.3%, +27.8%] | 0.000 |
| `chain/10000` | +13.3% | [+8.9%, +18.0%] | 0.000 |
| `chain/1000` | +12.3% | [+9.3%, +16.2%] | 0.000 |
| `random_seeded/256` | **−15.8%** | [−23.8%, −5.1%] | 0.000 |

The lone improvement on `random_seeded/256` (smallest fixture) is consistent with the pattern: at very small workloads, the CSR build cost is dominated by other factors (RNG, label intern setup); at larger workloads, the O(|E|) flatten pass dominates.

### `composition_only` — neutral

All 5 benches within ±10% threshold. Median deltas range from −8.9% to +8.4%, p values mixed. No clear win or loss on the consumer side.

### `mu_calculus_only/synthesis_product_game` — neutral

Both fixtures (ring_1k, grid_32x32) at +2.8% / +3.6% (below threshold, statistically significant per Criterion but not large enough to act on).

## Why CSR-via-staging-flatten loses

The implementation builds the existing `Vec<Vec<Transition>>` staging structure FIRST (the path mununu has always used), then flattens it into `Vec<Transition>` at the end via `AdjacencyCsr::from_rows`. That flatten is an **O(|E|) extra copy** of every transition.

Original build path:
1. Allocate per-state `Vec<Transition>` (with capacity).
2. Push transitions into per-state Vecs.
3. Move per-state Vecs into the Clts struct (zero-copy).

CSR build path (this EXP):
1. Allocate per-state `Vec<Transition>` (with capacity). [unchanged]
2. Push transitions into per-state Vecs. [unchanged]
3. **Flatten: copy every transition from per-state Vecs to a single flat Vec<Transition>.** [new]
4. Move flat Vec into the Clts struct.

For 4M edges × 24 bytes per Transition = **96 MB of additional memcpy at build time**. The savings (eliminating ~24 MB of Vec header overhead) are smaller than the cost.

For the runtime accessor (`outgoing(state)`):
- Original: `&self.outgoing[state.index()]` — one indirection through the outer Vec, returns the per-state Vec's backing slice.
- CSR: `self.outgoing.row(state.index())` — two slice indexes (offsets[i], offsets[i+1]) followed by a slice into the flat transitions. Slightly more arithmetic but better cache locality (transitions for adjacent states are contiguous).

The runtime improvement is real but small (~1-3% per call); not enough to amortize the build-time flatten cost.

## What would actually deliver the §A3 win

A from-scratch CSR build path that bypasses the staging Vec<Vec<>>:
1. Pre-compute per-state outgoing/incoming counts (already done at line 1881-1886).
2. Allocate flat `Vec<Transition>` with total capacity.
3. Compute offsets via prefix sum.
4. Use a `cursor: Vec<u32>` to track per-state insertion position.
5. Push each transition directly into `flat[cursor[from]++]`.

This eliminates the staging Vec<Vec<>> entirely. Net: zero copies vs the original (which has zero copies as well, just allocates per-state Vecs). The win comes from:
- No per-state Vec allocations (24 bytes × N states).
- Better cache locality on consumer side.

This is a bigger refactor (~2-3 hours additional work on top of EXP-0030) that wasn't pre-registered. Not scheduled; could be EXP-0031-csr-direct-build if the §A3 hypothesis is worth pursuing.

## Decision

**Revert the EXP-0030 changes.** Plan §A3 (CSR-via-staging-flatten) is falsified. A from-scratch direct-CSR-build implementation might still deliver, but is a separate experiment.

## Soundness

All 828 lib tests + 33 test groups + 22 soundness + 5 proptests pass. The change is semantically equivalent; only performance differs.

## Status

`closed` — falsified; reverted. Archive stays as historical evidence.

## Cross-refs

- Plan §A3 (CSR adjacency): falsified-via-staging-flatten. ADR-0014 records the decision.
- Sibling falsifications: EXP-0010 (FxHashMap), EXP-0003 (LabelSetTable), EXP-0012 (track-during-merge). Four falsifications now.
- Followup (not scheduled): EXP-0031-csr-direct-build with a from-scratch CSR construction path that bypasses staging.

The "structural wins, drop-ins lose" taxonomy from sittings 7-9 needs refinement: **structural changes that require extra setup work can also lose.** EXP-0030 confirms structural-but-with-setup-cost is its own failure category.

## Replay

```bash
make replay EXP=EXP-0030-csr-adjacency
```
