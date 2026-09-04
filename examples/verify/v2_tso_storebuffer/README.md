# `v2_tso_storebuffer` — V.2 store-buffering litmus (TSO vs SC)

> **Status: PASS.** The classic store-buffering (SB) litmus test under two
> memory models. The outcome `r0 == 0 && r1 == 0` is **reachable under TSO**
> (store buffers) and **forbidden under SC** (or TSO + a fence) — mununu
> reproduces the canonical distinction. §Phase 7 domain-validation track;
> controlling doc
> [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md)
> §5 (memory consistency).

> **Claims integrity.** This is a **generated design-pattern demonstration**
> of the memory-consistency verification domain, cross-checked against the
> textbook SB litmus result (herd / rmem / any MCM reference). It is **not**
> a finding about a real processor's memory model.

## The litmus

```
            Thread 0            Thread 1
            x = 1               y = 1
            r0 = y              r1 = x
                  (initially x = y = 0)
```

The question is whether `r0 == 0 && r1 == 0` is observable:

| Memory model | `r0=r1=0` | Why |
|---|---|---|
| **SC** (sequential consistency) / **TSO + fence** | **forbidden** | a total order consistent with program order cannot put both reads before both writes |
| **TSO** (x86-TSO store buffers, no fence) | **allowed** | each thread's write sits in its private store buffer; each read sees the other location's stale `0` before the buffer drains |

mununu's discriminating verdict is `reachable(BothReadZero)`:

```
tso/ : both_read_zero_reachable = true    (the relaxation is observable)
sc/  : both_read_zero_reachable = false   (SC / the fence forbids it)
```

(Both models also reach `OtherOutcome`, the sanity check.)

## How it's modelled

[`generate.py`](generate.py) BFS-enumerates an **explicit-state operational
model** of the litmus for a chosen memory model and emits CTXDSL + a
`verify.toml`. State = `(mem_x, mem_y, sb0, sb1, pc0, pc1, r0, r1)`; events
are `w0 d0 r0 w1 d1 r1` (TSO; `d*` = store-buffer drain) or `w0 r0 w1 r1`
(SC; writes commit to memory immediately). When both threads finish, a
`commit` edge routes to the absorbing state `BothReadZero` or `OtherOutcome`
by the read results; `reachable(BothReadZero)` is the litmus check.

**Why explicit states (not CTXDSL variables).** The outcome is encoded in the
state name and checked as a dedicated state by the `reachable` template,
because **state-name atoms bind on every path while variable atoms do not**:
`r0 == 0` returns `false` across a multi-source `verify` composition, and a
`reachable` template's `TARGET` must be a bare identifier in any case, so an
`r0 == 0` target is rejected outright. (Re-verified 2026-09-04. Variable atoms
*do* bind on `context eval` and on a single-source `verify` project via the
raw `formula = "…"` field — the 2026-06-23 note was correct for the composition
path but read as though it covered all of them. Full matrix:
[`docs/ctxdsl-modelling-guide.md`](../../../docs/ctxdsl-modelling-guide.md) §9.)

```
v2_tso_storebuffer/
├── generate.py     # operational-model generator (tso | sc)
├── validate.sh     # regenerate + diff + assert the litmus verdict per model
├── tso/  sc/       # generated: sb_<model>.ctxdsl + verify.toml
└── README.md
```

## Reproduce

```bash
cargo build -p mununu-cli
LIBRARY_PATH=/usr/local/opt/z3/lib bash examples/verify/v2_tso_storebuffer/validate.sh
```

Expected: `tso` both-read-zero **true**, `sc` both-read-zero **false**, both
`other_outcome` true → `V.2 VALIDATION PASSED`.

## Scope

The headline is the **SB write→read relaxation** (TSO ≠ SC) and that a fence
restores SC — the load-bearing memory-consistency property. The broader V.2
plan items (multi-copy atomicity via self-composition, full per-location SC
across an instruction mix) are not modelled here; this fixture isolates the
canonical SB distinction. Litmus results are cross-checked against the
textbook MCM expectation, not a live `herd`/`rmem` run (those tools are not
installed in this environment).

## See also

- [`v0_mesi_2agent`](../v0_mesi_2agent/) / [`v4_mesi_parametric`](../v4_mesi_parametric/) — the coherence-domain V-track fixtures
- [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md) §5 — the memory-consistency domain framing
