# `langgraph_workflow` — verify-framework example for native LangGraph sources

> **Source of truth:** [`crates/mununu-core/src/adapter/langgraph/`](../../../crates/mununu-core/src/adapter/langgraph/) — surface: CLI+API+UI.

End-to-end demonstration that `mununu verify` accepts a LangGraph
`.langgraph.json` source as one of its `[[sources]]` entries. The
[`LangGraphAdapter`](../../../crates/mununu-core/src/adapter/langgraph/mod.rs)
translates the `StateGraph` into one CTXDSL state per node and one
transition per edge — conditional edges get
`node_<from>_<condition>_enter` labels, the `kind = "end"` source's
enter-label is flipped to uncontrollable per Doc C §C.5.

## What it demonstrates

- **Native LangGraph dispatch.** `[[sources]] adapter = "langgraph"` —
  no manual rewrite. The adapter handles the `nodes` + `edges` shape
  directly, including conditional-edge fan-out (`classify` ->
  `billing | tech`) and the `end` node-kind controllability flip.
- **Direct alphabet binding.** `[alphabet] strategy = "direct"` — the
  adapter emits its own canonical alphabet (`node_classify_is_billing_enter`,
  `node_billing_enter`, etc.); no renaming is required.
- **Property templates over LangGraph state names.** Both properties
  reference node ids verbatim as state predicates:
  - `reachable(TARGET = done)` — liveness: from the `classify` entry,
    the `done` terminal is reachable.
  - `mutual_exclusion(A = done, B = classify)` — safety smoke: the
    two states are mutually exclusive (vacuous on a deterministic
    per-step encoding, but demonstrates the template wires through).

## Files

| File | Purpose |
|---|---|
| `workflow.langgraph.json` | 4-node LangGraph (classify -> {billing, tech} -> done) |
| `verify.toml` | Project config (single source + composition + 2 properties) |
| `validate.sh` | End-to-end reproduction script |
| `transcript.txt` | Byte-deterministic expected output |

## Reproduce

```bash
bash examples/verify/langgraph_workflow/validate.sh
```

Re-running against the same commit must produce a byte-identical
`transcript.txt`.

## Run manually

```bash
mununu verify examples/verify/langgraph_workflow/verify.toml
mununu verify examples/verify/langgraph_workflow/verify.toml --json
mununu verify examples/verify/langgraph_workflow/verify.toml --strict  # exits 0 here (both props satisfied)
```
