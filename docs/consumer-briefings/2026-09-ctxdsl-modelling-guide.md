# Consumer briefing — 2026-09 CTXDSL hand-authoring guide + the `v10_mem_fabric_client_mux` worked example

> **Audience:** monono (direct CLI consumer; hand-authors CTXDSL models in `verify/models.sh`), ROSF (API consumer), anyone who writes a `.ctxdsl` source by hand or generates one.
>
> **TL;DR:** **no code changed, no binary rebuild required, no wire format touched.** This is a documentation + example delivery. It writes down five silent CTXDSL behaviours that produce *wrong verdicts without any error*, and ships a four-variant worked example that decides a real RTL design question. If you hand-author CTXDSL, at least two of these five almost certainly affect a model you already have — the guide's §Checklist is the fastest way to find out.

## Why this exists

A consumer spent a day on a CTXDSL model for an RTL decision and hit four dead
ends in a row, three of which were the tool failing silently and one of which
was a documented syntax that does not parse. Every one is now written down,
verified against the shipped binary, and anchored to source.

## What changed

**New — [`docs/ctxdsl-modelling-guide.md`](../ctxdsl-modelling-guide.md).** What
binds and what silently doesn't when you hand-author CTXDSL. Eleven sections,
each verified against the shipped binary with a positive *and* negative control,
each anchored to a `Source of truth:` line. The load-bearing five:

1. **Transition `guard`s accept exactly ONE comparison.** `&&`, `||` and `!`
   inside a guard are parsed lossily and **silently disable the transition** —
   no error, no warning. `crate::guard::GuardExpr` has no `And`/`Or`/`Not`
   variant, so `split_comparison` turns `a == b && a == 2` into the comparison
   `a == "b && a == 2"`, which nothing satisfies. **This is an
   under-approximation**: the model has fewer behaviours than the system, so it
   reports "not reachable" for a bug that is reachable. A model built this way
   is *too kind*. Conjunction IS available in mu-calculus formulas.
2. **`effects` are simultaneous and read the pre-state** — non-blocking
   assignment. `effects { a = b; b = a; }` swaps. This is what you want for a
   clock edge, and it is now stated rather than implied.
3. **Only `i64` variables bind in formula atoms.** An atom naming a `bool`
   variable — or a misspelled name — resolves to `Maybe`, is *included*, and
   therefore evaluates to **true at every state**. A safety property over a
   `bool` is vacuous; a reachability property spuriously holds.
4. **Across a multi-source `verify.toml` composition, variable atoms return
   `false`** while an unknown name still returns `true`. A negative control
   cannot detect this; **a positive control can**, which is why the guide asks
   for both. State-name atoms bind on every path.
5. **A `predicates { predicate p = state X; }` naming an *unreachable* location
   is a hard realization error**, not `false`. This breaks contrast pairs
   specifically: the *sound* variant of a design is the one where the bug state
   is unreachable, so the sound model refuses to load while the broken one
   answers.

Plus: every unrolled copy of an `initial` location is marked initial (so
`Initial states satisfying: k/N` is ambiguous — use a dedicated `Reset`
location); an automaton with no `controllable {}` block defaults every label to
Controllable, so two automata sharing a label collide; `context summarize`
reports the *declared* state count while `context eval` runs the *unrolled* one.

**New — [`examples/verify/v10_mem_fabric_client_mux/`](../../examples/verify/v10_mem_fabric_client_mux/README.md).**
Four generated variants of one RTL issue-path decision (combinational mux vs.
three ways of registering it), with a contrast pair, a mandatory non-vacuity
gate, and a **mechanism control** — the model is regenerated with the suspected
cause removed and the verdict must flip. Reusable as a template for any
"which of these N designs is correct" question.

**Documentation drift fixed** (all three verified by running them):

