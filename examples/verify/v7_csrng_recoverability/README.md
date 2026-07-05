# `v7_csrng_recoverability` — recoverability of a real OpenTitan FSM (AG EF idle)

> **Status: PASS.** Asks whether OpenTitan's `csrng_main_sm` can return to its
> `MainSmIdle` state — a recoverability (branching-time `AG EF`) question — and
> gets a definite, sound answer from the exact-symbolic engine. §Phase 7 V-track /
> Track-B recoverability showcase.

> **Claims integrity.** `csrng_main_sm.sv` is real OpenTitan RTL (Apache-2.0),
> vendored and pinned under [`../m2_opentitan_csrng_main_sm/source/`](../m2_opentitan_csrng_main_sm/source/).
> What follows is a **demonstration of a property class on real silicon, not a
> vulnerability finding**: the reset-dependence it surfaces is the design's
> *intended* behaviour — the SEC_CM sparse-FSM-plus-alert pattern relies on reset
> to leave the error state. The value here is being able to *state and soundly
> decide* that branching property at all.

## The question

A control FSM that must never wedge raises a natural question: from any state it
can reach, can it still get back to a known-good idle state? That is recoverability,
and as a temporal property it is `AG EF idle` — "on all paths (`AG`), idle remains
reachable (`EF`)". mununu writes it as a fixpoint:

```
recoverable        = mu X. (idle || <> X)            # EF idle  — idle reachable from here
always_recoverable = nu Y. (recoverable && [] Y)     # AG EF idle — ...from every reachable state
```

with `idle = (state_q == MainSmIdle)` (`MainSmIdle = 6'b110111 = 55`,
`MainSmError = 6'b101001 = 41`). It is decided by the **exact-symbolic engine**
over the full bit-blasted state of the standard sv2v + Yosys lift — no abstraction,
so a **definite** two-valued verdict, never `⊥`.

## What the verdict says

Recovery depends on whether reset is in play, and that is the whole point:

| Setup | `AG EF idle` | Reading |
|---|---|---|
| normal operation (init = `MainSmIdle`, verified out of reset) | **HOLDS** | the running FSM always returns to idle — it never wedges in normal operation |
| fault premise (FSM in `MainSmError`, reset withheld) | **VIOLATED** | from the hardened error state, idle is unreachable without reset — the error state is a permanent trap |

Both verdicts are **definite** (exact, no abstraction) and sound. Read together
they say something precise and honest about the RTL: csrng recovers on its own in
normal operation, but its security-hardened error state can only be left through
reset (the flop's `state_q_next = rst_ni ? state_d : MainSmIdle` mux). Recovery from
a fault therefore depends on reset — exactly what the SEC_CM design intends.

The fault premise forces the FSM's reset value to `MainSmError` to *model* a fault
that has driven the FSM into its error state; the error self-loop (the behaviour
under test) is unchanged, so the VIOLATED verdict reflects the real design's
error-trap behaviour.

## A soundness note (why exact, not predicate-cube)

An earlier version of this example evaluated the property over the predicate-cube
CEGAR path (`btor2 cegar`) with the default sampling-based may-edges (`may=off`).
Sampling under-approximates the may-relation (one representative per cube plus a
capped input set), which violates the KMTS `concrete ⊆ may` precondition and is
**unsound for this branching property** — it produced a spurious reset-dependent
"flip" (`T=0 F=244 ⊥=12`) that does not match the RTL. The exact-symbolic engine
has no such abstraction, so it decides `AG EF idle` definitely and soundly. For the
recoverability class, prefer the exact engine (or, on the predicate-cube path,
`--may-edge-inference smt-all-pairs`, with the soundness of the may-relation
verified for the property at hand).

## Why this is worth showing

`AG EF idle` is a branching property: it quantifies over the reachable state space
("from *every* state, idle is *still* reachable"), not over individual execution
traces. SystemVerilog Assertions are linear-time, so this question cannot be
written directly in SVA — the usual route is an auxiliary monitor FSM or a
tool-specific deadlock check. Here it is one fixpoint, decided exactly over the
real module.

## Reproduce

```bash
# In the mununu-sva image (slang + sv2v + yosys):
cargo build -p mununu-cli
MUNUNU=/path/to/mununu bash examples/verify/v7_csrng_recoverability/validate.sh
```

Expected: `AG EF idle` HOLDS in normal operation and is VIOLATED under the fault
premise → `V.7-c VALIDATION PASSED`. Requires `slang`, `sv2v`, and `yosys` on
`PATH` (the exact-symbolic engine consumes the SVA-extraction front-end).

## See also

- [`m2_opentitan_csrng_main_sm`](../m2_opentitan_csrng_main_sm/) — the source of the vendored RTL (M.2 milestone)
- [`sv_yosys_caliptra_rtl_150`](../sv_yosys_caliptra_rtl_150/) — M.4, the predicate-cube CEGAR safety verdict on Caliptra
- [`v1_noc_mesh_4router`](../v1_noc_mesh_4router/) — the same νμ liveness shape on an exact CTXDSL model
