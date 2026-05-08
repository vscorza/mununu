# Adapter Formats

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change.

Mununu can import specifications from external formats and export synthesized controllers back to those formats. The adapter pipeline translates between external formats and the internal CTXDSL representation.

## Supported Formats

| Format | Extensions | Import | Export | Emission Mode |
|--------|-----------|--------|--------|--------------|
| **CTXDSL** | `.ctxdsl` | Native | Native | — |
| **TLSF** | `.tlsf` | Yes | No | Signal-state (turn-based) |
| **AIGER** | `.aag`, `.aig` | Yes | No | Signal-state (turn-based) |
| **BTOR2** | `.btor`, `.btor2` | Yes | No | Explicit automaton |
| **Promela** | `.pml`, `.promela` | Yes | No | Explicit automaton |
| **XState** | `.xstate`, `.json` | Yes | Yes | Explicit automaton |
| **SystemVerilog** (hand-written parser) | `.sv`, `.v` | Yes | Yes | Explicit automaton |
| **SystemVerilog via Yosys** | `.sv`, `.v` (with `--adapter sv-yosys`) | Yes | No | Explicit automaton |
| **Extraction Spec** | `.espec.json` | Yes | No | Explicit automaton |

> The **Extraction Spec** adapter handles `.espec.json` files from the extraction pipeline (source code analysis) and game engine integration. Properties can use `template_ref` to reference [Property Templates](Property-Templates) instead of raw mu-calculus formulas. See [Game Engine Integration](Game-Engine-Integration) for game-specific use cases.
>
> **Agentic orchestration** (CrewAI, LangGraph, A2A) does not have a dedicated native adapter today. Models for these frameworks are authored either as native CTXDSL (`examples/agentic/*.ctxdsl`) or as XState JSON (`examples/agentic/*.xstate.json`) consumed by the XState adapter. See [Agentic Orchestration](Agentic-Orchestration) for patterns and examples.

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

## BTOR2 + Yosys (RTL Phase 1)

BTOR2 (Niemetz–Preiner–Wolf, FMCAD 2018) is the de facto open-source word-level verification IR. Mununu's BTOR2 adapter parses the file, bit-blasts state and input bit-vectors into explicit valuations, and turns each `bad` / `constraint` / `fair` / `justice` line into a μ-calculus property. The **Yosys driver** (`adapter::yosys`) chains a child-process Yosys invocation onto the BTOR2 reader so a `.sv` file flows end-to-end as `SV → Yosys → BTOR2 → CLTS`.

### Pipeline

```
.sv  ──►  yosys (child process)  ──►  design.btor  ──►  BTOR2 reader  ──►  AdapterIR  ──►  CTXDSL  ──►  CLTS
                  │                                            │
                  └─ async2sync, chformal -lower, …            └─ per-signal labels, drop clock,
                                                                   filter chformal latches
```

**Yosys script** (`adapter::yosys::build_script`):

```text
read_verilog -formal -sv <source.sv>;
hierarchy -auto-top;
proc; flatten;
async2sync;          # NOT clk2fflogic — preserves the synchronous structure
chformal -lower;     # SVA → bad / constraint / fair / justice
dffunmap;
setundef -zero;      # X → 0; bit-blaster does not model X-prop
write_btor design.btor
```

### How the BTOR2 reader treats the design

Four CLTS-aligned transformations applied at read time (per [`adapter::btor2::bit_blast`](../crates/mununu-core/src/adapter/btor2/bit_blast.rs)):

1. **Per-signal labels.** Each transition carries one `<signal>=<value>` label per non-clock input — `transition s0 -> s253 on label rst_0;` — using mununu's native multi-label transition support. Properties refer to individual signals via `[(rst_0)] φ` rather than enumerating compound `env_NNNN` strings. The CLTS alphabet shrinks from `2^N` (compound) to `2N` (per-signal-value) entries.
2. **Implicit clock.** Each CLTS transition represents one clock edge; `clk` does not appear in the alphabet. The reader auto-detects clock-shaped input names (`clk`, `clock`, `ck`, `clk_i`, `i_clk`, `iclk`, `clki`) and elides them from enumeration. Multi-clock and negedge designs are out of scope for Phase 1 — the reader errors explicitly.
3. **Synchronous Yosys script.** `async2sync` (rather than `clk2fflogic`) preserves the synchronous structure. `clk2fflogic` would introduce a `value + shadow + previous-clk` triple-state-cell encoding per FF group; `async2sync` produces one BTOR2 state cell per FF, matching mununu's "transition = one clock edge" semantics natively.
4. **Enumerated state names + valuations side-channel.** State names are `s0, s1, …, sN-1`. Per-state register valuations are carried via the existing `StateSpec.valuations` side-channel (the same mechanism the SV adapter uses) so user-written formulas like `state == 0` resolve via the on-demand expression evaluator. Synthetic `chformal -lower` property-tracking latches are filtered out of valuations so user formulas don't reference them.

