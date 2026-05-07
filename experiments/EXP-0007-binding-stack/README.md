# EXP-0007-binding-stack: replace per-fixpoint-iteration HashMap clone with enter/restore RAII

**One-line summary.** Falsified. The §A6b "BindingStack" pattern (mutate bindings in place inside `eval_fixpoint`, save/restore at scope boundaries) regressed 2/10 benches at p<0.001 (+11.6% and +28.1%) with no improvements; reverted.

## Motivation

`eval_fixpoint` (`crates/mununu-core/src/mu_calculus/evaluator.rs:1741`) clones the entire `HashMap<FormulaVarId, BitVec<usize, Lsb0>>` on every iteration of the fixpoint loop. At alternation depth K, that's K BitVec deep-clones per iteration just to maintain lexical scoping. Plan §A6b proposed: take `bindings: &mut HashMap`, save the previous binding for `var` at fixpoint entry, mutate `bindings` in place each iteration, restore the previous binding at exit. This eliminates the per-iteration HashMap clone in favor of a single `insert` per iteration and a single `remove`+restore at the boundary.

Per ADR-0014's prediction model ("changes that reduce total work win, changes that add work lose"), this is shaped as a "reduce total work" change — so the prediction was **likely win**. EXP-0007 tested that prediction.

Prior literature: stack-discipline interpretation of mu-calculus binders (Bradfield & Stirling, "Modal mu-calculi" handbook chapter, 2007); standard environment-frame optimization in interpreters (Appel, "Compiling with Continuations", 1992).

## Hypothesis

≥1.3× speedup on alternation-depth-3 fixpoint benches; near-neutral on alternation-1 (where K=0 means HashMap.clone is empty-map memcpy). Pre-registered before the run.

## Headline result

`bench_diff.sh exp-0007-bindings --robust` (Mann-Whitney p<0.01, ±10% threshold):

| bench | A median | B median | Δ | p |
|-------|---------:|---------:|--:|--:|
| `mu_calculus_only/reachability_mu/grid_32x32` | 16.96 ms | 21.21 ms | **+28.1%** | <0.001 |
| `mu_calculus_only/synthesis_product_game/grid_32x32` | 1.132 s | 1.219 s | **+11.6%** | 0.001 |
| `mu_calculus_only/propositional/chain_1k` | 12.7 µs | 13.6 µs | +9.3% | <0.001 |
| `mu_calculus_only/invariance_nu/chain_1k` | 452.5 µs | 478.1 µs | +7.4% | 0.011 |
| `mu_calculus_only/invariance_nu/grid_32x32` | 557.3 µs | 587.9 µs | +7.4% | 0.451 |
| `mu_calculus_only/synthesis_product_game/ring_1k` | 16.0 ms | 16.4 ms | +5.5% | 0.548 |
| `mu_calculus_only/propositional/grid_32x32` | 12.1 µs | 12.9 µs | +4.9% | <0.001 |
| `mu_calculus_only/reachability_mu/chain_1k` | 9.75 ms | 9.84 ms | +0.7% | 0.128 |
| `mu_calculus_only/reachability_mu/ring_1k` | 11.20 ms | 10.76 ms | -1.3% | 0.882 |
| `mu_calculus_only/invariance_nu/ring_1k` | 456.7 µs | 435.4 µs | -6.1% | <0.001 |

5 of 10 benches significantly regress (p<0.05). 1 improves. 4 neutral.

Tests: 825 lib tests pass (the change is observably equivalent — same fixpoint result). Pre-existing `properties::minimization::idempotence` proptest failure (seed 9382785361923416088) is orthogonal to this change.

## Why it lost (the prediction was wrong)

The "reduce total work" framing missed an empirical fact: **`HashMap::clone()` on a small map is faster than `HashMap::insert`**. The std HashMap stores entries in a contiguous `RawTable` and clones via bulk memcpy of the bucket array; insert recomputes the hash and probes the table.

At alternation depth K=0 (most of `propositional`, `reachability_mu`, `invariance_nu` — bindings is empty when entering the outer fixpoint):
- OLD per-iter: `bindings.clone()` of empty HashMap = ~24 bytes memcpy + 1 BitVec clone for the var insert. Total: ~24 bytes + 1 deep clone.
- NEW per-iter: `bindings.insert(var, ...)` = 1 hash compute + 1 bucket probe + 1 BitVec drop + 1 BitVec deep clone. Total: ~30 ns of hash/probe + 1 deep clone + 1 drop.

NEW is **more work** at K=0. The per-iteration savings hypothesis only kicks in at K≥1, but even then:
- K=1: OLD does `bindings.clone()` (1 BitVec deep-clone for the existing var) + 1 BitVec clone for the new var. NEW does 1 BitVec drop + 1 BitVec clone.
- Difference: 1 BitVec deep clone (~128 bytes for 1024-state BitVec). At memory bandwidth that's ~10 ns saved per iteration. The hash+probe overhead in NEW is ~30 ns. **NEW still loses at K=1** by ~20 ns/iter.

The crossover where NEW wins would be at K≥2 (alternation-3 or deeper) — which mununu doesn't have in its test fixtures. In practice no production formula is deeper than alternation-2. The "less work" prediction was *asymptotically* correct but *measurably* wrong at the alternation depths that matter.

## How to replay

```bash
make replay EXP=EXP-0007-binding-stack
```

Or directly: see [`command.txt`](command.txt) for the A/B sequence.

## Status

`closed` — hypothesis falsified, code change reverted. Joins §A2 (LabelSetTable), §A3-via-staging (CSR), §B2 (FxHashMap), §B4 (changed-flag) in the falsification column. Plan budget after sitting 12: 2 confirmed (§A1 SoA, §A4 Vec doubling), 1 infrastructure-orphan (§B3), 5 falsified (§A2, §A3-via-staging, §B2, §B4, §A6b).
