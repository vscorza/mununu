# `xstate_pair` — verify-framework smoke test

> **Source of truth:** [`crates/mununu-core/src/verify/`](../../../crates/mununu-core/src/verify/) — surface: CLI+API+UI.

The simplest end-to-end demonstration of the
[`mununu verify`](../../../crates/mununu-core/src/verify/orchestrator.rs) flow:
two XState machines, asynchronous composition, direct alphabet binding
(no renaming), and two properties (one template-sourced, one inline
formula). No real-world domain content — the point is to anchor the
framework's smoke tests against a minimal, deterministic fixture.

## What it demonstrates

- **Multiple sources, same adapter.** `[[sources]] adapter = "xstate"`
  twice; the orchestrator dispatches each through
  `XStateAdapter::translate` independently.
- **Direct alphabet binding.** `[alphabet] strategy = "direct"` (the
  default when `[alphabet]` is omitted) — the two machines use
  disjoint event names (`open_request`/`close_request` vs.
  `tick_gate`), so no renaming is required.
- **Asynchronous composition.** `[composition] semantics =
  "asynchronous"` — at each step either machine can take a transition,
  not both simultaneously.
- **Mixed property sources.** One property uses
  `template = "no_deadlock"` (resolved via the builtin
  [`TemplateRegistry`](../../../crates/mununu-core/src/adapter/templates/));
  the other uses an inline `formula = "true"` literal.

## Files

| File | Purpose |
|---|---|
| `lock.xstate.json` | XState machine: a 2-state lock |
| `gate.xstate.json` | XState machine: a 2-state gate |
| `verify.toml` | Project config (sources + composition + properties) |
| `validate.sh` | End-to-end reproduction script |
| `transcript.txt` | Byte-deterministic expected output |

## Reproduce

```bash
bash examples/verify/xstate_pair/validate.sh
```

Re-running against the same commit must produce a byte-identical
`transcript.txt`.

## Run manually

```bash
mununu verify examples/verify/xstate_pair/verify.toml
mununu verify examples/verify/xstate_pair/verify.toml --json
mununu verify examples/verify/xstate_pair/verify.toml --strict   # exits 0 here (both props satisfied)
```
