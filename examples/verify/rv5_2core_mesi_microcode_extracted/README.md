# `rv5_2core_mesi_microcode_extracted` — microcode-adapter parity fixture

> **Source of truth:** [`crates/mununu-core/src/adapter/microcode/`](../../../crates/mununu-core/src/adapter/microcode/) — surface: CLI+API+UI.

Same 2-core MESI + 3-step microprogram scenario as the hand-authored [`rv5_2core_mesi_microprogram/`](../rv5_2core_mesi_microprogram/) fixture — but the microprogram is **extracted from JSON microcode** via the v1 microcode adapter (plan Part 5.5 + Part 6 item 5).

## What it demonstrates

- **The microcode adapter as a drop-in replacement for hand-authored CTXDSL.** The hand-authored sibling fixture writes ~50 lines of CTXDSL declaring the microprogram's states + transitions + controllability. This fixture replaces it with ~30 lines of JSON microcode; the adapter does the CLTS translation.
- **Verdict equivalence.** Identical state count (14 reachable composed states), identical verdicts on every property: `mesi_coherence_invariant`, `write_visible_to_memory`, `microprogram_runs_to_completion` all SATISFIED. The adapter preserves semantics; only the authoring surface changes.
- **Per-issuer label tagging.** Memory ops carry an optional `tag` field that the adapter folds into the emitted label (e.g. `wr_mem_x_core_0`). Lets one microcode source distinguish its writes from another core's snoops.
- **`extra_controllable` for cross-cutting labels.** The microcode source declares two labels (`wr_mem_x_core_1`, `rd_mem_x_core_0`) it doesn't itself fire but the L1 caches reference — claims sole ownership to resolve the realiser's duplicate-controllable check.

## Files

| File | Purpose |
|---|---|
| `microprogram.microcode.json` | JSON microcode; the verification target. ~30 LOC. |
| `l1_cache_core0.ctxdsl` | Per-core L1 cache for core 0; 4 MESI states; same shape as the hand-authored sibling, label naming updated to the microcode-adapter's convention |
| `l1_cache_core1.ctxdsl` | Mirror of `l1_cache_core0` |
| `memory.ctxdsl` | 2-state shared-memory model |
| `verify.toml` | 4 sources + asynchronous composition + 3 properties |
| `validate.sh` | End-to-end reproduction script |
| `transcript.txt` | Byte-deterministic expected output |

## Reproduce

```bash
bash examples/verify/rv5_2core_mesi_microcode_extracted/validate.sh
```

Re-running against the same commit produces a byte-identical `transcript.txt`.

## Authoring delta vs the hand-authored sibling

| Concern | Hand-authored (`rv5_2core_mesi_microprogram/microprogram.ctxdsl`) | Extracted (`microprogram.microcode.json`) |
|---|---|---|
| Lines (microprogram source) | ~50 | ~30 |
| Hand-authored states + transitions | yes | no — adapter generates |
| Hand-authored controllability block | yes | no — adapter generates from op classification |
| Round-trip after a microcode revision | re-edit CTXDSL | re-run `mununu verify` |
| Counterexample readability | each step opaque | each state named after the microcode `step.id` |

This is the **parity** demo. The next fixture in the queue — [`dma_engine_microcode/`](../dma_engine_microcode/) — is the industrial-impactful significant extraction the plan's Part 5.5 names: a real DMA-engine sequencing model that demonstrates the adapter on a problem space mununu can't already handle by hand-authoring at scale.
