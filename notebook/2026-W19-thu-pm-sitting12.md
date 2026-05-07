# Notebook — 2026-W19, Thu PM (2026-05-07, sitting 12)

## Sitting 12 — EXP-0007 BindingStack falsified; §B3 cache architecturally orphaned

### Context

Sitting 12 opened with two intents:
1. Wire the EXP-0011 §B3 composition cache into real callers and measure the end-to-end win.
2. If §B3 is delivering, build out the next plan item — §A6b BindingStack.

### Landed

#### Finding 1: §B3 cache is architecturally orphaned (ADR-0016)

A grep across the workspace for `compose_named_cached` returned only the EXP-0011 bench, the `cache_check` ad-hoc binary, and the implementation. No production code calls it. The original `compose_named` is invoked exactly once in `context_dsl/realize.rs:1279` inside an associative-compose loop — and each iteration creates a fresh `temp_context`, calls compose once, and drops it. Post-realize the composed CLTS is stored as a named entry; downstream evaluation operates on that name.

The 135,132× microbench result remains a true characterization of the cache's hit-path cost, but it doesn't translate to wallclock improvement until a workload that recomposes on the fly is added (REPL, interactive editor, batch tools varying composition mode). The implementation is kept (correct + extensible). ADR-0016 documents the downgrade from "third confirmed plan item" to "infrastructure-correct, no caller today."

#### Finding 2: §A6b BindingStack falsified (ADR-0017)

EXP-0007 implemented the BindingStack pattern: change `eval_fixpoint`'s per-iteration `bindings.clone()` to an enter-on-entry / restore-on-exit RAII pattern that mutates `bindings` in place. ADR-0014 predicted **likely win** (work-reducing structural change).

Result: 2 hard regressions (+11.6% and +28.1%, p<0.001), 0 improvements, 5 of 10 benches significantly slower. Reverted.

The prediction missed because at K=0 (alternation-1 fixpoints, dominant in mununu's fixtures), `HashMap::clone()` on an empty map is a 24-byte memcpy — essentially free. The replacement `HashMap::insert` does ~30 ns of hash + probe + drop + insert, *more* than the memcpy. The crossover where NEW wins is K≥2 (alternation-3+), which mununu doesn't reach.

This pairs with ADR-0013 (§B4 changed-flag) under the same root cause: **the std primitives' hot loops are already heavily optimized**. ADR-0017 promotes this from a one-off observation to a load-bearing prediction rule (Q3 in the three-bit taxonomy).

### Headline plan budget after sitting 12

- **Confirmed (2)**: §A1 SoA (EXP-0002b), §A4 Vec doubling (EXP-0004).
- **Infrastructure-orphan (1)**: §B3 cache (EXP-0011) — implementation correct, no real caller.
- **Falsified (5)**: §A2 LabelSetTable (EXP-0003), §A3-via-staging CSR (EXP-0030), §B2 FxHashMap (EXP-0010), §B4 changed-flag (EXP-0012), §A6b BindingStack (EXP-0007).
- **Partial-falsification (1)**: §A1 heap (memory not measured to win in EXP-0002b).

### Refined ADR-0014 prediction model

ADR-0017 promotes the previous "reduces total work wins" rule into a three-bit taxonomy:

| bit | question | favorable answer |
|-----|----------|------------------|
| Q1  | structural or drop-in? | structural |
| Q2  | reduces or adds work? | reduces |
| Q3  | does OLD already use a heavily-optimized std primitive? | no |

The plan items that have passed Q1+Q2 but failed Q3:
- §B4 changed-flag: replaces `BitVec::==` (word-parallel early-exit) with branchful word-loop. Failed Q3.
- §A6b BindingStack: replaces `HashMap::clone` (bulk memcpy) with `HashMap::insert/remove` (hash+probe). Failed Q3.

Plan items still on the favorable side:
- §A6a predicate name interning: replaces `HashMap<String, BitVec>` probe with `Vec<BitVec>` index. String hashing IS expensive enough that this should win — but Q3 needs pre-validation via microbench.
- §A3-direct CSR build: from-scratch CSR (no staging-flatten path). Q1+Q2+Q3 all favorable.
- §B1 Paige-Tarjan: O(m log n) vs O(k·m·n). Q3 trivially favorable (no std primitive does partition refinement).
- §B6 modal pre-image CSR: precompute transposed adjacency. Q3 favorable.

### Bench-fixture concern

While running EXP-0007's tests I confirmed the `properties::minimization::idempotence` proptest fails reliably with seed `9382785361923416088` (states=19, density_pct=7). This is a real bug in `minimize_bisimulation` (`composition/minimize.rs`). The seed is checked in to `minimization.proptest-regressions`, so it's been failing for some time. Open EXP-0040-minimize-bug as a separate investigation; not in scope for sitting 12.

### Followups (not scheduled)

- **EXP-0007-deep-alternation.** Build a synthetic alternation-3+ formula and re-run BindingStack. If NEW wins decisively at K≥2, the design has a niche use.
- **EXP-0040-minimize-bug.** Investigate the K-S minimization non-idempotence on the small-state random input.
- **EXP-0011-followup.** If a Mununu workload that benefits from §B3's cache shape ever lands (REPL, batch tool varying composition mode), wire `compose_named_cached` into its callers.

### Sitting summary

Two negatives in one sitting. The §B3 finding is a correction-to-self; the §A6b finding is the fifth falsification. Combined, they say:
- The plan's hypotheses about where mununu spends time were partly wrong about the call graph (§B3) and partly wrong about the optimization opportunity (§A6b).
- The std/bitvec/hashmap primitives are doing more work for us than the plan credited them with. The "obvious" structural replacements lose at our scale.
- The plan items most likely to deliver remaining wins are the algorithmic ones (§B1 Paige-Tarjan, §B6 modal pre-image CSR) and the from-scratch ones (§A3-direct). The drop-in or shallow-restructure items are exhausted.

The honest plan budget: 2 confirmed wins + 1 infrastructure shelf for ~12 sittings of work. The next sitting should pick the highest-value algorithmic item — §B6 modal pre-image CSR — because §B1 Paige-Tarjan needs the differential oracle (E5) landed first, and the differential oracle needs ~1.5 days that haven't been put in.
