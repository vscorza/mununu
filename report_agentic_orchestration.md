# Formal Verification for Agentic AI Orchestration: Opportunities for Mununu

**Date:** April 2026  
**Scope:** State-of-the-art survey, applicability analysis, and three proposed use cases

---

## 1. Background: The State of Agentic Orchestration

### 1.1 The Landscape in 2026

Agentic AI orchestration — the coordination of autonomous LLM-powered agents that reason, use tools, and collaborate — has moved from experimental to production. The global agentic AI market is projected to grow from $7B (2025) to $93B (2032) at 44.6% CAGR, and over 80% of enterprises are deploying some form of agent system.

**Major frameworks** include:

| Framework | Approach | Key Abstraction |
|-----------|----------|-----------------|
| **LangGraph** | Graph-first, state-machine workflows | Nodes, edges, reducer-driven state schemas |
| **CrewAI** | Role-based organizational model | Agent roles, tasks, crews |
| **Microsoft AutoGen / Semantic Kernel** | Conversational agent teams | Multi-turn message passing |
| **OpenAI Agents SDK** | Lightweight handoff patterns | Agent transfer with context forwarding |
| **AWS Bedrock AgentCore** | Multi-framework runtime | A2A protocol support |
| **Google A2A Protocol** | Standardized agent interop | HTTP/JSON-RPC peer discovery |

### 1.2 Orchestration Patterns

The industry has converged on several canonical patterns:

- **Supervisor/Worker (Hierarchical):** A central orchestrator decomposes tasks, delegates to specialist agents, monitors progress, and synthesizes results. Standard for enterprise-scale systems (50+ agents typically require 3-tier hierarchy: meta-agent, supervisors, workers). Each supervisor manages 5-10 agents.

- **Handoff/Transfer:** One agent transfers control and conversational context to another specialist. Common implementation: handoff as a special tool call that switches the active agent and carries conversation state. Used by OpenAI Agents SDK and LangGraph.

- **Mesh/Peer-to-Peer:** Direct communication between agents without a central coordinator. Better fault tolerance but requires consensus mechanisms. Emerging with the A2A protocol.

- **Pipeline/Sequential:** Linear workflow where output of one agent feeds into the next. Most deterministic, least flexible.

### 1.3 The Safety Problem

Agent orchestration introduces a class of correctness problems that current frameworks address with ad-hoc engineering rather than formal guarantees:

**Deadlocks and infinite loops.** Agents enter repetitive cycles consuming token budgets. Three variants manifest in practice: simple logic traps (agent retries the same failing tool call), multi-agent deadlocks (agents wait for each other), and conversational deadlocks (agents produce output but make no progress). Current mitigations are engineering workarounds: inject warning messages after N identical calls, strip tool calls after M repetitions, impose hard retry budgets.

**Unauthorized actions.** 39% of companies reported agents accessing unintended systems in 2025. 32% saw agents allowing inappropriate data downloads. The root cause is overly broad tool permissions combined with the absence of formal access control models.

**State consistency.** Full context forwarding between agents is expensive (token costs scale with conversation history), but partial forwarding risks incoherence. LangGraph addresses this with explicit reducer-driven state schemas, but correctness is not formally verified.

**Non-termination.** There is no general mechanism to guarantee that an agent workflow will terminate. Budget caps and timeouts are blunt instruments that cannot distinguish productive long-running work from pathological loops.

### 1.4 The Formal Verification Gap

Recent academic work has begun formalizing the properties that agent systems should satisfy:

- **arXiv 2510.14133** defines 31 formal properties for agentic systems (17 for host agents, 14 for task lifecycles), categorized as safety, liveness, completeness, and fairness, expressed in temporal logic.

- **AgentGuard (arXiv 2509.23864)** proposes runtime verification integrating online model learning with probabilistic model checking for continuous behavioral guarantees.

