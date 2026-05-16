# Extraction Spec Adapter

> Source of truth: [`crates/mununu-core/src/adapter/extraction/`](../../crates/mununu-core/src/adapter/extraction/) and JSON schema at [`tools/extraction_spec_schema.json`](../../tools/extraction_spec_schema.json) — surface: CLI+API+UI

The extraction adapter translates `.espec.json` files into CTXDSL. These specs are produced by the extraction pipeline for analyzing real source code:

```
Source code → human extraction → JSON spec (.espec.json) → Rust adapter → CTXDSL
```

## Key features

- **Mode filtering.** Each transition can be tagged `"mode": "vulnerable"`, `"mode": "fixed"`, or `"mode": "both"` (default). The `--mode` CLI flag selects which transitions to include.
- **Declarative automata.** States, transitions, compositions, properties, and controllers are all declared in the JSON spec — no per-target code in the adapter.
- **Provenance tracking.** Source commit, file, line numbers, CVE references, and attack chains are preserved from the extraction spec through to CTXDSL comments.
- **Detection.** Content-based (`"extraction_spec_v1"` + `"model_config"`) and extension-based (`.espec.json`).

## Usage

```bash
mununu context eval spec.espec.json --mode vulnerable --formula safety --automaton Main
mununu context eval spec.espec.json --adapter extraction --mode fixed --formula safety --automaton Main
```

## Spec format

See `tools/extraction_specs/*.espec.json` in the `mununu-private` repo for examples (those specs are private because they reference CVE data — see CLAUDE.md → Private Files Policy).

The `model_config` section carries declarative automaton definitions with `states`, `transitions` (mode-filtered), `composition`, `properties` (with per-property `over` targets), and `controllers`.

## Property templates

Properties in `.espec.json`, `.mununu.json`, and XState `__mununu` blocks can use `template_ref` instead of raw `formula` to reference a named property template. Templates are parameterized mu-calculus patterns (e.g., `no_deadlock`, `reachable(TARGET)`, `bounded(OVERFLOW, UNDERFLOW)`). Resolution happens at adapter translation time — the emitter and evaluator see no difference.

See [`crates/mununu-core/src/adapter/templates/`](../../crates/mununu-core/src/adapter/templates/) for the registry and catalog.

## Validation

```bash
mununu extraction validate <spec.espec.json>   # schema + cross-reference check
mununu extraction check <spec.espec.json>      # validate + dry-run translation
```

## Claims integrity reminder

Per [`policies/claims-integrity.md`](../policies/claims-integrity.md): a model written from an extraction spec is only a finding about the real system if the spec's line anchors actually point at the cited source at the pinned commit. Specs without line anchors or with anchors that no longer resolve are pattern studies, not findings.
