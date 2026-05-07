# Free-form observations for EXP-0004-with-capacity

## 2026-05-06 — second confirmed plan item, complementing EXP-0010 falsification

### What this archive proves

- Plan §A4's predicted ≥1.1× speedup is empirically confirmed across the entire `clts_construction` bench matrix at p<0.001.
- The win scales with workload size: -5% on random_seeded/1024 (small, RNG-dominated), -31% on chain/100000 (large, realloc-dominated).
- The original 20% wrapper's stated rationale (peak memory control) is wrong; Vec doubling has the same peak in practice with fewer reallocations.

### What this archive does NOT prove

- Vec doubling is universally better. It's the right call for mununu's CLTS construction at chain-size 1k-100k. For workloads that fill billions of small entries with tight peak-memory budgets, a custom 1.5× or 1.2× wrapper might still win. EXP-0004 doesn't claim universality.
- The win generalizes to other builder workloads. EXP-0004 specifically benched `clts_construction`. The mu_calculus parser, abstraction unrolling, and other builder paths might have different growth profiles.

### Methodology lessons

- The L3 protocol caught the win cleanly: 40 samples per side, p<0.001 across 7 benches, no contamination. Same protocol that caught EXP-0010's falsification.
- The bench_diff per-iter MW fix (sitting 5) means the robust gate now correctly classifies all 7 improvements as significant. Without the fix, the smaller wins (-5% to -12%) might have been demoted to neutral by Mann-Whitney's noise threshold.

### Why this is more interesting than a typical "drop the wrapper" PR

EXP-0010 falsified plan §B2 (a similar "drop in for the modern std implementation" hypothesis). EXP-0004 confirms plan §A4 (also a "drop the bespoke wrapper" hypothesis). The two together demonstrate that:

1. **Bespoke growth strategies are worth questioning** — they often predate mature std implementations and can lose to them.
2. **Bench measurement distinguishes intuition that holds from intuition that fails.** §A4 and §B2 sounded similar on paper; one wins and one loses, by 30-60% in opposite directions.
3. **The "obvious" answer (drop the bespoke code, use std) wins more often than the "tweak the bespoke code" answer** — but only on benchmarked workloads. Without benchmarks, you can't tell which class your workload is in.

This is the kind of finding that strengthens the blog series and the paper. EXP-0004 + EXP-0010 are a balanced pair: both are 0.5-1 day items, both look like drop-ins, one wins and one loses. Section in blog post 2 ("Layout matters") will pair them.

### Iteration policy actions

- Used `--fresh` (implicit via new bench_compare invocation; the criterion archive after this run contains only EXP-0004-with-cap baseline + new).
- Following ADR-0004 "supersede, don't rewrite": the EXP-0004 archive stays as canonical. If a future EXP-0004-deep re-records at L4, both archives stay; the deep version supersedes for paper-grade citation but EXP-0004 remains as the L3 confirmation.

### Anti-patterns avoided

- Did not silently delete the 3 grow_capacity_* tests because they test functionality that no longer exists. Their names are misleading but they verify builder correctness; renaming is a future cleanup, not a removal.
- Did not skip the L3 protocol because the change "looked obviously correct." EXP-0010 proved that obvious-looking changes can be wrong. EXP-0004's L3 measurement converts intuition into evidence.
- Did not over-claim. The README headline says "1.45× on chain/100000" with the workload caveat; it doesn't say "1.45× on construction" without qualification.