- **SAGA (arXiv 2504.21034)** encodes token secrecy and authentication guarantees using PROVERIF's automated reasoning.

However, a critical gap remains: **no existing tool bridges the distance between temporal-logic specifications of agent properties and executable verification or synthesis over concrete agent workflow definitions.** The properties are formalized on paper, but practitioners have no way to check them against their actual orchestration graphs.

---

## 2. Applicability of Mununu

Mununu is a formal verification and controller synthesis tool for reactive systems modeled as Compositional Labeled Transition Systems (CLTS). Its capabilities align with the agentic orchestration domain along four axes.

### 2.1 Agent Workflows Are Reactive Systems

An agent orchestration workflow is, at its core, a reactive system:

- The **system** (orchestrator/controller) chooses which agent to activate, which tool to invoke, and when to hand off.
- The **environment** (user input, LLM responses, tool results, external events) produces non-deterministic outcomes that the system must react to.
- The interaction is **ongoing** — the system does not terminate after a single computation but continuously responds to stimuli.

This is precisely what CLTS models capture. Each agent state (idle, working, waiting for tool response, handing off) is an explicit state. Each event (user message, tool call, tool result, handoff) is a labeled transition. The distinction between **controllable** labels (orchestrator decisions) and **uncontrollable** labels (environment events) maps directly to the agent/environment boundary.

### 2.2 XState as the Bridge

Mununu already imports XState v5 statecharts — the same abstraction that modern orchestration frameworks use internally. LangGraph's nodes-and-edges model is structurally a statechart. XState is an established standard (W3C SCXML heritage) for executable state machines in JavaScript/TypeScript, which is the primary language of agent orchestration tooling.

The existing XState adapter supports:
- Simple, compound (hierarchical), and parallel states
- Events as labeled transitions with guards
- Context variables as bounded-domain automata
- Controllability annotations via `__mununu` metadata
- Controller export back to XState JSON

This means an agent workflow defined as an XState statechart can be **imported into mununu, verified against temporal properties, and — if synthesis succeeds — exported as a provably-safe controller back to XState JSON** for execution.

### 2.3 Controller Synthesis for Orchestration

Mununu's controller synthesis goes beyond verification (yes/no answer) to produce a **concrete strategy**: a restricted version of the system that satisfies the specification by construction. For agent orchestration, this means:

- Given a workflow graph and a safety property ("never invoke tool X after tool Y without re-authentication"), synthesis produces a controller that **enforces the property by restricting which transitions the orchestrator may take**.
- For GR(1) specifications (environment assumptions + system guarantees), synthesis produces controllers for the most common agent properties: "if the user eventually responds, the agent eventually completes the task" (liveness under fairness).
- When synthesis fails (the specification is unrealizable), mununu produces a **counterstrategy** — a concrete scenario showing how the environment can force a violation — which directly translates to a test case for the orchestration system.

### 2.4 Property Language Coverage

The properties identified in the formal verification literature for agent systems map to mununu's specification languages:

| Agent Property | Category | Mununu Encoding |
|----------------|----------|-----------------|
| No unauthorized tool calls | Safety | `G !(state_X && tool_Y_called)` via mu-calculus invariant |
| Every request eventually served | Liveness | `G(request -> F response)` via GR(1) guarantee |
| No deadlock | Safety | `G(exists enabled transition)` via mu-calculus |
| Fair scheduling among agents | Fairness | `GF agent_i_active` via GR(1) fairness |
| Budget not exceeded | Safety | Bounded variable automaton + invariant |
| Task eventually completes | Liveness | `F completed` via mu-calculus least fixpoint |
| No infinite retry loops | Liveness | Bounded liveness with counter automaton |
| Mutual exclusion on resources | Safety | `G !(agent_A_holds_R && agent_B_holds_R)` |

### 2.5 Limitations and Boundaries

Mununu's explicit state-space approach means it is best suited for **structural verification of orchestration protocols** (tens to hundreds of states), not for verifying the semantic content of LLM outputs. Concretely:

