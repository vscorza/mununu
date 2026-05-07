# Free-form observations for EXP-0001-baseline-cliff

## 2026-05-06 — initial recording

### Why a "smoke" set of numbers in `README.md` if the archive carries the full Criterion data?

Two reasons. (1) The README needs to be skimmable in 30 seconds; pasting the full Criterion JSON would defeat that. (2) The smoke numbers were collected via `--quick` (Criterion's 10-sample fast mode) during scaffolding to verify the benches even ran. The `criterion-archive.tar.zst` carries the full default-sample run, which is what blog and paper figures will draw from. If the smoke numbers and the full-archive numbers disagree by more than the smoke run's confidence interval, we have a measurement reproducibility problem worth investigating.

### What's deliberately *not* measured here

- **Memory.** dhat is wired in via Cargo feature but not yet instrumented in any bench. Followup: add `dhat::Profiler::new_heap()` blocks around the heaviest benches in EXP-0001-mem (or supersede this EXP).
- **Cache misses, branch mispredicts.** `perf stat`/`Instruments` numbers belong in a separate EXP; criterion only measures wall-clock. Worth attaching to the EXPs that target memory-layout changes (A3 CSR, A5 field reorder).
- **Workspace-level operations.** Composition + minimization end-to-end is what `clts_composition.rs` (the existing, non-isolated bench) measures. We deliberately keep that around as the pipeline-level tracker; isolated benches are the per-subsystem signal.

### Why the random density is 0.10 here but 0.20 in the cached `random_512_d20` fixture

The construction bench uses a *new* `RandomClts::build()` each iteration so the call is what's being measured. Density 0.10 keeps that inner loop fast enough to bench at sample-size 40. The `random_512_d20` cached fixture is only loaded by the minimization bench, where density 0.20 produces enough redundancy for some merges to actually happen — that's the workload we want for measuring the merge path. Both fixtures use seed `0xC0FFEE`; if a future refinement re-seeds either, the EXP that records new numbers must supersede this one (per `notebook/REFINEMENT.md`).

### Reproducibility caveats

- Run was on a host laptop, not in the dev container. Subsequent EXPs should run inside `mununu-dev` (per CLAUDE.md). The `manifest.json` records this (`container: "no"`); future EXP-0001 re-records at `container: "yes"` will be a separate archive (EXP-0001-deep) so we can compare host vs container.
- Power state was not pinned — the laptop was on battery during the smoke run. For final paper-grade numbers, plug in and disable Turbo Boost; mark in `manifest.json` once we wire that into `capture_hw.sh`.
- macOS has no governor knob equivalent to Linux `cpufreq`; `capture_hw.sh` notes "powermetrics available" but doesn't sample. If the paper reviewer asks for governor confirmation, attach a `sudo powermetrics --samplers cpu_power -i 1000 -n 5` log.

### Archive provenance quirk (2026-05-06)

The `criterion-archive.tar.zst` contains data for **all four subsystems**, not just `clts_construction` named in `manifest.json`'s `command` field. This is because `target/criterion/` accumulates results across `cargo bench` invocations, and the smoke runs (composition_only, minimization_only, mu_calculus_only at `--quick`) preceded the recorded `clts_construction --quick` run. The archive snapshot captures everything in `target/criterion/` at archive time.

For EXP-0001 baseline this is **acceptable**: the smoke runs and the recorded run were both `--quick`, both on the same host, both within minutes of each other; the numbers are coherent. The README headline table cites smoke numbers because they're representative of the archive contents.

For paper-grade EXPs starting at EXP-0002, use `scripts/bench_record.sh --fresh <EXP-ID> ...` so the archive contains only what was just measured. The `--fresh` flag clears `target/criterion/` before the run. Added in the same sitting as EXP-0001 in response to this quirk.

A future **EXP-0001-deep** will re-record with `--fresh`, full Criterion samples (no `--quick`), inside the `mununu-dev` container, with Turbo Boost disabled, and supersede this archive for paper-grade citation.

### Anti-patterns avoided

- Not editing the existing `clts_composition.rs` bench. It stays as the pipeline-level tracker; the new isolated benches add coverage rather than replace.
- Not re-baselining the existing `mu_calculus.rs` bench fixture. It uses different labels (`tick`, `sync`, `ack`) and predicates (`safe_pred`); not directly comparable to `mu_calculus_only.rs`. Both stay; both produce numbers.
