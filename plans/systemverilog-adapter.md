# SystemVerilog Adapter — Implementation & Tracking Plan

> **Status:** NOT STARTED
> **Last updated:** 2026-04-08
> **Tracking:** Update the "Progress Log" and phase status markers as work proceeds.

---

## Agent Instructions

**Before summarizing the current chat or ending a session, the agent MUST:**

1. Read this file to get the latest state.
2. Update the **Progress Log** section at the bottom with a dated entry summarizing:
   - What was accomplished in this session
   - What decisions were made and why
   - What files were created or modified
   - Any blockers, open questions, or deferred items
   - What the next step should be
3. Update the status marker (NOT STARTED / IN PROGRESS / BLOCKED / DONE) on every phase heading to reflect current reality.
4. If a viability concern was discovered, add it to the **Risk Register** section.
5. Save the file before responding to the user.

This ensures continuity across sessions — the plan file IS the source of truth for project state.

---

## Context

Mununu's adapter pipeline translates external formats into CTXDSL for formal verification and reactive synthesis. The SystemVerilog adapter targets **behavioral RTL descriptions** — a level above the existing AIGER adapter (gate-level netlists).

### Why SystemVerilog (not just AIGER)

| Dimension | AIGER (existing) | SystemVerilog (proposed) |
|-----------|-------------------|--------------------------|
| Signal names | Generic (`i0`, `l0`) | Preserved from source (`state`, `req`, `grant`) |
| State names | Bitvector indices (`v001`) | Enum values (`IDLE`, `WAIT`, `ACTIVE`) |
| Transition structure | 2^N full enumeration | Sparse `case`/`if-else` (far fewer transitions) |
| Properties | Bad outputs, justice sets | SVA assertions from source with intent |
| User experience | Opaque post-synthesis | Readable, maps to source concepts |

The key value: **behavioral FSM extraction** produces named states and sparse transitions directly from RTL, without requiring synthesis-to-netlist first.

### State Space Constraints

Mununu uses explicit state enumeration. Practical limit: ~18 bits (262K states). This restricts the adapter to:
- Protocol FSMs (SPI, I2C, AXI handshakes)
- Arbiters with ≤8 clients
- Small control-path modules (no datapaths wider than ~8 bits)

Designs with >18 state bits are **rejected at parse time** with `StateSpaceOverflow`.

---

## Phase 0: Viability Evaluation — `DONE`

**Goal:** Confirm that behavioral FSM extraction from SystemVerilog is practical and produces CLTS that match hand-written CTXDSL equivalents.

### 0.1 Back-Translation Spike

Take the 3 existing hw examples and write equivalent SystemVerilog source:

| Existing CTXDSL | SystemVerilog Target | States | Key Feature |
|----------------|---------------------|--------|-------------|
| [handshake.ctxdsl](examples/hw/handshake.ctxdsl) | `handshake.sv` | 4 | Basic enum FSM, req/ack protocol |
| [arbiter.ctxdsl](examples/hw/arbiter.ctxdsl) | `arbiter.sv` | 3 | Round-robin, mutual exclusion |
| [traffic_light.ctxdsl](examples/hw/traffic_light.ctxdsl) | `traffic_light.sv` | 4 | Timer-driven FSM |

For each:
1. Write the SystemVerilog module with `typedef enum`, `always_ff`, `case(state)`
2. Hand-extract the FSM (states, transitions, controllability from ports)
3. Compare against the existing CTXDSL: state counts, transition counts, and label sets must match
4. Document any semantic gaps

Tests live in `tests/adapter_systemverilog_viability.rs`.

### 0.2 Parser Feasibility Assessment

Evaluate parser strategy by attempting to tokenize and extract FSMs from the 3 spike files:

| Strategy | Effort | Precision | Dependency |
|----------|--------|-----------|------------|
| Custom recursive-descent (like TLSF/Promela) | High (but proven pattern) | Subset only | None |
| `sv-parser` crate | Medium | Broader SV coverage | External crate |
| `slang` (C++ via FFI) | Very high | Full SV | C++ build chain |
| `verilator --xml` AST | Medium | Good coverage | External tool |
| `surfer`/`synlig` AST | Medium | Partial | External tool |

**Recommendation:** Start with custom recursive-descent for the supported subset (consistent with TLSF and Promela adapters). Investigate `sv-parser` crate as a potential accelerator. Decide during Phase 0.

