# EXP-0001-baseline-cliff: pre-optimization baseline freeze

**One-line summary.** Captures the wall-clock cost of CLTS construction, composition, bisimulation minimization, and mu-calculus evaluation on canonical fixtures *before* any optimization in the programme lands. Every subsequent EXP regresses against these numbers.

## Motivation

The optimization programme described in [`notebook/0000-overview.md`](../../notebook/0000-overview.md) needs a reference point. Without a frozen baseline, "5× speedup" claims have no anchor; reviewers can't verify whether reported gains exceed measurement noise.

The inventory in the plan file (`~/.claude/plans/do-a-deep-evaluation-sparkling-origami.md`) cites several hot spots: `Vec<Vec<Transition>>` adjacency (`clts/mod.rs:949-951`), naive Kanellakis-Smolka minimization (`composition/minimize.rs:60-197`), per-iteration `HashMap<FormulaVarId, BitVec>` cloning in mu-calculus fixpoints (`evaluator.rs:1593`). EXP-0001 measures the cost of these without changing anything.

## Hypothesis

This experiment has no hypothesis under test — it is a measurement-establishing experiment. We expect:

- **CLTS construction** to scale super-linearly in state count due to repeated `Vec` reallocations (~20%-growth amortization plus the staging-buffer-then-reflush at `build()`).
- **Naive minimization** of an *already-minimal* CLTS to be measurably expensive: the K-S loop touches every transition every iteration, and termination is detected only when the partition stops changing.
- **Mu-calculus reachability** to scale roughly linearly in state count for the simple `mu X. (target or <> X)` formula, with a small constant per state for the modal pre-image walk.

## Headline result (smoke run, `--quick`)

Numbers from `cargo bench -p mununu-core --features test_support --bench <name> -- --quick` on the runner described in `hw-fingerprint.txt`. Full Criterion runs (100 samples, 3s warmup) recorded under `criterion-archive.tar.zst`.

| Bench | Median (smoke) |
|-------|----------------|
| `clts_construction/chain/100000` | **556 ms** |
| `clts_construction/grid/64x64` | 16.9 ms |
| `clts_construction/random_seeded/1024` (density 0.10) | 90 ms |
| `composition_only/chain_sync` (chain_1k × ring_1k) | 14.8 µs |
| `composition_only/grid_async` (grid_32x32 × grid_32x32) | 10.3 µs |
| `minimization_only/chain_minimal` (chain_1k, already minimal) | **1.51 s** |
| `minimization_only/grid_minimal` (grid_32x32) | 82 ms |
| `minimization_only/random_redundant` (random_512_d20) | 168 ms |
| `mu_calculus_only/propositional/chain_1k` | 72 µs |
| `mu_calculus_only/reachability_mu/grid_32x32` | 109 ms |
| `mu_calculus_only/invariance_nu/grid_32x32` | 3.1 ms |

The 1.5-second cost on `chain_1k` minimization is the most striking single number: chains of 1000 states are already strongly minimal under bisimulation, so K-S converges in two iterations. Each iteration scans every transition + signature-hashes — that's the brute-force cost we expect Paige-Tarjan to retire.

Tests: green `make ci` at the recording commit.

## How to replay

```bash
cargo bench -p mununu-core --features test_support --bench clts_construction
cargo bench -p mununu-core --features test_support --bench composition_only
cargo bench -p mununu-core --features test_support --bench minimization_only
cargo bench -p mununu-core --features test_support --bench mu_calculus_only
```

Or, after `make replay EXP=EXP-0001-baseline-cliff` (replays the canonical command in `command.txt`).

## Status

`open` (recording) → flips to `closed` once `make publish-prep` is green.

## Files

- `README.md` — this file.
- `log.md` — lab notebook entry.
- `notes.md` — observations + dead-ends + followups.
- `manifest.json` — provenance.
- `command.txt` — exact replay command.
- `hw-fingerprint.txt` — runtime hardware/software fingerprint.
- `criterion-archive.tar.zst` — raw Criterion JSON (one per bench).
