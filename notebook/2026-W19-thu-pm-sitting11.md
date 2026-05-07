# Notebook — 2026-W19, Thu PM (2026-05-07, sitting 11)

## Sitting 11 — EXP-0011: Context-level composition cache; first 5-orders-of-magnitude win

### Context

After four falsifications in a row (§A2 LabelSetTable, §B2 FxHashMap, §B4 changed-flag, §A3 CSR-via-staging), sitting 11 picked plan §B3 (Context-level composition cache) for its highest a-priori probability of winning under the refined ADR-0014 taxonomy: "changes that reduce total work win." Memoization is the canonical work-elimination pattern.

### Landed

- **`Context::compose_named_cached(left, right, options) -> Arc<Clts>`** in [`crates/mununu-core/src/context/mod.rs`](../crates/mununu-core/src/context/mod.rs). Memoizes BFS product on `(left, right, mode)`. Cache key: precomputed `u64` via `DefaultHasher`; cache value: `(String, String, Discriminant<CompositionSemantics>, Arc<Clts>)` for collision-safe rechecking.
- **Bench** `composition_only::bench_compose_cache` in [`crates/mununu-core/benches/composition_only.rs`](../crates/mununu-core/benches/composition_only.rs). Two arms (uncached / cached) on 32-state random CLTSs with `with_uncontrollable_prefix(3)` so the alphabet can overlap without tripping `compose()`'s controllability validation.
- **Standalone verifier** at [`crates/mununu-core/src/bin/cache_check.rs`](../crates/mununu-core/src/bin/cache_check.rs).
- **EXP-0011 archive** at [`experiments/EXP-0011-compose-cache/`](../experiments/EXP-0011-compose-cache/): full README/log/notes, manifest schema_version 1, criterion archive, hw-fingerprint.
- **ADR-0015** records §B3 as the third confirmed plan item (after §A1 SoA, §A4 Vec doubling) and breaks the four-falsification streak.

### Headline result

| arm                            | median       | 95% CI                  |
|--------------------------------|-------------:|-------------------------|
| uncached `compose_named`       | **6.247 ms** | [6.189, 6.316] ms       |
| cached `compose_named_cached`  | **46.23 ns** | [46.02, 49.07] ns       |
| **speedup**                    | **≈135,132×** |                         |

All 825 lib tests pass.

### Methodology contribution: a bug in prior `composition_only` benches

While picking fixtures I discovered the existing `composition_only::bench_chain_sync` and friends use `chain_1k`/`ring_1k`/`grid_32x32` fixtures whose controllable alphabets overlap. `compose()` rejects shared-controllable-alphabet pairs in validation, so those benches were measuring the **error path**, not real composition.

EXP-0010 (FxHashMap), EXP-0003 (LabelSetTable), EXP-0030 (CSR-via-staging) all reported numbers on this bench. The falsification *directions* still stand (a slower error path is still a slower path), but the magnitudes don't reflect real compose cost. A corrigenda note is being appended to those archives, not a re-open. The fix in EXP-0011's bench is `RandomClts::new(seed).with_uncontrollable_prefix(3)` so labels can be shared across the uncontrollable prefix only.

### Key design decisions

1. **`Arc<Clts>` return** — cache hits are a refcount bump rather than a deep `Clts` clone. Callers that want `&Clts` use `&*arc`.
2. **`u64` hash key** — first cut used `(String, String, Discriminant)` directly as the HashMap key, forcing two `String::to_string()` allocs per lookup (~110 ns). Switching to a precomputed `u64` via `DefaultHasher` and storing the full key alongside the `Arc<Clts>` for recheck dropped the hit cost to ~46 ns.
3. **Collision safety** — the recheck on hit guards against the (~2^-64) hash-collision case. On collision the recheck fails and the bucket is treated as a miss; correct rebuild + re-store. The recheck cost is dwarfed by the BFS cost on a true miss.
4. **`RefCell` interior mutability** — the cache is `RefCell<HashMap<...>>` because `compose_named_cached` takes `&self` (matches the existing `compose_named` signature). `Context` is `!Send`, so `RefCell` is fine.

### Why this win is shaped differently from the prior wins

§A1 SoA, §A4 Vec doubling, §B3 cache: all confirmed by the refined "reduces total work" rule. But the magnitudes are radically different:
- §A1 SoA: ~5–10% on synthesis-bound workloads.
- §A4 Vec doubling: ~5% on builder.
- §B3 cache: ≈135,000×.

The reason is the *unit of work* eliminated. SoA eliminates per-rank-read HashMap probes (savings: ~50 ns × N states); Vec doubling eliminates one full edge-buffer copy at `build()` (savings: O(|E|) once); the cache eliminates the entire BFS product construction on the hit path (savings: O(|S₁|·|S₂|·|Σ|) every time). The cache wins by orders of magnitude precisely because what it skips is *much larger* than what it costs to skip.

This is consistent with the ADR-0014 prediction model: the cache is a structural change AND the work it eliminates is asymptotically larger than the work it adds (a hashmap probe + Arc bump). Both conditions matter.

### Plan budget after sitting 11

- Confirmed (3): §A1 SoA (EXP-0002b), §A4 Vec doubling (EXP-0004), §B3 cache (EXP-0011).
- Falsified (4): §A2 LabelSetTable (EXP-0003), §A3-via-staging CSR (EXP-0030), §B2 FxHashMap (EXP-0010), §B4 changed-flag (EXP-0012).
- Partial-falsification (1): §A1 heap (memory not measured to win in EXP-0002b).
- Open (rest): §A3-direct, §A6 (predicate interning split into two EXPs per ADR-0014), §A7 store-type pin, §B1 Paige-Tarjan, §B5 chaotic, §B6 modal CSR, §C1 parallel modal, §C2 parallel composition, §C3 batch driver, §D educational.

### Followups (not scheduled in this sitting)

- **EXP-0011-callers** — replace `compose_named` callers in DSL evaluation and synthesis paths with `compose_named_cached`. Until done, real Mununu workflows don't see the speedup; the bench result is "this implementation works" rather than "the DSL is faster."
- **EXP-0011-witness audit** — confirm the cache key doesn't need to include `EvaluationOptions` fields. (Inspection-level, ~5 min.)
- **EXP-0011-corrigenda** — append the methodology note to EXP-0010, EXP-0003, EXP-0030.

### Estimated cumulative wins so far

The five orders of magnitude on EXP-0011 dwarfs everything else, but only matters if real workflows hit the cache. On steady-state DSL workflows (evaluate N properties on the same composition), the cumulative speedup is dominated by §B3. On one-shot compose calls, the cumulative is roughly:

- §A1 SoA: ~0.95× on synthesis-bound (≤8% improvement).
- §A4 Vec doubling: ~0.95× on builder (≤5% improvement).
- §B3 cache (one-shot, miss): ~1.00× (no benefit when each compose is unique).
- §B3 cache (multi-property hit): up to 135,000× on the cached call(s); 1× on the cold one.

The honest summary: we have not measured an end-to-end Mununu CLI run pre/post programme. The EXP archives are isolated benches. EXP-0011-callers + a `pipeline_e2e.rs` bench (planned but not landed) is the right place to measure cumulative wallclock on a real DSL workflow.
