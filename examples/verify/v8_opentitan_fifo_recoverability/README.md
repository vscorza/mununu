# `v8_opentitan_fifo_recoverability` — relational recoverability of a real OpenTitan FIFO

> **Status: PASS.** Asks whether OpenTitan's `prim_fifo_sync_cnt` can always drain
> back to **empty** — a recoverability (`AG EF`) question — but over a datapath
> **relation** (`wptr == rptr`), not a control-FSM state. The small FIFO gets a
> definite `Holds`; the wide FIFO lands on the honest ranking boundary. §Phase 8
> V-track recoverability showcase (the relational-target companion to
> [`../v7_csrng_recoverability`](../v7_csrng_recoverability), which targets a
> control state).

> Source of truth: [`verify_recoverability`](../../../crates/mununu-core/src/adapter/recoverability.rs) — surface: CLI (`mununu btor2 verify-recoverability --target 'wptr == rptr'`)

> **Claims integrity.** `prim_fifo_sync_cnt.sv` and `prim_count_pkg.sv` are real
> OpenTitan RTL (Apache-2.0), vendored + pinned under [`source/`](source/) at the
> commit in [`source/UPSTREAM_COMMIT.txt`](source/UPSTREAM_COMMIT.txt). This is a
> **demonstration of a property class on real silicon, not a vulnerability finding**:
> the ranking boundary it surfaces is intrinsic to the property (draining is
> unbounded progress), not a design defect.

## The question

A FIFO that must not wedge raises a branching-time question: from any fill level it
can reach, can it always get back to **empty**? That is recoverability, `AG EF empty`.
But "empty" is not a control-FSM state — in `prim_fifo_sync_cnt.sv` it is a **relation
over two datapath registers**:

```systemverilog
assign empty_o = wptr_wrap_cnt_q == rptr_wrap_cnt_q;   // line 63
```

So the recoverability target is **relational** (`REG == REG`), decided by mununu's
compound-good machinery rather than a `REG == VALUE` control atom:

```
recoverable        = mu X. (empty || <> X)            # EF empty
always_recoverable = nu Y. (recoverable && [] Y)      # AG EF empty
        with  empty = (wptr_wrap_cnt_q == rptr_wrap_cnt_q)
```

The `<>` (some-successor) inside the `[]` (all-successors) is the branching content a
linear formalism (LTL / SVA) cannot state.

## What the verdict depends on — the honest ranking boundary

| Setup | `always_recoverable` | Reading |
|---|---|---|
| **Small FIFO** (`Depth=16`, ≤ ~40 bits of pointer state) | **`Holds`** (exact engine) | from any fill level, empty is reachable (drain, or assert reset) |
| **Wide FIFO** (`Depth=2^21`, beyond the exact cap) | **`Unknown`** (cube path — a *sound* abstain) | draining needs the read pointer to progress up to the write pointer — a **ranking** the cube cannot capture with a bounded predicate set |

The contrast that makes this precise: an **invariant** relation (two registers kept
equal — `data == target`, both incrementing together) decides `Holds` at *any* width,
because the relation holds throughout (the must-edge is one exact step). The FIFO's
`empty` is **not** invariant — the two wrap counters advance **independently** (writes
vs. reads), so `wptr == rptr` is *achieved* by draining, a well-founded descent. That
descent is the **ranking class**, mununu's honest ⊥ boundary — `Unknown` is sound
(never a false `Holds` or `Violated`).

This is the value: mununu can *state and soundly decide* a relational branching-time
property on real RTL where it can, and *soundly abstain* where the property needs a
ranking argument — rather than silently over-claiming.

## Run it

```bash
LIBRARY_PATH=/usr/local/opt/z3/lib cargo build -p mununu-cli
./examples/verify/v8_opentitan_fifo_recoverability/validate.sh
```

Requires `sv2v`, `yosys`, and `python3` on `PATH` (the standard plain-SV → BTOR2 flow,
same as `v7`). The two wrap-pointer counters are the only state; `validate.sh` names
the two anonymous state lines `cnta` / `cntb` (the `empty` relation is symmetric) and
targets `cnta == cntb`.
