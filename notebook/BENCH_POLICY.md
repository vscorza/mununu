# Bench execution policy

## TL;DR

- **CI never runs benches.** `make ci` = `lint + test`. GitHub Actions calls `make ci` (and `cargo test --features api`); no workflow invokes `cargo bench`.
- **Benches require an explicit feature.** The four EXP-anchored benches (`clts_construction`, `composition_only`, `minimization_only`, `mu_calculus_only`) have `required-features = ["test_support"]` in `crates/mununu-core/Cargo.toml`. Without that feature on the cargo invocation, cargo skips those targets even with `--all-targets`.
- **Existing benches** (`clts_composition`, `mu_calculus`, `controller`, `xstate`) have no required features and will compile under `cargo build --benches`, but still don't *run* unless invoked via `cargo bench`.

## How to run a single bench

```bash
# Local development, fast feedback (Criterion --quick = 10-sample minimum):
cargo bench -p mununu-core --features test_support --bench mu_calculus_only -- --quick

# Full Criterion run with default 100 samples + 3s warmup:
cargo bench -p mununu-core --features test_support --bench minimization_only

# Filter to a specific function within a bench:
cargo bench -p mununu-core --features test_support --bench mu_calculus_only -- reachability_mu

# Save a baseline for later comparison:
cargo bench -p mununu-core --features test_support --bench minimization_only -- --save-baseline EXP-0009-pre

# Compare against a saved baseline:
cargo bench -p mununu-core --features test_support --bench minimization_only -- --baseline EXP-0009-pre
```

## How to record an experiment archive

```bash
scripts/bench_record.sh --fresh EXP-NNNN-<slug> \
    -p mununu-core --features test_support --bench <bench-name>
```

`--fresh` clears `target/criterion/` first so the archive contains only what was just measured. Without `--fresh`, accumulated results from prior runs end up in the archive — fine during iterative development, problematic for paper-grade evidence.

## Why benches don't run in CI

Three reasons:

1. **Wall-clock cost.** A full Criterion run of all four `_only` benches takes ~30-90 minutes depending on samples. PR latency would be unacceptable.
2. **Hardware variance.** GitHub-hosted runners are noisy (shared kernels, varying CPU types, no isolation). Numbers from `ubuntu-latest` are not paper-citable. The reproducibility contract requires a dedicated runner; benches run there.
3. **Determinism risk.** Criterion auto-detects "regression" or "improvement" based on Welch's t-test against a saved baseline. Without a stable baseline + stable hardware, false positives are routine and would flap CI.

The plan's E10 (`notebook/0000-overview.md` referenced) calls for:
- **PR check** (today): lint + test, no benches.
- **Nightly** (planned): + stress + cargo-fuzz + property tests at 4096 cases. Still no benches in this tier.
- **Weekly/release** (planned): + replay every archived EXP on a dedicated runner; this IS where benches run, with full provenance comparison via `scripts/bench_diff.sh`.

## How to add a new bench without breaking CI

1. Place the file at `crates/mununu-core/benches/<name>.rs`.
2. Register in `crates/mununu-core/Cargo.toml` with `harness = false` and `required-features = ["test_support"]` (or another opt-in feature).
3. Document the EXP it anchors in `notebook/decisions.md` if it's load-bearing for a paper claim.
4. Add fixtures via `crates/mununu-core/src/test_support.rs` and `crates/mununu-core/src/bench_support.rs::fixtures` so the bench doesn't pay construction cost.
5. Smoke with `cargo bench -p mununu-core --features test_support --bench <name> -- --quick` before committing.

## Regression mitigation

Cache state, page faults, and binary-warmup costs differ between `cargo bench` invocations even on the same hardware with the same code. Naively comparing two separate runs can show false 5-10× speedups that disappear once both binaries are warm. The EXP-0001 vs EXP-0002 ratios in their respective archives (5-7× apparent) are exhibit A.

Mitigation protocol — apply at increasing rigor levels depending on the claim's importance:

### Level 1 — Smoke / iterative development

`cargo bench --quick`. Numbers are indicative, not citable. Use during local optimization work.

### Level 2 — EXP recording with warmup

