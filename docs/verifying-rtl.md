# Verifying RTL with Mununu — the property verbs, no-sidecar SV, and agent integration

This page answers three practical questions:

1. **What properties can I check, and how?** — the three property *verbs* over a
   BTOR2 design, all speaking one verdict vocabulary.
2. **Can I verify a SystemVerilog module with no hand-written setup?** — yes,
   `sv verify-auto` extracts the design's own assertions and checks them, no sidecar.
3. **Can an external tool (e.g. an agent writing RTL) drive this over HTTP to check
   its generated designs?** — yes, with three honest caveats spelled out below.

Every verdict below is one of the canonical labels `holds` / `violated` / `unknown`
/ `skipped`.

> Source of truth: [`PropertyVerdict`](../crates/mununu-core/src/verdict.rs#L22) — surface: (CLI+API+UI)

`holds` and `violated` are **definite** (sound within the engine's cap / abstraction);
`unknown` is an honest abstention (over-cap, timeout, or an undecided abstraction);
`skipped` means the property was not evaluated (out of the supported fragment).

---

## The property verbs (BTOR2-direct)

Three commands decide the three property classes over a BTOR2 file. Each ships on
CLI (`mununu btor2 <verb>`), HTTP API (`POST /api/v1/btor2/<verb>`), and the UI
client. They take a BTOR2 design plus, where needed, an explicit atom naming the
signal/value of interest (`"state_q == 3"`).

### `btor2 verify` — safety / reachability

> Source of truth: [`decide_reach_portfolio_parallel`](../crates/mununu-core/src/adapter/reach_portfolio.rs#L202) — surface: (CLI+API+UI)

Decides `bad`-reachability with the multi-engine safety portfolio — the exact BDD
engine, the in-house native BMC + k-induction and SPACER (IC3/PDR) engines (all
in-process), plus the `btormc` and `Pono` subprocess members when present — merged
under a **differential-oracle** discipline: the first definite verdict wins, and two
sound engines disagreeing raises a `contradiction` soundness alarm rather than a
guess. `verdict` is the property reading (`bad` unreachable = `holds`, reachable =
`violated`); the per-engine `reachable_by` / `unreachable_by` breakdown and the
`contradiction` flag carry the detail.

```bash
mununu btor2 verify design.btor2
```

### `btor2 verify-liveness` — response liveness `AG(request → AF grant)`

> Source of truth: [`response_liveness_rescue_atoms`](../crates/mununu-core/src/adapter/liveness_rescue.rs#L239) — surface: (CLI+API+UI)

Decides the canonical request/grant property — "whenever `request` holds, `grant` is
eventually reached on **every** path" — by a sound liveness-to-safety reduction
(Biere–Artho–Schuppan): a `pending` latch, a nondeterministic snapshot of the state,
and loop-closure detection turn "a reachable cycle leaves a request forever
ungranted" into a single `bad`-reachability query the portfolio decides at scale.
Cross-checked against the exact engine.

```bash
mununu btor2 verify-liveness design.btor2 --request "st == 1" --grant "st == 2"
```

### `btor2 verify-recoverability` — recoverability `AG EF good`

> Source of truth: [`verify_recoverability`](../crates/mununu-core/src/adapter/recoverability.rs#L46) — surface: (CLI+API+UI)

Decides "from every reachable state, can the design still get **back** to a good
state?" — the CTL formula `AG EF good = ν Y. ((μ X. (good ∨ ◇X)) ∧ □Y)`. This is an
alternating fixpoint: the `◇` (some-successor) inside the `□` (all-successors) is
branching content that **SVA / LTL cannot express** (see
[`recoverability-vs-sva.md`](design/recoverability-vs-sva.md)). A `violated` verdict
means a reachable state is a trap from which `good` is unreachable. Decided by the
exact 3-valued engine (sound at every alternation depth, definite within its 40-bit
cone cap; `unknown` over the cap).

```bash
mununu btor2 verify-recoverability design.btor2 --target "state_q == 3"
```

For designs wider than the exact engine's cap, the same `AG EF` formula scales via
predicate abstraction on the `btor2 cegar … --must-edge-inference smt-hyper-must`
path (validated on real OpenTitan `csrng` RTL — see `recoverability-vs-sva.md` §3.2).

---

## No-sidecar SystemVerilog verification: `sv verify-auto`

> Source of truth: [`verify_auto`](../crates/mununu-core/src/adapter/slang/verify_auto.rs#L1537) — surface: (CLI+API+UI)

`sv verify-auto` takes a SystemVerilog module and checks its properties with **no
hand-written sidecar and no supplied formula**. End to end:

1. **slang** elaborates the source and extracts the design's own SVA `assert` /
   `assume` / `cover` properties, translating each to mu-calculus.
2. **sv2v → Yosys** lift the RTL to a BTOR2 model.
3. Cube predicates are **auto-seeded from each property's atoms** (no sidecar to
   author) and the CEGAR loop refines on an undecided verdict.
4. Each property gets a `holds` / `violated` / `unknown` / `skipped` verdict.

The HTTP request is just the source plus options — there is no `sidecar` or `formula`
field:

> Source of truth: [`sv_verify_auto_handler`](../crates/mununu-core/src/api/handlers.rs#L846) — surface: API

```bash
mununu sv verify-auto my_module.sv --preprocess-sv2v
```

```jsonc
// POST /api/v1/sv/verify-auto
{ "source": "module m(...); ... assert property (...); endmodule",
  "use_sv2v": true,
  "must_edge_inference": "smt-hyper-must" }   // sound νμ verdicts; all fields optional
// → { "properties": [ { "name": ..., "outcome": "holds" | "violated" | ... } ], "unsupported": [...] }
```

The response reports a verdict per translated assertion, plus a list of assertions
that did **not** translate (surfaced honestly, never silently dropped) and
model-level diagnostics.

---

## Driving it from an external agent (e.g. an agent writing RTL)

> Concept: integrating mununu's HTTP API into an automated design/verify loop.

An external tool — including an LLM agent generating RTL — can `POST` a generated
module to `/api/v1/sv/verify-auto` and get back per-property verdicts to gate or
repair its output. Start the server with:

```bash
mununu server --addr 127.0.0.1:8080   # built with --features api
```

Three caveats are load-bearing; plan for them.

### 1. The server host needs the SV toolchain

`sv verify-auto` shells out to **slang** (elaboration + SVA), **sv2v**
(normalisation), and **Yosys** (SV → BTOR2). These are **not bundled** — mununu
discovers each via a `locate_*` helper (a `MUNUNU_<TOOL>_PATH` env var, then `$PATH`)
and returns a **structured error** if one is missing, rather than crashing. Run the
API on a host that has them installed (the pinned `docker/Dockerfile.sva` image is the
supported environment). See [`external-tools.md`](external-tools.md).

### 2. Properties come from the module's own assertions

`sv verify-auto` checks the SVA the design *carries*; it takes no formula. An agent
that emits RTL **with** `assert property` statements gets those checked automatically.
An agent that emits assertion-free RTL gets zero properties back — it must either
embed SVA, or use the **explicit-property path**:

```bash
mununu sv emit-btor2-per-module my_module.sv   # SV → BTOR2 (one file per module)
mununu btor2 verify-recoverability my_module.btor2 --target "state_q == 3"
```

The BTOR2-direct verbs above let the agent name *any* property — including the
branching ones (recoverability) that SVA cannot state at all — against its design.

### 3. Coverage is a fragment, and verdicts are honest

The SVA translator supports a defined fragment (implication `|->` / `|=>`, `$past` /
`$stable` / `$rose` / `$fell`, `$onehot` / `$onehot0`, …); assertions outside it come
back in the `unsupported` list. Some designs hit the abstraction ceiling and return
`unknown`. Crucially, **a `holds` or `violated` verdict is sound** (Bruns–Godefroid
3-valued preservation for the abstraction; k-induction / IC3 / BDD for the exact and
portfolio engines) — an agent can trust a definite verdict and treat `unknown` /
`skipped` as "not decided here," not as "safe."

---

## See also

- [`design/recoverability-vs-sva.md`](design/recoverability-vs-sva.md) — why `AG EF`
  recoverability is outside SVA, with the OpenTitan `csrng` worked example.
- [`cli-cookbook.md`](cli-cookbook.md) — common `mununu` CLI invocations.
- [`external-tools.md`](external-tools.md) — installing slang / sv2v / Yosys and the
  discovery env vars.
- [`../wiki/Verify-Project-Flow.md`](../wiki/Verify-Project-Flow.md) — the N-source
  `verify.toml` framework for composing multiple sources.