| Page | Was | Now |
|---|---|---|
| `wiki/CTXDSL-Language-Reference.md` | `variables { count: i64 = 0; }` | **does not parse** — the `var` keyword is required. Corrected, plus the three traps above called out inline. |
| `wiki/Property-Templates.md` | documented `mununu templates` (4 invocations) and `context eval --template` / `--template-arg` | **none of these exist** in the shipped CLI. Replaced with a drift notice, the `verify.toml` form that does work, and the identifier-only `TARGET` restriction with its `formula = "…"` escape hatch. |
| `examples/verify/v2_tso_storebuffer` README + generator | "`r0 == 0` atoms do not bind through the verify composition path" | **correct for multi-source, over-broad as written** — atoms do bind on `context eval` and on single-source `verify` (that example's own shape). Scoped rather than removed. |

Also: `wiki/Verify-Project-Flow.md` and `CLAUDE.md`'s reference index now point
at the guide.

## What did NOT change

- **No Rust source.** `git diff --stat` touches zero `.rs` files.
- Wire format, response shapes, verdict values, verdict semantics — unchanged.
- CLI surface — no flags added, removed or renamed. (The `mununu templates`
  subcommand documented in the wiki never existed; documenting its absence is
  not a removal.)
- HTTP routes, sidecar schemas, `docs/api-schemas/` — unchanged.
- Engine behaviour, soundness posture, subprocess tool versions — unchanged.

## For monono

**What to update.** Nothing is forced, but run the guide's §Checklist against
every hand-authored `.ctxdsl` in `verify/models.sh`. In priority order:

1. **`grep -n '&&\|||' ` inside any `guard`.** Every hit is a transition your
   model has silently been missing. This is the one that can have been hiding a
   real bug behind a green gate.
2. **Any `bool` variable named in a property.** Change it to `i64` and compare
   `== 1`. Then re-run: a verdict that changes was vacuous before.
3. **Add a positive and a negative control atom to each model.** The negative
   catches silent-true; the positive catches the multi-source silent-false.
   Neither alone is sufficient.
4. **Add a non-vacuity gate** — assert the "good" outcome is reachable — and,
   for any contrast pair, a control that removes the mechanism you believe
   causes the failure and makes the verdict flip.

**What to expect.** A model that was silently under-approximating will start
reporting *more* reachable states, and a property that was vacuously holding may
flip to violated. **That is the fix working, not a regression** — treat any flip
as a finding to investigate, not as a change to suppress.

**Report-parsing impact:** none. No verdict value, note kind or JSON field
changed.

**Docker rebuild:** none. See the table below.

## For ROSF

No action. Nothing ROSF consumes changed. The guide is relevant only if ROSF
hand-authors CTXDSL sources; if it only drives `sv verify-auto` / `btor2` verbs
over the API, this delivery is a no-op for it.

## Docker rebuild table

| Image | Impact | Rebuild required? |
|-------|--------|-------------------|
| mununu `Dockerfile` (prod) | none — no binary change | **No** |
| mununu `Dockerfile.dev` | none — no toolchain or dependency change | **No** |
| mununu `Dockerfile.sva` | none | **No** |
| mununu `Dockerfile.extract`, `.extract-llvm`, `.extract-circt` | none | **No** |
| rosf `Dockerfile` / `Dockerfile.dev` / `.hw` | none | **No** |
| monono Docker (if any) | none — docs and examples only | **No** |
| mununu-ui deployment | none | **No** |

Every cell is **No**: this delivery contains no compiled artifact. Adopting it
means reading the guide and re-checking your models, not pulling a new binary.
Any mununu build that already runs `mununu verify` on a `verify.toml` will
reproduce the example — its models use only long-shipped surface
(`template = "reachable"`, the raw `formula` field, multi-label transitions,
`predicates`).

## Verification steps

```bash
# The worked example, its contrast pair, and its mechanism control:
bash examples/verify/v10_mem_fabric_client_mux/validate.sh

# The example whose README this delivery corrects, still green:
bash examples/verify/v2_tso_storebuffer/validate.sh
```

Both pass. The first also asserts that the checked-in models are byte-identical
to `generate.py`'s output, so the generator and the artifacts cannot drift.

## Provenance

- Branch: `docs/ctxdsl-modelling-guide-and-v10-mem-fabric`.
- Findings verified 2026-09-04 against `target/debug/mununu` at `8580d84`.
- Guide: [`docs/ctxdsl-modelling-guide.md`](../ctxdsl-modelling-guide.md).
- Example: [`examples/verify/v10_mem_fabric_client_mux/`](../../examples/verify/v10_mem_fabric_client_mux/README.md).
- Policy: [`../policies/cross-repo-impact.md`](../policies/cross-repo-impact.md).

## Not covered here (follow-ups)

- **Widening the guard parser.** `crate::guard::GuardExpr` gaining
  `And`/`Or`/`Not` would remove trap 1 entirely;
  `abstraction::expression::GuardExpr` already has the variants, they are just
  unreachable from CTXDSL source because the conversion goes through the lossy
  parser first. Until then the guide is the contract.
- **Diagnosing an unparseable guard instead of silently disabling it.** Even
  without widening the fragment, `parse_guard` could reject a right-hand side
  containing `&&` rather than accepting it as a string literal.
- **Warning on an unbound formula atom.** The `Maybe → include` default is a
  deliberate over-approximation, but it is silent; a warning naming the unbound
  identifier would convert trap 3 from a wrong verdict into a message.
- **The multi-source composition variable-atom gap.** Variable atoms returning
  `false` across a multi-source `verify` composition (while state-name atoms
  bind) is documented here as a boundary, not diagnosed as a defect. Root-cause
  work is a separate track.