### 0.3 Supported SystemVerilog Subset

| Feature | Supported | Notes |
|---------|:---------:|-------|
| `module` with port list | Yes | `input`/`output`/`inout` |
| `always_ff @(posedge clk)` | Yes | Primary FSM extraction target |
| `always_comb` | Yes | Inlined into transition guards |
| `case`/`casez`/`if-else` | Yes | Transition extraction |
| `typedef enum logic [N:0]` | Yes | Named states |
| `reg`/`logic`/`wire` ≤ 8 bits | Yes | State variables or combinational |
| `parameter`/`localparam` | Yes | Resolved at elaboration (concrete values) |
| SVA `assert property` (subset) | Yes | See Phase 2 |
| Module instantiation (flat) | Yes | Single-level composition |
| `assign` statements | Yes | Continuous combinational assignment |
| `generate` blocks | Deferred | Only with fully resolved parameters |
| Arrays/memories | No | State space too large |
| Interfaces/classes/packages | No | OOP features |
| `fork`/`join` | No | Dynamic concurrency |
| Real/string types | No | Not synthesizable |

### 0.4 Viability Gate Criteria

| Criterion | Pass Condition |
|-----------|---------------|
| All 3 back-translated SV files tokenize without error | Parser handles the supported subset |
| Extracted FSM state counts match existing CTXDSL | handshake=4, arbiter=3, traffic_light=4 |
| Extracted transitions match existing CTXDSL | Same source→target→label triples |
| Port-based controllability matches manual annotation | Input ports = uncontrollable, output ports = controllable |
| State space warning fires for >12 state bits | Create a test with a 16-bit counter |
| State space rejection fires for >18 state bits | Create a test with a 20-bit register |

### 0.5 Testing Approach for Phase 0

- Spike tests compare extracted CLTS against known-correct CTXDSL
- No full adapter pipeline yet — just lexer/tokenizer + FSM extraction
- Each spike test documents the SV source inline as a `const &str`
- Assertions: state names match, transition count matches, initial state matches

---

## Phase 1: Core Adapter — SystemVerilog → CTXDSL — `DONE`

**Prerequisite:** Phase 0 viability gate passed.

### 1.1 File Structure

```
src/adapter/systemverilog/
  mod.rs        — SystemVerilogAdapter, FormatAdapter impl, to_ir()
  ast.rs        — AST types for supported SV subset
  parser.rs     — Lexer + recursive-descent parser
  fsm.rs        — FSM extraction from always_ff blocks
```

### 1.2 Parser (`parser.rs`)

Custom recursive-descent parser. Key grammar productions:

```
module_decl     = "module" IDENT port_list ";" module_body "endmodule"
port_list       = "(" port ("," port)* ")"
port            = direction? type? IDENT width?
direction       = "input" | "output" | "inout"
type            = "logic" | "reg" | "wire"
width           = "[" expr ":" expr "]"
module_body     = (declaration | always_block | assign_stmt | sva_assertion | instance)*
always_block    = "always_ff" sensitivity statement
                | "always_comb" statement
sensitivity     = "@" "(" edge_list ")"
edge_list       = edge ("or" edge)*
edge            = "posedge" IDENT | "negedge" IDENT
statement       = if_stmt | case_stmt | block | assignment
block           = "begin" statement* "end"
if_stmt         = "if" "(" expr ")" statement ("else" statement)?
case_stmt       = "case" "(" expr ")" case_item+ "default" ":" statement "endcase"
assignment      = IDENT "<=" expr ";"  -- nonblocking (sequential)
                | IDENT "=" expr ";"   -- blocking (combinational)
```

Detection function: content contains `module` AND (`always_ff` OR `always_comb` OR `always @`).

### 1.3 FSM Extraction (`fsm.rs`)

Algorithm:

1. **Identify state registers:** Scan `always_ff` blocks. Any LHS of a nonblocking assignment (`<=`) is a flip-flop.
2. **Detect FSM state variable:** Look for `typedef enum` used as `case` selector in `always_ff`. Heuristic: variable in `case(X)` inside `always_ff` = FSM state var.
3. **Extract states:** From enum variants or case labels. Each variant/label = named state.
4. **Extract initial state:** From reset branch: `if (rst) state <= IDLE` → initial is `IDLE`.
5. **Extract transitions:** Per case branch:
   - Source = case label (state name)
   - `state <= TARGET` → target state
   - `if (guard)` → transition guard
   - Other assignments → output actions (become labels)
