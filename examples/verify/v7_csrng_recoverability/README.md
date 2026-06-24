# `v7_csrng_recoverability` — recoverability of a real OpenTitan FSM (AG EF idle)

> **Status: PASS.** Asks whether OpenTitan's `csrng_main_sm` can always return to
> its `MainSmIdle` state — a recoverability (branching-time `AG EF`) question — over
> a predicate abstraction of the real RTL, and gets a definite answer that depends
> on whether reset is available. §Phase 7 V-track / Track-B recoverability showcase.

> **Claims integrity.** `csrng_main_sm.sv` is real OpenTitan RTL (Apache-2.0),
> vendored and pinned under [`../m2_opentitan_csrng_main_sm/source/`](../m2_opentitan_csrng_main_sm/source/)
> (this fixture reuses that source rather than re-vendoring it). What follows is a
> **demonstration of a property class on real silicon, not a vulnerability finding**:
> the reset-dependence it surfaces is the design's *intended* recovery behaviour
> (the SEC_CM sparse-FSM-plus-alert pattern relies on reset to leave the error
> state). The value here is being able to *state and soundly decide* that branching
> property at all.

## The question

A control FSM that must never wedge raises a natural question: from any state it
can reach, can it still get back to a known-good idle state? That is recoverability,
and as a temporal property it is `AG EF idle` — "on all paths (`AG`), idle remains
reachable (`EF`)". mununu writes it as a fixpoint:

```
recoverable        = mu X. (idle || <> X)            # EF idle  — idle reachable from here
always_recoverable = nu Y. (recoverable && [] Y)     # AG EF idle — ...from every reachable state
```

with `idle = (state_q == MainSmIdle)` and `err = (state_q == MainSmError)`
(`MainSmIdle = 6'b110111 = 55`, `MainSmError = 6'b101001 = 41`). It is evaluated
over a predicate-cube abstraction of the BTOR2 the standard sv2v + Yosys flow
produces, through the CEGAR path with SMT-proved (hyper-)must edges
(`--must-edge-inference smt-hyper-must`).

## What the verdict depends on

The answer turns on whether reset is in play, and that turns out to be the whole
point:

| Setup | `always_recoverable` | Reading |
|---|---|---|
| reset available (`rst_ni` free) | **definite-TRUE** (`T=4`) | asserting reset returns the FSM to `MainSmIdle` from any state, so idle is always reachable again |
| reset held inactive (`rst_ni` tied `1'b1`) | **definite-FALSE** (`T=0, F=4`) | once reset is withheld, `MainSmError` and the unreachable sparse encodings become permanent traps |

Both verdicts are **definite** (no `KleeneBot`) and sound for the model — they
sit inside the audited `Control::All` / bare-modality / unbounded fragment, where
the Bruns–Godefroid preservation result carries a definite abstract verdict back
to the concrete design. Read together they say something precise and honest about
the RTL: csrng's ability to return to idle rests on reset, which is exactly what
its security-hardened state machine is built to do.

## Why this is worth showing

`AG EF idle` is a branching property: it quantifies over the reachable state space
("from *every* state, idle is *still* reachable"), not over individual execution
traces. SystemVerilog Assertions are linear-time, so this question cannot be
written directly in SVA — the usual route is an auxiliary monitor FSM or a
tool-specific deadlock check. Here it is one fixpoint, decided over an abstraction
of the real module, with the three-valued machinery reporting `⊥` ("not enough
abstraction to decide") rather than guessing when it cannot.

## Reproduce

```bash
LIBRARY_PATH=/usr/local/opt/z3/lib cargo build -p mununu-cli
LIBRARY_PATH=/usr/local/opt/z3/lib bash examples/verify/v7_csrng_recoverability/validate.sh
```

Expected: `always_recoverable` HOLDS with reset available and is VIOLATED with
reset held → `V.7-c VALIDATION PASSED`. Requires `sv2v` and `yosys` on `PATH`.

## See also

- [`m2_opentitan_csrng_main_sm`](../m2_opentitan_csrng_main_sm/) — the source of the vendored RTL (M.2 milestone)
- [`sv_yosys_caliptra_rtl_150`](../sv_yosys_caliptra_rtl_150/) — M.4, the predicate-cube CEGAR safety verdict on Caliptra (the depth-1 companion to this branching property)
- [`v1_noc_mesh_4router`](../v1_noc_mesh_4router/) — the same νμ liveness shape on an exact CTXDSL model
