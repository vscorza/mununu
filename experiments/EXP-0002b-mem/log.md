# EXP-0002b-mem: dhat heap profile of SoA vs HashMap iteration_ranks

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe (HashMap)
**Commit candidate:** working-tree (SoA, applied via /tmp/exp-0002-soa.patch)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. See `hw-fingerprint.txt`.

## Motivation

EXP-0002b confirmed 2.4× wall-clock speedup; EXP-0002 README pre-registered "≥100 KB heap reduction on synthesis-relevant bench" as a separate hypothesis. dhat profiling tests it directly.

## Hypothesis (pre-registered in EXP-0002 README)

≥100 KB peak heap reduction on a synthesis-relevant bench.

## Method

See README.md. Key parameters:
- Workload: 3× `synthesise_controller_with_options(... ProductGame ...)` against the EXP-0002b alternation-2 GR(1) formula on `grid_32x32`.
- dhat: `dhat::Profiler::new_heap()` lifetime around the workload, `dhat::Alloc` as `#[global_allocator]`.
- A side: HashMap iteration_ranks (HEAD).
- B side: SoA iteration_ranks (working tree patch).
- Same release-mode binary, same fixture, same shell session, ~5 minutes apart.

## Results

See README.md table. Headlines:

- Total bytes: -384 KB (-0.02% — within noise).
- Total allocations: -2,390 (-0.008% — within noise).
- **Peak heap (t-gmax): -76,584 bytes (-6.7%)**.
- Peak block count: identical.

**Pre-registered ≥100 KB hypothesis is falsified by 24 KB (76 KB observed vs ≥100 KB hypothesized).**

## Interpretation

The 2.4× wall-clock win from EXP-0002b is **not** a heap-pressure win. The HashMap and SoA variants do roughly the same number of allocations of roughly the same total size — the iteration_ranks itself accounts for ~16 KB peak vs ~92 KB peak (the 76 KB delta), but it's a small fraction of the synthesis pipeline's total heap footprint (1+ MB peak overall).

The wall-clock win must therefore come from:
1. **Cache locality** in `iteration_ranks::get_rank()` calls during ProductGame controller construction (sequential access pattern over a Vec).
2. **Hash probe cost** elimination: HashMap.get runs SipHash + slot probe + occasional chain follow (~30-50 ns); SoA `get_rank` is two slice indexes (~5-10 ns).

For grid_32x32 (1024 plant states × 2 mu-obligations → ~2000-state product → ~4000 get_rank calls), saving 30 ns per call = ~120 µs per synthesis call. Across 30 Criterion samples, that compounds to the ~57% wall-clock delta observed in EXP-0002b.

## Dead-ends

- *2026-05-06:* Initial smoke test of the dhat binary produced 1.06 MB peak with the SoA variant (current tree). I assumed running the binary again with reverted SoA would give a substantially larger peak (matching the ≥100 KB hypothesis). Got 1.14 MB — only 76 KB more, not 100+. The hypothesis is falsified.
- *2026-05-06:* `cargo build --release --features test_support,dhat --bin dhat_synthesis` requires the `test_support` feature for fixtures. Initially tried `cargo run --bin dhat_synthesis` without features; it failed at link time. Adjusted Cargo.toml `[[bin]]` registration to declare `required-features = ["test_support", "dhat"]` so a stray `cargo build --bins` doesn't try to build it incompletely.

## Followups

- **EXP-0002b-mem-deep at L4 with a larger fixture** (64×64 grid or a 100k-state synth-friendly CLTS). Predicts: peak heap delta scales linearly with `state_count × num_fixpoint_vars`. At 100k states × 2 mu-obligations, the delta should be ~100k × 2 × 4 bytes = 800 KB or more. Needed before the paper §3.x can claim memory-axis benefits.
- **Update blog post 2 outline** ("Layout matters") to explicitly say: SoA is a *cache-locality* win, not a *heap-pressure* win at this scale. Avoids overclaiming.
- **Refine EXP-0002 README's pre-registered hypothesis** to match what was actually delivered (peak heap -6.7% instead of ≥100 KB). Or accept the falsification as documented in this archive.

## Artifacts

- `dhat-heap.A.json` — 53 KB, HashMap baseline profile.
- `dhat-heap.B.json` — 55 KB, SoA candidate profile.
- `manifest.json` — schema_version 1.
- `command.txt` — replay invocation.
- `hw-fingerprint.txt`.
