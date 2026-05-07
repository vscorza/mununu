# EXP-0030-csr-adjacency: §A3 CSR-via-staging-flatten falsified

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe + SoA + EXP-0004
**Commit candidate:** working-tree (CSR adjacency via AdjacencyCsr::from_rows)

## Hypothesis

Plan §A3 (partial, without §A2 LabelSetTable cache): per-state Vec<Transition> headers eliminated; predicted ~24 MB memory savings at 1M states, neutral build time, neutral-to-positive consumer time.

## Method

L3 protocol on three benches: clts_construction (build path), composition_only (build via compose()), mu_calculus_only/synthesis_product_game (consumer path). 30+ samples per side.

## Results

- **clts_construction**: 6 regressions (+12% to +65% slower), 1 improvement (-15.8% on smallest fixture).
- **composition_only**: 5 neutral (all within ±10% threshold).
- **synth**: 2 small regressions (+2.8%, +3.6%), below 10% threshold.

See README.md for the full table. Falsified.

## Interpretation

The CSR-via-staging-flatten implementation adds an O(|E|) extra copy at build time (the `AdjacencyCsr::from_rows` flatten). For 4M edges × 24 bytes = ~96 MB of additional memcpy per CLTS build. The predicted savings (eliminating 24 MB of per-row Vec header overhead) are smaller than the cost.

Consumer-side accessor cost is roughly neutral: the original `&self.outgoing[i]` is one indirection; CSR `self.outgoing.row(i)` is two slice indexes (offsets[i], offsets[i+1]) into the flat transitions. Cache locality is slightly better for adjacent-state iteration but not enough to amortize the build-time penalty.

## Why this is the fourth structural-style falsification

Falsifications so far:
- EXP-0010 §B2 FxHashMap (drop-in): hash-key swap, +30-60%.
- EXP-0003 §A2 LabelSetTable (drop-in): key-shape swap, +18-85%.
- EXP-0012 §B4 track-during-merge (algorithm shape change): +1-94%.
- **EXP-0030 §A3 CSR-via-staging-flatten (structural with setup cost): +12-65% on construction.**

The taxonomy now has three failure modes:
1. **Drop-in hash/key swaps** (§B2, §A2): targeted bottleneck wasn't actually the bottleneck.
2. **Algorithm-shape changes that lose to optimized primitives** (§B4): `BitVec::==` already early-exits; manual word-loop loses to it.
3. **Structural changes with extra setup cost** (§A3 here): the new layout's runtime savings are smaller than the build-time price to construct it.

Wins so far (EXP-0002b §A1 SoA, EXP-0004 §A4 Vec doubling):
- Both restructure access patterns AND don't add setup cost.
- §A1 SoA: replaced HashMap with Vec<Vec<u32>>. Build cost equivalent (lazy init). Read cost lower. Net win.
- §A4 Vec doubling: removed an explicit growth wrapper. Less code, less work per iteration. Net win.

The refined rule: **structural changes win when they reduce total work; structural changes with extra setup work can lose.**

## Followups

- **Revert** done.
- **Mark plan §A3 as falsified-via-staging-flatten.** ADR-0014.
- **Future EXP-0031-csr-direct-build:** redesign that bypasses the staging Vec<Vec<>> and builds directly into the flat CSR. This eliminates the flatten cost and potentially delivers the predicted §A3 savings. Not scheduled; it's a 2-3 hour additional refactor on top of what EXP-0030 already did.

## Artifacts

- `criterion-archive.tar.zst` — 653 KB, all 3 benches × A+B sides.
- `manifest.json` — schema_version 1, outcome `hypothesis_falsified`.
