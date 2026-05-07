# Free-form observations for EXP-0012-changed-flag-fixpoint

## 2026-05-06 — third drop-in falsification, fourth in the §A/§B series

### What this archive proves

- Plan §B4's "track changes during merge" hypothesis is empirically a regression at our state-count scale (≤1024 states). 7/10 benches +10% to +94% slower.
- The change passes all soundness/property tests; the issue is performance, not correctness.
- BitVec equality + clone is faster than word-loop with per-word branch at this scale because:
  - `BitVec::==` early-exits on first word mismatch.
  - Vec clone is hardware-optimized memcpy.
  - The branchful word loop has a serial dependency chain (`changed |= ...`).

### What this archive does NOT mean

- The technique is universally bad. At larger state counts (10k+), with more fixpoint iterations per call, the cumulative savings might amortize. Not tested.
- BitVec needs replacement. The std `bitvec::BitVec` library is doing its job correctly; my added word loop just has higher constant overhead at small sizes.

### The four-strike pattern

1. **EXP-0002 contamination** (sitting 3): apparent 5-7× speedup was cache warmup. Retracted.
2. **EXP-0010 FxHashMap drop-in** (sitting 5): +30-60% regression on composition.
3. **EXP-0003 LabelSetTable drop-in** (sitting 8): +18-85% on composition, +3-5% on synth.
4. **EXP-0012 track-during-merge drop-in** (sitting 9, this archive): +10-94% on mu_calculus_only.

The wins:
- **EXP-0002b SoA structural change**: 2.4× on synth.
- **EXP-0004 drop 20% growth wrapper**: 1.06× to 1.45× on construction.

The pattern is now starkly empirical: drop-in optimizations on small data structures consistently lose; structural changes to access patterns / growth strategies consistently win.

### What this means for the remaining plan items

§A3 (CSR adjacency) and §A6 (predicate interning + Vec bindings) are next on the §A-series. Both have structural-change components and drop-in components. Per ADR-0009/0012/0013:
- The structural parts (CSR layout, RAII BindingStack) are predicted to win.
- The drop-in parts (predicate name interning) are predicted to lose at our scale.

Future experiments should split these into separate EXPs to isolate the structural win from the drop-in failure.

### Methodology lessons

- L3 protocol caught EXP-0012 cleanly: 30+ samples per side, p<0.001 across 7 benches.
- Running on TWO axes (composition_only AND mu_calculus_only) for EXP-0003 was useful; for EXP-0012, only mu_calculus_only matters because the change is in the mu-calculus path.
- The bench_diff per-iter MW fix (sitting 5) keeps paying off — without it, the +10-25% small regressions might have been demoted to neutral. With it, they stand as significant.

### Iteration policy actions

- Used `--fresh` on bench_compare.
- Saved EXP-0012 patch + a pre-EXP-0012 evaluator.rs snapshot for clean A/B switching. The snapshot approach is cleaner than patch-juggling for changes localized to one file.
- Following ADR-0004: archive stays as historical evidence. Reverted code change is in next commit.

### Anti-patterns avoided

- Did not retrofit a "the change actually wins on bigger fixtures" argument. None of the workloads I have benches for shows a clear win; that's the verdict.
- Did not skip the bench because the change "felt right" semantically. The protocol exists exactly to disprove felt-right intuition.
- Did not declare the lone -8.7% as a partial win. Mann-Whitney p=0.351 says noise; that's the answer.
