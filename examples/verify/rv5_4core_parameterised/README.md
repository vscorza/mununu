# `rv5_4core_parameterised` — parameterised-instance support demo

> **Source of truth:** [`crates/mununu-core/src/verify/config.rs`](../../../crates/mununu-core/src/verify/config.rs) (`SourceSection::count`) and [`crates/mununu-core/src/verify/orchestrator.rs`](../../../crates/mununu-core/src/verify/orchestrator.rs) (`{instance_id}` substitution) — surface: CLI+API+UI.

ONE `[[sources]]` declaration with `count = 4` expands to four independent per-core pipeline automata. Without parameterisation (plan Part 6 item 6), the same model would require four duplicated `[[sources]]` blocks plus four near-identical CTXDSL files. With it, the manifest stays tiny and one source-of-truth CTXDSL file is the only thing the user authors.

## What it demonstrates

- **`count = N` field on `[[sources]]`**: declares that the entry should expand to N virtual instances named `<id>_0` .. `<id>_<N-1>`.
- **`{instance_id}` placeholder substitution**: the verify orchestrator replaces every `{instance_id}` occurrence in the source file with the instance's full id (`core_0`, `core_1`, etc.) before the adapter sees the content. The CTXDSL emits unique automaton + state + label names per instance.
- **The composition references each expanded instance directly** (`members = ["core_0", "core_1", "core_2", "core_3"]`). Alternatively the wildcard form `<src>.*` from plan Part 6 item 4 would also work — these features compose.
- **State-space scaling matches expectation**: 3 states per core × 4 cores = 3^4 = 81 reachable composed states. All four per-core reachability properties hold.

## Files

| File | Purpose |
|---|---|
| `core_pipeline.ctxdsl` | Parameterised per-core pipeline (3 states: Idle → Working → Done). `{instance_id}` placeholders make every name instance-unique. |
| `verify.toml` | One source declaration (`count = 4`) + composition + four per-core properties |
| `validate.sh` | End-to-end reproduction script |
| `transcript.txt` | Byte-deterministic expected output including the full introspection report |

## Reproduce

```bash
bash examples/verify/rv5_4core_parameterised/validate.sh
```

## Why this matters

The plan's Part 3 RISC-V 4-core scenario estimated ~80 lines of `[[sources]]` declarations across four near-duplicate CTXDSL files when each per-core component (pipeline, L1 cache, register file) needed its own block. With `count = N` + `{instance_id}` substitution, each component class becomes **one** source declaration regardless of the core count.

For a 64-core SoC verification model, this is the difference between 192 lines of manifest boilerplate (3 components × 64 cores) and 9 lines (3 components × 1 `count = 64` block each). The CTXDSL source files are written once and reused per instance.

## What this slice deliberately does not cover

- **Per-instance options.** Each instance receives identical `options`. Use-case-specific per-instance configuration is a follow-up — for now, parameterised instances must be structurally identical.
- **Instance-specific binding renamings.** The renamings strategy still keys on the original source id, applying the same renaming map to every instance. Per-instance renamings would need a `{instance_id}` placeholder in `[[alphabet.renamings]]` (queued).
- **Wildcard member resolution.** The `<src>.*` syntax from item 4 expands an UNPARAMETERISED multi-automaton source. For a parameterised source where each instance emits one automaton, listing each instance directly (as in this fixture) is the canonical form. A future combined `<src>.*` over parameterised sources would expand to every (instance, automaton) pair.
