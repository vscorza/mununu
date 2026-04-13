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

> **New:** XState import enables [Agentic AI Orchestration](Agentic-Orchestration) — verify and synthesize safe controllers for multi-agent workflows, MCP tool authorization, and handoff protocols.

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

Imports behavioral SystemVerilog RTL descriptions by extracting FSMs from `always_ff` blocks.

### Supported Subset

| Feature | Supported |
|---------|-----------|
| `module` with input/output ports | Yes |
| `always_ff @(posedge clk)` | Yes |
| `always_comb` | Yes |
| `typedef enum logic` | Yes |
| `case` / `if-else` | Yes |
| `assign` statements | Yes |
| `// @mununu` property comments | Yes |
| Registers/logic up to 8 bits | Yes |
| Module instantiation | Not yet |
| Arrays/memories | No |
| Interfaces/classes | No |

### Controllability

Derived from port directions:
- `input` ports → **uncontrollable** (environment)
- `output` ports → **controllable** (system)

Transitions guarded by input signals are classified as uncontrollable.

### Property Specification

Properties are specified via inline comments:

```systemverilog
// @mununu ltl safety: nu X. ([] X)
// @mununu assume env_constraint: G(req -> X req)
// @mununu guarantee liveness: G(req -> F grant)
```

### FSM Extraction

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

### State Space Limits

- Designs with > 18 state bits (262K states) are **rejected** at parse time
- Designs with > 12 state bits emit a **warning**
- Only enum-typed FSMs are supported (no binary-coded state machines)

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

## CLI Usage

### Import

```bash
# Auto-detect format from extension
mununu context eval design.sv --formula safety --automaton FSM

# Explicit adapter selection
mununu context eval machine.json --adapter xstate --formula safety --automaton light
```

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
