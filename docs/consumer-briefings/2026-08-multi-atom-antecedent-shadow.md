# Consumer briefing — 2026-08 multi-atom antecedent shadow-synthesis (mununu#476 follow-up)

> **Audience:** primarily **monono** (SVA `|=>` consumer via `sv verify-auto`), secondarily **ROSF** (subprocess consumer via `--profile industrial`) — either may benefit from the wider antecedent-shape coverage.
>
> **Related:** the initial shadow-synth landed in PR mununu#478 for the single-atom case. This closes a follow-up that PR explicitly named. Design: [`docs/design/antecedent-shadow-synthesis.md`](../design/antecedent-shadow-synthesis.md).
>
> **TL;DR:** the shadow-synth detector now handles multi-atom antecedents — `(a && b) |=> c`, `(a || b) |=> c`, `(a && !b) |=> c`, and any Boolean tree of `And` / `Or` / nested `Not` over `Predicate` leaves. Every leaf gets an independent shadow. Verdicts that were previously `Skipped` under a multi-atom antecedent now decide. Additive-only — no verdict semantics change for anything that already decided.

## What changed

- **`detect_pipeimplies_antecedent_atoms`** in `mu_calculus/mod.rs` — the walker under the antecedent-side `Not` now recursively descends into `And` / `Or` / nested `Not` nodes and collects every `Predicate` leaf. Single-atom antecedents (the shipped case) still work identically.
- **Independent shadow synthesis per leaf** — each collected atom gets its own `_mununu_antshadow_<N>` state cell (`init=0`, `next=A`); the rewritten formula uses each shadow in place of the original atom.
- **Six new unit tests** in the `mu_calculus::tests` module + one new integration test in `symbolic_bitblast::tests` (`shadow_synth_flips_multi_atom_antecedent_to_decided`), exercising AND-antecedent, OR-antecedent, mixed AND+NOT, dedup on repeated leaves, and the full safety-envelope shape.

## What did NOT change

- `PropertyVerdict` — unchanged.
- `AutoVerifyReport` — unchanged.
- The Phase A refusal fallback — unchanged. If any single leaf inside a multi-atom antecedent hits one of the five refusal conditions (non-Boolean, array-in-cone, bare-primary-input, anonymous-input reach, unresolvable name), THAT leaf falls through to the Phase A refusal; the other leaves still get shadows.
- Non-`|=>` formulas — unchanged. Bare `EF`, `AF`, and other shapes are untouched by the detector.
- HTTP API routes — unchanged.
- Existing single-atom shadow-synth behaviour — unchanged.

## Soundness argument

Independent shadows compose over the SVA `|=>` semantics:

- `(A ∧ B) |=> C` = `AG((A ∧ B) → next C)`. Shadowing gives `AG((S_A ∧ S_B) → C)`; since `S_A@N+1 = A@N` and `S_B@N+1 = B@N`, this evaluates to `AG((A@N ∧ B@N) → C@N+1)`. ✓
- `(A ∨ B) |=> C` = `AG((A ∨ B) → next C)`. Shadowing gives `AG((S_A ∨ S_B) → C)` = `AG((A@N ∨ B@N) → C@N+1)`. ✓
- `(A ∧ ¬B) |=> C` = `AG((A ∧ ¬B) → next C)`. Shadowing gives `AG((S_A ∧ ¬S_B) → C)` = `AG((A@N ∧ ¬B@N) → C@N+1)`. ✓

The composition preserves the SVA obligation cycle-for-cycle. No extra assumptions.

## For monono-agents (direct CLI consumer)

**What to update:** nothing forced — old commands still work; new capability is automatic. To adopt: any previously-`Skipped` property whose antecedent was a compound expression like `(valid && ready) |=> next_state == X` should now decide. Re-run `sv verify-auto` on your tree and inspect the `Skipped` count — it should drop for this class.

**What to expect on the `wb_mem_client` family**: the shipped single-atom case (`mem_rvalid_mine |=> …`) already decided post-#478. Any similar block whose antecedent has two-or-more input-derived signals (`valid && ready`, `sel && wr`, `state == 3 && mem_rvalid`, etc.) now decides too.

**Docker rebuild:** required only if monono pins a specific mununu binary version.

## For ROSF-agents (subprocess `--profile industrial`)

**What to update:** nothing. This PR extends the same shadow-synth machinery ROSF already benefits from via `sv verify-auto`; verdict quality improves on multi-atom-antecedent properties without any API-shape change.

**Docker rebuild:** required only if `rosf/docker/Dockerfile` pins a specific mununu binary version.

## Docker rebuild table

| Image | Impact | Rebuild required? |
|-------|--------|-------------------|
| mununu `Dockerfile` (prod CLI + API) | binary carries wider antecedent-shape coverage | Yes if consumers pin a version tag |
| mununu `Dockerfile.dev` | binary bump | Only if the dev workflow requires the new binary |
| mununu `Dockerfile.sva` | binary bump; e2e tests exercise the extended shadow-synth path | **Yes** if a downstream needs the SVA-gated end-to-end validation before merge (per CLAUDE.md §"SVA-verification e2e validation") |
| mununu `Dockerfile.extract`, `.extract-*` | no impact | No |
| rosf `docker/Dockerfile` | consumes mununu subprocess; verdict quality changes on multi-atom-antecedent properties | Only if pinned to a version tag; behavior updates on next binary pull otherwise |
| rosf `docker/Dockerfile.dev`, `.hw` | no direct dependence | No |
| monono Docker (any) | consumes `sv verify-auto` CLI | Only if pinned to a version tag |

## Shared footer — verification you should run after adopting

1. **Confirm multi-atom cases decide**: pick any `(atom1 && atom2) |=> next` shape in your tree that previously `Skipped` with a "combinationally driven by primary input" refusal message. It should now return a definite verdict.
2. **Confirm single-atom cases still work**: any single-atom antecedent that decided post-#478 should still decide identically. Regression protection is in the pre-existing `shadow_synth_flips_derived_input_antecedent_to_decided` test.
3. **Confirm bare-primary-input leaves in multi-atom shapes still refuse**: `(bare_input && mem_rvalid_mine) |=> C` should refuse on `bare_input` (via the Phase A `IsPrimaryInput` message) even though `mem_rvalid_mine` gets a shadow — the fallback protects the author-confirmation intent per the design doc.

## Provenance

- Fix commit: (pending merge — see branch `fix/multi-atom-antecedent-shadow` in the mununu repo)
- Issue: mununu#476 follow-up (multi-atom antecedents were listed in the design doc's Rollout Plan §7 and in the closing "Not covered here" of the prior briefing).
- Design: [`docs/design/antecedent-shadow-synthesis.md`](../design/antecedent-shadow-synthesis.md) — the "Multi-atom antecedents" callout added in this PR.
- Policy this briefing satisfies: [`docs/policies/cross-repo-impact.md`](../policies/cross-repo-impact.md).
- Sibling briefings: [`2026-08-antecedent-shadow-synth.md`](2026-08-antecedent-shadow-synth.md) (initial single-atom landing).

## Not covered here (follow-ups)

- `--no-antecedent-shadow` CLI flag + API field (currently env-var only). Next item.
- `docs/api-schemas/verdict.md` — stable JSON schema doc for `PropertyVerdict` / `AutoVerifyReport`. Next-next item.
