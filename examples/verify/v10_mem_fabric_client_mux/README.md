# `v10_mem_fabric_client_mux` — registering the fabric's client mux

> **Status: PASS.** Four issue-path designs for a fixed-slot memory fabric's
> client mux, decided by `reachable(DUP)` / `reachable(SKIP)` plus a
> back-to-back-grant control. Reproduces the failure of the registered-mux
> attempt, decides the two proposed fixes, and measures what each costs.

> **Claims integrity.** This is a **design-pattern demonstration** of the
> issue-path question, cross-checked against monono's own measured hardware
> evidence (`mem_board` 10/12: 5,125 returns for 5,120 words and 4,857 data
> errors; card B-21's 0.949 accepted beats per granted read slot). It is a
> model of the issue path, **not** a measurement of monono's RTL, and the
> throughput figures below are model cycles, not a replacement for card
> B-13's measured entitlement.

## The question

A fixed 64-cycle slot schedule serves five client ports from one SDRAM, and
it **backfills**: a slot whose owner is not asking is offered to whoever is.
So a client can be granted in **consecutive cycles** — its own slot, then the
next slot backfilled to it.

The fabric selects the granted client's address with a combinational 5:1 mux
that sits between the clients' flops and `sdram_burst`'s row/column decode.
It costs 22% of the memory clock (75.4 MHz before the fabric, 58.6 MHz
through it, against an 87.5 MHz requirement). Registering it is the obvious
fix, and it failed.

| | Variant | Verdict |
|---|---|---|
| **V1** | combinational mux (today) | sound, 22% too slow |
| **V2** | registered mux, no other change | **duplicate issue reachable** |
| **V3** | registered mux + the client advances on `c_gnt` (presents its next address before the current one is accepted) | **skipped word reachable** |
| **V4** | registered mux + the fabric holds the request until accepted, with backpressure | **sound — and half the throughput** |

## Results

```
./examples/verify/v10_mem_fabric_client_mux/validate.sh
```

| | `reachable(DUP)` | `reachable(SKIP)` | `AG EF DONE` | steady-state cycles/word |
|---|---|---|---|---|
| **V1** combinational | false | false | true | **1.00** |
| **V2** registered | **TRUE** | false | false | 2.00 |
| **V3** advance-on-grant | false | **TRUE** | false | 1.00 |
| **V4** hold + backpressure | false | false | true | **2.00** |

Read `AG EF DONE` ("the stream can always still complete") carefully: it is
`false` for V2 and V3 **because `DUP`/`SKIP` are absorbing**, i.e. the
corruption is unrecoverable. It is not a separate starvation finding. For V1
and V4 the `true` is the real result: a refusing `sdram_burst` only ever
**delays** the client, it never wedges it, so **no sound variant can starve a
client**. (Under branching-time `AG EF`, which is the sound liveness question
for an over-approximating model with no fairness constraint on the memory. A
linear-time "the client eventually gets served" would need a fairness
assumption on `sdram_burst` that the hardware does not offer.)

### The contrast pair, and why it is honest

V1 answering `false` where V2 answers `true` is what makes the other verdicts
mean anything. Three things keep that pair from being an artifact:

1. **`DUP`, `SKIP` and `DONE` are declared in every variant**, reachable or
   not, so all four models are asked the identical question. (Necessary, not
   cosmetic: a `reachable(DUP)` whose target state does not exist is a
   *realization error*, not a `false` verdict — the sound variant would
   refuse to load instead of answering.)
2. **A mandatory non-vacuity gate.** `clean_completion_reachable(DONE)` is
   `true` in all four, so no model is answering `false` because it is stuck.
3. **A back-to-back-grant control.** `validate.sh` regenerates every variant
   with `--strict-schedule` — the arbiter may not grant while a request is in
   flight — and asserts that **V2's duplicate disappears**:

   | | backfill | strict schedule |
   |---|---|---|
   | V2 `reachable(DUP)` | **true** | **false** |

   So the duplicate is caused by the coincident `grant`+`accept` edge, not by
   the register and not by the model. A model that reported the duplicate
   under both schedules would not be modelling this bug — and a scheduler
   that granted once every 64 cycles could not have found it at all.

   V3's skip *survives* the strict schedule, because its cause is different:
   a **refused** grant, not simultaneity. The model separates the two.

## V2's failure, in three edges

Straight out of [`v2/fabric_v2.ctxdsl`](v2/fabric_v2.ctxdsl) — state names are
`S_<loc>_p<ptr>_r<reg>_d<dlv>`, where `dlv` is the address `sdram_burst`
expects next:

```
S_IDLE_p0_rx_d0  --grant-->          S_BUSY_p0_r0_d0
      the grant latches c_addr = ptr = 0

S_BUSY_p0_r0_d0  --accept, grant-->  S_BUSY_p1_r0_d1
      ONE EDGE, three things in it:
        · sdram_burst accepts address 0     → dlv 0 → 1
        · the client advances on c_acc      → ptr 0 → 1
        · backfill grants the next slot too → reg latches the PRE-state
                                              ptr, which is still 0
      All three are non-blocking, so reg stays 0 while ptr becomes 1.

S_BUSY_p1_r0_d1  --accept-->         DUP
      address 0 is handed to the memory a second time.
```

That is the 5,125-returns-for-5,120-words symptom: one extra beat per client,
five clients.

## What each sound variant demands of a client

This is the actual deliverable — V3 and V4 are not fabric-local changes.

### V4 (sound) — the obligation is one every client already meets

1. Hold `c_addr` / `c_we` / `c_wdata` / `c_dqm` stable from asserting `c_req`
   until `c_acc`. **Already the contract.**
2. Advance the pointer on `c_acc`, never on `c_gnt`. **Already the contract**,
   and already a landed fix (card B-21).
3. **New:** `c_gnt` stops being an event a client may use *at all*. Under
   backpressure the fabric absorbs a grant that arrives while a request is
   held, so a client that counts grants, or sequences anything off `c_gnt`,
   breaks. Everything must key off `c_acc`.

Obligation (3) is a grep, not a redesign: does any of the five clients,
`sprite_fetch`, `wb_mem_client` or the loader use `c_gnt` for anything other
than a don't-care? Because the pointer-advance fix already moved everyone to
`c_acc`, this is very likely already satisfied — which is what makes V4 a fix
rather than a wish.

### V3 (unsound) — do not adopt

The obligation V3 would impose is that a client's run-ahead is **never
wasted**, i.e. that a granted request is always accepted. `sdram_burst` does
not offer that (card B-21: one grant in twenty moves nothing), so the
obligation is unmeetable and the model shows the consequence directly: the
refused address is dropped while the client has already moved past it, and
the next accept delivers a word out of order.

```
S_IDLE_p0_rx_d0  --grant-->             (reg = 0, ptr advances to 1)
   ...            --refuse, nogrant-->  (the request is dropped; ptr is
                                         already 1, address 0 never issued)
   ...            --grant-->            (reg = 1)
   ...            --accept-->           SKIP
```

V3 is the reverted bug — advancing on the grant — with a register in front of
it. Its 1.00 cycles/word is real, but it is bought by assuming a refusal that
happens ~5% of the time never happens.

## The cost, and the decision this exposes

The throughput column is the part that should change the plan.

| | steady-state cycles/word |
|---|---|
| V1 combinational | 1.00 |
| V2 registered | 2.00 *(on its correct paths)* |
| V3 advance-on-grant | 1.00 |
| V4 hold + backpressure | **2.00** |

These are **model cycles**, measured as the marginal cost of the shortest path
to `DONE` under a maximally cooperative environment (the arbiter always willing
to grant, `sdram_burst` always willing to accept), comparing a 4-word and a
16-word stream so the pipeline fill is separated from the steady state. They
are a throughput *proxy* the model can answer honestly — not a substitute for
card B-13's measured entitlement. Reproduce:

```python
# from examples/verify/v10_mem_fabric_client_mux/
import importlib.util; from collections import deque
spec = importlib.util.spec_from_file_location("g", "generate.py")
g = importlib.util.module_from_spec(spec); spec.loader.exec_module(g)

def best(v, n):
    g.N = n; init = (g.IDLE, 0, None, 0, None)
    dist = {init: 0}; q = deque([init])
    while q:
        s = q.popleft()
        if g.state_name(s) == "DONE": return dist[s]
        for _, t in g.succ(s, v):
            if t not in dist: dist[t] = dist[s] + 1; q.append(t)

for v in ("v1", "v2", "v3", "v4"):
    print(v, (best(v, 15) - best(v, 3)) / 12.0)
```

**Registering the mux halves this client's issue rate**, and V2's apparent
full rate was duplicates: on the paths where V2 is *correct* it is exactly as
slow as V4. The 2.00 is structural — capture on one edge, present-and-accept
on the next, and the coincident grant absorbed — so V4 buys 87.5 MHz by
giving up the back-to-back slot that backfill exists to provide. Against
*"25% is a floor, not a cap"* that is a real regression, and card B-13's
entitlement numbers would have to be re-measured.

The variant that would recover it is **V4 + a skid**: hold until accepted as
in V4, but let a grant land on the retiring edge by capturing the **post**-accept
address. The fabric cannot get that from `c_addr` (a flop output, still the
pre-accept value at that edge), so it needs either a `c_addr_next` port from
the client or a replicated `+stride` in the fabric. That is a materially
heavier client contract than V4's — a burst reader can expose `c_addr_next`
trivially, `wb_mem_client` may have no "next address" at all — which is
exactly why it should be decided by the model before it is RTL. It is one
branch in [`generate.py`](generate.py)'s `succ()`; the properties and the
control are unchanged.

## How it is modelled

[`generate.py`](generate.py) BFS-enumerates an explicit-state operational
model of **one** client port and emits CTXDSL + a `verify.toml` per variant.
One client suffices: the failure is per-client (five extra returns, one each),
and the arbiter appears only as a free `grant`/`nogrant` choice on every edge.

State = `(loc, ptr, reg, dlv, err)`; events are `grant` / `nogrant`,
`accept` / `refuse` / `grant_absorbed`, emitted as **multi-label transitions**
so the events that coincide in one clock edge are one transition:
`on label accept, label grant`.

`dlv` — the address `sdram_burst` expects next — is the oracle, and it is the
*definition* of correct delivery rather than a hypothesis about the
mechanism: the memory must be handed address 0, then 1, then 2, exactly once
each. On an accept of address `a`: `a == dlv` is progress, `a < dlv` is `DUP`,
`a > dlv` is `SKIP`.

**Scope, stated plainly.** Four-word stream (addresses 0..3) — enough for a
duplicate, which needs two words, and for a skip, which needs a dropped word
and its successor. Decoupled returns (`r_valid`, `rid`) are **not** modelled:
this question is about the issue path, and tying a return to its issue would
model a different memory. Write data follows the address through the same
register and is not modelled separately.

### Why explicit states, and not CTXDSL `variables` + `guards`

**CTXDSL transition guards accept exactly one comparison.** `&&`, `||` and
`!` inside a `guard` are parsed lossily and **silently disable the
transition** — no error, no warning. Every enabling condition in this model is
conjunctive ("in `BUSY`, *and* `reg == dlv`, *and* the pointer is in range"),
so a hand-written variable+guard model would have quietly lost transitions and
under-approximated. It would have been **too kind**, and would have missed the
bug it exists to find.

Conjunction *is* available in mu-calculus **formulas** (`&&`, `||`, `!`,
`reg == ptr` between two variables, labelled modalities). So the working
discipline for this domain is: enumerate the operational semantics in the
generator, encode the configuration in the state name, and keep the
properties simple — the same shape as
[`v2_tso_storebuffer`](../v2_tso_storebuffer/) and
[`v1_noc_mesh_4router`](../v1_noc_mesh_4router/).

The guard fragment, the `variables` traps that surround it, and the cases
where `variables` **do** work are written up in
[`docs/ctxdsl-modelling-guide.md`](../../../docs/ctxdsl-modelling-guide.md).

## Files

```
v10_mem_fabric_client_mux/
├── generate.py     # operational-model generator (v1|v2|v3|v4 [--strict-schedule])
├── validate.sh     # regenerate + diff + assert the verdicts + the control
├── v1/  v2/  v3/  v4/
│   ├── fabric_<v>.ctxdsl
│   └── verify.toml
└── README.md
```

Regenerate with:

```bash
for v in v1 v2 v3 v4; do
  python3 examples/verify/v10_mem_fabric_client_mux/generate.py \
      "$v" examples/verify/v10_mem_fabric_client_mux
done
```
