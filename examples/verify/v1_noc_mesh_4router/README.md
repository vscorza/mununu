# `v1_noc_mesh_4router` — V.1 4-router NoC liveness

> **Status: PASS.** A single flit traverses a 2×2 mesh NoC (4 routers) from
> corner R0 to the diagonally-opposite R3. Deadlock-freedom, end-to-end
> liveness, and bounded (≤ diameter) delivery hold; strong inevitability
> discriminates a fair scheduler from an unfair one. §Phase 7 domain-validation
> track; controlling doc
> [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md)
> §4 (NoC liveness).

> **Claims integrity.** This is a **generated design-pattern demonstration**
> of the NoC-liveness verification domain — a hand-authored routing
> abstraction, **not** a finding about any real interconnect. The single-flit
> abstract `wait` over-approximates contention from other flows; every
> liveness verdict below is reported under exactly that reading. The
> properties hold *of the model*; transferring them to a real NoC would
> require extracting that NoC, not this template.

## The mesh

```
   R0 ─── R1        A flit injected at R0, destined R3. Adaptive minimal
   │       │        routing: at R0 it may take either column
   R2 ─── R3        (R0→R1→R3 or R0→R2→R3); both are diameter-length
                    (2 hops). R3 is the absorbing delivered state.
```

A `hops` counter variable increments on each forward and is the **counter
valuation** the V.1 done-criterion calls for. Because the realize/unroll path
binds `hops == k` / `hops <= k` atoms (the 2026-06-23 CTXDSL variable-atom
binding fix), bounded delivery is expressed **directly over the counter**
rather than encoded into state names — the first V-track fixture to do so.

## What it demonstrates

Two scheduling disciplines, generated from one template:

- **`progress`** — every router forwards on the next step (a fair / always-
  making-progress scheduler). No router ever stalls.
- **`contention`** — every intermediate router *may* stall (a `wait_r*`
  self-loop), modelling an unfair scheduler where another flow holds the
  output port indefinitely. The flit can dawdle for unboundedly many time
  steps — but **a stall is not a hop**, so the hop count is unaffected.

| Property | Formula | `progress` | `contention` | Why |
|---|---|---|---|---|
| `deadlock_freedom` | `nu X. (<> true && [] X)` (`no_deadlock`) | true | true | every reachable state has an enabled transition — non-vacuous (drop R3's stutter and R3 deadlocks) |
| `liveness_possible` | `nu Y. ((mu X. (delivered \|\| <> X)) && [] Y)` (`always_eventually`) | true | true | **AG EF delivered** (νμ depth-2): delivery *remains reachable* from every reachable state — no deadlock/livelock trap |
| `liveness_inevitable` | `mu X. (delivered \|\| ([] X && <> true))` | **true** | **false** | **AF delivered**: on *every* path the flit is eventually delivered. Holds only under a fair scheduler — the unfair all-stall path never delivers |
| `hop_bound` | `nu X. (hops <= 2 && [] X)` | true | true | bounded delivery (safety over the counter): hops never exceed the mesh diameter (2) |
| `delivered_at_diameter` | `mu X. ((delivered && hops == 2) \|\| <> X)` | true | true | bounded delivery (reachability over the counter): a delivered state at exactly 2 hops is reachable |

The discriminating verdict is **`liveness_inevitable`**, and it is the honest
fairness boundary. Per CLAUDE.md §Soundness Guarantees, a liveness verdict on
an over-approximating model needs fairness; mununu imposes none on the stall.
So mununu reports the *possibility* of delivery (`liveness_possible`, true in
both) unconditionally, but reports *inevitable* delivery only for the model
that actually makes progress. The `hops` counter cleanly separates the two
axes: latency (time steps) is unbounded under contention, yet path length
(hops) stays bounded at the diameter — exactly the NoC distinction.

## How it's parametric

[`generate.py`](generate.py) is the template. Given `progress` or `contention`
it emits one `noc_<model>.ctxdsl` (the variants differ only by the presence of
the `wait_r*` self-loops) and a `verify.toml`. The checked-in `progress/` and
`contention/` directories are its instantiations; [`validate.sh`](validate.sh)
re-runs the generator and diffs against them (determinism) before verifying.

```
v1_noc_mesh_4router/
├── generate.py        # the template (progress|contention → fixture)
├── validate.sh        # regenerate + diff + verify both disciplines
├── progress/          # noc_progress.ctxdsl, verify.toml
├── contention/        # noc_contention.ctxdsl, verify.toml
└── README.md
```

## Reproduce

```bash
cargo build -p mununu-cli
LIBRARY_PATH=/usr/local/opt/z3/lib bash examples/verify/v1_noc_mesh_4router/validate.sh
```

Expected: `deadlock_freedom`, `liveness_possible`, `hop_bound`,
`delivered_at_diameter` all `true` in both disciplines; `liveness_inevitable`
`true` under `progress` and `false` under `contention` → `V.1 VALIDATION PASSED`.

## See also

- [`v2_tso_storebuffer`](../v2_tso_storebuffer/) — the other two-variant V-track
  litmus (TSO vs SC); same regenerate-diff-verify discipline
- [`v4_mesi_parametric`](../v4_mesi_parametric/) — parametric cache coherence
- [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md) §4 — the NoC-liveness domain framing
