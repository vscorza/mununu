# Agentic Orchestration Models

> Source of truth: [`crates/mununu-core/src/adapter/templates/`](../../crates/mununu-core/src/adapter/templates/) (templates registry), [`crates/mununu-core/src/adapter/xstate/`](../../crates/mununu-core/src/adapter/xstate/) (XState adapter), [`crates/mununu-core/src/adapter/crewai/`](../../crates/mununu-core/src/adapter/crewai/) (CrewAI adapter), [`crates/mununu-core/src/adapter/langgraph/`](../../crates/mununu-core/src/adapter/langgraph/) (LangGraph adapter) — surface: CLI+API+UI

Agentic AI orchestration is supported through four entry points:

## 1. Native CTXDSL

Files under [`examples/agentic/`](../../examples/agentic/) — `mcp_auth.ctxdsl`, `handoff_protocol.ctxdsl`, etc. — describe agent / supervisor / worker FSMs directly as automata + properties + controllers. Use this when you want full control over the state space and label vocabulary.

## 2. XState JSON via the existing adapter

Files like `examples/agentic/support_pipeline.xstate.json` use the standard `__mununu` block to declare controllable / uncontrollable events and properties. The XState adapter handles parallel regions and translates them to a synchronous composition.

## 3. CrewAI JSON via the native CrewAI adapter

`.crewai.json` files (canonical `Crew` exports from CrewAI v0.50+) are consumed by [`CrewaiAdapter`](../../crates/mununu-core/src/adapter/crewai/mod.rs). Each agent becomes a per-agent automaton (`Idle -> Executing -> Done` on `agent_<role>_start` / `agent_<role>_complete`), plus a sequential supervisor enforcing declared task order, all composed asynchronously per Doc C §C.5 (LLM latency is non-deterministic). `__mununu` overrides win. `process = "sequential"` is fully modelled; `hierarchical` / `consensual` emit an `ApproximateTranslation` warning and fall back to sequential.

## 4. LangGraph JSON via the native LangGraph adapter

`.langgraph.json` files (LangGraph `StateGraph` exports) are consumed by [`LangGraphAdapter`](../../crates/mununu-core/src/adapter/langgraph/mod.rs). Each node becomes a state; each edge becomes a transition with label `node_<from>_enter` (or `node_<from>_<condition>_enter` for conditional edges). Node-enter labels from `kind = "end"` sources default to uncontrollable; everything else is controllable. `__mununu` overrides win.

Pick the entry point that matches your authoring environment: native CTXDSL for hand-written models, the XState / CrewAI / LangGraph adapters when an upstream tool already emits one of those formats.

## Verifying multi-source agentic projects

For verification problems that compose multiple agentic sources — or one agentic source with non-agentic peers — use the verify framework: drop a `verify.toml` listing the sources, an alphabet binding, a composition shape, and a list of properties, then run `mununu verify <verify.toml>`. The agentic adapters plug into the framework on equal footing with `xstate`, `sv-yosys`, `c-codesign`, etc. See [`wiki/Verify-Project-Flow.md`](../../wiki/Verify-Project-Flow.md) for the conceptual model and the example fleet under [`examples/verify/`](../../examples/verify/).

## Property templates

The property templates registry has an `agentic` domain (see [`crates/mununu-core/src/adapter/templates/`](../../crates/mununu-core/src/adapter/templates/)) that ships parameterized formulas usable from every entry point. Three templates are specific to agentic flows:

| Template | Kind | What it checks |
|---|---|---|
| `bounded_handoff(HANDOFF_TRIGGERED, HANDOFF_COMPLETE)` | liveness | Once the handoff is triggered, completion is reachable from there |
| `no_delegation_cycle(FORWARD, BACKWARD)` | safety | After a forward delegation, the reverse delegation never fires |
| `eventual_completion(TASK_STARTED, TASK_DONE)` | liveness | Every started task reaches a terminal state |

List them with:

```bash
mununu templates --domain agentic
```

## What is NOT shipped

A2A protocol JSON and AutoGen JSON adapters are queued as follow-ups — both have JSON serialisations but enough surface-shape difference from CrewAI / LangGraph to warrant separate adapters. CrewAI `hierarchical` / `consensual` processes fall back to sequential with a warning; LangGraph state-schema lifting to per-state predicates is also a follow-up.

Native parsers' claims about handling these formats end-to-end must be backed by real examples per [`policies/claims-integrity.md`](../policies/claims-integrity.md). The verify fleet's [`crewai_handoff/`](../../examples/verify/crewai_handoff/) and [`langgraph_workflow/`](../../examples/verify/langgraph_workflow/) entries are the supported reference points.
