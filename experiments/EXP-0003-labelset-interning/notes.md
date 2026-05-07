# Free-form observations for EXP-0003-labelset-interning

## 2026-05-06 — second drop-in falsification

### What this archive proves

- Plan §A2 ("drop the SmallVec key, use a u32 LabelSetId") is empirically a regression at this implementation. Compositional wall-clock +18-85%, synth +3-5%.
- The assumption that the consumer already has a LabelSetId in hand is wrong. The consumer builds SmallVecs of uncontrollable subsets fresh per transition; the SmallVec hash dominates regardless of map key shape.
- The `LabelSetTable` infrastructure compiles, all tests pass, but the bench numbers are negative.

### What this archive does NOT mean

- LabelSetTable interning is universally bad. A redesign that caches LabelSetId on Transition (avoiding consumer-side SmallVec construction) might still win. EXP-0003 falsifies the specific drop-in pattern, not the underlying interning idea.
- The plan's §A-series items are unreliable. EXP-0002b (§A1) and EXP-0004 (§A4) are confirmed wins. EXP-0003 (§A2) is falsified. Mixed-bag suggests bench-first methodology is the right gate.

### Two-falsification pattern

Three "drop-in" hypotheses now tested:
- EXP-0010 §B2 (FxHashMap on composition HashMaps): falsified, +30-60% regression.
- EXP-0003 §A2 (LabelSetId on uncontrollable_groups): falsified, +18-85% on composition, +3-5% on synth.

Both share the failure mode: the optimization changes the wrapper, not the access pattern. The actual workload spends its time elsewhere (RefCell+Rc churn, SmallVec construction, fixture iteration). Hash-speed optimizations on small maps don't move the needle.

Confirmed wins so far have been:
- EXP-0002b (§A1, SoA iteration_ranks): 2.4× on synth, p<0.001.
- EXP-0004 (§A4, drop 20% growth wrapper): 1.06-1.45× on construction, p<0.001.

The pattern: **structural changes to data layout / growth strategy win; hash-key swaps lose.**

### Methodology lessons

- The L3 protocol caught both EXP-0003 and EXP-0010 cleanly. The test suite alone would have missed both (both pass functionally).
- Two benches (composition + synth) gave complementary signals: composition revealed the build-time overhead; synth revealed the consumer-time overhead. Single-bench comparisons would have under-stated the regression.
- Patch management for L3 with multiple stacked changes (SoA + EXP-0004 + EXP-0003) is awkward. `git apply --include='*specific.rs'` works but is fragile. Future sittings should use feature flags instead of patch swapping where possible.

### Iteration policy actions

- Used `--fresh` on bench_compare.
- Both benches archived (composition + synth) — reproducibility contract requires complete archives.
- Following ADR-0004: archive stays in place as the falsification record. Reverted code change is in next commit.

### Anti-patterns avoided

- Did not try to find a workload where the regression looks smaller. The numbers stand as measured on the workloads pre-registered for EXP-0003.
- Did not retro-rationalize the regression as "actually intentional." The hypothesis was wrong; the experiment falsified it; we revert and move on.
- Did not skip the synth bench because composition was already a clear regression. Both benches matter; running only one would leave open the question of whether the synth-side win compensates. It doesn't — both are regressions.

## 2026-05-07 — Corrigenda (per EXP-0011 methodology finding)

The `composition_only` benches used in this experiment (`bench_chain_sync`, `bench_grid_async`, `bench_modes_compare`) compose CLTSs whose controllable alphabets overlap (`chain_1k`/`ring_1k`/`grid_32x32` fixtures share controllable labels). `composition::compose()` rejects shared-controllable-alphabet pairs in validation, so those benches were measuring the validation **error path**, not real composition.

The falsification *direction* of this experiment still stands — a slower error path is still a slower path, and the L3 protocol caught a real regression — but the **magnitudes do not reflect real compose cost**. EXP-0011 fixed the bench by switching to `RandomClts::new(seed).with_uncontrollable_prefix(3)` so labels can be shared across the uncontrollable prefix; the resulting compose call takes ~6 ms on 32-state inputs (vs the ~5–10 µs error-path numbers reported here).

This archive is not being re-opened. Future composition-touching EXPs should use the EXP-0011-style fixture.
