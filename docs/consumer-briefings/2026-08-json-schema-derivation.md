# Consumer briefing — 2026-08 machine-readable JSON schemas for the API wire types

> **Audience:** ROSF (API consumer via `--profile industrial`), monono (direct API + CLI), any custom orchestrator or codegen consumer.
>
> **Related:** the human-facing schema doc landed in mununu#484 ([`docs/api-schemas/verdict.md`](../api-schemas/verdict.md)). This PR adds the machine-readable counterpart.
>
> **TL;DR:** every type in the `sv verify-auto` wire format now derives `schemars::JsonSchema`, and the derived schemas are shipped as `docs/api-schemas/*.schema.json` files. A drift-detector test fails CI on any wire-format change without a schema update — the shipped schema is now the authoritative wire contract. Consumers can point `quicktype` / `openapi-typescript` / `datamodel-code-generator` / etc. at the files to auto-generate typed clients.

## What changed

- **New dep** — `schemars = "0.8"` on mununu-core, feature-gated behind `api`.
- **`JsonSchema` derives** on 8 wire types in `crates/mununu-core/src/api/models.rs`:
  `FileContent`, `SvVerifyAutoRequest`, `SvVerifyAutoResponse`,
  `PropertyVerdictView`, `UnsupportedAssertionView`, `ModelDiagnosticsView`,
  `VerificationNoteView`, `CounterexampleView`, `CexCellView`.
- **`crates/mununu-core/src/api/schema.rs`** — new module exposing
  `sv_verify_auto_request_schema()` and `sv_verify_auto_response_schema()`,
  each returning a `serde_json::Value`.
- **Shipped schema files** under `docs/api-schemas/`:
  - `sv-verify-auto-request.schema.json`
  - `sv-verify-auto-response.schema.json`
- **Drift-detector tests** at `api::schema::tests::api_schema_drift_*` — regenerate the schemas on every CI run and diff against the committed files. Any drift fails the build.
- **`docs/api-schemas/README.md`** — new — explains the consumer flow, the drift guarantee, and how to regenerate on a wire-format change.
- **`docs/api-schemas/verdict.md`** — narrative doc now links to the machine-readable files.
- **`CLAUDE.md`** and **`docs/policies/cross-repo-impact.md`** cross-refs updated.

## What did NOT change

- Wire format — unchanged.
- `PropertyVerdict` values — unchanged.
- Existing API routes / request-response shapes — unchanged.
- Engine behaviour — unchanged.

## For ROSF and monono

**What to update:** nothing forced. To adopt machine-readable typing on your side:

```sh
# TypeScript:
npx openapi-typescript docs/api-schemas/sv-verify-auto-response.schema.json --output types/verify-auto.d.ts

# Python:
datamodel-code-generator \
  --input docs/api-schemas/sv-verify-auto-response.schema.json \
  --input-file-type jsonschema \
  --output verify_auto_response.py

# Any language via quicktype:
quicktype --lang python --src-lang schema docs/api-schemas/sv-verify-auto-response.schema.json > verify_auto.py
```

Pin a specific mununu binary release + the corresponding schema file. On a version bump, either accept the drift (the new release's schema is the new contract) or pin the previous release while you migrate. A briefing in [`../consumer-briefings/`](.) accompanies any wire-format change.

**Docker rebuild:** none required. The schema files are docs; the mununu binary itself does not carry the schemas at runtime (drift-detector runs at build/test time, not runtime).

## Docker rebuild table

| Image | Impact | Rebuild required? |
|-------|--------|-------------------|
| mununu `Dockerfile` (prod) | binary picks up new `schemars` dep + JsonSchema derives + `schema` module. No behavioural change. | No — unless you consume the derived schemas out of the binary via a helper endpoint (there isn't one shipped) |
| mununu `Dockerfile.dev` | binary bump; drift-detector test now runs in the dev image | Only if the dev workflow requires the new binary |
| mununu `Dockerfile.sva` | binary bump; no e2e-test behaviour change | Only if a downstream requires the new binary |
| mununu `Dockerfile.extract`, `.extract-*` | no impact | No |
| rosf/monono Docker | consumes CLI/API; no wire-format change | No — new schema files land as docs, not as binary contract |
| mununu-ui deployment | no impact | No |

## Regenerate flow (when the wire format changes)

The drift-detector fails on any mismatch. To update:

```sh
MUNUNU_UPDATE_API_SCHEMAS=1 \
  cargo test -p mununu-core --lib --features api api_schema_drift_ -- --test-threads=1
git add docs/api-schemas/
```

Then ship a consumer briefing per [`../policies/cross-repo-impact.md`](../policies/cross-repo-impact.md) explaining the transition.

## Provenance

- Fix commit: (pending merge — branch `docs/api-schemas-json-schema`).
- Related PRs (session cumulative on the verdict-schema track): mununu#484 (human-facing doc).
- Policy: [`../policies/cross-repo-impact.md`](../policies/cross-repo-impact.md).
- Sibling briefings: [`2026-08-antecedent-shadow-synth.md`](2026-08-antecedent-shadow-synth.md), [`2026-08-multi-atom-antecedent-shadow.md`](2026-08-multi-atom-antecedent-shadow.md), [`2026-08-no-antecedent-shadow-flag.md`](2026-08-no-antecedent-shadow-flag.md), [`2026-08-sv-mutate-param-api-parity.md`](2026-08-sv-mutate-param-api-parity.md).

## Not covered here (follow-ups)

- Schema files for BTOR2-direct verbs (`sv verify`, `sv verify-liveness`, `sv verify-recoverability`, `sv check-fsm`, `sv cegar`) and synth surfaces — the JsonSchema derives here cover `sv verify-auto` only. Adding the others is mechanical (matching derives + drift-detector rows). Landing next.
- OpenAPI / `utoipa` integration (full spec + `/openapi.json` endpoint) — not needed today; the per-response schema files serve every codegen. Can be added additively later without breaking the schema-file contract.
- Enum-typing for `outcome` / `kind` / `level` in the schemas (currently `type: "string"`). Would require the Rust source to switch from `String` to a serde-enum type, which is a wire-format shape change with a mandatory briefing. Not scoped for this PR.