```bash
scripts/bench_record.sh --fresh --warmup EXP-NNNN-<slug> \
    -p mununu-core --features test_support --bench <name>
```

`--warmup` runs the bench once at `--quick` and discards the result before the real recording. This pays the page-fault, mmap-load, and binary-cache costs that the first measurement would otherwise absorb. Adds ~30-60 seconds; produces measurably more comparable numbers when the same binary is benched repeatedly.

### Level 3 — Same-session A/B compare

```bash
# A side: save baseline (this includes a warmup by default)
scripts/bench_compare.sh exp-0009-pre -- -p mununu-core --features test_support \
    --bench minimization_only

# ... apply the patch ...

# B side: compare against the saved baseline (warmup is intrinsic to back-to-back execution)
scripts/bench_compare.sh exp-0009-pre --baseline-only -- -p mununu-core \
    --features test_support --bench minimization_only

# Robust regression gate (ignores statistically insignificant differences)
scripts/bench_diff.sh exp-0009-pre --robust
```

Same-session execution keeps the OS scheduler, thermal state, and binary mmap regions in roughly identical conditions across A and B. This is the recommended protocol for any blog-worthy speedup claim made on a developer laptop.

### Level 4 — Dedicated runner with environmental controls

Required for paper-grade evidence (per `notebook/0000-overview.md` reproducibility contract):

- Plug the laptop in (no battery / power-save throttling).
- Disable Turbo Boost / CPU frequency scaling: `sudo sysctl -w machdep.xcpm.mpsafe_idle=0` on Darwin or `cpupower frequency-set --governor performance` on Linux.
- Pin to a specific core: `taskset -c 4` on Linux. macOS lacks core pinning; document the runner instead.
- Run inside `mununu-dev` container so OS kernel + glibc are constant across machines.
- Record both A and B sides on the same physical runner within minutes of each other.
- Use `scripts/bench_diff.sh --robust` so the regression gate uses Mann-Whitney U test (p<0.01) in addition to the median-ratio threshold.

EXP-0001-deep is a planned follow-up that re-records EXP-0001 baseline at level 4 inside the dev container; EXP-0002-deep does the same for EXP-0002 against EXP-0001-deep's baseline. Until both ship, the EXP-0001 vs EXP-0002 ratios in the current archives are flagged as smoke-comparable only.

### Statistical robustness

`scripts/bench_diff.sh --robust` reads Criterion's per-iteration sample data (`target/criterion/<bench>/<run>/sample.json`) and computes a Mann-Whitney U test p-value alongside the median ratio. A regression is flagged only if BOTH:

1. Median exceeds the threshold (default ±10%), AND
2. Mann-Whitney p-value < 0.01.

This avoids false positives from bimodal/long-tailed distributions caused by intermittent CPU frequency drops, GC pauses, or other-process noise. The Mann-Whitney implementation is normal-approximation-based and assumes ≥ 8 samples per side; below that, the test is skipped and only the median ratio is consulted.

### What this still doesn't cover

- **Inter-machine portability.** Two machines with the same `make ci` green can disagree on absolute timings by 2-3×. Always cite the `hw-fingerprint.txt` from the EXP archive.
- **Long-tail noise.** Some pathological bench inputs are bimodal even on a quiet runner (e.g., partition-refinement convergence in 2 vs 5 iterations). Increase `sample_size` in the bench config to 200+ for these.
- **Thermal throttling on long runs.** Multi-bench batches over 20+ minutes can show cumulative degradation. Split into multiple `bench_record.sh` invocations with 5-minute idle gaps.

## Anti-patterns

- **Adding a bench without `required-features`.** Cargo will then attempt to compile it during `cargo clippy --all-targets`, slowing CI for no benefit.
- **Running `cargo bench` in pre-commit.** Pre-commit must stay sub-60s; benches break that contract. Use `scripts/bench_record.sh` ad-hoc instead.
- **Comparing wall-clock numbers across machines.** All EXP archives carry an `hw-fingerprint.txt`; comparing across hardware requires explicit acknowledgment in the EXP `notes.md`.
- **Letting `target/criterion/` accumulate across EXPs.** Use `--fresh` on `bench_record.sh`. The EXP-0001 archive has multi-subsystem residue; it's documented but not the pattern to follow.
