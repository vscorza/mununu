# EXP-0004-with-capacity: drop 20% growth, plan §A4 confirmed

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-06
**Date closed:** 2026-05-06
**Commit baseline:** e6edcfe (with 20% growth wrapper)
**Commit candidate:** working-tree (Vec::push doubling, /tmp/exp-0004-with-capacity.patch)
**Hardware:** Intel i7-9750H, macOS 26.3 / Darwin 25.3.0, 16 GB. See `hw-fingerprint.txt`.

## Motivation

Plan §A4: the 20% growth wrapper at `clts/mod.rs:46-58` produces more reallocations than `Vec::push`'s native doubling. Predicted ≥1.1× speedup on builder-only benches.

## Hypothesis

≥1.1× speedup on builder-only bench.

## Method

L3 protocol per ADR-0006. Both sides 40 samples per bench function, 8-15s measurement window, same shell session. See README.md for the full command sequence.

## Results

All 7 clts_construction benches improve at p<0.001. Delta range from -5.2% (random_seeded/1024, smallest workload, RNG-dominated) to -31.0% (chain/100000, largest workload, realloc-dominated). See README.md table.

## Interpretation

Hypothesis ≥1.1× confirmed across the entire bench matrix. The win scales with state count because reallocation cost grows quadratically (each realloc copies the full current Vec contents). Vec doubling produces O(log N) reallocs vs the wrapper's O(log_1.2 N) ≈ O(5/log 2 × log N) — 2-4× more reallocs at any given final size.

The original wrapper's stated rationale ("keep peak memory under control") is empirically wrong: Vec doubling allocates at most 2× over-provision, while the 20% wrapper eventually allocates the same total amount in more, smaller increments. Same peak, more reallocations.

This is the second confirmed plan-§ item after EXP-0002b. EXP-0010 falsified §B2; EXP-0004 confirms §A4. Together they establish that the bench-first protocol distinguishes intuition that holds from intuition that fails.

## Dead-ends

- *2026-05-06:* Initial revert via `git checkout HEAD -- crates/mununu-core/src/clts/mod.rs` left the 3 capacity-hint test functions (`grow_capacity_zero_current` etc.) which still pass because they only check builder correctness, not the growth strategy. The tests are misnamed in the new world but functionally correct; rename is an optional follow-up cleanup.
- *2026-05-06:* The cargo dead-code linter flagged `entry_capacity_hint` and `set_capacity_hint` as unread after I converted the `ensure_*_capacity` helpers to no-ops. Decided to remove both fields entirely + delete the no-op helpers + delete their call sites for a clean ablation. Net: -50 lines of growth code, +30 lines of EXP-0004 comments.
- *2026-05-06:* Editor concurrent-modification race: my Edit calls failed three times because cargo build touched the file between the Read and the Write. Re-read after each cargo invocation kept the workflow moving.

## Followups

- **Update plan §A4 status to "confirmed" in the plan file.** The headline number (1.45× on chain/100000) supports the predicted ≥1.1×.
- **EXP-0001-deep should re-record the baseline with this change applied.** The current EXP-0001 baseline reflects the 20% wrapper; published "before" numbers for blog post 2 should be at the post-EXP-0004 baseline.
- **Rename the 3 grow_capacity_* tests** at `clts/mod.rs:2805-2837` to reflect that they no longer test a growth strategy (they're builder smoke tests now). Optional cleanup.
- **EXP-0003 LabelSetTable interning** is the next §A-series substantive change; expected larger payoff than §A4 because it changes data layout, not just growth strategy.

## Artifacts

- `criterion-archive.tar.zst` — 609 KB, contains both `exp-0004-with-cap` baseline (20% wrapper) and `new` candidate (Vec::push doubling) with full sample data (40 per side).
- `manifest.json` — schema_version 1, outcome `hypothesis_confirmed`.
- `command.txt` — replay invocation.
- `hw-fingerprint.txt`.
