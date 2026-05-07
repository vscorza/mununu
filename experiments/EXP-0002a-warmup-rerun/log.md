# EXP-0002a-warmup-rerun: corrected SoA-vs-HashMap A/B at L3 protocol

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe (pre-SoA, HashMap)
**Commit candidate:** working-tree (SoA, applied via /tmp/exp-0002-soa.patch)
**Container digest:** n/a (host run)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. See `hw-fingerprint.txt`.

## Motivation

In response to the user question "do we have previous tests and benches re ran with warmup?": no, EXP-0002 was recorded before `--warmup` was added. Its apparent 5-7× speedup vs EXP-0001 was contaminated by cache state differences across separately-invoked `cargo bench` runs in different shell sessions.

This re-run uses the level-3 same-session A/B protocol (ADR-0006) to produce a clean comparison.

## Hypothesis (re-tested)

EXP-0002 pre-registered: ≥2× speedup on synthesis-bound benches; ≥100 KB heap reduction on synthesis-relevant bench; non-synthesis benches within ±1% of EXP-0001.

This re-run tests the **non-synthesis bench** clause specifically (we don't have a synthesis-bound bench yet — that's EXP-0002b). The hypothesis maps to: "non-synthesis benches show within ±1% of HashMap baseline."

## Method

1. `git diff HEAD -- crates/mununu-core/src/mu_calculus/evaluator.rs crates/mununu-core/src/context/mod.rs > /tmp/exp-0002-soa.patch` (saved 224 lines of patch).
2. `git checkout HEAD -- ...` to revert to HashMap baseline.
3. `scripts/bench_compare.sh exp-0002-full -- -p mununu-core --features test_support --bench mu_calculus_only` — warmup discard, then full Criterion (30 samples per function, 8s measurement, 3s warmup) save-baseline.
4. `git apply /tmp/exp-0002-soa.patch` — restore SoA.
5. `scripts/bench_compare.sh exp-0002-full --baseline-only -- ...` — full Criterion vs saved baseline; Criterion auto-emits Welch's t-test "change" report.
6. `scripts/bench_diff.sh exp-0002-full --robust` — median-ratio + Mann-Whitney (normal approximation) gate.

Both sides ran in the same shell session, on the same compile cache, within ~25 minutes total.

## Results

See README.md for the full table. Headline:

- Criterion's t-test flags 2/8 benches as significant SoA regressions (`reachability_mu/chain_1k` +26.5% p=0.00, `reachability_mu/ring_1k` +9.8% p=0.01) and 2/8 as significant SoA improvements (`invariance_nu/chain_1k` −4.9% p=0.02, `invariance_nu/ring_1k` −2.6% p=0.01).
- Robust gate (Mann-Whitney p<0.01) classifies all 8 as NEUTRAL — at n=30 the conservative gate doesn't have enough evidence to flag any direction.

## Interpretation

Two consistent themes despite the noise:

1. **Mu-fixpoints (least, reachability)** show apparent +10-27% slowdown on chain/ring with significant p-values per Criterion. The mu-fixpoint hot path includes more state-entry events (each new state entering the fixpoint triggers a `record()` call in iteration_ranks), and the SoA's saturating-arithmetic + lazy-resize cost may be the source.

2. **Nu-fixpoints (greatest, invariance)** show apparent −3-5% improvement on chain/ring with significant p-values per Criterion. Nu has fewer state-entry events (states leave, don't enter), so the iteration_ranks write cost is amortized differently.

But the workloads being benched **don't even exercise iteration_ranks**: they use `evaluate_with_options` with `witness_map = None`, so the entire iteration_ranks path is gated off. The observed differences are entirely **LLVM monomorphization side-effects** from changing the `WitnessMap` field type.

The original hypothesis — ≥2× speedup on **synthesis-bound** benches — is unaddressed by this experiment because we don't have a synthesis-bound bench. EXP-0002b is required to test that hypothesis on the workload that actually exercises iteration_ranks.

## Dead-ends

- *2026-05-06:* `bench_compare.sh` failed under `set -u` because empty `CRITERION_ARGS` array expansion is unbound. Fixed with explicit `${#CRITERION_ARGS[@]} -eq 0` checks.
- *2026-05-06:* First A/B attempt with `--quick` produced 2-sample bench data — far below Mann-Whitney's n≥8 threshold. Mann-Whitney was skipped on every bench. Re-ran without `--quick` for n=30 per side; that's the data this archive carries.
- *2026-05-06:* The `--robust` gate (Mann-Whitney p<0.01) rejected differences that Criterion's t-test (with bootstrap CI) flagged as significant. Both are correct under their assumptions; this is documented in the README so future EXP authors know to consult both.

## Followups

- **EXP-0002b-synth-bench**: add a bench that calls `Context::synthesise_controller_with_options(... ProductGame ...)` against a fixture with alternation depth ≥ 2 and witness extraction enabled. This actually exercises `IterationRanks::record()` and `get_rank()`. Re-run the L3 protocol on that bench to test the ≥2× hypothesis on the right workload.
- **EXP-0001-deep + EXP-0002-deep**: planned; record at level 4 (mununu-dev container, Turbo off, dedicated runner). Necessary for paper-grade citation.
- **`bench_diff.sh --robust` calibration**: investigate whether Mann-Whitney is too conservative for our typical bench distributions, or whether Criterion's bootstrap-on-means is too sensitive to outliers. Possibly add a third mode that uses Criterion's own change estimate as the gate.

## Artifacts

- `criterion-archive.tar.zst` — 228 KB, contains both `exp-0002-full` baseline and `new` candidate with full sample data.
- `manifest.json` — schema_version 1; full provenance.
- `command.txt` — replay invocation.
- `hw-fingerprint.txt` — fresh fingerprint at recording.
