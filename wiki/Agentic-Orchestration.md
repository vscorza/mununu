# Agentic AI Orchestration

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change.

Mununu can formally verify and synthesize controllers for **agentic AI orchestration workflows** — the coordination of autonomous LLM-powered agents that reason, use tools, and collaborate.

Agent orchestration workflows are reactive systems: the orchestrator (controller) chooses which agent to activate, which tool to invoke, and when to hand off, while the environment (user input, LLM responses, tool results) produces nondeterministic outcomes. This maps directly to Mununu's CLTS model with controllable/uncontrollable label semantics.

## Why Formal Verification for Agents?

Current orchestration frameworks (LangGraph, CrewAI, OpenAI Agents SDK) use state-machine abstractions but lack formal guarantees:

| Problem | Current Mitigation | Mununu Approach |
|---------|-------------------|-----------------|
| Deadlocks / infinite loops | Retry budgets, timeouts | Exhaustive reachability analysis |
| Unauthorized tool calls | Per-tool `if` checks | Protocol-level safety synthesis |
| State inconsistency | Reducer-driven schemas | Compositional verification |
| Non-termination | Hard timeout limits | GR(1) liveness with fairness |
| Handoff cycles | Convention and testing | Bounded delegation proofs |

When synthesis succeeds, Mununu produces a **concrete safe controller** — a restricted state machine that provably satisfies the specification. When it fails, the **counterstrategy** shows the exact scenario where the environment forces a violation — directly usable as a regression test.

## Use Cases

### 1. Agent Workflow Verification (XState Import)

Model an agent workflow as an XState statechart with `__mununu` annotations, import into Mununu, verify safety/liveness properties, and export the synthesized controller back to XState JSON.

**Example:** Customer support pipeline with budget tracking.

```json
{
  "id": "support_pipeline",
  "initial": "system",
  "states": {
    "system": {
      "type": "parallel",
      "states": {
        "triage": {
          "initial": "waiting",
          "states": {
            "waiting":    { "on": { "TICKET_IN": "classifying" } },
            "classifying": { "on": {
              "ROUTE_BILLING": "routed_billing",
              "ROUTE_TECH": "routed_tech",
              "ESCALATE": "escalated"
            }},
            "routed_billing": { "on": { "RESOLVED": "waiting", "ESCALATE": "escalated" } },
            "routed_tech":    { "on": { "RESOLVED": "waiting", "ESCALATE": "escalated" } },
            "escalated":      { "on": { "HUMAN_RESOLVED": "waiting" } }
          }
        },
        "budget": {
          "initial": "under_limit",
          "states": {
            "under_limit": { "on": { "TOOL_CALL": "under_limit", "LIMIT_HIT": "over_limit" } },
            "over_limit":  { "on": { "BUDGET_RESET": "under_limit" } }
          }
        }
      }
    }
  },
  "__mununu": {
    "controllable": ["ROUTE_BILLING", "ROUTE_TECH", "ESCALATE", "TOOL_CALL"],
    "uncontrollable": ["TICKET_IN", "RESOLVED", "HUMAN_RESOLVED", "LIMIT_HIT", "BUDGET_RESET"],
    "properties": [{
      "name": "no_tool_over_budget",
      "formula": "nu X. ((!over_limit || ([TOOL_CALL] false)) && ([] X))",
      "role": "guarantee"
    }]
  }
}
```

```bash
# Verify and synthesize
mununu context synth support_pipeline.xstate.json \
    --adapter xstate \
    --formula no_tool_over_budget \
    --automaton support_pipeline_system \
    --output-format xstate
```

### 2. MCP Tool Authorization Protocol

Model Context Protocol (MCP) tool authorization as a CLTS composition of session and confirmation automata. Synthesize a controller that enforces session-aware access and confirmation gates.

