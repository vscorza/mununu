# Adapter Formats

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change.

Mununu can import specifications from external formats and export synthesized controllers back to those formats. The adapter pipeline translates between external formats and the internal CTXDSL representation.

## Supported Formats

| Format | Extensions | Import | Export | Emission Mode |
|--------|-----------|--------|--------|--------------|
| **CTXDSL** | `.ctxdsl` | Native | Native | — |
| **TLSF** | `.tlsf` | Yes | No | Signal-state (turn-based) |
| **AIGER** | `.aag`, `.aig` | Yes | No | Signal-state (turn-based) |
| **Promela** | `.pml`, `.promela` | Yes | No | Explicit automaton |
| **XState** | `.xstate`, `.json` | Yes | Yes | Explicit automaton |
| **SystemVerilog** | `.sv`, `.v` | Yes | Yes | Explicit automaton |
| **CrewAI** | `.crew.json` | Yes | No | Explicit automaton (via XState) |
| **LangGraph** | `.langgraph.json` | Yes | No | Explicit automaton (via XState) |
| **A2A** | `.a2a.json` | Yes | No | Explicit automaton (via XState) |

> The agentic adapters (CrewAI, LangGraph, A2A) are native Rust adapters that translate directly from framework JSON into CTXDSL. They build XState-compatible state machines internally and delegate to the XState adapter pipeline. See [Agentic AI Orchestration](Agentic-Orchestration) for use cases, property templates, and examples.

## Pipeline

```
Source file → Adapter Parser → AdapterIR → CTXDSL Emitter → CLTS Realization → Synthesis
                                                                                    ↓
                                                               Controller → Native Format
```

---

## XState / Statecharts

Imports XState v5 JSON machine definitions. Supports simple, compound (hierarchical), and parallel states.

### Supported Features

- Simple states with event transitions
- Compound (nested) states — flattened with underscore-joined names
- Parallel states — each region becomes a separate automaton in synchronous composition
- Guards on transitions
- Context variables (boolean, bounded integer) as variable automata
- `__mununu` annotations for controllability and properties

### Controllability

Events are classified via the `__mununu` annotation block:

```json
{
  "__mununu": {
    "controllable": ["TIMER", "PROCESS"],
    "uncontrollable": ["USER_INPUT", "SENSOR"],
    "bounds": { "counter": [0, 10] },
    "properties": [
      { "name": "safety", "formula": "nu X. ([] X)", "role": "guarantee" }
    ]
  }
}
```

Events not classified default to **uncontrollable** (conservative).

### Limitations

- History states (shallow/deep) are not supported
- Delayed transitions (`after`) are not supported
- Arbitrary JavaScript actions are ignored — only `assign` with simple values is modeled
- String context variables are not supported
- Hierarchy is lost in round-trip (output is flat)

### Example

```json
{
  "id": "trafficLight",
  "initial": "green",
  "states": {
    "green": { "on": { "TIMER": "yellow" } },
    "yellow": { "on": { "TIMER": "red" } },
    "red": { "on": { "TIMER": "green" } }
  },
  "__mununu": {
    "controllable": ["TIMER"],
    "properties": [
      { "name": "safety", "formula": "nu X. ([] X)", "role": "guarantee" }
    ]
  }
}
```

### Controller Output

Synthesized controllers are emitted as flat XState v5 JSON with a `__mununu` metadata block:

```json
{
  "id": "trafficLight_controller",
  "initial": "green",
  "states": { ... },
  "__mununu": { "synthesis_result": "realizable", "generated_by": "mununu" }
}
```

---

## SystemVerilog

Imports behavioral SystemVerilog RTL descriptions. Two modes:

1. **FSM mode** (default): extracts `typedef enum` FSMs from `always_ff` blocks
2. **Kripke mode**: builds a Kripke structure from all registers, supporting counters, data-path registers, and mixed enum+register designs

### Supported Subset

| Feature | Supported |
|---------|-----------|
| `module` with input/output ports | Yes |
| `always_ff @(posedge clk)` | Yes |
| `always_comb` | Yes |
| `typedef enum logic` | Yes |
| `case` / `if-else` (including numeric labels) | Yes |
| `assign` statements | Yes |
| `// @mununu` property comments | Yes |
| `// @mununu domain` register annotations | Yes (Kripke mode) |
| Ternary `? :`, bit-select, bit-slice, concat | Yes |
| Arithmetic, shifts, comparisons | Yes |
| `localparam` / `parameter` constants | Yes |
| Module instantiation | Not yet |
| Arrays/memories | No |
| Interfaces/classes | No |

### Controllability

