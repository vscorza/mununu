# `microprogram_plus_sv` — CTXDSL microcode + SV peripheral

> **Source of truth:** [`crates/mununu-core/src/verify/`](../../../crates/mununu-core/src/verify/) — surface: CLI+API+UI.

Third example in the verify-framework fleet. Demonstrates the
framework's **adapter-agnosticism** by pairing a hand-authored
CTXDSL microprogram with a real SystemVerilog peripheral
(`SystemVerilogAdapter::translate`) under one `verify.toml`.

## What it demonstrates

- **Heterogeneous sources end-to-end.** One `ctxdsl` source +
  one `sv-rtl` source compose without code changes on the
  framework side. Each adapter runs independently; the assembler
  merges their CTXDSL outputs into a single context.
- **Asynchronous composition with disjoint alphabets.** The two
  sides share no labels — the microprogram cycles on
  `tick_microcode`, the SV peripheral on its port-binding labels.
  Asynchronous composition produces a **product state space**:
  4 microcode states × 4 peripheral states = 16 composed states.
- **Property reasoning over the product.** The orchestrator's
  realiser builds the 16-state composed CLTS and the μ-calculus
  evaluator answers `no_deadlock` (template) and reachability
  (inline formula) queries over both the source automata and the
  composition.

## Files

| File | Purpose |
|---|---|
| `verify.toml` | Project config (two sources + composition + properties) |
| `microprogram.ctxdsl` | Hand-authored 4-state microcode FSM |
| `handshake_peripheral.sv` | SV req/ack handshake module |
| `validate.sh` | Reproduction script |
| `transcript.txt` | Checked-in expected output |
| `README.md` | This file |

## Reproduce

No special prerequisites (the SV adapter parses inline; no clang
shell-out required):

```bash
bash examples/verify/microprogram_plus_sv/validate.sh
```

## Run manually

```bash
mununu verify examples/verify/microprogram_plus_sv/verify.toml
mununu verify examples/verify/microprogram_plus_sv/verify.toml --json
```

Successful run produces:

```
composition: asynchronous Driver { members = [Microprogram, handshake_peripheral] }
microcode_no_deadlock: SATISFIED (4/4 states, 1/1 initial)
peripheral_reaches_active: SATISFIED (4/4 states, 1/1 initial)
composed_no_deadlock: SATISFIED (16/16 states, 1/1 initial)
```

## Limitations and future extensions

The two automata don't currently **synchronise** — the alphabets are
disjoint. The asynchronous composition just interleaves their
independent state spaces. To make the microprogram and peripheral
actually coordinate (e.g., a microcode `AssertReq` step driving the
peripheral's `req → WAIT_ACK` transition), the SV side would need
either:

1. A `.mununu.json` sidecar declaring the SV adapter's
   port-binding labels with names matching the microprogram's
   events, or
2. A `[alphabet] strategy = "renamings"` entry in `verify.toml`
   pointing the SV adapter's `req_T` / `req_F` labels at the
   microprogram's `tick_microcode`-style events.

These extensions are tracked as future verify-framework work; the
present fixture exercises the **adapter dispatch and composition**
layer at minimum.
