# `uart_codesign_protocol_spec` — C firmware + hand-authored peripheral protocol spec

> **Source of truth:** [`crates/mununu-core/src/verify/`](../../../crates/mununu-core/src/verify/) — surface: CLI+API+UI.

End-to-end verify-framework example for the **codesign C+protocol-spec
pairing**. The firmware side is real C source compiled via clang to
LLVM IR and lifted into a CTXDSL automaton; the peripheral side is a
hand-authored CTXDSL spec encoding the silicon's intended protocol.
Mirrors the canonical recipe at
[`examples/industrial/codesign_uart/extract_verify_recipe/`](../../industrial/codesign_uart/extract_verify_recipe/)
but driven from a single `verify.toml` instead of `run.sh`'s shell
pipeline.

## What it demonstrates

- **`c-codesign` adapter**, end-to-end. The firmware source's
  `[sources.options]` declares `cmsis_stubs = true` (bundles the
  `mununu_annotations.h` include path) and `register_map = …` (so
  `coupling::rendezvous_label_name` matches each LLVM-IR
  register access to a canonical rendezvous label). The verify
  orchestrator shells out to clang, parses the resulting LLVM IR,
  synthesises one CTXDSL automaton per function, wraps the lot in a
  `context FwSource { … }` block, and hands it to the assembler.
- **Direct alphabet binding** on the rendezvous-label alphabet. The
  firmware extraction emits `wr_ctrl_tx_start`, `wr_data_byte`,
  `rd_status_tx_busy`; the peripheral spec uses the exact same names
  in its transitions. The peripheral source declares only its own
  internal `tick` label (no redeclaration of firmware-driven labels).
- **Asynchronous composition** per Doc C §C.5 — bus arbitration is
  non-deterministic; synchronous coupling is unsound for racy access.
- **Three properties** that exercise the realiser end-to-end:
  - `firmware_reaches_sending` — firmware-side reachability of the
    `Sending` state. **VIOLATED** in the synthesised automaton
    because the polling loop can stay in `Loop0` indefinitely; this
    matches the existing recipe's verdict (the recipe also reports
    0/1 initial-state satisfaction) and is the property's known
    over-approximation under the linear-automaton synthesis L2-L3
    semantics.
  - `peripheral_reaches_transmitting` — peripheral reaches its
    `Transmitting` state from `Idle`. **SATISFIED**.
  - `safety_protocol_respected` — composed-system safety. **SATISFIED**
    (vacuous on this finite-state system).

## Files

| File | Purpose |
|---|---|
| `verify.toml` | Project config (two sources + composition + properties) |
| `peripheral_protocol.ctxdsl` | Hand-authored UART protocol spec (3-state FSM with no outbound `wr_ctrl_tx_start` from `Transmitting`) |
| `validate.sh` | End-to-end reproduction script |
| `transcript.txt` | Captured expected output |
| `README.md` | This file |

Firmware C + register-map sidecar are referenced from the canonical
codesign-uart example
(`../../industrial/codesign_uart/{extract_verify_recipe/firmware.c, register_map.json}`)
so the verify framework example reuses the same well-tested fixture
the recipe consumes.

## Reproduce

Requires `clang` on `$PATH` (the c-codesign adapter shells out to it):

```bash
bash examples/verify/uart_codesign_protocol_spec/validate.sh
```

## Run manually

```bash
mununu verify examples/verify/uart_codesign_protocol_spec/verify.toml
mununu verify examples/verify/uart_codesign_protocol_spec/verify.toml --json
```

## Relationship to the existing codesign recipe

`examples/industrial/codesign_uart/extract_verify_recipe/run.sh` is
the **canonical demonstration** of the C-firmware + protocol-spec
workflow. It drives:
  - `mununu codesign extract-c` (CLI)
  - `jq` to extract the synthesised CTXDSL fragment
  - Python to splice the fragment into a hand-authored template
  - `mununu context eval` to verify each formula

This example is the **same workflow driven from one `verify.toml`**
via the general verify framework (A2 stage). Same fixture, same
verdicts; the difference is the entry point and the inferred
composition.
