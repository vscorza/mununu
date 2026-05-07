# EXP-0007-binding-stack: BindingStack falsified

**Author:** Mariano Cerrutti (vscorza@gmail.com)
**Date opened:** 2026-05-07
**Date closed:** 2026-05-07
**Commit baseline:** e6edcfe831d8cdd736aed857719ec2a498219bb9
**Commit candidate:** e6edcfe (working tree, reverted post-measurement)
**Container digest:** host run (uncontainerized) — see hw-fingerprint.txt
**Hardware:** Intel Core i7-9750H @ 2.6 GHz, macOS 26.3, 16 GB RAM

## Motivation

`eval_fixpoint` clones the entire `bindings: HashMap<FormulaVarId, BitVec>` on every iteration of the fixpoint loop (`mu_calculus/evaluator.rs:1741`). At alternation depth K, that's K BitVec deep-clones per iteration purely to maintain lexical scoping. The plan §A6b proposed a stack-discipline interpretation: take `bindings: &mut HashMap`, save the previous binding for `var` at fixpoint entry, mutate `bindings` in place each iteration, restore the previous binding at exit.

ADR-0014 prediction: **likely win** (work-reducing structural change).

## Hypothesis

≥1.3× speedup on alternation-depth-3 fixpoint benches; near-neutral on alternation-1 (where K=0 means HashMap.clone is empty-map memcpy with no BitVec deep-clones).

## Method

- **Inputs.** Fixtures: `chain_1k`, `ring_1k`, `grid_32x32` from the cached fixture set. Formulas: propositional (alternation-0), `mu X. (target or <> X)` (alt-1), `nu X. (safe and [] X)` (alt-1), `nu X. ((mu Y1. (target or <> Y1)) and (mu Y2. (safe or <> Y2)) and [] X)` (alt-2 GR(1)).
- **Bench.** `crates/mununu-core/benches/mu_calculus_only.rs` (existing, unchanged).
- **L3 protocol.** Same-session A/B with `--quick` warmup discard. A=clone-bindings (HEAD baseline). B=enter-restore (BindingStack candidate). 30 samples each at 10s measurement window (synthesis at 30s × 15 samples).
- **Test gate.** `cargo test -p mununu-core --features test_support --lib` — 825 lib tests must stay green.
- **Statistical test.** `bench_diff.sh --robust`: Mann-Whitney U with p<0.01 + ±10% threshold.

## Results

`bench_diff.sh exp-0007-bindings --robust`:

REGRESSIONS (2):
- `mu_calculus_only/reachability_mu/grid_32x32`: 16.96 ms → 21.21 ms, **+28.1%** (CI [+21.6%, +34.3%], p<0.001)
- `mu_calculus_only/synthesis_product_game/grid_32x32`: 1.132 s → 1.219 s, **+11.6%** (CI [+5.2%, +18.7%], p=0.001)

IMPROVEMENTS (0).

NEUTRAL (8): `propositional/chain_1k` +9.3% (p<0.001), `invariance_nu/chain_1k` +7.4% (p=0.011), `invariance_nu/grid_32x32` +7.4% (p=0.451), `invariance_nu/ring_1k` -6.1% (p<0.001), `synthesis_product_game/ring_1k` +5.5% (p=0.548), `propositional/grid_32x32` +4.9% (p<0.001), `reachability_mu/chain_1k` +0.7% (p=0.128), `reachability_mu/ring_1k` -1.3% (p=0.882).

5 of 10 benches significantly regress at p<0.05; 1 improves.

Tests: 825 lib tests pass with the BindingStack change. The pre-existing `properties::minimization::idempotence` proptest failure (seed 9382785361923416088 in `tests/properties/minimization.proptest-regressions`) is orthogonal — the change touches `mu_calculus/evaluator.rs`, not `composition/minimize.rs`.

## Interpretation

The hypothesis was wrong about the *crossover point*. NEW does fewer BitVec deep-clones per iteration than OLD when K≥1, but it does MORE work at K=0:

| Path | Per-iteration cost (K=0) |
|------|--------------------------|
| OLD: `bindings.clone()` + 1 var insert + 1 deep clone | ~24-byte memcpy of empty-HashMap + 1 BitVec clone |
| NEW: `bindings.insert(var, ...)` | 1 hash compute (~10 ns) + 1 probe (~5 ns) + 1 BitVec drop (~10 ns) + 1 BitVec clone |

At K=0, NEW is ~25 ns *more* work per iteration. With ~10⁴–10⁵ inner iterations on the larger fixtures, that compounds. The synthesis_product_game/grid_32x32 fixture has alternation-2 (K=1 inside Y1/Y2), where NEW saves ~10 ns of memcpy but pays ~30 ns of hash/probe per iteration — net loss.

Mununu's actual formulas top out at alternation-2. The crossover where NEW wins is alternation-3+. So the §A6b *implementation* is asymptotically correct but *measurably* wrong at the depths mununu cares about.

## Refined ADR-0014 prediction model

The taxonomy "work-reducing wins, work-adding loses" is necessary but not sufficient. We need a third bit: **does the work-reduction operate on a path the std/bitvec/hashmap primitives haven't already optimized?** If the OLD path uses a primitive's hot loop (`HashMap::clone` is a bulk memcpy; `BitVec::==` is a word-parallel early-exit loop), replacing it with seemingly-leaner per-step ops loses to the constant factor.

This is the same finding as ADR-0013 (§B4 changed-flag falsification): "the std/bitvec primitives already optimize the targeted path." §A6b is the second instance.

## Dead-ends

- **Initial design used a closure for RAII restore.** Compiled but ugly; switched to two-function split (outer `eval_fixpoint` manages prev save/restore, inner `eval_fixpoint_in_scope` runs the loop).
- **Considered moving bindings to `EvalContext` field.** Rejected — borrow checker friction with `&mut self.bindings` while `eval_node` borrows `&mut self`. The `&mut HashMap` parameter pattern is the cleanest Rust idiom.

## Followups

- **EXP-0007-deep-alternation.** Build a synthetic alternation-3 or alternation-4 fixture and re-run the BindingStack bench. If NEW wins decisively at depth ≥3, the implementation has a niche use; otherwise the design is dead.
- **EXP-0040-minimize-bug.** Pre-existing `properties::minimization::idempotence` failure with seed 9382785361923416088. Investigate whether `minimize_bisimulation`'s K-S algorithm has a refinement-order bug or the property test claim is too strong.

## Artifacts

- `criterion-archive.tar.zst` (sha256: 6e90ae918b655d4eed95e67f83b5bc933795a8962106ee6ae4bfd0f609c55576)
- `hw-fingerprint.txt` (sha256: 4100ef26f1a755e17c7181f730787cf76275ac4e116d5d312fc5a33da7f83231)
- `manifest.json` (links the archives + commit + container)