- **State-space limits:** Practical synthesis up to ~2^18 states. Agent workflows with bounded tool sets, bounded agent counts, and bounded context variables fit well. Unbounded conversation history does not.
- **Abstraction required:** LLM responses must be abstracted into discrete categories (success/failure/ambiguous, or domain-specific classifications). The verification targets the orchestration logic, not the LLM reasoning.
- **Controllability assumption:** Mununu assumes the controller can choose among controllable transitions. This holds when the orchestrator determines which agent to activate and which tool to call, but does not apply to the content of LLM-generated text.

---

## 3. Proposed Use Cases

### Use Case 1: Agent Workflow Verification via XState Import

**Problem.** A development team builds an agentic workflow (e.g., a customer support pipeline with triage, specialist, and escalation agents) using a state-machine framework. They want to verify that the workflow satisfies safety and liveness properties before deployment — not through testing (which covers finite traces) but through exhaustive formal verification (which covers all possible executions).

**How mununu applies.** The workflow is expressed as an XState statechart, which is mununu's native import format. The team annotates controllability (orchestrator decisions vs. environment events) and specifies properties in the `__mununu` metadata block.

**Concrete example.** Consider a 3-agent customer support workflow (see `examples/agentic/support_pipeline.xstate.json`):

```json
{
  "id": "support_pipeline",
  "initial": "routing",
  "states": {
    "routing": {
      "type": "parallel",
      "states": {
        "triage": {
          "initial": "waiting",
          "states": {
            "waiting":    { "on": { "TICKET_IN": "classifying" } },
            "classifying": { "on": {
              "ROUTE_BILLING": "routed_billing",
              "ROUTE_TECH":    "routed_tech",
              "ESCALATE":      "escalated"
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
            "over_limit":  { "on": { "HUMAN_RESOLVED": "under_limit" } }
          }
        }
      }
    }
  },
  "__mununu": {
    "controllable": ["ROUTE_BILLING", "ROUTE_TECH", "ESCALATE", "TOOL_CALL"],
    "uncontrollable": ["TICKET_IN", "RESOLVED", "HUMAN_RESOLVED", "LIMIT_HIT"],
    "properties": [
      {
        "name": "no_tool_when_over_budget",
        "formula": "G (over_limit -> !TOOL_CALL)",
        "role": "guarantee"
      },
      {
        "name": "eventual_resolution",
        "formula": "G (classifying -> F waiting)",
        "role": "guarantee"
      },
      {
        "name": "tickets_keep_arriving",
        "formula": "GF TICKET_IN",
        "role": "assumption"
      }
    ]
  }
}
```

**Workflow:**
```bash
mununu context synth support_pipeline.xstate.json \
  --adapter xstate \
  --formula no_tool_when_over_budget \
  --automaton support_pipeline \
  --extract-strategy \
  --emit-native safe_controller.json \
  --output-format xstate
```

If realizable, `safe_controller.json` is a restricted statechart that provably never invokes tools when over budget. If unrealizable, the counterstrategy shows the exact sequence of environment events that forces a violation — directly usable as a regression test.

**Value.** Exhaustive verification replaces manual review of state-machine edge cases. The synthesized controller can be used as a runtime guardrail layer wrapping the original workflow.

---

### Use Case 2: MCP Tool Authorization Protocol Synthesis

**Problem.** An agent system exposes tools via the Model Context Protocol (MCP). Tools have different authorization requirements: some require user confirmation, some are restricted by role, some must not be called in sequence without re-validation. Current MCP implementations handle authorization at the individual tool level but do not verify protocol-level properties (e.g., "a destructive tool is never called without a preceding confirmation step" or "credential-bearing tools are never called after a session expires").

