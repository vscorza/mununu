# `sv_multi_module` — multi-file source dispatch demo

> **Source of truth:** [`crates/mununu-core/src/verify/orchestrator.rs`](../../../crates/mununu-core/src/verify/orchestrator.rs) (`dispatch_sv_rtl`, `read_additional_files`) — surface: CLI+API+UI.

The verify framework now passes every file listed in `[[sources]].files` to the adapter, not just `files[0]`. When the SystemVerilog adapter receives a primary file that looks like a multi-module sidecar (`$schema = "mununu_sv_multi_v1"`), it routes through `SystemVerilogAdapter::translate_multi_module_content` and reads each module's SV from the additional files.

## What it demonstrates

- **Multi-file source dispatch (Stream A1).** One `[[sources]]` declaration consumes a sidecar JSON + N SystemVerilog modules.
- **Schema-driven adapter routing.** The sv-rtl dispatcher detects the multi-module marker and switches to the in-memory multi-module entry point automatically; no separate `adapter = "sv-yosys"` or `adapter = "sv-multi"` setting needed.
- **Warning, not error, on extra files for single-file adapters.** A `tracing::warn!` records the dropped extras when a single-file adapter (xstate, crewai, langgraph, microcode, ctxdsl, extraction, c-codesign) receives `files = [primary, extra1, extra2]` — the verify run still succeeds against the primary file.

## Files

| File | Purpose |
|---|---|
| `system.mununu.json` | Multi-module sidecar declaring two modules: `producer` + `consumer`. References each module's SV source by filename. |
| `producer.sv` | RTL for the producer module (3-state FSM). |
| `consumer.sv` | RTL for the consumer module (3-state FSM). |
| `verify.toml` | Single source declaration with `files = [sidecar, producer.sv, consumer.sv]` |
| `validate.sh` | End-to-end reproduction |
| `transcript.txt` | Byte-deterministic expected output |

## Reproduce

```bash
bash examples/verify/sv_multi_module/validate.sh
```

## Authoring shape

For any SV multi-module composition under the verify framework, the convention is:

```toml
[[sources]]
id = "system"
adapter = "sv-rtl"
files = [
    "system.mununu.json",   # multi-module sidecar (mununu_sv_multi_v1 schema)
    "module_a.sv",
    "module_b.sv",
    # ... one entry per module
]
```

The sidecar's `modules[*].source` fields reference each SV by filename (basename); the orchestrator builds the source-name → content map from the additional-files list and hands it to `translate_multi_module_content`.

## What this slice does not cover

- **Multiple C source files** under `c-codesign`. The current dispatcher logs the dropped files via the warn helper. A multi-translation-unit C extraction is queued as a v2 — would need the LLVM-IR extractor to accept and link multiple translation units.
- **Adapter-output-as-additional-file**. Today every additional file is text fed through `std::fs::read_to_string`. Future binary adapters (e.g., a `.btor2` byte-stream additional input) would need a different ingest path.
- **Cross-source file references**. A `verify.toml` source can reference files in its own `files` list, but not files belonging to another source. Each source is self-contained.
