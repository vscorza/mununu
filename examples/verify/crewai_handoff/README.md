# `crewai_handoff` — verify-framework example for native CrewAI sources

> **Source of truth:** [`crates/mununu-core/src/adapter/crewai/`](../../../crates/mununu-core/src/adapter/crewai/) — surface: CLI+API+UI.

End-to-end demonstration that `mununu verify` accepts a CrewAI
`.crewai.json` source as one of its `[[sources]]` entries. The
[`CrewaiAdapter`](../../../crates/mununu-core/src/adapter/crewai/mod.rs)
translates the crew into per-agent automata (`Idle -> Executing -> Done`
on `agent_<role>_start` / `agent_<role>_complete`) plus a sequential
supervisor, all composed asynchronously per
[Doc C §C.5](../../../docs/design/hw-sw-codesign-extraction.md).

## What it demonstrates

- **Native CrewAI dispatch.** `[[sources]] adapter = "crewai"` — no
  manual rewrite into XState. The adapter handles the `agents`, `tasks`,
  `process = "sequential"` shape directly.
- **First-automaton-per-source composition.** The orchestrator's
  `[composition].members = ["crew"]` resolves to `Agent_Researcher`
  (the first automaton emitted by the source). The CrewAI source's
  full internal composition (with all three automata) is preserved
  verbatim inside the assembled CTXDSL; full multi-automaton-per-source
  composition is queued as an orchestrator follow-up.
- **Agentic property templates in action.** Both properties target
  `Agent_Researcher` directly via the `ResearcherSlice` composition:
  - `no_deadlock` — every reachable agent state has a successor (the
    `Done -> Idle` cycle keeps the agent live).
  - `bounded_handoff(HANDOFF_TRIGGERED = Executing, HANDOFF_COMPLETE
    = Done)` — once in `Executing`, `Done` is always reachable. Maps
    directly to the agentic-domain template added in A3.3.

## Files

| File | Purpose |
|---|---|
| `crew.crewai.json` | Two-agent sequential CrewAI crew (Researcher -> Writer) |
| `verify.toml` | Project config (single source + composition + 2 properties) |
| `validate.sh` | End-to-end reproduction script |
| `transcript.txt` | Byte-deterministic expected output |

## Reproduce

```bash
bash examples/verify/crewai_handoff/validate.sh
```

Re-running against the same commit must produce a byte-identical
`transcript.txt`.

## Run manually

```bash
mununu verify examples/verify/crewai_handoff/verify.toml
mununu verify examples/verify/crewai_handoff/verify.toml --json
mununu verify examples/verify/crewai_handoff/verify.toml --strict  # exits 0 here (both props satisfied)
```
