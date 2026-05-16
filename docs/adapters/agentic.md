# Agentic Orchestration Models

> Source of truth: [`crates/mununu-core/src/adapter/templates/`](../../crates/mununu-core/src/adapter/templates/) (templates registry) and [`crates/mununu-core/src/adapter/xstate/`](../../crates/mununu-core/src/adapter/xstate/) (XState adapter) — surface: CLI+API+UI

Agentic AI orchestration is currently modeled in two ways:

## 1. Native CTXDSL

Files under [`examples/agentic/`](../../examples/agentic/) — `mcp_auth.ctxdsl`, `handoff_protocol.ctxdsl`, etc. — describe agent / supervisor / worker FSMs directly as automata + properties + controllers. Use this when you want full control over the state space and label vocabulary.

## 2. XState JSON via the existing adapter

Files like `examples/agentic/support_pipeline.xstate.json` use the standard `__mununu` block to declare controllable / uncontrollable events and properties. The XState adapter handles parallel regions and translates them to a synchronous composition.

Pick XState when an upstream tool already emits XState JSON; pick CTXDSL when the model is being authored fresh.

## Property templates

The property templates registry has an `agentic` domain (see [`crates/mununu-core/src/adapter/templates/`](../../crates/mununu-core/src/adapter/templates/)) that ships parameterized formulas — mutual-exclusion, no-livelock, bounded-handoff — usable from either entry point.

List them with:

```bash
mununu templates --domain agentic
```

## What is NOT shipped

There is **no native CrewAI / LangGraph / A2A JSON parser** in the Rust workspace today. The Python scripts under [`tools/`](../../tools/) are the only path for live introspection of CrewAI / LangGraph / A2A Python objects; for JSON input you must either rewrite as XState or hand-author CTXDSL.

Adding native parsers is a deliberate future-work item, not a shipped feature. Anyone documenting these capabilities must label them accordingly under [`policies/claims-integrity.md`](../policies/claims-integrity.md) — claims about handling these formats end-to-end require a real example, not a hand-authored sketch.