Derived from port directions and `@mununu` annotations:
- `input` ports → **uncontrollable** (environment)
- `output` ports → **controllable** (system)
- `// @mununu controllable sig1, sig2` → override to controllable
- `// @mununu input sig1, sig2` → override to uncontrollable

### Property Specification

Properties are specified via inline comments:

```systemverilog
// @mununu ltl safety: nu X. ([] X)
// @mununu assume env_constraint: G(req -> X req)
// @mununu guarantee liveness: G(req -> F grant)
```

### FSM Extraction (Default Mode)

The adapter extracts FSMs from `typedef enum` + `always_ff` + `case(state)` patterns:

```systemverilog
typedef enum logic [1:0] {IDLE, WAIT, ACTIVE, DONE} state_t;
state_t state;
always_ff @(posedge clk or posedge rst) begin
    if (rst) state <= IDLE;
    else case (state)
        IDLE: if (req) state <= WAIT;
        WAIT: state <= ACTIVE;
        ACTIVE: if (!req) state <= DONE;
        DONE: state <= IDLE;
    endcase
end
```

### Kripke Mode (Register-Level Verification)

Activated by `// @mununu mode kripke` or automatically when no `typedef enum` FSM is found. Builds a Kripke structure where each state is a valuation of all active registers.

#### Register Domain Annotations

Each register can be annotated with a domain to control how it contributes to the state space:

```systemverilog
// @mununu domain counter: bounded_counter 0..7    // 8 abstract values
// @mununu domain cmd: enum {NOP=0, ADD=1, OTHER}  // 3 named values with numeric mapping
// @mununu domain data: ignored                     // excluded from state space
// @mununu domain flag: boolean                     // 2 values (auto-inferred for 1-bit)
```

**Domain types:**

| Domain | Syntax | Values | Use for |
|--------|--------|--------|---------|
| `boolean` | `boolean` | `{false, true}` | 1-bit flags (auto-inferred) |
| `bounded_counter` | `bounded_counter L..U` | `{L, L+1, ..., U}` | Counters, fill levels, indices |
| `enum` | `enum {A, B, C}` | Named variants | State machines, command opcodes |
| `enum` (value-mapped) | `enum {IDLE=0, RUN=3, OTHER}` | Named variants with numeric mapping | Wide registers with significant constants |
| `ignored` | `ignored` | (excluded) | Data-path registers, buffers |

**Value-mapped enums:** When a wide register (e.g., `logic [7:0] cmd`) is used in comparisons against specific constants (`cmd == 3`, `case(cmd) 0: ... 1: ...`), the constants can be mapped to named variants. The last variant without `=` acts as a catch-all for all other values.

#### Automatic Optimizations

**Cone-of-influence reduction:** Registers not referenced by any property formula (transitively through the dependency graph) are automatically excluded. No annotation needed.

**Constant discovery:** When a wide register is ignored but appears in comparisons or case statements with specific numeric values, a warning is emitted suggesting a value-mapped enum annotation:

```
warning: Register 'cmd' (8-bit, ignored) uses significant constants: [0, 3, 255].
         Suggested: // @mununu domain cmd: enum {VAL_0=0, VAL_3=3, VAL_255=255, OTHER}
```

#### Complete Kripke Example

```systemverilog
// @mununu mode kripke
// @mununu domain fill: bounded_counter 0..4
// @mununu domain data_out_r: ignored
// @mununu input wr_en, rd_en
// @mununu ltl safety: nu X. ([] X)
module fifo(
    input  logic       clk, input logic rst,
    input  logic       wr_en, input logic rd_en,
    input  logic [7:0] data_in,
    output logic [7:0] data_out,
    output logic       full, output logic empty
);
    typedef enum logic [1:0] {IDLE, WRITING, READING, RDWR} state_t;
    state_t state;
    logic [2:0] fill;
    logic [7:0] data_out_r;
    localparam DEPTH = 4;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin state <= IDLE; fill <= 0; end
        else case (state)
            IDLE: begin
                if (wr_en && rd_en) state <= RDWR;
                else if (wr_en)     state <= WRITING;
                else if (rd_en)     state <= READING;
            end
            WRITING: begin
                if (fill < DEPTH) fill <= fill + 1;
                state <= IDLE;
            end
            READING: begin
                if (fill > 0) fill <= fill - 1;
                state <= IDLE;
            end
            RDWR: begin
                if (fill == 0) fill <= fill + 1;
                state <= IDLE;
            end
        endcase
    end
endmodule
```

State space: 4 (state) x 5 (fill 0..4) = 20 states. `data_out_r` is ignored, `data_in`/`data_out` are ports (not registers).

#### Multi-label transitions

