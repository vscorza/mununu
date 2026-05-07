# EXP-0003-labelset-interning: LabelSetTable as drop-in for SmallVec keys — HYPOTHESIS FALSIFIED

> **⚠ HYPOTHESIS FALSIFIED.** Plan §A2's predicted ≥1.5× speedup is not delivered by this implementation. The substitution introduces a SmallVec→LabelSetId lookup BEFORE every `uncontrollable_groups.contains_key` call, which adds work rather than removing it. Net: composition_only +18% to +85% slower; synth +3% to +5% slower; both p<0.001.

**One-line summary.** Added `LabelSetTable` (interned canonical label sets) and changed `uncontrollable_groups` keys from `SmallVec<[LabelId; 4]>` to `LabelSetId(u32)`. The change passes all 830+ unit tests + soundness suite + property tests, but is a wall-clock regression on the workloads it was supposed to accelerate. Reverted.

## Motivation

Plan §A2: `Vec<HashMap<SmallVec<[LabelId; 4]>, Vec<usize>>>` at `clts/mod.rs:935` hashes a 24-byte SmallVec on every probe. Predicted ≥1.5× speedup from interning canonical label sets to `LabelSetId(u32)` (4-byte hash, 1-cycle equality).

## Hypothesis (pre-registered, plan §A2)

≥1.5× on `uncontrollable_groups` lookup; edge memory drops from ~16 to 4 bytes/edge for the label-set field.

## Method

L3 protocol per ADR-0006:

1. New `crates/mununu-core/src/clts/label_set_table.rs` — `LabelSetTable<L>` with `intern`, `lookup`, `find`. 5 unit tests.
2. `Clts.uncontrollable_groups` field changed: `Vec<HashMap<SmallVec, Vec<usize>>>` → `Vec<HashMap<LabelSetId, Vec<usize>>>`.
3. New `Clts` field: `label_set_table: LabelSetTable<L>` populated during `CltsBuilder::build()`.
4. Public accessors: `Clts::label_set(id)` resolves an ID to its canonical SmallVec; `Clts::find_label_set(&smallvec)` performs the inverse lookup.
5. `transitions_grouped_by_uncontrollable_labels` return type updated.
6. Internal test at `clts/mod.rs:3160` updated to resolve via `clts.label_set(*id)`.
7. `mu_calculus/evaluator.rs` updated: `GroupedTransitions` type alias, inner consumer at line 823 (now uses LabelSetId directly), four `contains_key(&smallvec)` sites converted to `find_label_set` + `contains_key(&id)`.
8. L3 A/B run on `composition_only` AND `mu_calculus_only::synthesis_product_game`.

## Results

### `composition_only` (Criterion bootstrap, n=30 per side, p<0.001)

| Bench | A (SmallVec keys) | B (LabelSetId keys) | Δ | 95% CI |
|-------|------------------:|--------------------:|--:|--------|
| `chain_sync/chain1k_x_ring1k` | 2.62 µs | 3.09 µs | **+18.1%** | [+15.7%, +20.5%] |
| `grid_async/grid32_x_grid32` | 1.47 µs | 1.62 µs | **+27.0%** | [+12.4%, +43.6%] |
| `mode_compare/sync` | 2.47 µs | 3.13 µs | **+84.8%** | [+32.9%, +175.1%] |
| `mode_compare/async` | 2.48 µs | 2.51 µs | +0.5% | [-2.2%, +2.6%] |
| `mode_compare/superset` | 2.47 µs | 2.71 µs | **+13.8%** | [+8.1%, +20.4%] |

### `mu_calculus_only/synthesis_product_game` (n=15 per side, p<0.01)

| Bench | A | B | Δ | 95% CI |
|-------|--:|--:|--:|--------|
| `synthesis_product_game/ring_1k` | 13.92 ms | 14.22 ms | **+3.2%** | [+1.7%, +4.9%] |
| `synthesis_product_game/grid_32x32` | 1.06 s | 1.10 s | **+4.5%** | [+2.1%, +7.1%] |

