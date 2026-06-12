# phase10_caliptra_mbox_blackbox — Caliptra mailbox external-SRAM black-box contract

> **Status: stage 5.1 — chaotic-stub baseline captured.** This fixture
> models the Caliptra `mbox.sv` mailbox's **external SRAM** as a
> black-box module and drives it through mununu's contract subsystem.
> Stage 5.1 (this commit) establishes the interface + the chaotic-stub
> baseline. Stages 5.2 (author read-after-write + address-stability
> clauses) and 5.3 (discover → review → validate + monotonicity
> cross-check) follow.

## What this is — and is NOT (per CLAUDE.md §Claims Integrity)

**This is** a *contract-authoring demonstration* on a real industrial
memory interface: the Caliptra mailbox payload buffer is an external
SRAM (the `mbox_sram_req_*` / `mbox_sram_resp_*` interface in
chipsalliance/caliptra-rtl's `src/soc_ifc/rtl/mbox.sv`), which Yosys
cuts at the module boundary into I/O ports rather than an internal
memory array. mununu models it as a **black-box module with an
authored memory contract** and validates the contract through the
`mununu contract` pipeline.

**This is NOT** a "mununu verified Caliptra's mailbox" claim. The
contract-clause authoring + discharge validation is what ships here.
Turning an authored contract into a *composable verifying automaton*
(and thus an actual read-after-write verdict) is **task A6**
(chaotic-CLTS synthesis from contracts), an explicit follow-up — not
this milestone. See
[`.claude/plans/measurements/Phase10-stage5-caliptra-mbox-blackbox-2026-06-12.md`](../../../../.claude/plans/measurements/Phase10-stage5-caliptra-mbox-blackbox-2026-06-12.md)
for the full scoping.

## Why the black-box path (not the array encoder)

The §Phase 10 stage-3.c array encoder (`btor2_encode.rs`, Z3 array
theory) handles **internal** inferred memory — a BTOR2 array-sorted
`state` cell. Caliptra mbox's SRAM is **external** (a separate IP block
behind the `mbox_sram_*` handshake), so there is no internal array
cell to lift. The black-box contract subsystem is the correct tool for
an external memory interface. The natural fixture for the *array
encoder* is ibex `ibex_register_file_ff` (internal `$mem`), addressed
separately by the uf-memory Option-4 track
([`docs/design/uf-memory-end-to-end.md`](../../../docs/design/uf-memory-end-to-end.md)).

## Files

| File | What it is |
|---|---|
| `mbox_sram.interface.json` | The `BlackBoxInterface` — 8 ports (clk + 5 request inputs + 2 response outputs) of the Caliptra mailbox external SRAM. |
| `baseline_discover.json` | Output of `mununu contract discover` BEFORE any contract clauses are authored. Shows the chaotic-stub default: inputs classified `Uncontrollable`, outputs `Controllable`, and an `output_sequencing` gap on the two response outputs. |

## The chaotic-stub baseline (stage 5.1)

```bash
mununu contract discover mbox_sram.interface.json --json   # → baseline_discover.json
```

With no authored clauses, phase-1 discovery emits one
`output_sequencing` gap covering `mbox_sram_resp_rdata` +
`mbox_sram_resp_ecc`. Per `gap.rs`, this gap's soundness contract is:
**safety verdicts hold; liveness verdicts depending on these labels
are unsound (no progress assumption).** The SRAM's read data is treated
as fully nondeterministic — a sound over-approximation, but it admits
the read-after-write violation (a read can return anything), so it
does not yet verify the memory's defining property.

The strict-contracts gate fails on this baseline (exit 1), proving the
gate fires when an output is left chaotic:

```bash
mununu contract discover mbox_sram.interface.json --strict-contracts ; echo $?   # → 1
```

## What stages 5.2 + 5.3 add

- **5.2** — author the **read-after-write guarantee**
  (`G( req_cs ∧ ¬req_we ∧ req_addr = A → resp_rdata = mem[A] )`) and
  the **address-stability assume** on the black box, plus a reusable
  `rtl_memory/sram_sync` corpus entry. Re-running `discover` should
  **downgrade** the gap from `output_sequencing` to `latency_bound`
  (the "user authored some sequencing behaviour" signal).
- **5.3** — run `discover → review → validate`; assert the discharge
  verdict is `Acyclic`; and show the **monotonicity cross-check**: the
  strict-contracts gate that *fails* on the baseline (above) *passes*
  once the clauses are authored — proving the contract actually
  constrains the verifier.

## Reproduce

```bash
cd examples/verify/phase10_caliptra_mbox_blackbox
mununu contract discover mbox_sram.interface.json --json
mununu contract discover mbox_sram.interface.json --strict-contracts ; echo "strict exit: $?"
```