### Yosys SVA support

Yosys 0.59's `read_verilog -formal -sv` is a synthesis frontend, not a full SystemVerilog assertion frontend. It accepts:

- Immediate assertions inside `always @(posedge clk)`: `assert (boolean_expr);`
- Immediate assumes / covers: `assume (...)`, `cover (...)`

It does **not** accept temporal SVA: `assert property (...)`, `|->`, `|=>`, `##N`, `s_eventually`, `$stable`, `$rose`, `$past`, `nexttime`, `default clocking`, PSL/FL syntax. The full SVA story arrives in **Phase 2** of the [RTL roadmap](RTL-Verification-Pipeline) (`adapter/sva/`), independent of the Yosys frontend. Until then, properties needing temporal operators are encoded with **shadow-register patterns** — a pre-cycle latch tracks the antecedent, an immediate Boolean assertion tests the consequent. This mirrors how every working OSS Yosys SVA example is actually written. See `examples/btor2/` in the repo for a corpus.

### State-bit budget

The bit-blaster caps total state at `MAX_STATE_BITS = 16` (≈ 65 K explicit states) and inputs at `MAX_INPUT_BITS = 10` per step. Beyond that, the design is rejected with `StateSpaceOverflow` — the documented escape hatch is compose-and-decompose (Phase 3) before BTOR2 hand-off to an external symbolic engine (Pono / AVR / BtorMC).

### CLI

```bash
# Drive Yosys end-to-end:
mununu context eval design.sv --adapter sv-yosys --formula safety_bad_0 --automaton Circuit

# Or hand mununu an existing BTOR2 file:
mununu context eval design.btor --adapter btor2 --formula safety_bad_0 --automaton Circuit

# Auto-detection works for both .sv (via Yosys when --adapter sv-yosys, hand-written parser otherwise) and .btor / .btor2 (via the BTOR2 reader directly).
```

### Soundness

- **Bit-blasting is exact** for the operators marked `is_blastable()` in [`btor2::ast::Op`](../crates/mununu-core/src/adapter/btor2/ast.rs).
- **Implicit clock is sound for posedge-only single-clock designs** — multi-clock and negedge are explicitly rejected at read time.
- **`async2sync` preserves synchronous structure** for both register cells and `chformal`-lowered assertions.
- **`setundef -zero`** in the Yosys script makes X / undef bits deterministic; X-aware verification is out of mununu's roadmap scope.
- **Verific check:** the driver refuses to use a Yosys binary built with the commercial Verific frontend (license-incompatible).

See the in-repo `examples/btor2/README.md` for a concrete corpus walkthrough (`safety_demo`, `traffic_light`, `bounded_counter_with_assume`, `fair_arbiter`, `handshake_protocol`).

---

## Adapter Architecture

All adapters follow the same three-stage pipeline via the `FormatAdapter` trait:

```
Source file → Format Parser → AdapterIR → CTXDSL Emitter → CLTS
```

Each adapter implements:
- `detect(content)` — content-based format detection heuristic
- `translate(content, options)` — full parsing and translation pipeline

Auto-detection works by file extension (e.g. `.sv`, `.tlsf`, `.aag`, `.pml`, `.xstate.json`, `.espec.json`) and by content inspection.

---

## CLI Usage

### Import

```bash
# Auto-detect format from extension
mununu context eval design.sv --formula safety --automaton FSM
mununu context eval support_pipeline.xstate.json \
    --formula safety_invariant --automaton support_pipeline_system

# Explicit adapter selection
mununu context eval machine.json --adapter xstate --formula safety --automaton light
mununu context eval design.sv --adapter sv --formula safety --automaton FSM
mununu context eval spec.espec.json --adapter extraction --formula safety --automaton Main
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
