# EXP-0010-fxhash-composition: FxHashMap drop-in — hypothesis falsified

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe (std HashMap)
**Commit candidate:** working-tree (FxHashMap, applied via /tmp/exp-0010-fxhash.patch)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. See `hw-fingerprint.txt`.

## Motivation

Plan §B2 hypothesis: 1.5-2× speedup on composition workloads from swapping std HashMap → FxHashMap. Pre-registered as low-risk; "zero unsafe, single-line replace."

## Hypothesis (pre-registered, in the plan)

≥1.5× speedup on composition-heavy workloads.

## Method

L3 protocol per ADR-0006. Both sides ran in the same shell session, 30 samples per bench function. See README.md for the full command sequence and command.txt for the exact replay invocation.

## Results

5/5 composition benches regress at p<0.001 (both Criterion's bootstrap and Mann-Whitney on per-iter times). See README.md table.

## Interpretation

Plan §B2 is wrong for this codebase. The composition's hot HashMaps:
- Mix integer-pair keys (ProductStateBuilder.state_map) with Vec<String> keys (intern cache) and small structural keys (StateKey, StatePairKey, LabelPairKey).
- Stay small (typically <2000 entries per composition).
- Are wrapped in RefCell.

At these sizes and with these key types, FxHash's faster hash computation gets dominated by:
1. Hash-quality clustering on weak/short keys.
2. RefCell::borrow_mut overhead.
3. Rc clone churn.
4. Vec<String> key hashing (which is byte-stream and benefits little from FxHash's int-multiplication shape).

Empirical takeaway: **don't swap HashMap → FxHashMap blindly on hot paths — measure first.** For mununu's composition, std's hashbrown-backed HashMap is faster than FxHash.

## Dead-ends

- *2026-05-06:* First A-side run failed because `git checkout HEAD -- Cargo.toml` reverted not just the EXP-0010 change but ALSO sitting-2's test_support feature. Result: bench wouldn't compile. Fixed by re-applying the full patch and surgically reverting only the EXP-0010 lines via Edit.
- *2026-05-06:* `git apply /tmp/exp-0010-fxhash.patch` failed on B-side restore because the patch was generated against HEAD but the working tree had additional changes from the prior session. Fixed by re-editing manually (tiny diff: 1 dep line + 1 import block + 1 method-call style change).
- *2026-05-06:* `bench_diff.sh --robust` was comparing raw `times` (total iter time) from Criterion's sample.json instead of per-iter times. Mann-Whitney saw the iter-ramp instead of the bench cost; reported p=0.7-0.99 on a clear +30-60% regression. Fixed: divide times by iters before feeding into MW. After fix, p<0.001 on all 5 regressions, agreeing with Criterion's bootstrap.

## Followups

- **Revert FxHashMap from composition.** Done in same sitting.
- **Update plan §B2 status to "falsified."** ADR-0009 records the decision.
- **No EXP-0010-deep at L4.** The regression is robust at L3 (n=30, p<0.001). Re-running at L4 would just confirm the direction.
- **Generalize the lesson:** any HashMap → FxHashMap drop-in candidate (e.g., the formula-var bindings in mu_calculus, the predicate map in Environment) should be benched at L3 BEFORE landing. The intuition "FxHash wins on integer keys" is too simplistic; it depends on map size, key complexity, and surrounding code.

## Artifacts

- `criterion-archive.tar.zst` — 420 KB, contains both `exp-0010-fxhash` baseline (std HashMap) and `new` candidate (FxHashMap) with full sample data (30 per side per bench).
- `manifest.json` — schema_version 1.
- `command.txt` — replay invocation.
- `hw-fingerprint.txt` — fresh fingerprint at recording.
