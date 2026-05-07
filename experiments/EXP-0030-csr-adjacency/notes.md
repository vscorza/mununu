# Free-form observations for EXP-0030-csr-adjacency

## 2026-05-06 — fourth falsification, refines the taxonomy

### What this archive proves

- CSR adjacency via staging-flatten is a build-time regression (12-65% on clts_construction). The runtime savings on the consumer side don't compensate.
- The §A3 design itself isn't necessarily wrong; this *implementation* of it is. A from-scratch direct-CSR-build (EXP-0031, not scheduled) would skip the flatten and might deliver.

### What this archive does NOT mean

- All structural changes are bad. EXP-0002b SoA and EXP-0004 Vec doubling are still confirmed wins. This experiment specifically falsifies CSR-with-staging.
- §A3 should be removed from the plan. The design is sound; the implementation path (via staging flatten) is what's broken.

### The taxonomy refinement

Before EXP-0030, the rule was: "structural changes win, drop-ins lose."

After EXP-0030, the refined rule: **changes that reduce total work win; changes that add work (whether drop-in or structural) lose.**

Specifically:
- **EXP-0002b SoA**: HashMap → Vec<Vec<u32>>. Lazy init. Total work down. ✅
- **EXP-0004 Vec doubling**: removed wrapper. Total work down. ✅
- **EXP-0010 FxHashMap**: same work, faster hasher. Hasher wasn't the bottleneck. ❌
- **EXP-0003 LabelSetTable**: extra find_label_set probe before existing probe. Total work UP. ❌
- **EXP-0012 track-during-merge**: replaced early-exit compare + memcpy with serial word-loop. Total work UP at small scales. ❌
- **EXP-0030 CSR-via-staging**: kept staging build, added flatten. Total work UP. ❌

The pattern: every falsification involved adding work (extra probe, extra branch, extra copy). Every confirmation involved eliminating work (lazy init replacing eager allocation, native growth replacing custom growth).

### Why the lone improvement on random_seeded/256 (-15.8%)

random_seeded/256 has 256 states × ~6500 edges (density 0.10). The CSR build cost is dominated by:
- Random number generation (~1 ms).
- Label set interning (still happens regardless of CSR).
- The flatten pass: 6500 × 24 bytes = ~156 KB (one cache region).

For tiny workloads, the cache locality of the flat representation might win on subsequent access patterns within the same bench iteration. As the workload grows (1024 states = ~104k edges = 2.5 MB), the flatten cost dominates.

This single bench shouldn't be over-interpreted — n=40, p=0.000 is statistically robust, but it might also be noise from cache state in a single hot run. Without dedicated-runner reproduction, can't be sure.

### Methodology lessons

- Running L3 across THREE benches (build, compose-builds, modal-eval-consumes) was useful: each isolates a different cost model. The build-bench showed the regression most clearly; the consumer-bench showed it's not made up by accessor wins.
- The "structural wins" intuition needs the "no extra setup work" caveat. Pure structural-change isn't sufficient; the change must also reduce total work (or move it to a phase where it doesn't matter, like one-time vs per-call).

### Iteration policy actions

- Used `--fresh` on bench_compare via separate baseline names per bench (clts, comp, mu).
- Following ADR-0004: archive stays. Reverted code is in next commit.
- Pre-EXP-0030 patch saved at /tmp/exp-0030-csr.patch for replay; the manual re-creation of csr.rs in B-side rebuild was needed because `git apply` of the saved patch failed due to context drift.

### Anti-patterns avoided

- Did not declare the lone -15.8% as a "partial win." On 7 benches, 1 improvement is consistent with multiple-comparison noise; the headline is the regression on 6/7.
- Did not blame "scale" — the regressions span chain/1000 to random/1024, all moderate-to-small workloads. Larger fixtures might amortize the flatten, but until a from-scratch CSR build is tried, the verdict is "broken at all tested scales."
- Did not skip the consumer benches just because construction was clearly broken. Both axes matter.

## 2026-05-07 — Corrigenda (per EXP-0011 methodology finding)

The `composition_only` benches used in this experiment (`bench_chain_sync`, `bench_grid_async`, `bench_modes_compare`) compose CLTSs whose controllable alphabets overlap (`chain_1k`/`ring_1k`/`grid_32x32` fixtures share controllable labels). `composition::compose()` rejects shared-controllable-alphabet pairs in validation, so those benches were measuring the validation **error path**, not real composition.

The falsification *direction* of this experiment still stands — a slower error path is still a slower path, and the L3 protocol caught a real regression — but the **magnitudes do not reflect real compose cost**. EXP-0011 fixed the bench by switching to `RandomClts::new(seed).with_uncontrollable_prefix(3)` so labels can be shared across the uncontrollable prefix; the resulting compose call takes ~6 ms on 32-state inputs (vs the ~5–10 µs error-path numbers reported here).

This archive is not being re-opened. Future composition-touching EXPs should use the EXP-0011-style fixture.
