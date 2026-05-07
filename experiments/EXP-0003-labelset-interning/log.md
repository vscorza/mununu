# EXP-0003-labelset-interning: plan §A2 falsified at this implementation

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe + SoA + EXP-0004 (SmallVec-keyed uncontrollable_groups)
**Commit candidate:** working-tree (LabelSetId-keyed, /tmp/exp-0003-labelset-v2.patch)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB.

## Motivation

Plan §A2: predicted ≥1.5× speedup on `uncontrollable_groups` lookup by interning canonical label sets to `LabelSetId(u32)` and using u32-keyed HashMap probes instead of SmallVec-keyed.

## Hypothesis

≥1.5× on uncontrollable_groups lookup; edge memory drops from ~16 to 4 bytes/edge.

## Method

L3 protocol on TWO benches: `composition_only` (which exercises CLTS build) and `mu_calculus_only::synthesis_product_game` (which exercises the modal-eval consumer of uncontrollable_groups).

## Results

- **composition_only**: +18% to +85% slower across 4 of 5 benches (p<0.001). The build path now includes `LabelSetTable::intern` for every transition; composition doesn't read the table back so it's pure overhead.
- **synth_product_game**: +3% to +5% slower (p<0.01). The four `contains_key` consumer sites in evaluator.rs added a `find_label_set` lookup BEFORE the HashMap probe — net 2 probes where there was 1.

See README.md for the full table.

## Interpretation

Plan §A2 hypothesis is FALSIFIED for this implementation. The optimization assumed the consumer already had a LabelSetId in hand; in reality the consumer constructs a fresh SmallVec from a transition's uncontrollable subset and looks it up. Adding the `find_label_set` indirection adds work without removing the SmallVec hash.

This is the second falsification (EXP-0010 was first). Both are "drop-in" hypotheses on small, RefCell-wrapped maps. The pattern: drop-in hash/key optimizations on these workloads consistently lose because the per-call overhead is dominated by RefCell+Rc churn and probe overhead matters less than expected.

## Dead-ends

- *2026-05-06:* Initial implementation passed all 830 lib tests. The L3 protocol caught the regression that the test suite couldn't.
- *2026-05-06:* Patch-management got messy: the EXP-0002 SoA patch and EXP-0004 patch had to be re-applied separately when reverting EXP-0003 for A side. `git apply --include='*evaluator.rs'` got me past the conflicting context. Future similar EXPs should use git stash with explicit paths instead of multiple patch files.
- *2026-05-06:* The first patch I generated didn't include the new `label_set_table.rs` file because I hadn't `git add`'d it. Lost ~5 minutes regenerating it; lesson for future EXPs that introduce new files.

## Why the optimization didn't deliver (deep)

The win was supposed to come from cheap u32 hashing replacing expensive SmallVec hashing. But `find_label_set` itself is a SmallVec-keyed HashMap probe. So the consumer pattern becomes:
1. Build SmallVec of uncontrollable labels (same as before).
2. Probe `label_set_table` with SmallVec hash (NEW work).
3. Probe `uncontrollable_groups` with u32 hash (cheaper than SmallVec but only saves ~5 ns).

Net: +1 SmallVec hash that wasn't there before. The "savings" from u32 hashing are dwarfed by the duplicated SmallVec work.

The redesign that might work: store LabelSetId on Transition itself, eliminating the SmallVec construction at consumer time. But that's a Transition-struct change with API ripple to dozens of `transition.labels()` callers.

## Followups

- **Revert EXP-0003 changes.** Done.
- **Mark plan §A2 status as falsified-at-this-implementation.** ADR-0012 records the decision.
- **EXP-0003-redesigned (future, not scheduled):** cache LabelSetId on Transition struct. Bigger refactor; needs separate EXP and bench-first methodology.
- **Update blog post 2** to include EXP-0003 alongside EXP-0010 as the second drop-in failure. Three falsifications now (EXP-0010, EXP-0002 contamination, EXP-0003).

## Artifacts

- `criterion-archive.tar.zst` — 617 KB, contains both A and B sides for composition_only AND mu_calculus_only/synthesis_product_game.
- `manifest.json` — schema_version 1, outcome `hypothesis_falsified`.
- `command.txt` — replay invocation (multi-bench).
- `hw-fingerprint.txt`.
