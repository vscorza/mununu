---
name: parity-check
description: >
  Verifies a feature is wired consistently across mununu's three user-facing
  surfaces — CLI, HTTP API, and UI. Reports drift (a flag with no API peer,
  an API field absent from the UI client, a route with no UI consumer) and
  the inverse case (code added to one surface but not exercised by another).
  Use when reviewing changes to user-exposed features, or as a sub-step of
  domain-adequacy and review-orchestrator.
---

Run a CLI ↔ API ↔ UI parity check on $ARGUMENTS (a feature name, file list, or "changed files" if no args).

## Scope determination

If $ARGUMENTS is empty, derive scope from `git diff --name-only HEAD~1 HEAD`. Otherwise, treat $ARGUMENTS as either:
- A feature/flag name (e.g., `--extract-strategy`, `synthesize`, `eval`) — search all three surfaces for it.
- A file path or list — check whether any of the three surfaces touched by those files have peer changes on the other two.

## The three surfaces

| Surface | Files |
|---|---|
| **CLI** | `crates/mununu-cli/src/main.rs` (clap `Args` structs and command handlers) |
| **HTTP API** | `crates/mununu-core/src/api/handlers.rs` (request/response signatures), `crates/mununu-core/src/api/models.rs` (types), `crates/mununu-core/src/api/server.rs` (routes) |
| **UI** | `mununu-ui/src/api/endpoints.ts` (typed clients), `mununu-ui/src/hooks/useCtxdslEditor.ts`, `mununu-ui/src/hooks/useSummary.ts`, and any other hook/component that consumes the endpoint |

## Procedure

1. Identify which surface(s) the change or feature touches.
2. For each touched surface, search the other two for the corresponding flag/field/route. Use `grep` on:
   - the flag name (`--foo` → `foo` in clap derive)
   - the API field name (snake_case Rust ↔ camelCase TS)
   - the route path (`/context/synth` etc.)
3. For every gap, classify it:
   - **CLI-only**: legitimate if the flag is a developer convenience (e.g., one-shot `--adapter` that the API decomposes into `/import` + `/eval`). Justify in writing.
   - **API-only**: legitimate if the surface is intentionally programmatic (e.g., raw graph dumps consumed only by tooling).
   - **UI-only**: almost always a bug — UI affordances should map to a documented API + CLI verb.
   - **CLI + API but no UI**: acceptable for power-user features; flag for follow-up.
   - **Inverse drift**: code added to one surface but not exercised by either of the others — call out.

## Report format

Emit a Markdown table. The caller (an agent or a review report) can quote it verbatim.

```markdown
### CLI / API / UI Parity

| Change / feature | CLI | API | UI | Drift? | Notes |
|---|---|---|---|---|---|
| `--extract-strategy` | ✓ main.rs:L | ✓ handlers.rs:L | ✗ | YES | API exposes `extract_strategy` field; UI client missing |
| `/context/synthesize` returns `counterstrategy` | ✓ | ✓ models.rs:L | ✓ endpoints.ts:L | none | aligned |
```

Each row must cite a file:line reference for any "✓" entry. "✗" entries should explain whether the gap is intentional (with one-line justification) or unintentional (with a recommended fix).

## When invoked as a sub-step

If the caller is `review-orchestrator` or `domain-adequacy`, return only the Markdown table block — no preamble, no closing summary. The caller embeds it directly under a `### CLI / API / UI Parity` section in its larger report.

When invoked standalone, prepend a one-paragraph executive summary: number of changes in scope, number with drift, number aligned.

## Important

- A pre-existing parity gap discovered during the audit is itself a finding. Report it under the same table even if it predates the current change.
- New examples or workflows added under `examples/` should be reachable from all three surfaces (loadable from the UI editor, summarizable via CLI, verifiable via API). If only one surface works, treat it as an incomplete deliverable.
- Do not modify any source file. This skill is read-only.