6. **Classify signals:**
   - `input` → `SignalKind::Input` (uncontrollable)
   - `output` → `SignalKind::Output` (controllable)
   - Internal → `SignalKind::Internal`
7. **Resolve combinational:** For `always_comb` outputs used in FSM guards, inline their definitions.

### 1.4 Emission Strategy

Use the **explicit-automaton** emission path (like Promela). Behavioral FSM extraction produces named states and sparse transitions. The signal-state path would re-enumerate 2^N states, losing structural advantage.

Non-FSM bounded registers (counters, shift registers ≤ 8 bits) → variable automata (following Promela's `create_variable_automaton`).

### 1.5 IR Construction (`to_ir()`)

1. Parse SV → AST
2. Extract FSM(s) → one `AutomatonSpec` per FSM
3. Input ports → `SignalKind::Input`, output ports → `SignalKind::Output`
4. Non-FSM bounded registers → variable automata
5. Module instances → `CompositionSpec::Synchronous` (connected ports = shared labels)
6. SVA assertions → `PropertySpec` (Phase 2)
7. `ControllerSpec` from first FSM + first property

### 1.6 Changes to Existing Code

- [adapter/mod.rs](src/adapter/mod.rs): `pub mod systemverilog`, `SourceFormat::SystemVerilog`, `.sv`/`.v` extension
- [main.rs](src/main.rs): `"systemverilog" | "sv"` case in `load_with_adapter`
- No changes to [adapter/ir.rs](src/adapter/ir.rs) or [adapter/emit.rs](src/adapter/emit.rs)

---

## Phase 2: Property Specification — `DONE`

### 2.1 SVA Subset Translation

| SVA Pattern | LTL Translation | Notes |
|-------------|-----------------|-------|
| `assert property (@(posedge clk) G(p))` | `G(p)` → `PropertyRole::Guarantee` | Direct |
| `assume property (@(posedge clk) G(p))` | `G(p)` → `PropertyRole::Assumption` | Environment constraint |
| `p \|-> q` (overlapping implication) | `G(p -> q)` | Same-cycle response |
| `p \|=> q` (non-overlapping) | `G(p -> X q)` | Next-cycle response |
| `##N` (cycle delay) | Chain of N `X` operators | Bounded delay |
| `##[1:$]` (eventually) | `F` | Unbounded eventual |
| `not p` | `!p` | Negation |
| `p throughout s` | `G(s -> p)` | Continuous hold |
| `$rose(p)` | `!p && X p` | Rising edge | 
| `$fell(p)` | `p && X !p` | Falling edge |

**Unsupported SVA:** `[*N]` repetition with N>3 (state explosion), `intersect`, `within`, `first_match` — emit `UnsupportedConstruct` warning.

### 2.2 Inline Comment Properties

```systemverilog
// @mununu ltl safety: G(!grant_a || !grant_b)
// @mununu ltl liveness: G(req -> F grant)
// @mununu assume: G(req -> req || X !req)
```

Parsed from `// @mununu` comments. Format: `@mununu [ltl|assume|guarantee] <name>: <formula>`.

### 2.3 Auto-Generated Defaults

When no explicit properties:
- Deadlock freedom: every FSM state has at least one outgoing transition
- State reachability: `mu X. (state_name || <> X)` per state
- Safety invariant: `nu X. ([] X)`

---

## Phase 3: Controller Output — CLTS → SystemVerilog — `DONE`

### 3.1 Output Format

Synthesized controller as a SystemVerilog module:

```systemverilog
// Generated by Mununu — correct-by-construction controller
module handshake_controller (
    input  logic clk, rst,
    input  logic req,           // preserved from original
    output logic ack            // driven by controller
);
    typedef enum logic [1:0] {IDLE, WAIT_ACK, ACTIVE, DONE} state_t;
    state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE:     if (req) state <= WAIT_ACK;
            WAIT_ACK: state <= ACTIVE;
            ACTIVE:   if (!req) state <= DONE;
            DONE:     state <= IDLE;
        endcase
    end

    always_comb begin
        ack = (state == ACTIVE);
    end
endmodule
```

### 3.2 Encoding Options

- **Binary encoding** (default): `typedef enum logic [N-1:0]`
- **One-hot encoding** (option `--encoding one-hot`): better for timing in real hardware

### 3.3 Port Interface

Preserve original module's port list. Controller drives output ports; input ports pass through unchanged.

---

## Phase 4: Benchmarks — Non-Trivial, Verifiable — `DONE`

### Benchmark V1: Handshake Protocol (Cross-Validation)

**Source:** Equivalent to [handshake.ctxdsl](examples/hw/handshake.ctxdsl).

```systemverilog
module handshake (
    input  logic clk, rst,
    input  logic req,
    output logic ack
);
    typedef enum logic [1:0] {IDLE, WAIT_ACK, ACTIVE, DONE} state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE:     if (req) state <= WAIT_ACK;
                      else    state <= IDLE;
            WAIT_ACK: state <= ACTIVE;
            ACTIVE:   if (!req) state <= DONE;
            DONE:     state <= IDLE;
        endcase
    end
    assign ack = (state == ACTIVE);
    // @mununu ltl safety: nu X. ([] X)
    // @mununu ltl reachability: mu X. (ACTIVE || <> X)
endmodule
```

**Expected state count:** 4
**Properties:** Safety invariant (realizable), ACTIVE reachability (realizable).
**Cross-validation:** Synthesis verdict must match `handshake.ctxdsl`.

### Benchmark V2: AXI4-Lite Slave Interface

**Source:** Write + read channel handshake.

```systemverilog
module axi4lite_slave (
    input  logic clk, rst,
    // Write channel (master drives VALID, slave drives READY)
    input  logic awvalid, wvalid, bready,
    output logic awready, wready, bvalid,
    // Read channel
    input  logic arvalid, rready,
    output logic arready, rvalid
);
    typedef enum logic [2:0] {
        IDLE, WR_ADDR, WR_DATA, WR_RESP, RD_ADDR, RD_DATA
    } state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE:    if (awvalid)     state <= WR_ADDR;
                     else if (arvalid) state <= RD_ADDR;
            WR_ADDR: if (wvalid)      state <= WR_DATA;
            WR_DATA: state <= WR_RESP;
            WR_RESP: if (bready)      state <= IDLE;
            RD_ADDR: state <= RD_DATA;
            RD_DATA: if (rready)      state <= IDLE;
        endcase
    end
    // @mununu ltl wr_response: G(state == WR_DATA -> F(bvalid && bready))
    // @mununu ltl no_data_loss: G(!(bvalid && !bready && state == IDLE))
    // @mununu assume wr_handshake: G(awvalid && !awready -> X awvalid)
endmodule
```

**Expected state count:** 6 FSM states
**Properties:**
| Name | Expected |
|------|----------|
| `wr_response` | Realizable — slave controls BVALID |
| `no_data_loss` | Realizable — structural (never in IDLE with pending response) |
| `wr_handshake` | Assumption — master holds VALID until READY |

**Expected realizability:** REALIZABLE.

### Benchmark V3: Round-Robin Arbiter (Parameterized, N=2,3,4)

**Source:** N-client arbiter with priority rotation.

```systemverilog
module rr_arbiter #(parameter N = 4) (
    input  logic clk, rst,
    input  logic [N-1:0] req,
    output logic [N-1:0] grant
);
    typedef enum logic [1:0] {IDLE, GRANT, HOLD} state_t;
    state_t state;
    logic [$clog2(N)-1:0] priority_ptr;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
            priority_ptr <= '0;
        end else case (state)
            IDLE:  if (|req) state <= GRANT;
            GRANT: state <= HOLD;
            HOLD:  if (!req[priority_ptr]) begin
                       state <= IDLE;
                       priority_ptr <= priority_ptr + 1;
                   end
        endcase
    end
    // @mununu ltl mutual_exclusion: G($onehot0(grant))
    // @mununu ltl no_starvation_0: G(req[0] -> F grant[0])
endmodule
```

**Expected state count:**
| N | FSM × Priority | Total Explicit |
|---|----------------|----------------|
| 2 | 3 × 2 = 6 | 6 |
| 3 | 3 × 3 = 9 | 9 |
| 4 | 3 × 4 = 12 | 12 |

**Properties:**
| Name | Formula | Expected |
|------|---------|----------|
| `mutual_exclusion` | `G($onehot0(grant))` | Realizable (structural) |
| `no_spurious` | `G(grant[i] -> req[i])` per i | Realizable |
| `no_starvation` | `G(req[i] -> F grant[i])` per i | Realizable (GR(1)) |

**Expected realizability:** All REALIZABLE. Round-robin is a valid GR(1) strategy.

**Scalability test:** Assert state count grows linearly with N. Measure synthesis time.

### Benchmark V4: UART Receiver (Boundary, Unrealizability)

**Source:** 12-state FSM × 16-value sample counter = 192 states.

```systemverilog
module uart_rx (
    input  logic clk, rst,
    input  logic rx,
    output logic data_valid
);
    typedef enum logic [3:0] {
        IDLE, START, D0, D1, D2, D3, D4, D5, D6, D7, STOP, ERR
    } state_t;
    state_t state;
    logic [3:0] sample_cnt;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin state <= IDLE; sample_cnt <= 0; end
        else case (state)
            IDLE:  if (!rx) begin state <= START; sample_cnt <= 0; end
            START: if (sample_cnt == 7) begin state <= D0; sample_cnt <= 0; end
                   else sample_cnt <= sample_cnt + 1;
            // D0..D7: sample at mid-bit, shift into register
            STOP:  if (rx) begin state <= IDLE; end
                   else state <= ERR;
            ERR:   state <= IDLE;
            // ...
        endcase
    end
    assign data_valid = (state == STOP);
    // @mununu ltl valid_after_byte: G(data_valid -> state == STOP)
    // @mununu ltl reception_terminates: G(state == START -> F(state == STOP || state == ERR))
endmodule
```

**Expected state count:** 12 × 16 = 192 (FSM × sample counter)

**Properties:**
| Name | Expected |
|------|----------|
| `valid_after_byte` | Realizable (structural — `data_valid` only when `STOP`) |
| `reception_terminates` | **UNREALIZABLE** — environment controls `rx`, can hold line low/high indefinitely preventing STOP |

**Expected realizability:** Safety REALIZABLE, liveness **UNREALIZABLE**.

### Benchmark V5: SPI Master (Medium Complexity)

**Source:** 5-state FSM + 4-bit counter.

**Expected state count:** 5 × 16 = 80

**Properties:**
| Name | Expected |
|------|----------|
| `cs_during_transfer` | `G(state != IDLE -> !cs_n)` — Realizable |
| `transfer_completes` | `G(start -> F done)` — Realizable (controller drives SCLK) |

---

## Phase 5: Test Plan — `DONE`

### Unit Tests (`src/adapter/systemverilog/`)

| Module | Tests |
|--------|-------|
| `parser.rs` | `parse_empty_module`, `parse_ports_with_widths`, `parse_always_ff`, `parse_always_comb`, `parse_enum_typedef`, `parse_case_statement`, `parse_if_else_chain`, `parse_sva_assert`, `parse_parameter`, `parse_assign`, `parse_inline_mununu_comment`, error cases (unsupported constructs) |
| `fsm.rs` | `extract_enum_fsm`, `extract_binary_fsm` (non-enum), `extract_initial_from_reset`, `extract_transitions_from_case`, `extract_transitions_from_if_else`, `extract_output_actions`, `extract_with_counter` (FSM + bounded register), `state_space_bound_warn` (>12 bits), `state_space_bound_reject` (>18 bits), `combinational_inlining` |
| `mod.rs` | `to_ir_basic_fsm`, `to_ir_port_classification` (input→uncontrollable, output→controllable), `to_ir_sva_to_property`, `to_ir_multi_module_composition`, `detect_sv_content`, `detect_v_content` |

### Integration Tests (`tests/adapter_systemverilog.rs`)

| Test | What It Validates |
|------|-------------------|
| `sv_detect` | Content detection for `.sv` / `.v` files |
| `sv_handshake_roundtrip` | Full pipeline: SV → IR → CTXDSL → parse → realize. Assert 4 states |
| `sv_axi4lite_roundtrip` | 6 states, SVA properties extracted |
| `sv_arbiter_n2_roundtrip` | Parameterized N=2, 6 states |
| `sv_arbiter_n4_roundtrip` | Parameterized N=4, 12 states |
| `sv_uart_roundtrip` | 192 states, boundary case |
| `sv_spi_roundtrip` | 80 states, counter variable automaton |
| `sv_state_space_overflow` | >18 bit design → `StateSpaceOverflow` error |
| `sv_enum_preserves_names` | Enum state names appear in CTXDSL output |
| `sv_auto_detect` | `auto_translate()` identifies SV content |

### System Tests (`tests/adapter_systemverilog_system.rs`)

| Test | Benchmark | Asserts |
|------|-----------|---------|
| `sv_handshake_synthesis` | V1 | 4 states, realizable, matches CTXDSL |
| `sv_axi4lite_synthesis` | V2 | 6 states, realizable |
| `sv_arbiter_mutex_n4` | V3 safety | 12 states, realizable |
| `sv_arbiter_no_starvation_n4` | V3 liveness | realizable (GR(1)) |
| `sv_uart_safety` | V4 safety | 192 states, realizable |
| `sv_uart_liveness_unrealizable` | V4 liveness | **unrealizable** |
| `sv_spi_synthesis` | V5 | 80 states, realizable |

### Cross-Format Tests (`tests/adapter_cross_format.rs`)

| Test | What It Validates |
|------|-------------------|
| `sv_vs_ctxdsl_handshake` | SV and CTXDSL produce identical synthesis verdict + winning region size |
| `sv_vs_ctxdsl_arbiter` | Same for arbiter |
| `sv_vs_aiger_simple_circuit` | Hand-craft 2-latch circuit in both AIGER and SV. Same verdict |

### Criterion Benchmarks (`benches/systemverilog.rs`)

| Benchmark | What It Measures |
|-----------|-----------------|
| `sv_parse_handshake` | Parse time for 4-state SV |
| `sv_parse_axi4lite` | Parse time for 6-state SV |
| `sv_parse_uart` | Parse time for 192-state SV |
| `sv_full_pipeline` | translate + realize + synthesize per benchmark |
| `sv_arbiter_scalable` | N=2,3,4 — state-space growth + synthesis time |
| `sv_vs_aiger_translate` | Translation time comparison on equivalent circuit |

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Custom SV parser too fragile for real-world code | Medium | High | Strict subset, clear error messages, consider `sv-parser` crate |
| Non-enum FSMs (binary-coded) harder to extract | Medium | Medium | Heuristic: `case(var)` with numeric patterns → infer state names |
| `always_comb` inlining creates complex guard expressions | Low | Medium | Flatten to CNF, warn on deeply nested combinational logic |
| Multi-module composition produces state explosion | Medium | Medium | Warn + reject at >18 total state bits across all modules |
| SVA translation incomplete for real assertions | High | Medium | Document supported subset clearly, emit warnings for unsupported |
| Parameterized modules with large N exceed limits | Medium | Low | Warn at elaboration time before expanding |

---

## Critical Files

| File | Role |
|------|------|
| [adapter/mod.rs](src/adapter/mod.rs) | FormatAdapter trait, registration |
| [adapter/ir.rs](src/adapter/ir.rs) | AdapterIR types (no changes expected) |
| [adapter/emit.rs](src/adapter/emit.rs) | Explicit-automaton emission path |
| [adapter/promela/mod.rs](src/adapter/promela/mod.rs) | Reference: variable automata pattern |
| [adapter/tlsf/mod.rs](src/adapter/tlsf/mod.rs) | Reference: signal-state encoding (comparison target) |
| [adapter/aiger/mod.rs](src/adapter/aiger/mod.rs) | Reference: gate-level equivalent (cross-validation) |
| [examples/hw/](examples/hw/) | Existing CTXDSL hw examples for cross-validation |

---

## Progress Log

_Update this section at the end of each working session._

| Date | Session Summary |
|------|----------------|
| 2026-04-08 | Plan created. No implementation work started. |
| 2026-04-08 | **Phases 0-2 completed.** Core adapter created with parser, FSM extraction, to_ir. 3 cross-validation tests against existing CTXDSL (handshake, arbiter, traffic_light). |
| 2026-04-08 | **Phases 4-5 completed.** Added AXI4-Lite slave (6 states, 3 tests), SPI master (5 states, 3 tests), UART receiver (8 states, 3 tests including controllability validation). Total: 18 SV system tests. |
| 2026-04-09 | **Phase 3 completed + parameterized modules.** Enhanced `emit_controller.rs` with port preservation (`controller_to_systemverilog_with_ports`), transition label comments, multi-target handling. Added 2 round-trip tests (handshake, arbiter) that verify emitted SV is re-parseable. Added `parameter`/`localparam` parsing to `parser.rs` (4 new parser tests). Added parameterized N=2,3,4 arbiter benchmarks (5 new system tests: N=2/3/4 structure+synthesis, linear scaling, reachability). Total: 25 SV system tests, 18 parser/FSM/unit tests. 768 total tests. All SV phases complete. |
