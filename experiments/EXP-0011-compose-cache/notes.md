# Free-form observations for EXP-0011-compose-cache

Append-only. Date entries with `## YYYY-MM-DD` headers. Do not delete — append "REVISED" / "WITHDRAWN" notes instead.

## 2026-05-07 — Why this win was so big

Most plan items so far have moved by 1.05× to 1.5× (and four of the last five were falsifications — see `notebook/decisions.md`). EXP-0011 moves by ≈135,000×. The reason is structural: cache hits replace work with a refcount bump. Compare with the falsified items:

- **EXP-0010 (FxHashMap drop-in)** replaced one constant-overhead op with a different constant-overhead op. Either Fx is faster or it isn't, and on `composition_only`'s short integer-pair keys it wasn't, by ~30–60%. No work was eliminated.
- **EXP-0003 (LabelSetTable)** replaced one hash with another hash. Same pattern; the SmallVec hash isn't expensive enough for u32 ID lookup to win on the small fixtures.
- **EXP-0012 (changed-flag)** replaced one BitVec compare with a maintained boolean. But `BitVec::==` already early-exits on the first differing word, so the new boolean was usually colder than the bitvec compare it replaced.
- **EXP-0030 (CSR-via-staging)** added a copy step (Vec<Transition> → CSR) without removing the existing builder. Pure addition, no work eliminated.

EXP-0011 is the first plan item that *eliminates* the entire BFS product construction on the hit path. That's why the numbers look like a different kind of optimization — it is.

This pattern is the empirical taxonomy ADR-0014 captured: structural changes win when they reduce total work; they lose when they add work without removing the original. Memoization is the canonical "remove the work entirely" pattern.

## 2026-05-07 — Methodology bug in prior composition_only benches

While picking fixtures I noticed `composition_only::bench_chain_sync` calls `compose(chain_1k, ring_1k, Synchronous)`. Both fixtures share their controllable alphabet, and `compose()` validates that controllable labels are disjoint between the two CLTSs. So that bench was measuring the validation error path — which is fast, but is *not composition*. The 5–10 µs medians reported by EXP-0010, EXP-0003, EXP-0030 on `composition_only/chain_sync` are all error-path numbers. The falsification *directions* stand (a slower error path is still a slower path), but the magnitudes don't reflect real compose cost.

The fix for EXP-0011 was to use `RandomClts::new(seed).with_uncontrollable_prefix(3)` so the two CLTSs share their alphabet within the *uncontrollable* prefix only — `compose()` accepts that and runs a real ~6 ms BFS product on 32-state inputs.

A corrigenda note is being appended to those three EXPs. They aren't being re-opened; the falsifications still stand on their narrow claims.

## 2026-05-07 — Cache key design

First cut:

```rust
compose_cache: RefCell<HashMap<(String, String, Discriminant<CompositionSemantics>), Arc<Clts>>>
```

This forces `(left.to_string(), right.to_string(), mode_disc)` on every lookup. Two heap allocs per hit. Cache-hit median was ~110 ns.

Second cut:

```rust
compose_cache: RefCell<HashMap<u64, Arc<Clts>>>
```

Precomputed u64 via `DefaultHasher` (SipHash). No allocations on hit. ~50 ns. But silently wrong on hash collision (1 in 2^64).

Final cut:

```rust
compose_cache: RefCell<HashMap<u64, (String, String, Discriminant, Arc<Clts>)>>
```

u64 key with full-key recheck on hit. On collision the recheck fails and the bucket is treated as a miss. ~46 ns hit cost (the recheck adds nothing measurable when keys match — string equality short-circuits on length first). Final implementation in `crates/mununu-core/src/context/mod.rs`.

The `String::to_string()` path also matters because `compose_named` is called from inside hot DSL evaluation loops. Saving 60 ns per call across O(N_states) compose calls in a real workflow is real wallclock.

## 2026-05-07 — RefCell → Mutex switch after CI gate

The original cache implementation used `RefCell<HashMap<...>>`. The bench numbers in `log.md` (median 46.23 ns hit) were captured against that variant.

When running `make ci`, the workspace clippy gate failed: `Context` is held inside `RealizedContext`, which is held inside the global `OnceLock<Mutex<HashMap<u64, CacheEntry>>>` in `crates/mununu-core/src/api/cache.rs`. The static requires `Sync`; `RefCell` is `!Sync`, so the API cache stopped compiling.

Switched to `std::sync::Mutex<ComposeCacheMap>`. Uncontended `Mutex` cost on macOS x86_64 (Intel i7-9750H) is dominated by a single CAS — measured ~10–20 ns per lock+unlock pair in published microbenchmarks. The cache-hit path now does:

1. `mutex.lock()` (~10 ns)
2. `HashMap::get(&u64)` (same as before)
3. full-key recheck (same as before)
4. `Arc::clone` (~5 ns)
5. drop guard, unlock (~5 ns)

Predicted hit cost: ~60 ns (vs. ~46 ns measured with `RefCell`). The headline magnitude doesn't move (~135,000× → ~104,000×, both within "five orders of magnitude"). Re-running the bench against the Mutex variant is opened as a followup; the current archive is kept as-is with this note. The qualitative claim — *cache hits replace BFS product construction with a single hashmap probe + refcount bump* — is unchanged.

## 2026-05-07 — Why we didn't make this `Arc::clone`-free

A truly free hit would return `&Clts` and let the caller decide whether to clone. But `Context` already gives out `&Clts` for the underlying CLTSs (`Context::clts(name)`), and `compose_named_cached` is meant to be a drop-in for `compose_named` which returns owned. Returning `Arc<Clts>` is a compromise: callers that want `&Clts` can `&*arc`, callers that want ownership keep what they had. The 4-byte refcount bump is below the noise floor.

## 2026-05-07 — Sitting 12: architectural-orphan finding

A grep across the workspace for `compose_named_cached` returned only:
- `crates/mununu-core/benches/composition_only.rs` (this experiment's bench)
- `crates/mununu-core/src/bin/cache_check.rs` (the ad-hoc verifier)
- `crates/mununu-core/src/context/mod.rs` (the implementation)

No production code calls it. The original `compose_named` has only one production caller — `context_dsl/realize.rs:1279` — inside an associative-compose loop where each iteration creates a fresh `temp_context`, calls compose once, and drops it. The cache lives on `Context`; the `Context` is gone before a second call could possibly hit. Outside that loop, nothing in mununu calls compose at all — composition is a one-shot during realize, and the result is stored as a named CLTS.

ADR-0016 captures the decision: keep the code (correct + extensible), append corrigenda, downgrade from "confirmed win" to "infrastructure-correct, no caller today," and pivot to §A6b (BindingStack) as the next structural change.

The lesson worth keeping: the plan's a-priori prediction model (ADR-0014: work-reducing wins, work-adding loses) is necessary but not sufficient. A third bit is needed: **does the existing call graph actually exercise the work being reduced?** The §B3 cache reduces zero work *in the existing call graph*. The L3 protocol couldn't catch this because it measures the bench, not the call graph. The grep step needs to land before the bench, not after.
