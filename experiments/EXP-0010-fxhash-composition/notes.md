# Free-form observations for EXP-0010-fxhash-composition

## 2026-05-06 — first plan-falsification finding

### What this proves

- The plan's §B2 hypothesis ("FxHashMap drop-in is 1.5-2× faster") is empirically false on mununu's composition workloads.
- The L3 protocol catches falsifications cleanly: 30 samples per side, p<0.001 on every bench, no contamination.
- bench_diff.sh agrees with Criterion's bootstrap once the per-iter bug is fixed.

### What this does NOT prove

- FxHashMap is universally bad. It wins on millions-of-entries integer-keyed maps. It just doesn't apply to mununu's composition.
- All HashMap → FxHashMap drop-ins will fail. The mu_calculus path has different shape (formula-var IDs as integer keys, larger maps in deep alternation). That experiment, if run, must be benched separately.

### How big a deal is this for the plan?

§B2 was a 0.5-1 day item rated "low-risk drop-in." Per the plan's sequencing (sitting 1's notebook 0000-overview.md):
- §B2 was rated 1.5-2× speedup, contributing to "blog post 4: Drop SipHash where it doesn't pay."
- Now that the hypothesis is falsified, the blog post pivots to: "Why FxHashMap is sometimes the wrong choice, and how the L3 protocol catches it."

This is actually a more interesting blog post than the original — falsification stories are rarer in the perf-blog literature than success stories. EXP-0010 will anchor a section in the planned post 4 (renamed if needed).

### Why didn't I expect this?

The plan was written based on intuition + Aumasson-Bernstein on SipHash overhead in non-adversarial settings. The intuition is correct in the abstract but breaks on this workload because:
1. Maps are small (<2k entries, the cluster-sensitivity threshold).
2. Keys are mixed (Vec<String> + integer pairs + structural).
3. RefCell + Rc churn dominates the per-call cost; hash itself is a small fraction.

Lesson: **bench-driven, not intuition-driven**, even for "obvious" drop-ins. Future EXPs in the plan should be re-evaluated with this in mind.

### Iteration policy actions taken

- Used `--fresh` on `bench_compare.sh` so the criterion archive contains only this run.
- Used L3 protocol (warmup discard + same session); the regression is reproducible.
- Discovered and fixed the per-iter bug in `bench_diff.sh --robust`. Bug-fix is on the master branch independent of EXP-0010.
- Following ADR-0004 "supersede, don't rewrite": EXP-0010 archive stays as the falsification record. The reverted code change is recorded in the next commit's git log; bench_diff bug-fix is recorded separately.

### Anti-patterns avoided

- Did not silently revert without archiving. The archive proves the hypothesis was tested at L3 with proper rigor.
- Did not blame the methodology. The L3 protocol caught a real regression that intuition alone would have missed; that's the protocol working as designed.
- Did not claim the SoA migration (EXP-0002b) as "ergo SoA is universally good." Each drop-in must be tested on its own workload.

## 2026-05-07 — Corrigenda (per EXP-0011 methodology finding)

The `composition_only` benches used in this experiment (`bench_chain_sync`, `bench_grid_async`, `bench_modes_compare`) compose CLTSs whose controllable alphabets overlap (`chain_1k`/`ring_1k`/`grid_32x32` fixtures share controllable labels). `composition::compose()` rejects shared-controllable-alphabet pairs in validation, so those benches were measuring the validation **error path**, not real composition.

The falsification *direction* of this experiment still stands — a slower error path is still a slower path, and the L3 protocol caught a real regression — but the **magnitudes do not reflect real compose cost**. EXP-0011 fixed the bench by switching to `RandomClts::new(seed).with_uncontrollable_prefix(3)` so labels can be shared across the uncontrollable prefix; the resulting compose call takes ~6 ms on 32-state inputs (vs the ~5–10 µs error-path numbers reported here).

This archive is not being re-opened. Future composition-touching EXPs should use the EXP-0011-style fixture.