```ctxdsl
context mcp_auth {
    alphabet {
        label call_read; label call_write; label call_delete;
        label request_confirm;
        label session_start; label session_expire;
        label user_confirm; label user_deny; label noop;
    }

    automata {
        automaton Session {
            controllable { label call_read; label call_write; label noop; }
            states { state NoSession initial; state Active; state Expired; }
            transitions {
                transition NoSession -> Active on label session_start;
                transition Active -> Expired on label session_expire;
                transition Active -> Active on label call_read;
                transition Active -> Active on label call_write;
                transition Active -> Active on label call_delete;
                // ... full model in examples/agentic/mcp_auth.ctxdsl
            }
        }

        automaton Confirmation {
            controllable { label request_confirm; label call_delete; }
            states { state Idle initial; state Pending; state Confirmed; state Denied; }
            transitions {
                transition Idle -> Pending on label request_confirm;
                transition Pending -> Confirmed on label user_confirm;
                transition Confirmed -> Idle on label call_delete;
                // ... full model in examples/agentic/mcp_auth.ctxdsl
            }
        }
    }

    composition {
        asynchronous auth_system { members [Session, Confirmation]; }
    }

    mu_formulas {
        // No write/delete without active session
        formula session_required {
            over auth_system;
            body = nu X. (
                ((! NoSession) || (([call_write] false) && ([call_delete] false)))
                && ((! Expired) || (([call_write] false) && ([call_delete] false)))
                && ([] X)
            );
        }
        // Delete requires user confirmation
        formula confirm_before_delete {
            over auth_system;
            body = nu X. (((Confirmed) || ([call_delete] false)) && ([] X));
        }
    }
}
```

### 3. Multi-Agent Handoff Protocol

Model a supervisor + specialist agent handoff system. Verify mutual exclusion, task completion, and graceful degradation.

```ctxdsl
context handoff_protocol {
    // Supervisor: 4 states (Idle, Dispatching, Waiting, Recovering)
    // AgentA, AgentB: 3 states each (Idle_i, Working_i, HandingOff_i)
    // Composed asynchronously

    mu_formulas {
        // At most one agent working at a time
        formula mutex {
            over handoff_system;
            body = nu X. (((! WorkingA) || (! WorkingB)) && ([] X));
        }
        // Every dispatched task eventually returns to Idle
        formula supervisor_completes {
            over handoff_system;
            body = nu X. (((! Dispatching) || (mu Y. (Idle || <> Y))) && ([] X));
        }
        // GR(1): GF(Dispatching) -> GF(Idle)
        formula no_orphaned_tasks {
            over handoff_system;
            body = (! (nu A. (mu B. (Dispatching || <> B)) && ([] A)))
                || (nu C. (mu D. (Idle || <> D)) && ([] C));
        }
    }
}
```

Full examples: `examples/agentic/handoff_protocol.ctxdsl`

## Property Templates

Common agentic properties and their mu-calculus encodings:

### Safety

| Property | Template |
|----------|----------|
| No unauthorized tool | `nu X. ((!FORBIDDEN_STATE \|\| ([TOOL] false)) && ([] X))` |
| Mutual exclusion | `nu X. ((!WORKING_A \|\| (!WORKING_B)) && ([] X))` |
| Session-required access | `nu X. ((!NoSession \|\| ([OP] false)) && (!Expired \|\| ([OP] false)) && ([] X))` |
| Confirmation gate | `nu X. ((CONFIRMED \|\| ([DESTRUCTIVE_OP] false)) && ([] X))` |
| Budget bound | `nu X. ((!OVER_LIMIT \|\| ([CONSUME] false)) && ([] X))` |

### Liveness

| Property | Template |
|----------|----------|
| Eventual completion | `nu X. ((!ACTIVE \|\| (mu Y. (DONE \|\| <> Y))) && ([] X))` |
| No starvation (GR(1)) | `(! (nu A. (mu B. (TRIGGER \|\| <> B)) && ([] A))) \|\| (nu C. (mu D. (RESPONSE \|\| <> D)) && ([] C))` |
| Eventual recovery | `nu X. ((!FAILURE \|\| (mu Y. (RECOVERED \|\| <> Y))) && ([] X))` |
| Reachability | `mu X. (TARGET \|\| <> X)` |

## Scalable Benchmarks

Generators for scalable agentic benchmarks (in the `mununu-private` artifact):

