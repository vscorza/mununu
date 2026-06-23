# `v3_specsafe_pipeline` — V.3 speculative non-interference

> **Status: PASS.** Self-composition of a speculative-load side channel reduces
> non-interference (a 2-safety hyperproperty) to ordinary safety on the product:
> `never(Leak)`. The vulnerable design leaks a secret-dependent cache footprint;
> the squashed-speculation design does not. §Phase 7 domain-validation track;
> controlling doc
> [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md)
> §6 (speculative-execution security / hyperproperties).

> **Claims integrity.** This is a **generated design-pattern demonstration** of
> the speculative-non-interference domain — an **abstract** model of the
> speculation→cache side channel, **not** an RTL pipeline and **not** a
> production Spectre checker (research-grade per the value-proposition doc §6).
> The pipeline, branch predictor, and cache are abstracted to the load-bearing
> behaviour: a speculative load whose cache footprint does (vulnerable) or does
> not (safe) depend on the secret. The properties hold *of the model*;
> transferring them to a real core would require extracting that core.

## The idea: non-interference by self-composition

Spectre-v1 essence: a victim speculatively executes an out-of-bounds load — the
branch is predicted taken, a **secret** byte is read under misspeculation, and
the secret-dependent address touches a cache line *before* the misprediction
resolves. The cache footprint is the attacker-**observable**.

Non-interference is a **2-safety hyperproperty**: two runs that agree on every
**public** input must agree on every observation, whatever the secret. A single
trace can't express it — so we **self-compose**: two copies (A, B) of the victim
run with the *same* public input (both speculate the same access) but
**independent secret values**, and a final `observe` step compares their cache
footprints. The hyperproperty then reduces to ordinary safety on the product:

```
noninterference  ≡  □(public_eq → obs_eq)  ≡  never(Leak)
```

where `Leak` is the product state in which the two footprints differ. This is
the standard 2-safety → safety reduction; the generator builds the product
explicitly (the public inputs are equal by construction, the secrets range
freely).

## What it demonstrates

Two designs, generated from one template:

- **`vulnerable`** — the speculative load touches `cache[secret]`, so each copy's
  footprint *is* its secret (∈ {0,1}). When secretA ≠ secretB the footprints
  differ ⇒ `Leak` reachable.
- **`safe`** — the speculative load is squashed before it can update the cache
  (the mitigation; speculation leaves no cache trace), so every footprint is
  `none` regardless of secret ⇒ only `Agree` reachable.

| Property | Formula | `vulnerable` | `safe` | Why |
|---|---|---|---|---|
| `noninterference` | `nu X. (!Leak && [] X)` (`never`) | **false** | **true** | the contract-conformance verdict: do the two copies' observable cache footprints ever disagree? |
| `leak_reachable` | `mu X. (Leak \|\| <> X)` (`reachable`) | true | false | non-vacuity witness (the dual of `noninterference`): the leak *is* observable |
| `deadlock_freedom` | `nu X. (<> true && [] X)` (`no_deadlock`) | true | true | every reachable product state has an enabled transition |

The discriminating verdict is **`noninterference`**. It doubles as
**contract conformance**: the hand-crafted speculation contract — *"a speculative
load leaves no secret-dependent cache trace"* — is exactly `never(Leak)`. The
`safe` design conforms; the `vulnerable` design violates it, and
`leak_reachable` exhibits the violating product state (secretA ≠ secretB ⇒
divergent footprint). `noninterference` and `leak_reachable` are duals, so a
passing safety verdict can't be vacuous.

## How it's parametric

[`generate.py`](generate.py) is the template. Given `vulnerable` or `safe` it
BFS-enumerates the self-composed product (two copies' joint cache-footprint
state + the `observe` step) and emits one `spec_<model>.ctxdsl` + a `verify.toml`.
The checked-in `vulnerable/` and `safe/` directories are its instantiations;
[`validate.sh`](validate.sh) re-runs the generator and diffs against them
(determinism) before verifying.

```
v3_specsafe_pipeline/
├── generate.py        # the template (vulnerable|safe → self-composed product)
├── validate.sh        # regenerate + diff + verify both designs
├── vulnerable/        # spec_vulnerable.ctxdsl, verify.toml
├── safe/              # spec_safe.ctxdsl, verify.toml
└── README.md
```

## Reproduce

```bash
cargo build -p mununu-cli
LIBRARY_PATH=/usr/local/opt/z3/lib bash examples/verify/v3_specsafe_pipeline/validate.sh
```

Expected: `deadlock_freedom` `true` in both; `noninterference` `false` /
`leak_reachable` `true` under `vulnerable`; `noninterference` `true` /
`leak_reachable` `false` under `safe` → `V.3 VALIDATION PASSED`.

## See also

- [`v2_tso_storebuffer`](../v2_tso_storebuffer/) — the memory-consistency litmus;
  same regenerate-diff-verify two-variant discipline
- [`v1_noc_mesh_4router`](../v1_noc_mesh_4router/) — NoC liveness, also two-variant
- [`docs/design/industrial-value-and-validation-domains.md`](../../../docs/design/industrial-value-and-validation-domains.md) §6 — the speculative-security domain framing
