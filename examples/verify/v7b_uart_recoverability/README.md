# `v7b_uart_recoverability` — recoverability of a real OpenTitan FSM, with a compound idle (AG EF idle)

> **Status: PASS.** Asks whether OpenTitan's `uart_tx` can always drain a frame and
> get back to idle — a recoverability (branching-time `AG EF`) question — over a
> predicate abstraction of the real RTL, where **idle is a compound condition over
> two registers**. The correct design holds; a one-line planted bug makes idle
> unreachable. §Phase 7 V-track / Track-B recoverability showcase (the compound-idle
> companion to the state-enum [`v7_csrng_recoverability`](../v7_csrng_recoverability/)).

> **Claims integrity.** `uart_tx.sv` is real OpenTitan RTL (Apache-2.0), vendored and
> pinned under [`../m1_opentitan_uart_tx/source/`](../m1_opentitan_uart_tx/source/)
> (M.1); this fixture reuses it for the **correct** variant. The **bug** variant
> ([`source/uart_tx_stuck.sv`](source/uart_tx_stuck.sv)) is a deliberately broken
> copy with a **single planted line**. What follows is a **demonstration of a
> property class on real silicon, not a vulnerability finding** — OpenTitan's real
> UART is correct; the planted bug exists only to show the property distinguishing a
> recoverable design from a non-recoverable one.

## The question

A transmitter that has started sending a frame should always be able to finish and
return to idle. As a temporal property that is `AG EF idle` — "on all paths (`AG`),
idle remains reachable (`EF`)". mununu writes it as a fixpoint:

```
recoverable        = mu X. (idle || <> X)            # EF idle  — idle reachable from here
always_recoverable = nu Y. (recoverable && [] Y)     # AG EF idle — ...from every reachable state
```

What makes this fixture distinct from the csrng showcase is `idle`. csrng's idle is a
single state-enum value (`state_q == MainSmIdle`). uart_tx has **no idle state
register** — its `idle` output is *derived*, and the meaningful "fully returned to
idle" condition spans two registers:

```
idle = (bit_cnt_q == 0) && (sreg_q == 2047)   # frame counter drained AND shift register back to its idle pattern (11'h7ff)
```

That is a **compound predicate**, declared in [`source/idle.mununu.json`](source/idle.mununu.json).
Because the sampling representative can't realise a conjunction, the cube lift routes
compounds through the eager all-pairs SMT may-relation plus **SMT hyper-must edges**
(B.1 + B.2), so the alternating-fixpoint (νμ) verdict comes back **clean-sound** — no
soundness-tag.

## What the verdict depends on

uart_tx self-recovers (a correct frame always drains), so — unlike csrng — the
contrast is **bug-vs-fix**, not reset-dependence. We tie `tx_enable = 1` and
`rst_ni = 1` (actively transmitting, no external reset) so the verdict reflects the
design's *own* ability to drain a frame:

| Variant | `always_recoverable` | Reading |
|---|---|---|
| upstream `uart_tx` | **definite-TRUE** (`T=4, F=0`) | the bit counter decrements each baud tick, so every started frame drains back to idle |
| planted-bug `uart_tx_stuck` | **definite-FALSE** (`T=1, F=3`) | the one-line change removes the decrement (`bit_cnt_d = bit_cnt_q` instead of `bit_cnt_q - 4'h1`), so a started frame never drains and idle is unreachable mid-frame |

The planted line is the only difference between the two modules:

```systemverilog
// upstream (correct):              // planted bug (uart_tx_stuck.sv):
bit_cnt_d = bit_cnt_q - 4'h1;       bit_cnt_d = bit_cnt_q;   // never re-arms
```

Both verdicts are **definite** (no `KleeneBot`) and sound for the model — the
GKMTS hyper-must edges (`--must-edge-inference smt-hyper-must`) make the νμ verdict
monotone under refinement (Shoham–Grumberg LMCS 2007), so the Bruns–Godefroid
preservation result carries the definite abstract verdict back to the concrete design.

## Why this is worth showing

Two things SVA cannot do directly, in one fixpoint over an abstraction of the real
module:

1. **A branching property** — `AG EF idle` quantifies over the reachable state space
   ("from *every* state, idle is *still* reachable"), not over individual traces. SVA
   is linear-time, so this needs an auxiliary monitor FSM or a tool-specific deadlock
   check.
2. **A compound idle over multiple registers** — the recoverability target is
   `(bit_cnt_q == 0) && (sreg_q == 2047)`, decided soundly as one cube dimension via
   the SMT may/hyper-must path rather than reduced to a single signal.

## Reproduce

```bash
LIBRARY_PATH=/usr/local/opt/z3/lib cargo build -p mununu-cli
LIBRARY_PATH=/usr/local/opt/z3/lib bash examples/verify/v7b_uart_recoverability/validate.sh
```

Expected: `always_recoverable` HOLDS on upstream `uart_tx` and is VIOLATED on the
planted-bug variant, with no soundness-tag → `V.7-b VALIDATION PASSED`. Requires
`sv2v` and `yosys` on `PATH`.

## See also

- [`m1_opentitan_uart_tx`](../m1_opentitan_uart_tx/) — the source of the vendored RTL (M.1 milestone)
- [`v7_csrng_recoverability`](../v7_csrng_recoverability/) — the state-enum recoverability companion (reset-dependence contrast)
- [`sv_yosys_caliptra_rtl_150`](../sv_yosys_caliptra_rtl_150/) — M.4, the predicate-cube CEGAR safety verdict on Caliptra (the depth-1 companion to this branching property)
