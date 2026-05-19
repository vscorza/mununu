# Proof-by-Fire — Findings (in progress)

> **Status: live audit log.** Empirical results of attempts to demonstrate
> mununu's auto-extraction pipelines against real, public, documented bugs in
> hardware / codesign / driver domains. Per the [active plan](../../../.claude/plans/create-a-plan-to-enumerated-patterson.md).

Start date: 2026-05-18. Each row is an honest record of an extraction attempt;
no row claims Success without a shipped `examples/verify/<id>/` directory.

## Candidate ledger

| # | Pipeline | Candidate | Upstream | Pinned commit | Bug citation | Outcome |
|---|---|---|---|---|---|---|
| 1 | D (sv-yosys) | Caliptra RTL boot FSM (RTL-002) | [chipsalliance/caliptra-rtl](https://github.com/chipsalliance/caliptra-rtl) `src/soc_ifc/rtl/soc_ifc_boot_fsm.sv` | `b436906f0b16ae0cfbb160e927499b79800ec9ce` (2023-07-08) | [Issue #150](https://github.com/chipsalliance/caliptra-rtl/issues/150) — CWE-1245, `unique casez` without `default:` | **Failure — systemic blocker** (Finding 1); **Finding 1 SV-parse blocker resolved 2026-05-18 via sv2v integration (Phase 1)**; retry now blocked by Finding 2 (state-bit cap) + a CLI multi-file gap (see Finding 1 update) |
| 2 | E (BTOR2) | Pono "data-integrity" FIFO `arbitrated_top_n2_w8_d8_e0.btor2` (smallest variant) | [makaimann/btor-benchmarks](https://github.com/makaimann/btor-benchmarks) | repo `master` 2026-05-18 | Mukherjee/Kroening/Melham (DAC 2016, [arXiv 1606.02347](https://arxiv.org/pdf/1606.02347)); used in Pono CAV 2022 tool paper | **Failure — systemic blocker** (Finding 2) |
| 3 | A (C-extract) | Zephyr I/O APIC signed-cast register-offset bug | [zephyrproject-rtos/zephyr](https://github.com/zephyrproject-rtos/zephyr) `drivers/interrupt_controller/intc_ioapic.c` | PR [#50337](https://github.com/zephyrproject-rtos/zephyr/pull/50337) — fix is `(char) → (unsigned char)` | [Issue #49803](https://github.com/zephyrproject-rtos/zephyr/issues/49803) — `(char)offset` sign-extends offsets ≥ 0x80; wrong index written to `IOAPIC_IND` | **Failure — semantic mismatch** (Finding 3) |
| 4 | A (C-extract) | RIOT-OS CC2538 endless-loop on spoofed length byte | [RIOT-OS/RIOT](https://github.com/RIOT-OS/RIOT) `cpu/cc2538/radio/cc2538_rf_radio_ops.c` | pre-fix `1a418ccfedeb…` / fix in PR [#20998](https://github.com/RIOT-OS/RIOT/pull/20998) | [GHSA-m75q-8vj8-wppw](https://github.com/RIOT-OS/RIOT/security/advisories/GHSA-m75q-8vj8-wppw) / [CVE-2024-53980](https://www.cve.org/CVERecord?id=CVE-2024-53980) | **Failure — semantic mismatch** (Finding 3, same shape) |

## Findings

### Finding 1 — Pipeline D (sv-yosys) cannot ingest modern SystemVerilog dialect

**Target.** Caliptra-RTL `soc_ifc_boot_fsm.sv` (Apache-2.0) at commit
`b436906f0b16ae`. The bug — `unique casez (boot_fsm_ps)` over a 5-arm enum
with no `default:` branch — is the most extensively documented real RTL bug
mununu has staged (see `.claude/reviews/prospector/staging/RTL-002/`).

**What was attempted.** Direct pass through `mununu`'s sv-yosys driver via
Yosys 0.59 (matches `mununu`'s required ≥ 0.40):

```bash
yosys -q -p "read_verilog -formal -sv -I. \
  caliptra_top_reg_defines.svh soc_ifc_pkg.sv soc_ifc_reg_pkg.sv \
  soc_ifc_boot_fsm_pre_fix.sv; hierarchy -auto-top"
```

**Iterations attempted.**

| # | Failure | Mitigation tried | Status |
|---|---|---|---|
| 1 | `ERROR: Can't open include file 'caliptra_top_reg_defines.svh'` | Created empty stub | Resolved iteration 1 |
| 2 | `ERROR: Unimplemented compiler directive or undefined macro 'CALIPTRA_TOP_REG_MBOX_CSR_BASE_ADDR'` | Stubbed three address macros (`CALIPTRA_TOP_REG_MBOX_CSR_BASE_ADDR`, `_SHA512_ACC_CSR_BASE_ADDR`, `_GENERIC_AND_FUSE_REG_BASE_ADDR`) | Resolved iteration 2 |
| 3 | `ERROR: syntax error, unexpected '[', expecting TOK_ID` on `parameter [4:0][31:0] CPTRA_MBOX_VALID_PAUSER = {…}` in `soc_ifc_pkg.sv` | Slimmed `soc_ifc_pkg.sv` to just the `boot_fsm_state_e` enum | Resolved iteration 3 |
| 4 | `ERROR: syntax error, unexpected TOK_IMPORT, expecting '#' or '(' or ';'` on the module's header `module soc_ifc_boot_fsm import soc_ifc_pkg::*; (…)` | **None possible without modifying the bug-bearing file** | **Hard blocker** |

The fourth error is the systemic one. The upstream Caliptra boot FSM uses
**SystemVerilog 2009/2012 module-header import syntax** (`module M import
pkg::*; (ports);`). Yosys 0.59's built-in `read_verilog -sv` parser does
**not** accept this construct. The same syntax pattern is endemic in modern
open-source RTL (OpenTitan, ibex, cv32e40p, Hazard3 all use it routinely).

**Why this is a *systemic* blocker, not a Caliptra-specific one.**

- The mununu sv-yosys adapter ([`crates/mununu-core/src/adapter/yosys/mod.rs`](../../crates/mununu-core/src/adapter/yosys/mod.rs)) invokes Yosys directly with `read_verilog -formal -sv` — no `sv2v` or
  preprocessing step. The driver also refuses Verific-built Yosys binaries
  (license incompatibility, by design).
- Any upstream module that uses package-import-in-header (introduced
  IEEE 1800-2009 §23.2.1) will fail at the parse phase before any of
  mununu's logic sees the design.
- This affects, at a minimum: Caliptra-RTL, OpenTitan IP blocks, lowRISC ibex,
  OpenHWGroup cv32e40p, Hazard3 — i.e. essentially the entire open-source RTL
  fleet built since ~2015.

**Mitigation does not exist within mununu's READY pipelines today.** The
options are:

- Hand-port the upstream module to Verilog-2005 syntax — violates condition
  (1) "models must be auto-extracted, no hand-authoring of bug-bearing
  artifacts".
- Add `sv2v` (open-source SV-to-Verilog-2005 translator) as a preprocessing
  stage in the sv-yosys driver — engineering work, not in any open plan.
- Use a Verific-licensed Yosys build — disabled by the mununu driver's
  Verific check.

### Phase 1 update — sv2v integration shipped (2026-05-18)

**Resolution status: SV-parse blocker resolved. Caliptra retry still blocked, but on a different blocker (Finding 2 + a CLI gap).**

Per the [active plan's Phase 1](../../../.claude/plans/create-a-plan-to-enumerated-patterson.md), sv2v ([zachjs/sv2v](https://github.com/zachjs/sv2v) v0.0.13) was integrated into the sv-yosys driver as an optional preprocessing pass. Opt-in via `MUNUNU_USE_SV2V=1` or `YosysOptions.use_sv2v = true`. Implementation lives in [`crates/mununu-core/src/adapter/yosys/mod.rs`](../../crates/mununu-core/src/adapter/yosys/mod.rs) (`locate_sv2v`, `run_sv2v`, `env_flag` helpers; pipe inserted before the Yosys subprocess). Three unit tests cover the integration: module-header `import pkg::*` parses through sv2v + Yosys; cross-file package imports resolve; missing-tool error is clean.

**Caliptra retry under the new pipeline.** Reproduction:

```bash
mkdir /tmp/caliptra_retry && cd /tmp/caliptra_retry
cp ~/git_repo/mununu/.claude/reviews/prospector/staging/RTL-002/source/*.sv .
# Plus minimal SVA-macro stub for caliptra_sva.svh (Yosys is Phase-1-SVA only;
# concurrent SVA macros expand to no-ops):
cat > caliptra_sva.svh <<'EOF'
`define CALIPTRA_ASSERT_KNOWN(ID, SIG, CLK, RST_B)
`define CALIPTRA_ASSERT_NEVER(ID, EXPR, CLK, RST_B)
EOF
# And empty stubs for the un-published address macros + reg pkg:
cat > caliptra_top_reg_defines.svh <<'EOF'
`define CALIPTRA_TOP_REG_MBOX_CSR_BASE_ADDR             32'h0
`define CALIPTRA_TOP_REG_SHA512_ACC_CSR_BASE_ADDR       32'h0
`define CALIPTRA_TOP_REG_GENERIC_AND_FUSE_REG_BASE_ADDR 32'h0
EOF
echo 'package soc_ifc_reg_pkg; endpackage' > soc_ifc_reg_pkg.sv

# Standalone sv2v works end-to-end:
sv2v -I. soc_ifc_pkg.sv soc_ifc_reg_pkg.sv soc_ifc_boot_fsm_pre_fix.sv > preprocessed.v
#   → exit 0, 211 lines emitted.

# Through mununu, single-file CLI path:
MUNUNU_USE_SV2V=1 mununu context eval soc_ifc_boot_fsm_pre_fix.sv \
    --adapter sv-yosys --formula safety_bad_0 --automaton Circuit
```

Two next-layer outcomes (both honest):

1. **State-bit cap (Finding 2) bites at 19 bits.** After sv2v + Yosys
   succeed on the *preprocessed* Verilog (running the pipeline directly on
   `preprocessed.v`):

   ```
   Yosys SV adapter error: adapter/yosys: BTOR2 reader failed:
     BTOR2 design has 19 state bits → 2^19 = 524288 states
     (max supported: 2^16 = 65536).
   ```

   The boot FSM module bundles a 3-bit state enum with an 8-bit wait
   counter and reset-window logic that together exceed the cap by 3 bits.
   Finding 2's unlock (Phase 3 compose-and-decompose or external-engine
   handoff) is the gate.

2. **CLI multi-file gap.** The single-file CLI invocation
   `mununu context eval soc_ifc_boot_fsm_pre_fix.sv` only stages the
   primary `.sv` into the tempdir; sv2v then cannot resolve the
   `soc_ifc_pkg::*` import because the sibling package files are not
   passed. sv2v silently emits zero output and exits 0 in this scenario,
   which Yosys then reports as "No top module found." The verify
   framework (`verify.toml`'s `[[sources]] files = [...]`) currently has
   **no sv-yosys dispatcher path**, only `c-codesign`, `ctxdsl`, `xstate`,
   `crewai`, `langgraph`, `microcode`, `sv-multi`. Wiring sv-yosys into the
   verify orchestrator's multi-file path is a small follow-up, scoped
   outside Phase 1.

**Phase 1 verdict.** SV-parse blocker (Finding 1) is **resolved at the
adapter level**. Caliptra-RTL #150 end-to-end PoC is **still blocked**, but
on a different layer — Finding 2 (state-bit cap) plus the CLI multi-file
gap. Both are addressable by subsequent phases / small follow-ups; Phase 2
or Phase 3 of the active plan would pick them up.

**Files touched in Phase 1.**

- [`crates/mununu-core/src/adapter/yosys/mod.rs`](../../crates/mununu-core/src/adapter/yosys/mod.rs) — `YosysOptions.use_sv2v`, `locate_sv2v`, `run_sv2v`, `env_flag`; integration in `translate_sv`; 3 new tests.

### Phase 1.5 update — CLI multi-file + state cap 16 → 20 (2026-05-18)

Per the user's incremental ask after Phase 1's two-blocker outcome, two
follow-up unblocks shipped together:

1. **CLI multi-file for sv-yosys.** [`crates/mununu-cli/src/loader.rs`](../../crates/mununu-cli/src/loader.rs) — new `load_with_adapter_mode_extra()` accepts additional source paths. `load_context_documents_mode()` now partitions `--sidecar` arguments by file extension: `.sv` / `.svh` flow to the sv-yosys driver's `YosysOptions::additional_sources` (multi-file SV elaboration); everything else continues as a regular CTXDSL sidecar. The driver's sv2v invocation canonicalizes the primary source path so its parent directory's `-I` flag resolves correctly under relative CLI invocations.

2. **State-bit cap 16 → 20.** [`crates/mununu-core/src/adapter/btor2/bit_blast.rs`](../../crates/mununu-core/src/adapter/btor2/bit_blast.rs) — `MAX_STATE_BITS` raised from 16 to 20 (2^20 ≈ 1 M states). The existing overflow-rejection test was rewritten to derive its threshold from the constant rather than hard-coding 17. Input-bit cap (`MAX_INPUT_BITS = 10`) was left at 10 — see the new caveat in Finding 2 below.

**Caliptra retry after Phase 1.5.**

```bash
cd /tmp/caliptra_retry  # holds the 3 upstream .sv files + the stub headers
MUNUNU_USE_SV2V=1 mununu context eval soc_ifc_boot_fsm_pre_fix.sv \
  --adapter sv-yosys \
  --sidecar soc_ifc_pkg.sv --sidecar soc_ifc_reg_pkg.sv \
  --formula safety_bad_0 --automaton Circuit
```

Outcome — the pipeline now runs cleanly through sv2v → Yosys → BTOR2 and stops on the *input-bit cap*, not the *state-bit cap*:

```
Yosys SV adapter error: adapter/yosys: BTOR2 reader failed:
  BTOR2 design has 16 input bits per step (max supported: 10).
```

Progress chain so far on Caliptra: SV-parse blocker (Finding 1) → cleared by sv2v; state-bit cap (Finding 2 / state side) → cleared by 16 → 20 lift; **next-layer blocker: input-bit cap** (`MAX_INPUT_BITS = 10`, design has 16). The pipeline reaches BTOR2 and would explore 19 state bits (~524 K states) given freedom over its inputs.

**Why the input cap was not also raised.** Raising it to 16 was attempted and reverted. With `MAX_STATE_BITS=20`, lifting `MAX_INPUT_BITS` to 16 produces a transition budget of 2^20 × 2^16 ≈ 6.8e10 — concrete enumeration runs for hours and was killed at the 5-minute mark on the Caliptra design. The cap is genuinely the right defense against explosion at the explicit-state engine's scale; the unlock path for designs with many inputs is **sidecar-based input pruning** (declare unused inputs as `Ignored` / `Boolean` / `Symbols` in a `.mununu.json`), not a higher numeric cap.

**Phase 1.5 verdict.** Two more layers of the Caliptra blocker chain are cleared. The next-layer blocker is the input-bit cap, which is a soundness-aware engineering task (sidecar input pruning) rather than a numeric-constant flip. Documented for the user to direct the next phase.

**Additional files touched in Phase 1.5.**

- [`crates/mununu-cli/src/loader.rs`](../../crates/mununu-cli/src/loader.rs) — `load_with_adapter_mode_extra`, sidecar extension partitioning.
- [`crates/mununu-core/src/adapter/btor2/bit_blast.rs`](../../crates/mununu-core/src/adapter/btor2/bit_blast.rs) — `MAX_STATE_BITS` constant 16 → 20; test threshold derives from constant.
- [`examples/btor2/README.md`](../../examples/btor2/README.md) — cap reference updated.

**Honesty disclosure.** Three mitigation iterations on dependency
stubbing were performed before the systemic blocker was hit. Stubs created
were trivial (empty macros, empty packages, an enum-only slim package); they
counted as "configuration" per the plan's permission. The fourth iteration's
blocker is in the bug-bearing file's syntax itself, and cannot be addressed
without modifying that file.

**Recommended engineering work to unlock pipeline D for modern RTL.**

- Add an `sv2v`-based preprocessing stage to the sv-yosys driver
  ([`crates/mununu-core/src/adapter/yosys/mod.rs:541`](../../crates/mununu-core/src/adapter/yosys/mod.rs#L541)). Source: [`zachjs/sv2v`](https://github.com/zachjs/sv2v) is the de-facto open-source preprocessor for this layer; integrates as a stdin/stdout filter.
- Tracking issue and scoping: outside this plan's READY-pipelines-only constraint.

**Reproduction.**

```bash
mkdir /tmp/caliptra_repro && cd /tmp/caliptra_repro
cp ~/git_repo/mununu/.claude/reviews/prospector/staging/RTL-002/source/*.sv .
yosys -q -p "read_verilog -formal -sv soc_ifc_pkg.sv soc_ifc_boot_fsm_pre_fix.sv"
# expect: ERROR: Unimplemented compiler directive or undefined macro `CALIPTRA_TOP_REG_MBOX_CSR_BASE_ADDR.
# ... (iteration through 4 errors; the fourth has no mitigation per the table above)
```

### Finding 2 — Pipeline E (BTOR2) state-bit cap of 16 excludes every real industrial target

**Target.** Pono BTOR2 benchmark suite — the smallest documented-bug variant in
`makaimann/btor-benchmarks/data-integrity/unsafe/btor2/`, namely
`arbitrated_top_n2_w8_d8_e0.btor2` (2 arbiters, 8-bit data, depth-8 FIFOs).
The benchmark family ships with safe (fixed) and unsafe (buggy) pairs and is
cited in Mukherjee/Kroening/Melham (DAC 2016, [arXiv 1606.02347](https://arxiv.org/pdf/1606.02347))
and the Pono CAV 2022 tool paper.

**What was attempted.**

```bash
curl -sL -o fifo.btor2 \
  "https://raw.githubusercontent.com/makaimann/btor-benchmarks/master/data-integrity/unsafe/btor2/arbitrated_top_n2_w8_d8_e0.btor2"
mununu context eval fifo.btor2 --adapter btor2 --formula safety_bad_0 --automaton Circuit
```

**Result.**

```
BTOR2 adapter error: BTOR2 design has 176 state bits → 2^176 = 9223372036854775808 states
(max supported: 2^16 = 65536). Compose-and-decompose (Phase 3) or hand-off to an
external symbolic engine.
```

The cap is documented at [`crates/mununu-core/src/adapter/btor2/bit_blast.rs`](../../crates/mununu-core/src/adapter/btor2/bit_blast.rs) (`MAX_STATE_BITS = 16`) and re-iterated in
[`examples/btor2/README.md`](../../examples/btor2/README.md) as "≤ 2^16-bit state space."

**Why this is a *systemic* blocker, not a Pono-specific one.**

- 16 state bits ↔ 65536 reachable states ↔ ~16 latches in AIGER terms.
  Industrial RTL benchmarks are routinely 50–500 latches.
- Of the Pono benchmark family's 20+ size variants, **`n2_w8_d8` is the
  smallest** and still uses 176 state bits. No combination of width / depth
  parameters gets the FIFO under 16 bits.
- The same cap applies to pipeline F (AIGER, [`crates/mununu-core/src/adapter/aiger/mod.rs`](../../crates/mununu-core/src/adapter/aiger/mod.rs)). HWMCC benchmarks are typically 20–200 latches, all over the cap.
- Existing in-repo `.btor` and `.aag` examples
  ([`examples/btor2/`](../../examples/btor2/), [`examples/aiger/`](../../examples/aiger/)) are all hand-sized to
  fit: 2-bit counters, 3-state FSMs, 32-state arbiters. None of them are
  real-world artifacts.

**Recommended engineering work to unlock pipelines E and F for real benchmarks.**

- Implement the documented Phase 3 "compose-and-decompose" escape hatch
  (referenced in the BTOR2 error message and in [`examples/btor2/README.md`](../../examples/btor2/README.md) "Soundness notes").
- Or wire mununu as a frontend to an external symbolic engine (BTOR2 →
  Pono/AVR/BtorMC) and ingest the symbolic engine's counterexample. Out of
  scope for this plan's READY-pipelines-only constraint.

**Reproduction.**

```bash
mkdir /tmp/pono_repro && cd /tmp/pono_repro
curl -sL -o fifo.btor2 \
  "https://raw.githubusercontent.com/makaimann/btor-benchmarks/master/data-integrity/unsafe/btor2/arbitrated_top_n2_w8_d8_e0.btor2"
~/git_repo/mununu/target/debug/mununu context eval fifo.btor2 \
  --adapter btor2 --formula safety_bad_0 --automaton Circuit
# expect: BTOR2 adapter error: ... 176 state bits → 2^176 = ... (max supported: 2^16)
```

The blocker is uniform across the AIGER cohort the agent also surfaced —
`cmu.dme1.B.aig` has 61 latches (≫ 16), and every other `data-integrity/unsafe/aig/`
variant is in the same regime.

### Finding 3 — Pipeline A (C-extract LLVM-IR) does not model register-access values, so value-based bugs are invisible

**Targets evaluated.** Two top-ranked candidates from the empirical CVE
search, both real and well-documented:

- **Zephyr I/O APIC** (Apache-2.0): [`drivers/interrupt_controller/intc_ioapic.c`](https://github.com/zephyrproject-rtos/zephyr/blob/main/drivers/interrupt_controller/intc_ioapic.c). The fix at [PR #50337](https://github.com/zephyrproject-rtos/zephyr/pull/50337) is a one-character change repeated twice:
  ```diff
  -	*((volatile uint32_t *) (IOAPIC_REG + IOAPIC_IND)) = (char)offset;
  +	*((volatile uint32_t *) (IOAPIC_REG + IOAPIC_IND)) = (unsigned char)offset;
  ```
- **RIOT-OS CC2538 radio** (LGPL-2.1): [`cpu/cc2538/radio/cc2538_rf_radio_ops.c`](https://github.com/RIOT-OS/RIOT/blob/main/cpu/cc2538/radio/cc2538_rf_radio_ops.c). The fix at [PR #20998](https://github.com/RIOT-OS/RIOT/pull/20998) masks bit 7 off the packet-length byte before a CRC offset calculation. Same semantic shape: a value bit-mask, not a control-flow change.

**Empirical capability check.** Read [`crates/mununu-core/src/codesign/c_extract.rs:32-61`](../../crates/mununu-core/src/codesign/c_extract.rs#L32-L61) — the `RegisterAccess` struct:

```rust
pub struct RegisterAccess {
    pub kind: AccessKind,          // Read | Write
    pub register: String,          // register name from register-map
    pub field: Option<String>,     // field name from register-map
    pub accessor: String,          // diagnostic string
    pub source_line: u32,
    pub flow: AccessFlow,          // Linear | PollingLoop
    pub source_state_hint: Option<String>,
}
```

**There is no field for the stored value.** The L2 extractor models register
accesses as "which register was touched, in what kind (read or write), in
what control-flow context". The actual value being written is not recorded.

**Consequence.** Both candidates' bugs are pure value-level defects:

- Zephyr: same store address, same store kind (write to `IOAPIC_IND`), same
  control-flow position; the difference is `sext` vs `zext` on the stored
  SSA value, which never reaches the extracted model.
- CC2538: same `rfcore_peek_rx_fifo(pkt_len)` access shape; the difference
  is whether `pkt_len` was masked first. Again value-level.

Mununu's auto-extracted automaton for the buggy version is **identical** to
the auto-extracted automaton for the fixed version. There is no property
expressible over the model that can distinguish them. Phase L4
read-modify-write recognition does inspect store-value SSA chains but only
to *identify the field* being touched, not to record the value semantically.

**Why this is a *systemic* blocker, not a candidate-specific one.**

- The C extractor is intentionally designed for **register-access-sequence**
  and **control-flow** bugs — wrong order of writes, missing read-before-write,
  wrong path traversed. The canonical demonstration is the nRF52 TWIM
  example, where the planted bug inverts the order of two writes — exactly the
  shape L2 captures.
- The CVE search empirically returned candidates where the documented bug
  shape is overwhelmingly **value-level** (sign-extension, mask-off, wrong
  arithmetic, integer overflow). Value-level bugs do not fit L2.
- Of the three top-ranked CVE candidates the search returned (Zephyr ioapic
  / RIOT CC2538 / Contiki uIPv6 TCP MSS), none has a sequence-of-writes or
  control-flow-path bug shape. The third (Contiki) is an OOB-read memory
  safety bug, also outside L2's reach.

**Recommended engineering work to unlock pipeline A for value-level bugs.**

- Extend `RegisterAccess` to carry the SSA value's symbolic form (constant
  pattern, sign-extension classification, mask-bit-pattern). This is a
  non-trivial addition — the L2 extractor would need to track value
  abstractions across the SSA web, and the CTXDSL emitter would need to
  emit labels that distinguish values.
- Or: ship a complementary pipeline targeted at value-level analysis
  (small-value abstraction over registers).
- Or: focus the proof-by-fire on bug *classes that L2 already captures*
  (sequence inversions, missing guards, missing barriers, polling-loop
  termination via composition with a hand-authored protocol spec). Find a
  real upstream bug of that shape — which is empirically rare in CVE
  databases, where security-relevant bugs skew toward memory-safety and
  value-level defects.

**Reproduction.**

```bash
gh pr diff 50337 --repo zephyrproject-rtos/zephyr
# Inspect: the fix is `(char)offset` → `(unsigned char)offset` ONLY.
# No control-flow change, no access sequence change — pure value-level fix.
grep -nE "pub struct RegisterAccess|pub kind|pub register|pub field|pub accessor" \
  crates/mununu-core/src/codesign/c_extract.rs
# Inspect: no value-recording field in RegisterAccess.
```

## Summary

Three real candidates evaluated against three READY pipelines (A, D, E/F).
Three systemic blockers documented. Zero PoCs shipped.

| Pipeline | Blocker | Severity |
|---|---|---|
| **A (C-extract LLVM-IR)** | RegisterAccess struct does not record store values — value-level bugs (sign-extension, mask, arithmetic) are invisible. | Systemic for CVE-database-style bugs (skew toward value-level). Sequence/control-flow bugs would still fit, but are rare in public CVE corpora. |
| **D (sv-yosys)** | Yosys 0.59 cannot parse SV2009/2012 module-header `import pkg::*` syntax. Affects essentially all modern open-source RTL. | Systemic across the open-source RTL fleet (Caliptra, OpenTitan, ibex, cv32e40p, Hazard3, ...). Requires sv2v preprocessor or Verific-licensed Yosys (refused by mununu driver). |
| **E (BTOR2)** | 16-state-bit hard cap — 65 536 reachable states max. Smallest Pono FIFO is 176 state bits; HWMCC entries are typically 20–200 latches. | Systemic across published model-checking benchmark suites. Requires Phase 3 compose-and-decompose or external symbolic engine. |
| **F (AIGER)** | Same state-bit cap as E. Same blocker. | Systemic. |

**Honest takeaway.** Mununu's auto-extraction pipelines today are
**capability-bounded to specific bug shapes and specific design scales**.
They demonstrate the verification machinery soundly on toy-scale, planted-bug
examples. They do **not** yet detect real upstream bugs in real public
codebases at the scale the broader publication audience would expect. The
gap is real, the gap is specific, and the gap is addressable — but the
addressing is engineering work that is *out of scope* for a READY-pipelines-only
proof-by-fire effort.

This findings doc is the deliverable of that effort. It serves the same
role as a negative experimental result in a research publication: the
honest, evidence-backed statement of what *doesn't* work today, with the
specific blockers documented and the unlock paths sketched.

## What would unlock at least one Pipeline A success

The narrowest unlock — finding **one** real public bug whose shape fits L2:

- The bug must be in the **sequence of register accesses** (wrong order,
  missing access, extra access) **or** in the **control-flow path** (loop
  termination guard, conditional bypass) — not in the value being written.
- The C source must be **self-contained** (no vendor SDK include trees) or
  trivially reducible to a minimal-repro `.c` file (the nRF52 pattern,
  documented in [`examples/industrial/codesign_nrf52_twim_i2c/README.md`](../../examples/industrial/codesign_nrf52_twim_i2c/README.md)).
- The bug must be **documented upstream** (CVE / GH security advisory /
  pinned-commit issue) with a corresponding fix at a pinned commit.

Searches in this category — CVE corpora filtered for "wrong order of
register writes" / "missed barrier" / "missed acknowledgment" — were not
exhaustive in this audit. They are the recommended next step *if and only
if* the proof-by-fire effort is to be continued with the L2 capability
boundary as a given.
