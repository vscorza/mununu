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

> Source of truth: [`decide_reach_portfolio_parallel`](../crates/mununu-core/src/adapter/reach_portfolio.rs#L260) — surface: (CLI+API+UI)

Decides `bad`-reachability with the multi-engine safety portfolio — the exact BDD
engine, the in-house native BMC + k-induction and SPACER (IC3/PDR) engines (all
in-process), plus the `btormc` and `Pono` subprocess members when present, and a
**last-resort** in-house interpolation engine (owned McMillan forward reachability;
[`native_interp`](../crates/mununu-core/src/adapter/btor2/native_interp.rs)) invoked
only when every other member abstains — merged
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

### Shrinking a parameterised design: `--param`

> Source of truth: [`build_chparam_passes`](../crates/mununu-core/src/adapter/yosys/mod.rs) — surface: (CLI+API+UI)

A design whose timing intervals are **parameters** sizes its counters by those
parameters — a 20 000-cycle power-up wait makes a 15-bit counter, and the exact
engine then abstains (`skip-diameter-bound`) even though the properties are about
the *order* of operations, not durations. `--param NAME=VALUE` (API `params: [...]`)
overrides a module parameter **before elaboration**, so the counters get smaller
without a wrapper module (a wrapper would rename the SVA atoms and break binding):

```bash
mununu sv verify-auto sdram_ctrl.sv --top sdram_ctrl --param INIT_WAIT=4
```

`NAME=VALUE` targets the top module; `MODULE.NAME=VALUE` a submodule. The
mechanism is frontend-appropriate — yosys `chparam` on the read_verilog frontend
(parameters stay unresolved until `hierarchy`), slang `-G` on the `--frontend
slang` path (slang elaborates at read time). A parameter that is **not** a real
parameter of the design is a hard **error** (it lists the declared parameters),
never a silent drop; the applied overrides are echoed as a `parameter-override`
scope note — the verdicts are scoped to those values.

`--param` is also the remedy for a **bit-blast size wall**: several wide counters
(e.g. six 32-bit counters, each doubled by its `$past` shadow) can push a
property's state cone past the BDD node budget, so the symbolic engine **abstains**
on that one property with a `skip-bitblast-oom` note (naming the cone's bit width)
rather than crashing the run — the other properties still report. Shrink a width
parameter (`--param BW_WIDTH=4`) so the counters fit, or fall back to
`--engine explicit`. This is a size threshold, not a malformed input.

> Source of truth: [`bitblast_oom_skip_note`](../crates/mununu-core/src/planner/mod.rs) — surface: (CLI+API+UI)

### Pinning a config input: `--config-value`

> Source of truth: [`partition_config_pins`](../crates/mununu-core/src/adapter/slang/verify_auto.rs) — surface: (CLI+API+UI)

`--config-value SIGNAL=VALUE` (API `config_values: [...]`) pins a primary input to
a decimal constant and substitutes it into every formula atom, scoping the verdicts
to that value (a `config-concretization` note lists the applied pins). A pin that
mununu **cannot apply** is a hard **error**, never a silent drop that would verify a
different question than the one asked: a non-decimal value (`rst_n=0x1` — decimal
`u64` only) is rejected at parse; a `SIGNAL` that is not a primary input of the
lifted model (misspelled, or a state/output/optimized-away name) is rejected against
the model's real inputs. Every applied pin — including one that coincides with an
auto-detected reset pin — is echoed in the `config-concretization` note.

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

**Net-type normalization (lift widening).** A net-type qualifier on a *port* header
(`input tri0 baud8x`) is rejected by both yosys's reader (a parse error) and slang's
lowering (`net type 'tri0' unsupported`) — the pull / wired-resolution of
`tri0`/`tri1`/`wand`/`wor`/… is a strength-resolution (simulation) concept a word-level
backend does not model. mununu strips such a qualifier on **port** declarations before
staging the source, so the design parses: a port net type only governs that port's
*external* resolution, so for isolated single-module verification an input is havoc'd and
an output is driven internally — dropping it is **sound** (exact for inputs). **Internal**
net-type declarations are left untouched (their pull / resolution semantics matter).

> Source of truth: [`strip_port_net_types`](../crates/mununu-core/src/adapter/yosys/mod.rs#L254) — surface: (CLI+API+UI)

**Wider front-end via yosys-slang (opt-in).** yosys's native `read_verilog` rejects some legal
SystemVerilog a full front-end accepts — e.g. a bounded `while` loop in an `always @*` block —
and black-boxes the module (all its properties then skip). Set `MUNUNU_YOSYS_FRONTEND=slang` (when
the yosys-slang `read_slang` plugin is present — `MUNUNU_YOSYS_SLANG_PLUGIN`, or the pinned image's
`share/yosys/plugins/slang.so`) to lift the RTL with `read_slang` (a complete SV front-end) instead.
It reads the RTL **only** (`--ignore-assertions` — SVA is extracted separately, so nothing is lost)
and names named registers/ports **identically** to `read_verilog`, so cutpoints / `--config-value` /
atoms resolve unchanged. *(Example: the `xgate` coprocessor, whose RISC core has a rejected `while`
loop, is black-boxed under `read_verilog` but lifts its full 125-register core under `read_slang`.)*

The default `--frontend auto` tries `read_verilog` (+sv2v) first and **falls back to `read_slang`**
on a lift error (`MUNUNU_YOSYS_FRONTEND=slang` flips the preference). The two paths are **not
interchangeable for soundness** — `read_slang` and `read_verilog` model a *partial* register write
(`q[hi:lo] <= d` / `q[idx] <= d`) differently (see the partial-write refusal / cone fix), so a silent
choice between them can be green about a different question than the one asked. Verify-auto therefore
**reports which front end produced the verdicts** as a `lift-frontend` note — Info when there was no
choice, and a **ScopeCaveat naming the fallback reason** when `auto` fell back. Note also that SVA in
a `bind`-ed file cannot be parsed by `read_verilog`, so `auto` **always** ends on slang there.

> Source of truth: [`slang_frontend_selection`](../crates/mununu-core/src/adapter/yosys/mod.rs#L1672) — surface: (CLI+API+UI)
> Source of truth: [`SvFrontend::lift_label`](../crates/mununu-core/src/adapter/yosys/mod.rs) + the `lift-frontend` note in [`build_notes`](../crates/mununu-core/src/adapter/slang/verify_auto.rs) — surface: (CLI+API+UI)

### 2. Properties come from the module's own assertions

`sv verify-auto` checks the SVA the design *carries*; it takes no formula. An agent
that emits RTL **with** `assert property` statements gets those checked automatically.
An agent that emits assertion-free RTL gets zero properties back — it names the
property it cares about with an **SV-direct verb**, which lifts the module *and*
decides the property in one call:

> Source of truth: [`sv_verify::sv_verify_recoverability`](../crates/mununu-core/src/adapter/sv_verify.rs#L84) — surface: (CLI+API+UI)

```bash
mununu sv verify-recoverability my_module.sv --target "state_q == 3"
mununu sv verify-liveness      my_module.sv --request "req == 1" --grant "grant == 1"
mununu sv verify               my_module.sv                       # safety of its assertions
```

Same over HTTP: `POST /api/v1/sv/verify` · `/sv/verify-liveness` ·
`/sv/verify-recoverability`. These let the agent name *any* property — including the
branching ones (recoverability) SVA cannot state at all — against raw SV, no BTOR2
round-trip. (The two-step `sv emit-btor2-per-module` → `btor2 verify-*` path still
works when you want the intermediate BTOR2.)

### 3. Coverage is a fragment, and verdicts are honest

The SVA translator supports a defined fragment (implication `|->` / `|=>` over
**bounded sequences** on either side, `$past` (including a depth `$past(x, k)`,
k ≥ 1) / `$stable` / `$rose` / `$fell`, `$onehot` / `$onehot0`, …); assertions
outside it come back in the `unsupported` list. The bounded-sequence layer covers
cycle delays (`##k`, `##[m:n]`), fixed consecutive repetition (`b[*n]`), and
multi-element `##` chains of booleans (`a |-> b ##1 c ##2 d`) — in the consequent
*and*, at fixed delays, the antecedent (`(a ##1 b) |-> c`, `a[*n] |-> c`). Each
lowers to nested `[]` — `a |-> ##2 b` → `AG(a → AX² b)`; `(a ##1 b) |-> c` →
`AG(a → AX(b → c))` — so a definite verdict transfers just like the plain `|=>`
(one `[]`) case. What stays out of fragment (rejected with a reason, never
dropped — it needs a SERE→automaton subsystem): **unbounded** `##[m:$]` / `[*n:$]`,
**goto** `[->n]` / **non-consecutive** `[=n]` repetition, range delays/repetition
*inside* a multi-element chain or in an antecedent, and repetition/nesting inside
a chain element.

The history functions work over a **register or a primary input**. That second
half matters more than it sounds: it is what makes a data-integrity property —
"what goes in comes out" — checkable at all, since the antecedent samples an
input.

> Source of truth: [`augment_with_past_shadows`](../crates/mununu-core/src/adapter/btor2/shadow.rs) — surface: (CLI+API+UI)

```systemverilog
(push && !pop && cnt_q == 0) |=> (d0_q == $past(din))
```

Each `$past` base becomes a real flop in the model (`next(b__past) = b`), so the
verdict transfers to the concrete design. `$past(x, k)` becomes a **k-stage shift
chain** (`b__past ← b`, `b__past2 ← b__past`, … `b__past{k}`); the common
`$past(x)` is the 1-stage case. Each stage is a **history variable** — its next
value is a function of the present, nothing in the design reads it, so the
augmented system is a conservative extension of the original: it can neither
create nor destroy a counterexample (the argument composes stage by stage). The
base may be a register or a primary **input**; both produce the same chain.

Only the initial cycles need care, because a history variable has no value before
the first transition. A state cell's stages mirror its `init`, so before the chain
fills (cycles `< j` for stage `j`) `$past(b, j)` reads `b`'s reset value. An input
has no `init` to mirror, so **every stage is pinned to an explicit zero** rather
than left free: an init-less BTOR2 cell reads as 0 to the cube and exact engines
but stays free for the reachability portfolio, and that split is a verdict
disagreement, not a nuance.

That invented value is read only in the first few cycles after reset. The lift
settles it structurally: `|=>` places the atom under a `[]`, so it is evaluated
only at states that have a predecessor — for depth 1 that fully hides the invented
value, while `|->` evaluates it at the initial state too. So a depth-1
data-integrity property (`|=>` by construction) is unaffected; a `|->` property
over `$past` of an input reads an invented cycle-0 history and its verdict there
does not transfer. For a **depth-k** `$past` of an input the chain takes k cycles
to fill, so even under a `[]` the first k-1 post-reset cycles read the invented
zero — an over-approximation bounded to those cycles (a register base reads its
reset value there, which is the SVA convention, not an approximation).

Two caveats that used to be about `$past` under must-edge inference. Both are now
fixed at the lift (2026-08-28): the abstraction enforces the KMTS invariant
`R_must ⊆ R_may` where must-edges are created — it no longer fabricates a must-edge
out of an unsatisfiable predicate cube, and a per-lift check rejects any residual
containment violation before it reaches an engine. So `must_edge_inference` no
longer reports a spurious `violated` for a correct `$past` design, and
`--engine symbolic` no longer aborts on one. (Default is still must-edge inference
off, which was already sound for `holds`.)
[`docs/design/past-shadow-soundness.md`](design/past-shadow-soundness.md) carries
the full argument, the per-engine ledger, the §6 fix note, and why no engine may
ever assume stutter equivalence here.

Some designs hit the abstraction ceiling and return
`unknown`. Crucially, **a `holds` or `violated` verdict is sound** (Bruns–Godefroid
3-valued preservation for the abstraction; k-induction / IC3 / BDD for the exact and
portfolio engines) — an agent can trust a definite verdict and treat `unknown` /
`skipped` as "not decided here," not as "safe."

---

## Using mununu in CI (GitHub Actions and friends)

> Source of truth: [`FailOn` / `ci_exit_code`](../crates/mununu-cli/src/main.rs) — surface: CLI

Every verify verb (and `sv verify-auto`) is a **CI gate**: it maps the verdict to a
process **exit code** so a workflow step fails on a real violation without parsing
JSON.

| Exit code | Meaning |
|-----------|---------|
| `0` | property holds (or all properties hold) — the step passes |
| `2` | a property is **violated** — the step fails |
| `3` | a property is **unknown** *and* `--fail-on unknown` was set |
| `1` | tool / usage error (bad file, unparseable atom, missing toolchain) |

- `--fail-on <violated\|unknown\|none>` picks the gate policy. Default `violated`:
  an undecided `unknown` does **not** fail the build (it is "not decided," not
  "broken"). Use `--fail-on unknown` for a strict gate, or `--fail-on none` to
  report-only (always exit `0`).
- `--quiet` (global) suppresses the `logs/mununu.log` workspace file and the startup
  banner — errors only to stderr, so the workspace stays clean and stdout is the JSON.

```yaml
# .github/workflows/verify.yml — fail the build if the FSM can't recover to idle
jobs:
  recoverability:
    runs-on: ubuntu-latest
    container: ghcr.io/vscorza/mununu-sva:latest   # bundles slang + sv2v + yosys
    steps:
      - uses: actions/checkout@v4
      - run: |
          mununu --quiet sv verify-recoverability rtl/fsm.sv \
            --preprocess-sv2v --target "state_q == 0"   # exit 2 ⇒ step fails
```

For a whole module's assertions in one gate, `mununu --quiet sv verify-auto rtl/mod.sv
--preprocess-sv2v --json` exits non-zero iff any property is violated; the JSON on
stdout carries the per-property detail for a summary step. `verify-auto` also
**auto-escalates** a *safety* property the cube abstraction leaves `⊥` to the
multi-engine reachability portfolio (`--no-rescue` opts out), recording a
`portfolio-rescue` note if the portfolio decides it.

The full CI recipe (a complete GitHub Actions workflow) and the agent-over-HTTP
recipe live in
[`wiki/CI-and-Agent-Integration.md`](../wiki/CI-and-Agent-Integration.md).

---

## See also

- [`../wiki/CI-and-Agent-Integration.md`](../wiki/CI-and-Agent-Integration.md) — the
  full CI (GitHub Actions) + agent-over-HTTP integration guide.
- [`design/recoverability-vs-sva.md`](design/recoverability-vs-sva.md) — why `AG EF`
  recoverability is outside SVA, with the OpenTitan `csrng` worked example.
- [`cli-cookbook.md`](cli-cookbook.md) — common `mununu` CLI invocations.
- [`external-tools.md`](external-tools.md) — installing slang / sv2v / Yosys and the
  discovery env vars.
- [`../wiki/Verify-Project-Flow.md`](../wiki/Verify-Project-Flow.md) — the N-source
  `verify.toml` framework for composing multiple sources.