**How mununu applies.** The tool authorization protocol is modeled as a CLTS where:
- **States** encode the authorization context: session status (active/expired), confirmation status (pending/granted/denied), role (admin/user/guest), and the last tool category invoked.
- **Controllable labels** are the agent's tool invocations (the orchestrator chooses which tool to call).
- **Uncontrollable labels** are environment events (session expiry, user confirmation/denial, role changes).

Mununu synthesizes a controller that restricts tool invocations to sequences that satisfy the authorization policy by construction.

**Concrete example.** A CTXDSL model for a 3-tool MCP server with session-aware authorization (see `examples/agentic/mcp_auth.ctxdsl`):

```
context mcp_auth

alphabet {
  controllable: call_read, call_write, call_delete, request_confirm
  uncontrollable: session_start, session_expire, user_confirm, user_deny
}

automaton Session {
  states: no_session, active, expired
  initial: no_session
  transitions:
    no_session -> active on session_start
    active -> expired on session_expire
    expired -> active on session_start
}

automaton Confirmation {
  states: idle, pending, confirmed, denied
  initial: idle
  transitions:
    idle -> pending on request_confirm
    pending -> confirmed on user_confirm
    pending -> denied on user_deny
    confirmed -> idle on call_delete
    denied -> idle on request_confirm
}

controller ToolAuth for Session || Confirmation {
  formula auth_safety:
    // Never call write/delete without active session
    G ((expired | no_session) -> !(call_write | call_delete))
    // Never call delete without confirmation
    && G (call_delete -> confirmed)
}
```

**Synthesis output:** A controller that intercepts tool calls and either allows them (when authorization state permits) or blocks them (redirecting to `request_confirm` or waiting for `session_start`). Exported as a state machine that can be embedded in the MCP server middleware.

**Value.** Authorization policies become formally verified properties of the protocol, not scattered `if` checks in handler code. The counterstrategy for unrealizable policies reveals exactly which environment behavior exploits a gap — directly translatable to a security test.

---

### Use Case 3: Multi-Agent Handoff Protocol Verification

**Problem.** In a multi-agent system with handoff semantics (OpenAI Agents SDK, LangGraph), agents transfer control to each other along with conversational context. The handoff protocol must satisfy:
1. **No orphaned tasks** — every task that enters the system is eventually handled (liveness).
2. **No concurrent handling** — at most one agent actively handles a task at a time (safety/mutual exclusion).
3. **No infinite delegation** — an agent cannot hand off indefinitely without making progress (bounded liveness).
4. **Graceful degradation** — if a specialist agent fails, the task returns to the supervisor (recovery).

Current frameworks implement these guarantees through convention and testing. A single missed edge case (e.g., a specialist that hands off to another specialist that hands off back, creating a cycle) can cause production failures.

**How mununu applies.** The handoff protocol is modeled as a composition of agent automata. Each agent is an automaton with states {idle, active, handing_off, failed}. The handoff event is a synchronizing label between the source and target agents. Mununu verifies the composition against the protocol properties.

**Concrete example.** A 3-agent handoff system — supervisor + 2 specialists (see `examples/agentic/handoff_protocol.ctxdsl`):

```
context handoff_protocol

alphabet {
  controllable: activate_A, activate_B, handoff_to_A, handoff_to_B,
                return_to_sup, retry
  uncontrollable: task_arrive, task_complete, agent_fail, timeout
}

automaton Supervisor {
  states: idle, dispatching, waiting, recovering
  initial: idle
  transitions:
    idle -> dispatching on task_arrive
    dispatching -> waiting on activate_A
    dispatching -> waiting on activate_B
    waiting -> idle on task_complete
    waiting -> recovering on agent_fail
    waiting -> recovering on timeout
    recovering -> dispatching on retry
}

automaton AgentA {
  states: idle, working, handing_off
  initial: idle
  transitions:
    idle -> working on activate_A
    working -> idle on task_complete
    working -> handing_off on handoff_to_B
    handing_off -> idle on activate_B
    working -> idle on agent_fail
}

automaton AgentB {
  states: idle, working, handing_off
  initial: idle
  transitions:
    idle -> working on activate_B
    working -> idle on task_complete
    working -> handing_off on handoff_to_A
    handing_off -> idle on activate_A
    working -> idle on agent_fail
}

controller HandoffController for Supervisor || AgentA || AgentB {
  // Safety: mutual exclusion
  formula mutex:
    G !(AgentA.working && AgentB.working)

  // Liveness: every task eventually completes (under fairness)
  formula completion:
    G (Supervisor.dispatching -> F Supervisor.idle)

  // Safety: if an agent fails, supervisor recovers
  formula recovery:
    G (agent_fail -> F Supervisor.recovering)

  // Bounded liveness: no infinite handoff chain
  formula no_infinite_delegation:
    G (handoff_to_A -> F (task_complete | agent_fail))
}
```

