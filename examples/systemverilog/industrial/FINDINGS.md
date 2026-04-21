# Industrial Multi-Module Verification Findings

## Test Date: 2026-04-20

## Example 1: AXI-Lite Write Channel (Xilinx Bug Pattern)

### Setup
- **Master:** `axilite_write_master.sv` — 5-state FSM with backpressure capability
- **Slave:** `axilite_write_slave_xilinx_bug.sv` — Faithful to Xilinx Vivado 2018.3 template pattern
- **Sidecar:** Generated via `mununu sv init` (per-module), manually composed into multi-module sidecar
- **Discovery:** Not applicable (all signals are boolean)

### Pipeline Steps Completed
1. `mununu sv init` on both modules — **SUCCESS** (correct signal/enum detection)
2. Multi-module sidecar creation — **MANUAL** (no `sv init --multi` exists yet)
3. `translate_multi_module` — **SUCCESS** (composition produced 6 reachable states)
4. Property verification — **BOTH PASS** (no_deadlock + response_integrity)

### Result: Bug DETECTED

**Property `no_response_window` is UNREALIZABLE** — the tool correctly identifies that the state `axi_bvalid=T, aw_flag=F` is reachable in the composed system.

The model faithfully reproduces the Xilinx pattern with separate `always_ff` blocks for:
- AWREADY + aw_flag control (clears flag on data acceptance)
- WREADY control
- BVALID control (asserts on data acceptance, clears on bready)

The bug: `aw_flag` clears on `wready && wvalid` (data acceptance), but `axi_bvalid` is set on the same condition. After clearing, a new AWVALID can be accepted before the master acknowledges the response (BREADY). This exactly matches the ZipCPU documentation.

**Composed state space:** 7 reachable states (5 master × 16 slave potential → 7 reachable after synchronization). The tool exhaustively explored all reachable states and found the vulnerability window.

### What the tool found:
- The invariant `!axi_bvalid || aw_flag` (response pending implies write-blocked) is VIOLATED
- This means a new write CAN be accepted while a response is still outstanding
- In a real system, this leads to permanent bus deadlock when the second write completes

### First attempt (failed — documented for honesty):
The initial SV model used a single `always_ff` block with `~axi_bvalid` as a guard on AWREADY. This inadvertently FIXED the bug by synchronizing the flag and bvalid in the same cycle. The real Xilinx code has them in SEPARATE blocks with DIFFERENT conditions (flag clears on WLAST, bvalid asserts on WLAST), creating the one-cycle window. Splitting them into separate blocks was key to reproducing the bug.

---

## Limitations Discovered

### 1. No `sv init --multi` (multi-module sidecar generation)
- Per-module init works perfectly
- Multi-module sidecar must be assembled manually
- Connections, composition mode, and cross-module properties are hand-written

### 2. Combinational outputs require explicit declaration
- Outputs driven by `assign` statements need `"combinational": true` in the sidecar
- `sv init` does not auto-detect which outputs are combinational
- Without this flag, composition cannot synchronize on combinational signals

### 3. Timing races not captured by synchronous composition
- Synchronous composition assumes all `always_ff` blocks fire atomically
- Bugs requiring specific ordering WITHIN a clock cycle are not modeled
- This is fundamental to the synchronous product approach, not a tool bug

### 4. Property formulation requires domain expertise
- `sv init` generates only `nu X. ([] X)` (no deadlock)
- Meaningful safety properties must be written by hand
- Predicate naming convention (`variable_value`) must be known

---

## What Works Well

1. **`sv init` signal detection** — correctly identifies enums, boolean flags, bounded counters
2. **Multi-module composition** — synchronous product works correctly for the modeled behavior
3. **Valuation-based predicates** — formulas can reference register values across composed systems
4. **Combinational output tracking** — `assign`-driven signals included in state valuations
5. **State space pruning** — reachability analysis reduces 5×16=80 potential states to 6 reachable
