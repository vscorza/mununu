# `v4_mesi_parametric` — V.4 parametric MESI cache coherence

> **Status: PASS.** Parametric N-agent MESI coherence demo (N = 2, 4, 8),
> generated from a single template. Coherence safety + deadlock-freedom +
> eventual write-visibility hold at every N. §Phase 7 domain-validation
> track; controlling doc
> [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md)
> §3 (parameterised cache coherence). Extends [`v0_mesi_2agent`](../v0_mesi_2agent/)
> (the 2-agent base) to N agents + a shared memory + a liveness property.

> **Claims integrity.** This is a **generated design-pattern demonstration**
> of the cache-coherence verification domain — a hand-authored protocol
> abstraction, **not** a finding about any real silicon coherence
> controller. The properties hold *of the model*; transferring them to a
> real implementation would require extracting that implementation, not
> this template.

## What it demonstrates

mununu verifies the three load-bearing properties of a parameterised
coherence protocol at N = 2, 4, and 8 caches, from **one** template:

| Property | Formula | Why it holds |
|---|---|---|
| `coherence_safety` | `nu X. (∧_{i<j} !(Ci_M && Cj_M) && [] X)` | A local write atomically takes the writer to `M` **and** invalidates every other cache (async rendezvous on the shared write label), so at most one cache is ever in `M` |
| `deadlock_freedom` | `nu X. (<> true && [] X)` (`no_deadlock`) | every reachable composed state has an enabled bus event |
| `write_visibility` | `nu Y. ((mu X. (Mem_Written \|\| <> X)) && [] Y)` (`always_eventually`) | a write is always *eventually* visible in memory — the νμ liveness shape |

The safety formula scales as `C(N,2)` pairwise terms (1 / 6 / 28 at N =
2 / 4 / 8); the generator emits it so the fixture stays correct as N grows.

## How it's parametric

[`generate.py`](generate.py) is the template. Given `N`, it emits `N`
per-cache `cache_<k>.ctxdsl` automata (each owning its local read/write and
snooping every other core), one `memory.ctxdsl`, and a `verify.toml` with
the N-way safety formula. The checked-in `n2/`, `n4/`, `n8/` directories are
its instantiations; [`validate.sh`](validate.sh) re-runs the generator and
diffs against them (determinism) before verifying.

```
v4_mesi_parametric/
├── generate.py        # the parametric template (N → fixture)
├── validate.sh        # regenerate + diff + verify N=2,4,8
├── n2/ n4/ n8/        # generated instantiations (cache_*.ctxdsl, memory.ctxdsl, verify.toml)
└── README.md
```

## Semantics note (vs the plan's "CEGAR converge" criterion)

The §Phase 7 V.4 done-criterion mentions "CEGAR must converge to definite
verdicts in ≤ 8 iterations per N." That criterion was written for a
predicate-abstraction path; this fixture is a **CTXDSL protocol model**
evaluated by **explicit asynchronous composition** (Sharp-everywhere
KMTS), which returns **definite 2-valued verdicts directly** — there is no
`KleeneBot` and no CEGAR loop to converge. Direct definite verdicts are
strictly stronger than "CEGAR converges," so the done-criterion's intent
(definite verdicts at N = 2, 4, 8) is met. Wall-clock: N=8 (4^8 × 2 ≈ 131K
composed states) verifies in a few seconds.

## Reproduce

```bash
cargo build -p mununu-cli
LIBRARY_PATH=/usr/local/opt/z3/lib bash examples/verify/v4_mesi_parametric/validate.sh
```

Expected: `coherence_safety`, `deadlock_freedom`, `write_visibility` all
`true` at N = 2, 4, 8 → `V.4 VALIDATION PASSED`.

## See also

- [`v0_mesi_2agent`](../v0_mesi_2agent/) — the 2-agent MESI base V.4 extends
- [`rv5_2core_mesi_microcode_extracted`](../rv5_2core_mesi_microcode_extracted/) — MESI + a microprogram coordinator + memory
- [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md) §3 — the domain framing
