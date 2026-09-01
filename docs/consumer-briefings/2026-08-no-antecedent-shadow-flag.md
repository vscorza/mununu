# Consumer briefing — 2026-08 `--no-antecedent-shadow` CLI + API (mununu#476 item 4)

> **Audience:** ROSF (industrial-profile API consumer), monono (CLI consumer), plus any downstream that runs differential-oracle checks against the exact-symbolic engine.
>
> **Related:** the antecedent shadow-synth landed in PR mununu#478; the design doc [`docs/design/antecedent-shadow-synthesis.md`](../design/antecedent-shadow-synthesis.md) explicitly listed this CLI/API opt-out as follow-up.
>
> **TL;DR:** shadow-synth now has three opt-out channels — CLI flag, per-request API field, and the pre-existing process-global env var. Per-request opt-out is thread-safe (the env var isn't). Default behaviour unchanged: shadow-synth stays enabled, verdicts on SVA `|=>` properties with input-derived antecedents continue to decide.

## What changed

- **`ExactSymbolicOptions` struct** in `crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs` — new per-invocation options with `antecedent_shadow_enabled: bool` (default `true`). Enables thread-safe per-call control of the shadow-synth path.
- **New public engine entries** `exact_symbolic_verdict_with_options` + `exact_symbolic_verdict_with_witness_and_options` — opts-accepting variants of the existing public entries. Existing entries wrap with `Default::default()` for backward compat.
- **`VerifyAutoOptions.antecedent_shadow: bool`** (default `true`) — plumbed through the two engine callsites in `verify_auto.rs` (`exact_symbolic` path + `rescue_skipped_via_exact` fallback). Both now pass through `ExactSymbolicOptions { antecedent_shadow_enabled: opts.antecedent_shadow }`.
- **CLI** — `--no-antecedent-shadow` on `SvVerifyAutoArgs`. `sv verify-auto` handler threads it into `VerifyAutoOptions.antecedent_shadow`.
- **API** — `no_antecedent_shadow: Option<bool>` on `SvVerifyAutoRequest` (defaults to `false` when absent). `sv_verify_auto_handler` threads it into `VerifyAutoOptions.antecedent_shadow`.
- **UI** — `no_antecedent_shadow?: boolean` on the mununu-ui `SvVerifyAutoRequest` TypeScript interface.
- **Env-var check refactored** — the check in `exact_symbolic_verdict_with_witness_inner` is now `!opts.antecedent_shadow_enabled || env::var(...).is_ok()`. Either channel disabling shadow-synth wins.

## What did NOT change

- Default behaviour — shadow-synth stays enabled; all previously-decided properties still decide.
- `PropertyVerdict` — unchanged.
- `AutoVerifyReport` — unchanged.
- Existing CLI flags / API fields — unchanged shape and semantics.
- The env-var opt-out (`MUNUNU_NO_ANTECEDENT_SHADOW=1`) — still works, kept for scripts and CI shells.
- Engine behaviour on any previously-decided property — unchanged.

## Thread-safety note

The env-var channel is **process-global** and races when a multi-tenant server has concurrent verify-auto calls with different `antecedent_shadow` intents. The new per-request API field is the correct choice for such consumers. Single-threaded CLI / test scripts can continue to use whichever channel is more ergonomic.

## For ROSF-agents (subprocess `--profile industrial`)

**What to update:** nothing forced — defaults preserve the shipped shadow-synth behaviour. If ROSF wants a differential-oracle mode where the industrial profile flips between shadow-synth-on/off per request, add `"no_antecedent_shadow": true` to specific requests. Thread-safe under concurrent invocation.

**Docker rebuild:** required only if ROSF pins a specific mununu binary version.

## For monono-agents (direct CLI consumer)

**What to update:** nothing forced. If monono runs a differential-oracle sweep that needs to compare shadow-synth verdicts against the Phase A refusal, replace `MUNUNU_NO_ANTECEDENT_SHADOW=1 mununu sv verify-auto …` with `mununu sv verify-auto --no-antecedent-shadow …`. Cleaner (no shell env-var management) and matches the API shape.

**Docker rebuild:** required only if monono pins a specific mununu binary version.

## Docker rebuild table

| Image | Impact | Rebuild required? |
|-------|--------|-------------------|
| mununu `Dockerfile` (prod CLI + API) | new CLI flag + new optional API field + engine opts refactor (backward-compat public entries) | Yes if consumers pin a version tag |
| mununu `Dockerfile.dev` | binary bump | Only if the dev workflow requires the new binary |
| mununu `Dockerfile.sva` | binary bump; test refactor uses the new opts channel (no env-var mutation) | Only if a downstream requires the new binary |
| mununu `Dockerfile.extract`, `.extract-*` | no impact | No |
| rosf `docker/Dockerfile` | consumes mununu subprocess/API | Only if pinned to a version tag; behavior updates on next binary pull otherwise |
| rosf `docker/Dockerfile.dev`, `.hw` | no direct dependence | No |
| monono Docker (any) | consumes CLI or API | Only if pinned to a version tag |
| mununu-ui deployment | new typed field on `SvVerifyAutoRequest` | Rebuild + redeploy on the ui-side change |

## Provenance

- Fix commit (mununu-side): (pending merge — branch `fix/no-antecedent-shadow-flag`).
- Fix commit (mununu-ui-side): matching `no_antecedent_shadow?: boolean` on TS `SvVerifyAutoRequest`, lands independently.
- Issue: mununu#476 item 4 (design-doc follow-up).
- Design: [`docs/design/antecedent-shadow-synthesis.md`](../design/antecedent-shadow-synthesis.md).
- Policy this briefing satisfies: [`docs/policies/cross-repo-impact.md`](../policies/cross-repo-impact.md).
- Sibling briefings: [`2026-08-antecedent-shadow-synth.md`](2026-08-antecedent-shadow-synth.md) (initial landing), [`2026-08-multi-atom-antecedent-shadow.md`](2026-08-multi-atom-antecedent-shadow.md) (multi-atom extension).

## Not covered here (follow-ups from the user's list)

- `docs/api-schemas/verdict.md` — stable JSON schema doc for `PropertyVerdict` / `AutoVerifyReport`. Next item.