In Kripke mode, each input signal produces its own label. Transitions carry the full set of input signal values as a multi-label, making the CTXDSL output readable at a glance:

```
transition fill_0_state_IDLE -> fill_0_state_WRITING on label rd_en_F, label wr_en_F;
transition fill_0_state_IDLE -> fill_0_state_RDWR    on label rd_en_T, label wr_en_T;
```

This is equivalent to the single-label encoding used by the signal-state path (TLSF/AIGER) but with named per-signal labels instead of compound bitvectors. For value-mapped enums, labels use the variant names (e.g., `cmd_LOAD`, `cmd_ADD`) rather than numeric indices.

### State Space Limits

- Designs with > 2^18 states (262K) are **rejected**
- Designs with > 2^12 states emit a **warning**
- Kripke mode: registers > 4 bits without annotation are auto-ignored with a warning

### Controller Output

Synthesized controllers are emitted as SystemVerilog modules:

```systemverilog
module FSM_controller (
    input  logic clk,
    input  logic rst
);
    typedef enum logic [1:0] {IDLE, WAIT, ACTIVE, DONE} state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: state <= WAIT;
            // ... synthesized transitions
        endcase
    end
endmodule
```

---

## CrewAI

Imports CrewAI workflow definitions from JSON. Supports sequential and hierarchical process types.

### Input Format

```json
{
  "agents": [
    {"role": "researcher", "allow_delegation": false, "tools": ["web_search"]},
    {"role": "writer", "allow_delegation": false, "tools": []}
  ],
  "tasks": [
    {"name": "research", "agent_role": "researcher"},
    {"name": "write_report", "agent_role": "writer"}
  ],
  "process": "sequential"
}
```

### Process Types

- **Sequential** (`"process": "sequential"`): Linear task chain. Each task has `COMPLETE_*` / `FAIL_*` (uncontrollable) and `RETRY_*` (controllable) events. Auto-generates a `can_finish` liveness property.
- **Hierarchical** (`"process": "hierarchical"`): Supervisor + parallel worker regions. Supervisor dispatches via `ACTIVATE_*` events (controllable). Agents with `allow_delegation: true` get `DELEGATE_*_TO_*` events.

### Usage

```bash
# Auto-detect from extension
mununu context eval crew.crew.json --formula can_finish --automaton crewai_workflow

# Explicit adapter
mununu context eval crew.json --adapter crewai --formula safety_invariant --automaton crewai_workflow
```

### Live Python Object Introspection

For users who want to introspect live `crewai.Crew` instances, the Python convenience script `tools/crewai_to_xstate.py` remains available. It exports XState JSON that can then be used with either `--adapter xstate` or `--adapter crewai`.

---

## LangGraph

Imports LangGraph workflow definitions from JSON. Supports nodes, unconditional edges, and conditional routing.

### Input Format

```json
{
  "nodes": ["router", "billing", "tech"],
  "edges": [["__start__", "router"], ["billing", "__end__"], ["tech", "__end__"]],
  "conditional_edges": {"router": {"billing": "billing", "tech": "tech"}}
}
```

### Controllability Heuristics

Events are classified automatically:
- `ROUTE_*` events (from conditional edges) → **controllable** (orchestrator decides routing)
- Events matching environment patterns (`human`, `user`, `tool_result`, `sensor`, `timeout`, `error`, `fail`) → **uncontrollable**
- Fallback: if no events match environment patterns, all `ROUTE_*` events are controllable, rest uncontrollable

### Usage

```bash
# Auto-detect from extension
mununu context eval workflow.langgraph.json --formula safety_invariant --automaton langgraph_workflow

# Explicit adapter
mununu context eval graph.json --adapter langgraph --formula safety_invariant --automaton langgraph_workflow
```

---

## A2A (Agent-to-Agent)

Imports A2A Agent Card JSON. Models the task lifecycle for each agent and composes them in parallel.

### Input Format

Single agent card or JSON array of cards:

```json
[
  {"name": "researcher", "skills": [{"id": "web_search", "name": "Web Search"}]},
  {"name": "writer", "skills": [{"id": "draft", "name": "Draft Report"}]}
]
```

### Task Lifecycle

Each agent gets a 5-state lifecycle FSM:

```
idle → queued → in_progress → completed → idle
                             → failed    → idle
```

- **Skills** → `INVOKE_{AGENT}_{SKILL}` events (controllable)
- **Cancel/Reset** → `CANCEL_*`, `RESET_*` (controllable)
- **Task progress** → `START_*`, `COMPLETED_*`, `FAILED_*`, `TIMEOUT_*` (uncontrollable)

