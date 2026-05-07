# EXP-0012-changed-flag-fixpoint: plan §B4 falsified at this scale

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe + SoA + EXP-0004 (clone+compare fixpoint termination)
**Commit candidate:** working-tree (in-place merge with changed flag)

## Hypothesis

Plan §B4: ≥1.2× on long-iteration fixpoints. Eliminates one BitVec clone + one BitVec compare per iteration.

## Method

L3 protocol. Both sides 30+ samples per bench function on the full mu_calculus_only matrix (10 benches across 4 formula classes). See README.md.

## Results

7/10 benches regress at p<0.001 (+10% to +94%). One non-significant -8.7% on synthesis_product_game/grid_32x32 (p=0.351 per Mann-Whitney). Two benches statistically tied. **Hypothesis falsified across the matrix.**

## Interpretation

The "obvious bottleneck" turned out not to be one. `BitVec::==` is implemented in `bitvec` as word-level scan with early exit; for non-converged iterations (the typical case), it returns after the first mismatching word in O(1) average. The clone is a small memcpy (≤ 128 bytes for our state counts). Replacing both with a serial word-loop that branches per word loses to the branchless memcpy + early-exit compare at small scales.

Three drop-in falsifications now: EXP-0010 (FxHashMap), EXP-0003 (LabelSetTable), EXP-0012 (track-during-merge). All failed because the "obvious bottleneck" wasn't the actual bottleneck. The wins (EXP-0002b SoA, EXP-0004 Vec doubling) come from structural changes to access patterns, not drop-in substitutions.

## Dead-ends

- *2026-05-06:* Initial implementation passed all 830 lib tests + soundness suite + property tests. The differential gate is necessary but not sufficient — the L3 protocol caught what the test suite couldn't.
- *2026-05-06:* The plan §B4 reasoning assumed clone+compare cost was significant. At our state counts (≤ 1024), it isn't. A bench at 10k+ states might tell a different story; not scheduled.

## Followups

- **Revert.** Done.
- **Mark plan §B4 falsified-at-this-scale.** ADR-0013 records the decision.
- **Future EXP-0012-hybrid:** size-conditional implementation that uses clone+compare on small BitVecs and track-during-merge on large. Not scheduled.

## Artifacts

- `criterion-archive.tar.zst` — 628 KB, all 10 benches with full sample data.
- `manifest.json` — schema_version 1, outcome `hypothesis_falsified`.
- `command.txt` — replay invocation.
- `hw-fingerprint.txt`.