- `gen_handoff.py --agents N` — N-agent handoff protocol (product states: 4 * 3^N)
- `gen_mcp.py --tools N` — N-tool MCP authorization (formula count scales linearly)
- `run_agentic.sh` — full benchmark suite with TSV output

## Composition and State Predicates

State name predicates (e.g., `Working`, `Idle`, `Confirmed`) used in formulas over composed automata are automatically projected from member automata onto the product state space. When a predicate name appears in multiple members, the projections are OR-merged — a composed state satisfies the predicate if **any** component is in that state.

```
Formula: (!WorkingA || !WorkingB)
Composed state: Waiting|WorkingA|IdleB
  → WorkingA projects to true (AgentA component)
  → WorkingB projects to false (AgentB component is IdleB)
  → (!true || !false) = (false || true) = true ✓
```

## Framework Integration

Mununu includes **native Rust adapters** for three agentic AI orchestration frameworks. Each adapter parses the framework's JSON format directly — no Python or external tools required.

### LangGraph

```bash
# Direct import via native adapter (auto-detected from extension)
mununu context synth workflow.langgraph.json \
    --formula safety_invariant --automaton langgraph_workflow

# Or with explicit adapter flag
mununu context synth graph.json --adapter langgraph \
    --formula safety_invariant --automaton langgraph_workflow
```

**Input:** JSON with `nodes`, `edges`, and `conditional_edges` keys. Nodes become states, edges become events, conditional edges produce routing events. Routing decisions (`ROUTE_*`) are controllable by default; environment-like events (human, tool_result, timeout) are uncontrollable.

### CrewAI

```bash
mununu context synth crew.crew.json \
    --formula can_finish --automaton crewai_workflow

# Or explicit
mununu context synth crew.json --adapter crewai \
    --formula safety_invariant --automaton crewai_workflow
```

**Input:** JSON with `agents`, `tasks`, and `process` keys. Sequential process → linear state chain with completion/failure/retry per task. Hierarchical process → supervisor + parallel worker regions with delegation support.

### A2A Protocol

```bash
mununu context synth protocol.a2a.json \
    --formula mutex_researcher_writer --automaton a2a_protocol_system

# Or explicit
mununu context synth cards.json --adapter a2a \
    --formula safety_invariant --automaton a2a_protocol
```

**Input:** Single agent card or JSON array of cards with `name` and `skills`. Each agent's skills become controllable invocation events. Task lifecycle (idle → queued → in_progress → completed/failed) modeled as state machine. Multi-agent cards produce parallel regions with auto-generated mutual exclusion properties.

### Python Convenience Scripts (Optional)

For users who want to introspect **live Python objects** (e.g., a `crewai.Crew` instance or a compiled `langgraph.graph.CompiledStateGraph`), the Python scripts in `tools/` remain available. They export XState JSON that can then be processed by either the XState or the native adapter:

```bash
# Live object introspection (requires framework install)
python3 tools/langgraph_to_xstate.py --input graph.json --output workflow.xstate.json
python3 tools/crewai_to_xstate.py --input crew.json --output crew.xstate.json
python3 tools/a2a_to_xstate.py --input card1.json card2.json --output protocol.xstate.json
```

For JSON dict input, the native Rust adapters (`--adapter langgraph|crewai|a2a`) are preferred — they are faster, have no Python dependency, and integrate with auto-detection and the web UI.

## Web UI

The `mununu-ui` web client supports agentic workflows:

1. **Import** XState JSON or CTXDSL agentic examples from the Examples picker (category: "Adapter Formats", badge: "Agentic")
2. **Verify** properties via the formula evaluation panel
3. **Synthesize** controllers via the synthesis panel
4. **Export** synthesized controllers as XState JSON for runtime enforcement

## See Also

- [Adapter Formats](Adapter-Formats) — XState import/export details, all supported formats
- [Controller Synthesis](Controller-Synthesis) — synthesis algorithms and strategy extraction
- [Counterstrategy](Counterstrategy) — using counterstrategies as test cases
- [Mu-Calculus Reference](Mu-Calculus-Reference) — formula syntax and patterns