For multi-agent cards, agents are composed in **parallel** with auto-generated **mutual exclusion** properties preventing concurrent `in_progress` states.

### Usage

```bash
# Auto-detect from extension
mununu context eval protocol.a2a.json --formula mutex_researcher_writer --automaton a2a_protocol_system

# Explicit adapter
mununu context eval cards.json --adapter a2a --formula safety_invariant --automaton a2a_protocol
```

---

## Adapter Architecture

All adapters follow the same three-stage pipeline via the `FormatAdapter` trait:

```
Source file → Format Parser → AdapterIR → CTXDSL Emitter → CLTS
```

The agentic adapters (CrewAI, LangGraph, A2A) use a **delegation pattern**: they parse their native JSON format, construct an XState-compatible state machine internally, and delegate to the XState adapter's `translate()` pipeline. This reuses the XState adapter's hierarchy flattening, parallel state handling, property parsing, and controllability logic without duplication.

```
CrewAI/LangGraph/A2A JSON → Build XState JSON → XState translate() → AdapterIR → CTXDSL
```

Each adapter implements:
- `detect(content)` — content-based format detection heuristic
- `translate(content, options)` — full parsing and translation pipeline

Auto-detection works by file extension (`.crew.json`, `.langgraph.json`, `.a2a.json`) and by content inspection (presence of format-specific JSON keys like `"agents"+"tasks"+"process"` for CrewAI).

---

## CLI Usage

### Import

```bash
# Auto-detect format from extension
mununu context eval design.sv --formula safety --automaton FSM
mununu context eval workflow.langgraph.json --formula safety_invariant --automaton langgraph_workflow

# Explicit adapter selection
mununu context eval machine.json --adapter xstate --formula safety --automaton light
mununu context eval crew.json --adapter crewai --formula can_finish --automaton crewai_workflow
mununu context eval graph.json --adapter langgraph --formula safety_invariant --automaton langgraph_workflow
mununu context eval cards.json --adapter a2a --formula safety_invariant --automaton a2a_protocol
```

### Inspecting the intermediate CTXDSL

When using an adapter, add `--print-ctxdsl` to see the translated CTXDSL model:

```bash
# Print to stdout (alongside normal verification output)
mununu context eval design.sv --adapter sv --formula safety --automaton FSM --print-ctxdsl

# Write to a file
mununu context eval design.sv --adapter sv --formula safety --automaton FSM --print-ctxdsl output.ctxdsl
```

This works with all adapters (`--adapter tlsf`, `--adapter sv`, `--adapter xstate`, etc.) and with both `context eval` and `context synth`. The CTXDSL is printed before verification runs, so you can inspect the model even if verification fails.

### SystemVerilog Pipeline

```bash
# Generate skeleton sidecar from SV module
mununu sv init design.sv

# Discover significant register values via SMT (requires --features smt)
mununu sv discover design.sv

# Verify with sidecar (auto-loaded from <stem>.mununu.json)
mununu context eval design.sv --adapter sv --formula safety --automaton FSM
```

See [RTL Verification Pipeline](RTL-Verification-Pipeline) for the full annotation workflow.

### Export

```bash
# Export controller as XState JSON
mununu context synth machine.xstate --adapter xstate \
    --formula safety --automaton Machine \
    --output-format xstate --emit-native controller.json

# Export controller as SystemVerilog module
mununu context synth design.sv --adapter sv \
    --formula safety --automaton FSM \
    --output-format systemverilog --emit-native controller.sv
```

---

## API Usage

### Import Endpoint

`POST /api/v1/context/import`

Translates external format content to CTXDSL.

**Request:**
```json
{
  "content": "<raw file content>",
  "format": "auto",
  "filename": "design.sv"
}
```

**Response:**
```json
{
  "success": true,
  "ctxdsl": "<translated CTXDSL>",
  "source_format": "SystemVerilog",
  "warnings": [],
  "signal_count": 4,
  "state_count": 0,
  "property_count": 1
}
```

### Synthesis with Native Output

`POST /api/v1/context/synthesize` with `output_format` in options:

```json
{
  "context": { "name": "design.ctxdsl", "content": "..." },
  "formula": "safety",
  "automaton": "FSM",
  "options": { "output_format": "xstate" }
}
```

The response includes `controller_native` with the format-specific output.

## See Also

- [Agentic Orchestration](Agentic-Orchestration) -- verify multi-agent workflows, MCP authorization, and handoff protocols
- [CLI Reference](CLI-Reference.md) -- full CLI documentation
- [API Reference](API-Reference.md) -- REST API documentation
- [Hardware Verification Patterns](Hardware-Verification-Patterns.md) -- RTL verification examples
