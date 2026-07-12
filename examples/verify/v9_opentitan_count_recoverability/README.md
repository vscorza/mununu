# `v9_opentitan_count_recoverability` — down-counter recoverability of a real OpenTitan counter

> **Status: PASS.** Asks whether OpenTitan's `prim_count`, wired as a down-counter,
> can always get back to **zero** — a recoverability (`AG EF`) question over a datapath
> **descent**. It decides `Holds` on both a small counter (exact) and a 48-bit counter
> (via the **ranking certificate**), where exact BDD walls and the predicate cube
> abstains. A single-register ranking on a real hardened primitive — the companion
> shape to [`../v8_opentitan_fifo_recoverability`](../v8_opentitan_fifo_recoverability)'s
> relational FIFO drain. §Phase 9 V-track recoverability showcase.

> Source of truth: [`verify_recoverability`](../../../crates/mununu-core/src/adapter/recoverability.rs) — surface: CLI (`mununu btor2 verify-recoverability --target 'cnt == 0'`)

> **Claims integrity.** `prim_count.sv` and `prim_count_pkg.sv` are real OpenTitan RTL
> (Apache-2.0), pinned under [`source/`](source/) at the commit in
> [`source/UPSTREAM_COMMIT.txt`](source/UPSTREAM_COMMIT.txt). `prim_flop.sv` is a minimal
> register matching OpenTitan's abstract-prim flop interface (identical to
> `prim_generic_flop`); `count_top.sv` is a thin down-counter harness (the usage
> configuration — `step = 1`, no clear/set). A property-class demonstration on real
> silicon, not a vulnerability finding.

## The question

A counter that must not wedge raises a branching-time question: from any value it can
hold, can it always get back to a known state — here, **zero**? That is recoverability,
`AG EF (cnt == 0)`:

```
recoverable        = mu X. (cnt == 0 || <> X)         # EF cnt==0 — zero reachable from here
always_recoverable = nu Y. (recoverable && [] Y)      # AG EF cnt==0 — ...from every value
```

The `<>` inside the `[]` is the branching content a linear formalism (LTL / SVA) cannot
state.

## What decides it — the ranking certificate

`prim_count` wired as a down-counter reaches zero by a **well-founded descent** (each
decrement lowers the count, saturating at 0). No bounded predicate set captures a
2^W-step descent, so the predicate cube abstains and — beyond ~40 bits — the exact BDD
engine walls. mununu's **ranking certificate** decides it directly over the exact
transition: for the ranking δ = `cnt`, from every non-zero state *some* input (decrement
+ commit) strictly decreases δ; with δ bounded below by 0, that forces a descent to
zero — so `AG EF (cnt == 0)` **Holds**, in ~1.4 s at Width = 48.

| Setup | `always_recoverable` | How |
|---|---|---|
| **Small** (`Width=8`, ≤ ~40 bits) | **`Holds`** | exact 3-valued engine |
| **Wide** (`Width=48`, beyond the exact cap) | **`Holds`** | the ranking certificate (∃-input variant) |

## A real-hardware wrinkle (and why the certificate is robust to it)

`prim_count` is **hardened**: it keeps a redundant *secondary* counter (for fault
detection) and an FPV backdoor. Lifted to BTOR2, those leave wide signals that never
touch the primary count's evolution. The ranking certificate enumerates only inputs in
the **counted register's own next-state cone**, so the secondary/backdoor signals are
left free (sound — they cannot affect the ranking) and do not swamp the search. This is
the kind of messiness real RTL brings that a synthetic counter never would.

## Run it

```bash
LIBRARY_PATH=/usr/local/opt/z3/lib cargo build -p mununu-cli
./examples/verify/v9_opentitan_count_recoverability/validate.sh
```

Requires `sv2v`, `yosys`, and `python3` on `PATH` (the standard plain-SV → BTOR2 flow,
same as v7/v8). `validate.sh` names the primary counter state (the one in `cnt_o`'s cone)
`cnt` and targets `cnt == 0`.
