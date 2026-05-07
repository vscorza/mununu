# Free-form observations for EXP-0002b-synth-bench

## 2026-05-06 — synthesis-bound A/B on the right workload

### What this archive proves

- The SoA migration (EXP-0002) is **not** a regression overall. It's neutral on workloads that don't exercise iteration_ranks (EXP-0002a) and a real ≥2× win on workloads that do (this EXP, grid_32x32).
- The ≥2× hypothesis from EXP-0002 is confirmed on at least one workload class (synthesis with ProductGame mode + non-trivial alternation), at one scale (grid_32x32 = 1024 states + 2 mu-obligations).
- The L3 protocol from ADR-0006 is sufficient to detect a real ~2× speedup with significance even at moderate sample sizes (n=15).

### What this archive does NOT prove

- The win generalizes to ParityGame mode. ParityGame is a different code path that constructs an explicit parity-game graph; it may or may not benefit from the SoA layout.
- The win generalizes to other mu-calculus benches under witness extraction. We tested only one formula shape (alternation-2 GR(1)-style with two mu-obligations); other shapes (alternation-3, single-mu reachability) might show different magnitudes.
- The win holds at smaller scales (~100 states). At ring_1k we saw -9% trend but noise-bounded; below that, the HashMap may actually be faster due to lower constant overhead.
- The win is robust on dedicated hardware. EXP-0002b-deep at L4 will be the citable test.

### Why ring_1k didn't reach significance and grid_32x32 did

ring_1k: 19 ms per call → 1500+ iterations per sample. Variance is small (tight CIs in absolute terms), but the absolute speedup is also small (~2 ms). Statistical signal-to-noise is borderline.

grid_32x32: 1.2-2.8 s per call → 15-30 iterations per sample. HashMap variance is huge (CI 2.0-3.8s) due to cache thrashing on the larger product space. SoA variance is tight (CI 1.17-1.26s). The combination produces a clear separation that bootstrap-on-means picks up immediately.

A side note: the HashMap's wide CI on grid_32x32 is itself evidence of the cache problem the SoA fixes. The HashMap's per-call timing is unstable because its access pattern thrashes against other concurrent processes' cache footprints. The SoA's contiguous layout localizes the working set and makes timing reproducible.

### Why we keep the SoA migration permanently

- 2.4× speedup on the workload class it was designed for (synthesis with non-trivial alternation).
- No measurable regression on workloads it wasn't designed for (EXP-0002a confirmed neutral).
- API is type-safer (HashMap accepts arbitrary `(usize, FormulaVarId)` keys; SoA enforces var indices via the struct).
- Lays groundwork for EXP-0007 (predicate interning + dense bindings) which depends on a stable IterationRanks shape.

### Comparing the three EXPs in the EXP-0002 lineage

- **EXP-0002** (sitting 3 morning): apparent 5-7× speedup on mu_calculus_only benches. **CONTAMINATED** by cache-warmup differences across separate `cargo bench` invocations. Superseded.
- **EXP-0002a** (sitting 3 afternoon): clean L3 same-session A/B on the same benches. Differences classified as LLVM-noise because the benches don't exercise iteration_ranks. Hypothesis untested.
- **EXP-0002b** (this archive, sitting 4): adds a synth-bound bench, runs L3 protocol. Hypothesis confirmed on grid_32x32 (2.4× speedup, p<0.001).

The full picture is now coherent and citable: the SoA migration delivers a real, statistically significant speedup on its intended workload. Public claims should specify "synthesis-bound workloads with non-trivial alternation" rather than implying a blanket speedup.
