# EXP-0001-baseline-cliff: pre-optimization baseline freeze

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** (open)
**Commit baseline:** (recorded in manifest.json)
**Commit candidate:** n/a (no optimization applied; this IS the baseline)
**Container digest:** (n/a — host run; subsequent EXPs use the dev container)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. Full manifest in `hw-fingerprint.txt`.

## Motivation

Establish reference numbers for the four isolated benches landed in the foundation sitting:

- `clts_construction.rs` — chain, grid, and random-seeded CLTS construction.
- `composition_only.rs` — sync, async, superset on pre-cached fixtures.
- `minimization_only.rs` — naive Kanellakis-Smolka on chain (already minimal), grid (already minimal), random (redundant).
- `mu_calculus_only.rs` — propositional, reachability (least fixpoint with `<>`), invariance (greatest fixpoint with `[]`).

The plan's inventory cited these subsystems' hot spots. Without numbers for the current state, no later EXP can claim a real improvement.

## Hypothesis

None under test. This is a measurement-establishing experiment.

## Method

- **Inputs:** deterministic fixtures from `crates/mununu-core/src/test_support.rs` keyed by canonical seed `0xC0FFEE`. Templates: `chain(n, alphabet)`, `ring(n, alphabet)`, `grid(w, h)`, `RandomClts::new(seed)`. All RNG uses `rand_chacha::ChaCha20Rng` for cross-platform reproducibility.
- **Bench:** Criterion 0.8 with per-bench config (typically 30-40 samples, 8-10s measurement window). Smoke run used `--quick` (10-sample minimum).
- **Test gate:** `make ci` (cargo fmt + cargo clippy --workspace --all-targets -D warnings + cargo test --workspace). Green at the recording commit.
- **Statistical methodology:** Criterion's median + 95% CI (default reporting); paired t-test for comparison vs candidate runs. Speedups (in subsequent EXPs) reported with Kalibera-Jones bootstrap CI.

## Results (smoke, `--quick`)

See `README.md` headline table and `criterion-archive.tar.zst` for raw JSON. The full-sample run is what `bench_record.sh` archives; the smoke numbers above are an order-of-magnitude sanity check.

## Interpretation

Three observations stand out:

1. **`minimization_only/chain_minimal` at 1.51 s for 1000 states** is the most expensive single bench in the smoke set. Chain CLTSs are already strongly minimal under bisimulation — no states merge — so the K-S loop converges in two passes, and the cost is essentially "scan every state's signature twice." This is the canonical workload for Paige-Tarjan to attack.

2. **`clts_construction/chain/100000` at 556 ms** suggests construction throughput around 180 K states/sec. The 20%-growth wrapper at `clts/mod.rs:46-58` and the staging-buffer flush at `build()` (`clts/mod.rs:1949-1973`) are the suspects per the inventory; EXP-0004/EXP-0005 will validate.

3. **`mu_calculus_only/reachability_mu/grid_32x32` at 109 ms** dominates the mu-calculus benches because reachability traverses the full 1024-state grid via diamond pre-image. This is the workload EXP-0014 (modal pre-image CSR) targets.

## Dead-ends

- *2026-05-06:* Tried to use `s.try_into().unwrap()` to convert state index → `StateId<u32>` in `test_support.rs` tests. `StateId<u32>` does not implement `TryFrom<usize>` — only `from_index(usize) -> Option<Self>`. Switched to the explicit constructor.
- *2026-05-06:* Initial bench code passed `CompositionOptions { ... }` by value; `compose()` takes `&CompositionOptions`. Trivial fix. Noted as a friction point — if a refinement EXP changes `compose()` to take owned options for batched mode, multiple bench files will need to track the change.
- *2026-05-06:* `mununu_core::composition::minimize_bisimulation` is at `composition::minimize::minimize_bisimulation` (sub-module not re-exported at the parent level). Used the qualified path; if a future refactor flattens the module, benches need updating in lock-step.

## Followups

- Open **EXP-0002-iter-rank-soa** as the first optimization (lowest-risk per the plan).
- Add **dhat memory profiling** to `clts_construction/chain/100000` and `minimization_only/chain_minimal` so the EXP-0004 and EXP-0009 follow-ups can quote both wall-clock and allocation counts.
- Open a **soundness regression suite skeleton** at `crates/mununu-core/tests/soundness/` ahead of B1/B5/C1 (per ADR-0002).

## Artifacts

- `criterion-archive.tar.zst` — raw Criterion JSON for the four benches.
- `hw-fingerprint.txt` — `scripts/capture_hw.sh` output at run time.
- `manifest.json` — provenance.
- `command.txt` — replay command.
