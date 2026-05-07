# Free-form observations for EXP-0002-iter-rank-soa

> **SUPERSEDED**: see [EXP-0002a-warmup-rerun](../EXP-0002a-warmup-rerun/). The performance numbers in this archive are cache-warmup-contaminated. Read the "2026-05-06 (later)" section below before citing.

## 2026-05-06 — initial recording (sitting 3)

### Why no feature flag for differential testing?

The original plan (README.md, hypothesis section) called for keeping the HashMap path under `#[cfg(test, feature = "iter_rank_oracle")]` to allow parallel runs of HashMap and SoA implementations on identical inputs. Rejected during implementation (sitting 3) because:

1. The consumer surface is small: 2 reads (signature + ProductGame), 1 write (fixpoint loop).
2. Existing test coverage is dense: 800+ unit tests, 57 doctests, 22 soundness tests (with explicit signature/ProductGame consumers), 5 property tests.
3. A feature flag would force every CI pass to build twice and would muddy the API.

The trade-off is honest: if a regression slips through despite the test surface, we'd have no oracle to reach for. Mitigation: the new `iteration_ranks_deterministic` proptest guards against nondeterminism, which is the most likely class of regression a future SoA refactor (e.g., parallel write reduction) would introduce.

### Measurement contamination

EXP-0001's baseline numbers were collected as smoke runs during scaffolding (sitting 1), before:
- The binary cache was warm.
- The fixtures were in `target/test-fixtures/`.
- The test_support feature path had been exercised.

EXP-0002's numbers were collected after the binary had been built repeatedly (sitting 2 + sitting 3 dev cycles). The compile/cache state is not the same. Therefore the apparent 5-7× ratios should be read as "≤7× upper bound, likely much smaller true SoA contribution; use EXP-0002-deep for a citable number."

This is exactly the kind of issue the iteration policy in `notebook/REFINEMENT.md` was designed to handle: open EXP-0001-deep + EXP-0002-deep as superseding archives, document the methodology improvement, leave the original EXP-0001 and EXP-0002 in place as historical evidence.

### What I'm confident about

- **No soundness regression.** The 22-test soundness suite at `tests/soundness.rs` exercises ProductGame controller synthesis (which reads iteration_ranks via signature), parity-game synthesis (via the alternative path), and signature-based functional strategy. All green after the SoA swap.
- **No correctness regression.** All 800+ unit tests + 57 doctests + 5 proptests green.
- **API faithful 1:1.** The HashMap → IterationRanks substitution preserves "absent → MAX" semantics; consumers required only the call-site changes documented in the README.

### What's deliberately NOT measured here

- **Memory delta via dhat.** The `dhat` feature exists; no bench is instrumented yet. A future EXP-0002-mem will instrument `tests/soundness.rs::signature_functional_strategy_gr1` (or a stress test built specifically for synthesis-heavy workloads) and report peak heap before/after.
- **Cache miss / branch mispredict.** Out of scope for `--quick` Criterion. EXP-0002-deep should be run with `cargo flamegraph` or `Instruments` attached to confirm the SoA layout actually delivers the locality win we hypothesised.

### Anti-patterns avoided

- Did not try to make iteration_ranks hold both a HashMap and a SoA "for safety." One source of truth.
- Did not introduce `unsafe` for the lazy-resize logic. `Vec::resize_with(n, Vec::new)` and `row.resize(state_count, u32::MAX)` are safe and unobservable in the hot path.
- Did not pre-allocate all rows up front. Rows lazily allocate on first write; non-recorded variables stay at zero memory cost. This matches the access pattern (most fixpoint variables are only entered by a subset of states).

### Iteration policy actions taken

- Used `--fresh` on `bench_record.sh` so the criterion-archive contains only this run, unlike EXP-0001 which had multi-subsystem residue.
- The `.draft` marker was cleared automatically by `bench_record.sh` on successful recording (per ADR-0004's policy).
- Schema v1 manifest validates green via `scripts/check_repro.sh`.
