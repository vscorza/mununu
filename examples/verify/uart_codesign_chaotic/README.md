# `uart_codesign_chaotic` — C firmware + chaotic-stub peripheral

> **Source of truth:** [`crates/mununu-core/src/verify/`](../../../crates/mununu-core/src/verify/) — surface: CLI+API+UI.

Fourth (and final) example in the verify-framework fleet. Demonstrates
the **canonical chaotic-stub codesign verification** pattern (Doc C
§C.4 / §C.5) under the general verify framework: real C firmware
composed asynchronously with a chaotic peripheral stub that admits
every register access at any time.

## What it demonstrates

- **Chaotic-stub soundness.** The peripheral models every register
  access as always-available, with no constraint on ordering or
  interleaving. Asynchronous composition over-approximates the real
  system; safety verdicts that hold against the chaotic stub also
  hold against any well-behaved silicon. This is the soundest model
  when the silicon's internal protocol is unspecified or untrusted
  (Doc C §C.5).
- **`c-codesign` adapter** end-to-end, like A2.7b — clang shells out
  to produce LLVM IR, the codesign extractor matches register
  accesses to the register-map sidecar, and the resulting per-function
  synthesised automata feed the verify assembler.
- **`[alphabet] strategy = "register_map"`.** The binding type
  is declared as the canonical home for register-map-derived
  renamings; today both sides already speak the rendezvous alphabet
  (firmware via the c-codesign adapter, peripheral by author
  convention) so the binding projection is a no-op. The orchestrator
  wires this strategy through cleanly; SV-side label rewriting fires
  when the peripheral is sourced from `sv-yosys` (the register-map
  rewriter gate, `is_sv_adapter`).
- **`allow_peripheral_superset = true`.** The peripheral exposes all
  register-map labels (chaotic stub admits everything); the firmware
  exercises only a subset. Without the flag this would be a
  reconcile-gate failure.

## Files

| File | Purpose |
|---|---|
| `verify.toml` | Project config with two sources + composition + properties |
| `peripheral_chaotic_stub.ctxdsl` | Hand-authored chaotic-stub peripheral (1 state, self-loops on every register-map label) |
| `validate.sh` | Reproduction script (requires clang) |
| `transcript.txt` | Checked-in expected output |
| `README.md` | This file |

Firmware C + register-map sidecar are reused from the canonical
codesign-uart example (`../../industrial/codesign_uart/`) to avoid
fixture duplication.

## Reproduce

Requires `clang` on `$PATH`:

```bash
bash examples/verify/uart_codesign_chaotic/validate.sh
```

## Run manually

```bash
mununu verify examples/verify/uart_codesign_chaotic/verify.toml
mununu verify examples/verify/uart_codesign_chaotic/verify.toml --json
```

Successful run produces:

```
composition: asynchronous UART_LITESystem { members = [Uart_send_byte, UART_LITE] }
firmware_reaches_sending: VIOLATED (2/5 states, 0/1 initial)
safety_holds_under_chaotic_stub: SATISFIED (5/5 states, 1/1 initial)
```

## Relationship to A2.7b

| Aspect | `uart_codesign_protocol_spec` (A2.7b) | `uart_codesign_chaotic` (A2.7d) |
|---|---|---|
| Peripheral source | Hand-authored 3-state protocol spec | Hand-authored 1-state chaotic stub |
| Peripheral behaviour | Constrained ordering (`Idle → Loaded → Transmitting → Idle`) | All transitions always enabled |
| Soundness | Tight (protocol-spec captures silicon's intended behaviour) | Loose / over-approximate (chaotic stub admits more behaviours than real silicon) |
| When to use | Late-stage verification with a vendor-authored protocol contract | Early-stage / unknown silicon; safety properties transfer to any implementation |
| Verdict transferability | Holds against the modelled protocol | Holds against any silicon that conforms to the register-map interface |

Both examples reuse the same firmware C source and register map —
the difference is purely in the peripheral model and the
verification posture.

## Future extension

When the verify framework wires SV-side label rewriting through the
`AlphabetBinding::RegisterMap` strategy (the orchestrator-level
post-process rewriter step), this example will be the canonical
testbed for swapping the chaotic-stub peripheral for a real
SystemVerilog model:

```toml
[[sources]]
id = "peripheral"
adapter = "sv-yosys"
files = ["uart_lite.sv"]   # real RTL
```

The register-map binding would then derive `sv_port → rendezvous_label`
renamings automatically.