**Workflow:**
```bash
mununu context synth handoff.ctxdsl \
  --formula mutex \
  --automaton HandoffController \
  --extract-strategy \
  --counterexample
```

If the mutex property is realizable, the synthesized controller enforces that activations are sequenced so no two agents work simultaneously. If the completion property is unrealizable, the counterstrategy reveals the exact scenario (e.g., repeated `agent_fail` + `timeout` cycle) that prevents completion — a concrete test case the team can address by adding a fallback path.

**Value.** Protocol-level guarantees replace convention-based assumptions. The synthesized controller can serve as a reference implementation or runtime enforcement layer. Counterstrategies become executable test scenarios documenting the protocol's failure modes.

---

## 4. Summary

| Dimension | State of the Art | Mununu Contribution |
|-----------|-----------------|---------------------|
| Orchestration abstraction | State machines (LangGraph), roles (CrewAI) | Formal CLTS with controllability semantics |
| Safety guarantees | Ad-hoc guardrails, retry budgets, timeouts | Exhaustive verification via mu-calculus |
| Liveness guarantees | Timeout-based termination | GR(1) synthesis with fairness assumptions |
| Authorization | Per-tool checks, FGA policies | Protocol-level synthesis with counterexamples |
| Handoff correctness | Convention and testing | Compositional verification with mutual exclusion |
| Failure analysis | Log inspection, manual debugging | Counterstrategies as executable failure scenarios |
| Import/Export | N/A | XState JSON import/export (native to orchestration tooling) |

The central thesis is that **agentic orchestration has adopted state-machine abstractions without adopting the formal methods that state machines enable.** Mununu bridges this gap by providing verification and synthesis over the same XState/statechart representations that orchestration frameworks already use, producing not just yes/no verdicts but concrete controllers and counterexamples.

---

## References

1. Ren, J. et al. "Formalizing Safety, Security, and Functional Properties of Agentic AI Systems." arXiv:2510.14133 (2025).
2. Bai, H. et al. "AgentGuard: Repurposing Agentic Orchestrator for Safety Evaluation of Tool Orchestration." arXiv:2509.23864 (2025).
3. SAGA Consortium. "SAGA: A Security Architecture for Governing AI Agentic Systems." arXiv:2504.21034 (2025).
4. Zhou, Y. et al. "The Orchestration of Multi-Agent Systems: Architectures, Protocols, and Enterprise Adoption." arXiv:2601.13671 (2026).
5. Liu, S. et al. "Agentic AI Security: Threats, Defenses, Evaluation, and Open Challenges." arXiv:2510.23883 (2025).
6. Harel, D. "Statecharts: A Visual Formalism for Complex Systems." Science of Computer Programming 8(3), 1987.
7. LangGraph Documentation. https://langchain-ai.github.io/langgraph/
8. Model Context Protocol Specification, v2025-03-26. https://modelcontextprotocol.io/
9. Google A2A Protocol. https://github.com/a2aproject/A2A
10. OpenAI. "A Practical Guide to Building Agents." 2025.
