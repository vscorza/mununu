# Consumer briefing — 2026-08 `sv mutate` `params` API/UI parity (mununu#475 item 4 follow-up)

> **Audience:** primarily **ROSF** and **monono** if either drives `POST /api/v1/sv/mutate` (rather than the CLI directly), plus the mununu-ui typed client.
>
> **Related:** the CLI-side landing of `--param` for `sv mutate` shipped in PR mununu#479. The prior briefing [`2026-08-sv-lint-mutate-ergonomics.md`](2026-08-sv-lint-mutate-ergonomics.md) explicitly listed API/UI parity as a follow-up. This closes it.
>
> **TL;DR:** `SvMutateRequest.params: string[]` is now accepted by the API and typed on the UI client. Same parse shape as the sibling `sv/verify-auto` endpoint: each entry is `"NAME=VALUE"` (top-module) or `"MODULE.NAME=VALUE"` (submodule scope). Backward-compatible; consumers that don't send `params` see zero behavioural change.

## What changed

- **`crates/mununu-core/src/api/models.rs`** — added `params: Vec<String>` (with `#[serde(default)]`) to `SvMutateRequest`.
- **`crates/mununu-core/src/api/handlers.rs`** — `sv_mutate_handler_impl` parses each entry the same way `sv verify-auto`'s CLI + API do (both sides non-empty; hard `BadRequest` on malformed) and threads into `YosysOptions::params`.
- **`mununu-ui/src/api/endpoints.ts`** — added `params?: string[]` to the `SvMutateRequest` interface.

## What did NOT change

- `PropertyVerdict` — unchanged.
- `SvMutateResponse` — unchanged (params are input-side only).
- Route `POST /api/v1/sv/mutate` — unchanged.
- Existing request fields on `SvMutateRequest` — unchanged shape and semantics.
- Engine behaviour on any previously-decided property — unchanged.

## For ROSF and monono

**What to update:** nothing forced. To adopt: send `"params": ["ROW_BITS=2", "COL_BITS=3"]` (or similar) alongside your existing mutate request when the block needs parameter shrinkage to lift under the bit-blast cap. Malformed values (missing `=`, empty NAME or VALUE) return HTTP 400 with a descriptive message — never a silent drop.

**Docker rebuild:** required only if a consumer pins a specific mununu binary version. Otherwise the field appears on the next binary pull; existing requests continue to work with an empty `params`.

## Docker rebuild table

| Image | Impact | Rebuild required? |
|-------|--------|-------------------|
| mununu `Dockerfile` (prod CLI + API server) | one new optional request field | Yes if consumers pin a version tag |
| mununu `Dockerfile.dev` | binary bump | Only if the dev workflow requires the new binary |
| mununu `Dockerfile.sva` | binary bump; no e2e-test behaviour change | Only if a downstream requires the new binary |
| mununu `Dockerfile.extract`, `.extract-*` | no impact | No |
| rosf `docker/Dockerfile` | consumes mununu subprocess and/or API | Only if pinned to a version tag |
| rosf `docker/Dockerfile.dev`, `.hw` | no direct dependence | No |
| monono Docker (any) | consumes CLI + potentially API | Only if pinned to a version tag |
| mununu-ui deployment | new typed field on `SvMutateRequest` | Rebuild + redeploy on the ui-side change |

## Provenance

- Fix commit (mununu-side): (pending merge — branch `fix/api-ui-parity-sv-mutate-param`).
- Fix commit (mununu-ui-side): landed independently — matching `params?: string[]` field.
- Issue: mununu#475 item 4 — API/UI parity follow-up.
- Policy this briefing satisfies: [`docs/policies/cross-repo-impact.md`](../policies/cross-repo-impact.md).
- Sibling briefing (CLI-side landing): [`2026-08-sv-lint-mutate-ergonomics.md`](2026-08-sv-lint-mutate-ergonomics.md).

## Not covered here

- `--exclude` and `--search-path` API/UI parity — deliberately CLI-only. On-disk directory scans have no API analog (the API stages sources by name). The docstrings on those flags carry the `surface: CLI-only` exemption per CLAUDE.md's Surface Parity rule.