## Why the optimization fails

The plan §A2 reasoning held one assumption: that the modal-evaluation hot path *already had a LabelSetId in hand* and was only paying for the SmallVec hash on the HashMap probe. In practice, the four `contains_key` consumer sites in `mu_calculus/evaluator.rs` start with a transition's labels, build a fresh `SmallVec` of just-the-uncontrollable-labels, and probe.

With SmallVec keys (the original): one HashMap probe with SmallVec hash (~30-50 ns).

With LabelSetId keys (this EXP): one `find_label_set` probe (still SmallVec-hash on the `label_set_table` index) PLUS one HashMap probe with u32 hash. **Two probes where there was one.** The u32 probe is faster but the SmallVec probe still happens — and for transitions whose uncontrollable subset isn't in the table (the common "in_uncontrollable_group = false" branch), the second probe is wasted.

For composition specifically (the largest regressions), the build path now includes `LabelSetTable::intern` for every transition, which is additional work without any compensating consumer-side win at composition time — composition doesn't read uncontrollable_groups; it just constructs the new CLTS.

## Why the inner-loop optimization at evaluator.rs:823 didn't compensate

The inner loop iterates over `precomputed_groups` (the `HashMap<LabelSetId, Vec<usize>>`) and clones the key. With LabelSetId, the clone becomes a `Copy` instead of a SmallVec clone — saving ~10-30 ns per iteration. But this savings is small relative to the per-call work in the inner body (guard matching, target lookup, bitset ops), which dominates the modal-eval hot path.

## Decision

**Revert the EXP-0003 changes.** Plan §A2 is marked falsified for this implementation. A future EXP-0003-redesigned might:

1. **Keep SmallVec keys but pre-compute the canonical SmallVec on Transition itself.** The inner loop would then read it directly instead of recomputing per call. Bigger refactor (touches Transition layout) but might actually deliver.
2. **Cache the LabelSetId on Transition** alongside the SmallVec. Eliminates `find_label_set` overhead. Same Transition-touching cost as #1.
3. **Skip the `contains_key` sites entirely** by computing the "in_uncontrollable_group" flag at build time and storing it on each transition. Avoids both the SmallVec construction and the probe.

None of these is a "drop-in." All require Transition struct changes plus consumer audits. EXP-0003 (this archive) demonstrates that a "drop-in" interning swap doesn't deliver.

## Soundness

All tests pass — 830 lib tests, 33 test groups, 22 soundness tests, 5 proptests. The change is semantically equivalent; only performance differs.

## Cross-refs with other falsifications

- **EXP-0010** (FxHashMap drop-in for composition): also a "drop the std HashMap, use a faster hasher" hypothesis. Falsified at +30-60% regression on the same workloads.
- **EXP-0003** (this archive): "drop the SmallVec key, use a u32 ID" hypothesis. Falsified at +18-85% on composition, +3-5% on synth.
- Both share the failure mode: the assumed bottleneck (hash speed) wasn't the bottleneck. RefCell+Rc churn and fixture iteration dominated.

These two together establish a strong empirical claim for the blog series: **drop-in hash optimizations on small, RefCell-wrapped maps in mununu's code are systematically NOT a win.** The win comes from changing the access pattern (EXP-0002b SoA, EXP-0004 Vec doubling), not from changing the hasher or the key shape.

## How to replay

```bash
make replay EXP=EXP-0003-labelset-interning
```

Or directly via `command.txt`.

## Status

`closed` — hypothesis falsified, change reverted. Archive stays in place as historical evidence.

## Cross-refs

- Plan §A2: falsified. ADR-0012 records the decision.
- Companion: EXP-0010 (also falsified drop-in) and EXP-0004 (confirmed structural change). Three EXPs together calibrate which optimization classes work.
- Followup: a future EXP-0003-redesigned could try caching LabelSetId on Transition. Not scheduled.
