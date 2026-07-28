# Recoverability — the branching-time differentiator

> Concept: what a *recoverability* property is and why a safety-only tool cannot express it.

`compute_engine.sv` is a small, self-contained `load → busy → done` compute core. It exists to
demonstrate — reproducibly, on RTL this repository owns — the class of property that separates
mununu's 3-valued KMTS μ-calculus engine from a bit-level safety checker (SVA assertions, plain
BMC/k-induction).

## The property

> Source of truth: [`compute_engine.sv`](compute_engine.sv) — surface: CLI (`sv verify-auto`) + API + UI

The core carries three `@mununu_guarantee` annotations (μ-calculus, verified through the same
pipeline as translated SVA):

| # | formula | meaning |
|---|---|---|
| 0 | `nu Y.((mu X.(busy==0 \|\| <> X)) && [] Y)` | **`AG EF(busy==0)`** — from *every* reachable state, the core can return to idle |
| 1 | `mu Z.(busy==1 \|\| <> Z)` | **`EF(busy==1)`** — *non-vacuity witness*: `busy` is genuinely reachable |
| 2 | `nu Y.((mu X.(done==1 \|\| <> X)) && [] Y)` | **`AG EF(done==1)`** — the core can *always* complete a computation |

Property 0 is the differentiator. A safety checker can only say *"`busy` is low on this trace"*;
only a branching-time engine says *"idle is **always re-reachable**, from anywhere"* — an `AG EF`
(a `μ` inside a `ν`) that collapses under plain over- or under-approximation. Property 1 is the
**non-vacuity gate**: without it, `AG EF(busy==0)` would hold trivially if `busy` were stuck at 0
(a signal that never leaves the target is "recoverable" for free). Because `busy` genuinely toggles
(0 on idle, 1 on `start`) and the FSM always returns to `IDLE`, the HOLDS is a *real* recoverability
claim.

## Run it

The properties are verified out of reset (the standard formal reset discipline — pin the reset to
its inactive value so recovery is proved via the design's own logic, not a reset escape):

```console
$ mununu sv verify-auto examples/recoverability/compute_engine.sv \
      --top compute_engine --engine exact-symbolic --config-value rst_n=1
```

Expected verdict (exact full-state model checking — a definite verdict transfers to the RTL):

```
  reset-gated (pinned inactive): rst_n=1
  [assert] ann_guarantee_0: HOLDS
  [assert] ann_guarantee_1: HOLDS
  [assert] ann_guarantee_2: HOLDS
  [coverage-summary] 3 assertion(s): 3 definite (HOLDS), 0 violated, 0 unknown, 0 skipped
```

The `busy`/`done` cone is the small FSM (`state`, `cnt`); the 32-bit `result` datapath is out of
cone, so the exact BDD engine decides all three in milliseconds.

## Why isolation is enough here

Unlike a *bus controller* (I2C/SPI), whose activity needs an external protocol partner and is
therefore unreachable in module isolation (its "recoverability" is vacuous when verified alone),
this core's `WORK` phase advances on its **own clock with no external handshake**. So the whole
`busy ↔ idle` cycle is reachable in isolation, and the recoverability is genuinely exercised — the
design-selection criterion for a *meaningful* self-contained recoverability check.

## Provenance

Authored in this repository (clean-room, under the repo's license) specifically as a public
reproducer. It is a minimal analog of the same property class mununu decides on real compute cores
(crypto round-FSMs, DSP pipelines) — those measurements live in the project's private benchmark
ledger; this example is the runnable demonstration of the capability.
