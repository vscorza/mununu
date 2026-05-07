# EXP-0011-compose-cache: Context-level composition memoization

**One-line summary.** Memoizing `compose_named` at the `Context` level turns the second-and-subsequent compose calls into an `Arc::clone` — measured ≈135,000× faster than rebuilding the product, on a 32-state × 32-state synchronous composition.

## Motivation

`compose_named` rebuilds the product CLTS from scratch on every call (`crates/mununu-core/src/context/mod.rs:350`). DSL workflows that evaluate multiple properties on the same composed automaton pay this cost N times, even though `Context.cltss` is insert-only after `ContextBuilder::finish` — the result is a pure function of `(left, right, mode)` and never goes stale within a Context's lifetime. Plan §B3 proposed memoization keyed on that triple, with the result stored as `Arc<Clts>` so cache hits are a refcount bump.

Prior literature: classical memoization (Michie 1968, "Memo functions and machine learning"); the immutability argument follows from `ContextBuilder` having no mutating methods on registered CLTSs.

## Hypothesis

≥10× wallclock improvement on multi-property DSL workflows where the same `(left, right, mode)` triple is composed repeatedly. Pre-registered before the run.

## Headline result

| arm                            | median       | 95% CI                  |
|--------------------------------|--------------|-------------------------|
| uncached `compose_named`       | **6.247 ms** | [6.189, 6.316] ms       |
| cached `compose_named_cached`  | **46.23 ns** | [46.02, 49.07] ns       |
| **speedup**                    | **≈135,132×** |                        |

Tests: all 825 lib tests green. The cache implementation is collision-safe: stores the full `(left, right, mode_discriminant)` alongside the `Arc<Clts>` and rechecks on hit, so a (vanishingly rare) `u64` hash collision between distinct keys produces a miss rather than wrong data.

## Methodology contribution (and a finding about prior benches)

The fixture choice mattered. The pre-existing `composition_only::bench_chain_sync` and friends use `chain_1k`/`ring_1k` fixtures that share their controllable alphabet. `compose()` rejects shared-controllable-alphabet pairs (validation in `composition::compose`), so those benches were measuring the **error path**, not real composition. The bench in this experiment uses `RandomClts::new(seed).with_uncontrollable_prefix(3)` so labels can be shared across the uncontrollable prefix without tripping validation, producing a real ~6 ms compose call on 32-state inputs.

This invalidates the magnitudes (not the directions) of the falsifications recorded in EXP-0010 (FxHashMap), EXP-0003 (LabelSetTable interning), and EXP-0030 (CSR adjacency) — see `notes.md` and the corrigenda note added to those archives.

## How to replay

```bash
make replay EXP=EXP-0011-compose-cache
```

Or directly:

```bash
cargo bench -p mununu-core --features test_support \
  --bench composition_only -- composition_only/cache_compare
```

Full provenance: `manifest.json`, hardware: `hw-fingerprint.txt`, raw output: `criterion-archive.tar.zst`, lab notebook: `log.md`, observations: `notes.md`.

## Files in this archive

- `README.md` — this file.
- `manifest.json` — provenance: commit, container, hw fingerprint sha, build flags, env, command, timestamps, outcome, code-change status.
- `command.txt` — exact replay command.
- `hw-fingerprint.txt` — output of `scripts/capture_hw.sh` at run time.
- `criterion-archive.tar.zst` — `target/criterion/composition_only_cache_compare` archive.
- `log.md` — dated lab-notebook entry.
- `notes.md` — free-form observations.

## Status

`closed` — implementation correct on the microbench; **downgraded** to "infrastructure-correct, no caller today" by ADR-0016 (sitting 12). The cache lives on `Context`, but mununu composes once during `realize_context` (using ephemeral `temp_context`s) and then operates on the resulting named CLTSs — no production caller invokes `compose_named` on the same `(left, right, mode)` triple twice. Code is kept; speedup will land if/when a workload that recomposes on the fly is added (REPL, interactive editor, etc.).

## Corrigenda (added 2026-05-07, sitting 12)

The 135,132× microbench number is an algorithm characterization, not an end-to-end win. A grep across the workspace found that the only callers of `compose_named_cached` are this experiment's bench and the `cache_check` ad-hoc binary. No production code path (DSL evaluation, synthesis, API server, CLI) invokes `compose_named` repeatedly on the same triple, so the cache hit path is exercised nowhere outside the bench.

The architectural reasons:
- `context_dsl/realize.rs` performs composition inside an associative-compose loop where each iteration creates a fresh `temp_context`, then drops it; the per-`Context` cache cannot survive the drop.
- Post-realize, the composed CLTS is stored as a *named* entry in the final `RealizedContext.context`; downstream evaluation (`evaluate_mu`, `synthesise_controller`) reads it by name, not by composing again.
- The API server caches `RealizedContext` end-to-end (`api/cache.rs`), so a cache below that level is shadowed.

The implementation is kept because (a) it's correct, (b) maintenance cost is trivial, and (c) future workloads (REPL, interactive editor, batch tools that vary composition mode) would benefit immediately. ADR-0016 captures the decision in full.
