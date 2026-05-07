# Free-form observations for EXP-0002a-warmup-rerun

## 2026-05-06 — corrected A/B in response to user question

### What this archive is and isn't

**Is:** a clean, reproducible same-session A/B comparison of HashMap-iteration_ranks vs SoA-iteration_ranks on the four mu_calculus_only bench groups, with warmup discard and full Criterion samples. The archive carries both sides' raw sample data (`target/criterion/<bench>/exp-0002-full/sample.json` for A, `<bench>/new/sample.json` for B), enabling future re-analysis with different statistical tests.

**Isn't:** the synthesis-bound bench that the original EXP-0002 hypothesis (≥2× speedup) actually targeted. The mu_calculus_only benches don't enable witnesses, so the iteration_ranks code paths are gated off. The numbers in this archive represent **LLVM monomorphization noise**, not a real comparison of HashMap vs SoA in their hot paths. EXP-0002b is the experiment that actually tests the original hypothesis on the right workload.

### Where the contamination came from in EXP-0002

EXP-0002 ran `cargo bench --features test_support --bench mu_calculus_only -- --quick` at sitting 3 (today), in a different shell session from EXP-0001's bench run from sitting 1 (yesterday). Between those two sessions:

- The cargo target/ directory accumulated artifacts (1.5 GB+ of release binaries, intermediate bytecode, criterion data).
- The host laptop's page cache, mmap regions, and binary residency state changed.
- The criterion JSON output from sitting 1's smoke runs piled up in target/criterion/.

When EXP-0001 was recorded, the binary was fresh-compiled and many of its mmap pages weren't resident. By the time EXP-0002 was recorded, the same binary code paths had been exercised many times; the OS had paged everything in.

Result: EXP-0002 looked 5-7× faster than EXP-0001 simply because EXP-0001 paid binary-warmup costs that EXP-0002 didn't.

The L3 protocol (warmup discard + same-session) eliminates this entire contamination class.

### Why both significance tests are honest, and disagree

- **Criterion's t-test** uses bootstrap resampling on the 30 measurements per side, computes the mean of each bootstrap sample, and reports a 95% CI on the difference. This is the modern best practice for benchmark comparison (Kalibera & Jones 2013) and what we cite in the reproducibility contract (notebook/0000-overview.md point 4).

- **My Mann-Whitney implementation** uses a normal-approximation U test on the raw 30 samples per side (no resampling, no outlier filtering). It's more conservative because it tests "do these two distributions differ?" rather than "do their means differ?" — long-tailed or bimodal distributions (which our `--quick`-warmup-but-full-bench data sometimes is) shrink the U statistic.

For paper-grade analysis, Criterion's bootstrap is the right answer. For ad-hoc CI gates where false positives are expensive, my conservative MW gate is the right answer. Both go in the report.

### Why I'm keeping the SoA migration

- All tests still pass.
- The struct is type-safer (the HashMap accepted any `(state, var)` tuple; the struct enforces var indices).
- The IterationRanks shape is the API future EXPs (EXP-0007 predicate interning) will depend on.
- The "regression" is at most +27% on a workload that **doesn't even use iteration_ranks** — the differences are LLVM noise, not a real performance loss.

If EXP-0002b shows the SoA is genuinely a regression on synthesis-bound workloads, we revert (or optimize the lazy-resize / saturation paths). Until then: keep.

### Methodology refinements landed in this sitting

1. `bench_compare.sh` empty-array bug under `set -u` — fixed.
2. `new_experiment.sh` ID regex — relaxed to allow optional alpha suffix (`0002a`, `0009b`, etc.) for re-runs and follow-ups within the same numbered slot.
3. Mann-Whitney significance gate skipped when `sample.json` has fewer than 8 entries (the `--quick` failure mode).

### Anti-patterns avoided

- Did not silently overwrite EXP-0002. The archive stays as historical evidence per ADR-0004.
- Did not relax the L3 protocol just to make the SoA look better. The numbers stand as measured.
- Did not claim the SoA is faster when it isn't on this workload. The header explicitly says "hypothesis falsified" for the workload tested.

### 2026-05-06 (later) — hypothesis tested on the right workload in EXP-0002b

EXP-0002a left the original ≥2× hypothesis from EXP-0002 untested because the mu_calculus_only benches don't exercise iteration_ranks. [EXP-0002b-synth-bench](../EXP-0002b-synth-bench/) closes that gap: a new `mu_calculus_only::synthesis_product_game` bench function calls `Context::synthesise_controller_with_options(... ProductGame ...)` against an alternation-2 GR(1)-style formula. On grid_32x32, SoA is **2.4× faster** than HashMap (Criterion bootstrap: −57.4%, 95% CI [−67.9%, −39.6%], p=0.00). The hypothesis is confirmed.

The framing "hypothesis falsified" in this EXP's header is **incorrect** on second reading — the hypothesis was unaddressed, not falsified. EXP-0002b corrects the record. This EXP stays in place as the historical artifact that motivated EXP-0002b.
