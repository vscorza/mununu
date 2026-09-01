# Consumer briefing — 2026-09 JSON schemas for the BTOR2 + SV verb endpoints

> **Audience:** ROSF (API consumer via `--profile industrial`), monono (direct API + CLI), any orchestrator or codegen consumer that hits `/btor2/*` or `/sv/*` property-verb endpoints.
>
> **Related:** the machine-readable JSON Schema mechanism landed in mununu#485 ([`docs/api-schemas/`](../api-schemas/)) with only the flagship `sv verify-auto` derived. This PR extends the coverage across the BTOR2-direct verbs and their SV wrappers.
>
> **TL;DR:** 14 additional `*.schema.json` files landed under [`docs/api-schemas/`](../api-schemas/) covering `POST /btor2/verify`, `verify-liveness`, `verify-liveness-all`, `verify-recoverability`, `check-fsm` and their `sv/*` siblings. **No wire-format change.** Consumers who point `quicktype` / `openapi-typescript` / `datamodel-code-generator` at the new files get typed clients for the full verify-verb surface. Consumers that continue to hand-parse are unaffected.

## What changed

- **`JsonSchema` derives** on 12 wire types in `crates/mununu-core/src/api/models.rs`:
  - Requests: `Btor2VerifyRequest`, `Btor2VerifyLivenessRequest`, `Btor2VerifyLivenessAllRequest`, `Btor2VerifyRecoverabilityRequest`, `Btor2CheckFsmRequest`, `SvVerifyRequest`, `SvVerifyLivenessRequest`, `SvVerifyLivenessAllRequest`, `SvVerifyRecoverabilityRequest`, `SvCheckFsmRequest`.
  - Responses: `Btor2VerifyResponse`, `Btor2VerifyLivenessResponse`, `Btor2VerifyRecoverabilityResponse`, `Btor2CheckFsmResponse` (nested `FsmRegisterFinding`). The SV wrappers reuse the BTOR2 responses one-for-one.
- **Cfg-attr `JsonSchema` derives** on 6 external types that the recoverability response transitively references (unchanged behaviour; the derive is `#[cfg_attr(feature = "api", …)]` so default builds are unaffected):
  - `crate::verdict::PropertyVerdict`, `VerdictRefinement`, `VacuityWitness`, `ConfigPartition`, `AssumptionKind`, `DiscoveredAssumption`.
  - `crate::adapter::recoverability::RecoverabilityBotDiagnosis`.
- **`crates/mununu-core/src/api/schema.rs`** — 14 new generator functions + 14 new drift-detector tests.
- **Shipped schema files** under `docs/api-schemas/`:
  - `btor2-verify-request.schema.json`, `btor2-verify-response.schema.json`
  - `btor2-verify-liveness-request.schema.json`, `btor2-verify-liveness-response.schema.json`
  - `btor2-verify-liveness-all-request.schema.json`
  - `btor2-verify-recoverability-request.schema.json`, `btor2-verify-recoverability-response.schema.json`
  - `btor2-check-fsm-request.schema.json`, `btor2-check-fsm-response.schema.json`
  - `sv-verify-request.schema.json`, `sv-verify-liveness-request.schema.json`, `sv-verify-liveness-all-request.schema.json`, `sv-verify-recoverability-request.schema.json`, `sv-check-fsm-request.schema.json`
- **`docs/api-schemas/README.md`** — extended with sectioned tables covering BTOR2-direct verbs, SV-wrapped verbs, and the SV → BTOR2 response-reuse mapping.
- **`docs/api-schemas/verify-verbs.md`** — new — narrative companion walking each response field-by-field and mapping every verb onto safety / liveness / recoverability / FSM.

## What did NOT change

- Wire format — unchanged.
- `PropertyVerdict` values — unchanged.
- Handler code, engine behaviour, and route table — unchanged.
- The `sv verify-auto` schema from mununu#485 — unchanged.

## For ROSF and monono

**What to update:** nothing forced. To adopt machine-readable typing on your side for the BTOR2 + SV verb surface:

```sh
# Point codegen at the response schemas your consumer parses. Example — TypeScript:
for f in docs/api-schemas/btor2-*-response.schema.json docs/api-schemas/sv-verify-auto-response.schema.json; do
  base=$(basename "$f" .schema.json)
  npx openapi-typescript "$f" --output "types/${base}.d.ts"
done

# For request bodies your consumer posts:
for f in docs/api-schemas/btor2-*-request.schema.json docs/api-schemas/sv-*-request.schema.json; do
  base=$(basename "$f" .schema.json)
  npx openapi-typescript "$f" --output "types/${base}.d.ts"
done
```

The SV → BTOR2 response reuse means a consumer only needs the BTOR2 response types plus the SV request types. See [`docs/api-schemas/verify-verbs.md`](../api-schemas/verify-verbs.md) for the mapping.

Pin a specific mununu binary release + the corresponding schema files. On a version bump, either accept the drift (the new release's schema is the new contract) or pin the previous release while you migrate. A briefing accompanies any wire-format change.

**Docker rebuild:** none required. The schema files are docs; the mununu binary itself does not carry the schemas at runtime.

## Docker rebuild table

| Image | Impact | Rebuild required? |
|-------|--------|-------------------|
| mununu `Dockerfile` (prod) | binary picks up new JsonSchema derives + generator functions. No behavioural change. | No — unless you consume the derived schemas out of the binary via a helper endpoint (there isn't one shipped) |
| mununu `Dockerfile.dev` | binary bump; drift-detector test now covers 16 schemas total | Only if the dev workflow requires the new binary |
| mununu `Dockerfile.sva` | binary bump; no e2e-test behaviour change | No — the slang-gated e2e set is unaffected by pure derive additions |
| mununu `Dockerfile.extract`, `.extract-*` | no impact | No |
| rosf `Dockerfile` / `Dockerfile.dev` / `.hw` | consumes mununu CLI/API; no wire-format change | No — new schema files land as docs, not as binary contract |
| monono Docker (if any) | consumes CLI; no CLI change | No |
| mununu-ui deployment | no impact | No |

## Regenerate flow (when the wire format changes)

Unchanged from mununu#485 — one command covers every derived schema:

```sh
MUNUNU_UPDATE_API_SCHEMAS=1 \
  cargo test -p mununu-core --lib --features api api_schema_drift_ -- --test-threads=1
git add docs/api-schemas/
```

Then ship a consumer briefing per [`../policies/cross-repo-impact.md`](../policies/cross-repo-impact.md) explaining the transition.

## Verification

- `make ci` green with the new derives in place.
- `MUNUNU_UPDATE_API_SCHEMAS=1 cargo test -p mununu-core --lib --features api api_schema_drift_` regenerates all 16 shipped schemas idempotently (unset the env var and the drift-detector confirms zero diff).
- `docs/api-schemas/verify-verbs.md` walks every response and every SV request; the `README.md` table lists every file.

## Provenance

- Fix commit: (pending merge — branch `docs/api-schemas-btor2-and-synth`).
- Related: mununu#485 (machine-readable schema mechanism + flagship `sv verify-auto`).
- Policy: [`../policies/cross-repo-impact.md`](../policies/cross-repo-impact.md).
- Prior briefing on this track: [`2026-08-json-schema-derivation.md`](2026-08-json-schema-derivation.md).

## Not covered here (follow-ups)

- **CEGAR schemas** — `POST /btor2/cegar` and `POST /sv/cegar`. Response has a large view-type tree (`CegarIterationView`, `PredicateView`, `WitnessCellView`, `CounterTraceView`, `CegarVerdictSummary`, `PredicateSpecRequest`) that adds ~10 more derives; lands separately to keep this PR focused.
- **Synth schemas** — `POST /context/synthesize` and `POST /synth/gr1`. Responses reference `SynthesisDiagnostics`, `CounterstrategyResult`, and `TemplateRef` outside the `api::` module; adds a small non-api-module derive set. Lands in the CEGAR follow-up or a synth-specific one.
- **Enum-typing for `verdict` / `AssumptionKind` in the schemas.** `verdict` is `String` in the Rust source for backward-compat; `AssumptionKind` is already a proper Rust enum and DOES appear as a JSON-Schema `oneOf` in `btor2-verify-recoverability-response.schema.json`. Migrating `verdict` to a serde-enum is a wire-format shape change with a mandatory briefing — not scoped here.
- **OpenAPI / `utoipa` integration.** Not needed today; the per-response schema files serve every codegen. Can be added additively later.
