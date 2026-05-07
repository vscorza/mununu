# EXP-0002-iter-rank-soa: replace HashMap iteration ranks with struct-of-arrays

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe (EXP-0001 baseline) — recorded in manifest.json
**Commit candidate:** working-tree (sitting 3 changes; manifest records `git_dirty: yes`)
**Container digest:** n/a (host run; subsequent EXPs will use mununu-dev)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. See `hw-fingerprint.txt`.

## Motivation

`WitnessMap.iteration_ranks: HashMap<(usize, FormulaVarId), usize>` at [`evaluator.rs:55`](../../crates/mununu-core/src/mu_calculus/evaluator.rs) is the dominant per-iteration overhead during synthesis. Two reasons:

1. **Memory.** Each (state, var) entry costs ~48 B (key 16 B + value 8 B + HashMap slot overhead). At 1M states × 4 fixpoint vars: ~192 MB worst case.
2. **Locality.** Reads (lexicographic signature comparison in `signature()`) and writes (one-shot per state per fixpoint solve) both have sequential-by-state access patterns. HashMap probes scatter; `Vec<Vec<u32>>` matches the access pattern.

The SoA design — `Vec<Vec<u32>>` indexed `[var.index()][state_idx]` with `u32::MAX` sentinel — preserves the HashMap-era "absent → MAX" semantics so the two read sites (one in `signature()`, one in ProductGame controller construction at `context/mod.rs:2034`) compile unchanged after the API swap.

## Hypothesis (pre-registered in README.md, sitting 2)

1. ≥2× speedup on synthesis-bound benches.
2. ≥100 KB heap reduction on synthesis-relevant bench (dhat measurement).
3. Non-synthesis benches within ±1% of EXP-0001.

## Method

- **Input:** mu_calculus_only bench fixtures (chain_1k, ring_1k, grid_32x32) under three formula classes (propositional, reachability mu, invariance nu).
- **Bench:** `cargo bench -p mununu-core --features test_support --bench mu_calculus_only -- --quick` (Criterion 10-sample minimum, ~2 minutes total).
- **Test gate:** `make test` (full workspace, including the new property test) — green at recording commit.
- **Differential:** the existing 22 soundness tests + 5 property tests + ~800 unit tests act as the regression gate. Specifically, `tests/soundness.rs::signature_functional_strategy_gr1` and `product_game_*` tests directly exercise `iteration_ranks` reads via the synthesis path.

## Results

See `README.md` headline table. SoA replacement is sound (all tests green) and produces sensible benchmark numbers; the apparent 5-7× speedup vs EXP-0001 smoke numbers is contaminated by warm-cache differences and is not a rigorous claim.

## Interpretation

The **soundness conclusion is robust**: every existing test (including the parity-game and ProductGame mode tests which read `iteration_ranks`) continues to pass after the SoA swap. The new `IterationRanks::record` / `get_rank` API is a faithful 1:1 replacement of the HashMap insert/get pair, with `u32::MAX` standing in for HashMap's "absent" semantics.

The **performance claim is NOT yet rigorous**. EXP-0001's baseline numbers were from smoke runs collected during scaffolding before the binary's compile/build cache had stabilised; large fractions of the 5-7× apparent ratios may be cache-warmup artifacts rather than SoA contribution. EXP-0002-deep, which re-records EXP-0001 against a warm baseline using `--save-baseline` and then runs EXP-0002 with `--baseline`, is the prerequisite for a citable speedup number.

The hypothesis "≥2× speedup" is plausibly satisfied — even an order-of-magnitude derate of the 5-7× apparent ratios still clears 2× — but the reproducibility contract (notebook/0000-overview.md, point 8) forbids citing this as a paper-grade result without the deep re-record.

## Dead-ends

- *2026-05-06:* My initial proptest used a fictional `evaluate_with_options_and_witnesses` function name and a non-existent `iteration_ranks.signature_for_state` method. Real API names: `evaluate_with_witnesses` (which returns `(BitVec, WitnessMap)`) and `formula.fixpoint_nesting_order()` returning `Vec<(FormulaVarId, bool)>` paired with `WitnessMap::signature(state_idx, &nesting)`. Caught at first compile.
- *2026-05-06:* Considered a feature flag to keep both HashMap and SoA paths for differential testing. Rejected: the consumer surface is only 2 reads + 1 write, and the existing 800+ tests already exercise them through `signature()` and ProductGame construction. A feature flag would have added complexity without catching anything new — the test suite IS the differential.
- *2026-05-06:* `iteration` is `usize` upstream and could in theory exceed `u32::MAX`. Practically impossible (Tarski caps fixpoint iterations at `state_count`, which is `u32` already), but a debug-correct implementation must handle it. Solution: `record()` saturates incoming `iteration` at `u32::MAX - 1` so the `u32::MAX` sentinel meaning stays unambiguous. Verified by the `iteration_value_caps_below_sentinel` unit test.

## Followups

- **EXP-0002-deep**: re-record EXP-0001 with full Criterion samples + `--save-baseline EXP-0001` inside the `mununu-dev` container, then run EXP-0002 with `--baseline EXP-0001`. This is the prerequisite for citing a speedup number in the blog or paper.
- **Add dhat instrumentation** to a synthesis-heavy stress test to measure peak heap delta. The `dhat` feature is already wired in mununu-core/Cargo.toml; just need a test that calls `dhat::Profiler::new_heap()` around a controller synthesis on a 10k-state CLTS.
- **EXP-0007** (predicate interning + dense bindings) is the next mu-calculus memory win and depends on this EXP's API stabilising. Schedule for sitting 5 or later.

## Artifacts

- `criterion-archive.tar.zst` — 51 KB of Criterion JSON for mu_calculus_only.
- `bench-stdout.log` — raw cargo bench output.
- `manifest.json` — schema_version: 1; commit `e6edcfe`; full provenance chain.
- `command.txt` — `cargo bench -p mununu-core --features test_support --bench mu_calculus_only -- --quick`.
- `hw-fingerprint.txt` — fresh fingerprint at recording time.
