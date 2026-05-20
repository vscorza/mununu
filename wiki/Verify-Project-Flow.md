# Verify Project Flow

> **Source of truth:** [`crates/mununu-core/src/verify/`](../crates/mununu-core/src/verify/) — surface: CLI+API+UI.

`mununu verify` is the **general N-source verification framework**. It accepts a `verify.toml` manifest listing any combination of supported sources (C firmware, SystemVerilog RTL, hand-authored CTXDSL, XState JSON, CrewAI / LangGraph JSON, microprograms, …), translates each source through its adapter, applies an alphabet-binding strategy, composes the results, and evaluates a list of properties. Returns a structured [`VerifyReport`](#report-shape) usable from the CLI, the HTTP API, and the web UI.

The codesign C+SV flow, the protocol-spec recipe, and the agentic verification examples are all **specialisations** of this same framework.

## Conceptual model

> **Source of truth:** [`verify::config::VerifyConfig`](../crates/mununu-core/src/verify/config.rs) — surface: CLI+API+UI.

A verification project is the tuple `(Sources, AlphabetBinding, Composition, Properties)`:

- **Sources** — N entries, each `{ id, adapter, files, options }`. The `adapter` field names a [`FormatAdapter`](../crates/mununu-core/src/adapter/mod.rs) implementation. Today's dispatch table: `ctxdsl`, `xstate`, `crewai`, `langgraph`, `sv-rtl`, `c-codesign`, `extraction`.

- **AlphabetBinding** — how labels across sources synchronise:
  - **`direct`** (default) — names must already match across sources.
  - **`renamings`** — explicit `{ from = "source_id.local_label", to = "canonical_label" }` map.
  - **`register_map`** — derives renamings from a register-map JSON sidecar via [`coupling::rendezvous_label_name`](../crates/mununu-core/src/codesign/coupling.rs); SV-rtl sources get an automatic post-process pass via [`verify::register_map_rewriter::derive_sv_renamings_from_register_map`](../crates/mununu-core/src/verify/register_map_rewriter.rs).

- **Composition** — `{ semantics: synchronous | asynchronous | superset, members: [...], name }`. Drives the existing `composition::compose` primitive.

- **Properties** — `[{ name, (template | formula), args, over }]`. Resolved via the builtin [`TemplateRegistry`](../crates/mununu-core/src/adapter/templates/builtin_templates.json) when a template id is given.

## Pipeline

> **Source of truth:** [`verify::orchestrator::verify_project`](../crates/mununu-core/src/verify/orchestrator.rs) — surface: CLI+API+UI.

```text
verify.toml --> parse + validate
              \--> for each source: dispatch_adapter
                       \--> adapter::partition::classify         (Phase A.3 — auto-COI)
                                  \--> Dropped signals → Ignored when no sidecar override
                                  \--> AdapterOutput.partition_summary captures the counts
                       \--> AdapterOutput.ctxdsl (+ state_valuations side-channel)
                       \--> apply_renamings (binding-driven)
                       \--> for sv-rtl + register_map binding:
                                apply derive_sv_renamings_from_register_map
              \--> assemble_unified_ctxdsl (N source bodies into one context)
              \--> parse + realize
                       \--> environment_for(automaton) wires CLTS state_valuations
                                into Environment::abstract_states when numeric (Phase A.3)
              \--> for each property: resolve template, evaluate via mu_calculus
                       \--> on-demand `signal == const` atoms resolve through
                            abstract_states + state-valuation binding (Phase A.3)
              \--> VerifyReport (includes per-source partition_summary)
```

## `verify.toml` schema

> **Source of truth:** [`verify::config::VerifyConfig`](../crates/mununu-core/src/verify/config.rs) — surface: CLI+API+UI.

```toml
[project]
name = "uart_codesign"
description = "UART firmware + RTL peripheral integrated verification."

[[sources]]
id = "fw"
adapter = "c-codesign"
files = ["firmware/firmware.c"]
options = {
  include_paths = ["firmware/include"],
  defines = ["F_CPU=64000000"],
  cmsis_stubs = true,
  register_map = "register_map.json",
  synthesize_automaton = true,
}

[[sources]]
id = "periph"
adapter = "sv-rtl"
files = ["rtl/uart_peripheral.sv"]

[alphabet]
strategy = "register_map"
register_map = "register_map.json"

[composition]
semantics = "asynchronous"
members = ["fw", "periph"]
name = "System"

[[properties]]
name = "no_double_start"
template = "label_blocked_in_state"
args = { STATE = "fw.Transmitting", LABEL = "wr_ctrl_tx_start" }
over = "System"

[[properties]]
name = "init_reachable"
formula = "mu X. (Init || <> X)"
over = "System"
```

## Automatic partition (Phase A.3)

> **Source of truth:** [`adapter::partition::classify`](../crates/mununu-core/src/adapter/partition/mod.rs) — surface: CLI+API.

SV (`sv-rtl`) and BTOR2 sources run an **automatic cone-of-influence
pass** during their `dispatch_adapter` translation. The pass walks the
frontend IR's dependency graph from property atoms and marks signals
outside the cone as `AbstractionType::Ignored`. The composition rule
with the sidecar:

- A signal explicitly listed in the `.mununu.json`'s `signals[]` /
  `inputs[]` always wins — the auto-partition never overrides a user
  declaration.
- A signal *absent* from the sidecar gets the partition's verdict.
  Most often `Kept` (in-cone); when `Dropped`, the bit-blaster pins
  the signal to a single value and emits an `AdapterWarning`.

The per-source `partition_summary` field on `VerifyReport.sources[*]`
carries the counts (kept / dropped / total) and bit-widths
(`state_bits_before` / `state_bits_after` on BTOR2) so users can read
the COI's reduction effect directly out of the report JSON.

Datapath UF substitution (combinational arithmetic collapsed to
uninterpreted functions per Andraus–Sakallah Reveal 2008) is reserved
for a follow-up; the `Datapath { uf_symbol }` variant on
`PartitionClass` is currently never produced.

See also:
[`docs/abstraction.md` §"Automatic cone-of-influence"](../docs/abstraction.md#automatic-cone-of-influence-phase-a3)
for the user-facing guidance.

## Report shape

> **Source of truth:** [`verify::report::VerifyReport`](../crates/mununu-core/src/verify/report.rs) — surface: CLI+API+UI.

```rust
pub struct VerifyReport {
    pub project: String,
    pub sources: Vec<SourceSummary>,           // id, adapter, resolved automaton
    pub composition: CompositionInfo,          // semantics, name, resolved members
    pub property_verdicts: Vec<PropertyVerdict>,
}

pub struct PropertyVerdict {
    pub name: String,
    pub formula_source: PropertyFormulaSource, // Inline | Template { id, args }
    pub formula: String,                       // concrete mu-calculus text
    pub over: String,
    pub satisfied: bool,
    pub total_states: usize,
    pub satisfying_states: usize,
    pub initial_states: Vec<String>,
    pub initial_satisfying: Vec<String>,
}
```

## CLI

> **Source of truth:** [`mununu-cli::handle_verify`](../crates/mununu-cli/src/main.rs) — surface: CLI.

```bash
mununu verify <verify.toml>                # human-readable verdict table
mununu verify <verify.toml> --json         # machine-readable VerifyReport
mununu verify <verify.toml> --strict       # exits non-zero on any violated property
```

Relative paths in the manifest resolve against the `verify.toml`'s parent directory.

## HTTP API

> **Source of truth:** [`api::handlers::verify_project_handler`](../crates/mununu-core/src/api/handlers.rs) — surface: API.

`POST /api/v1/verify`. Body accepts either a pre-parsed `config` JSON object **or** the raw `config_toml` string (exactly one — the handler 400s on both-set or neither-set):

```json
{
  "config_toml": "[project]\nname = \"Demo\"\n...",
  "base_dir": "/abs/path/to/project"
}
```

Returns the structured `VerifyReport`. `config_toml` lets thin clients (like the UI wizard) send the raw manifest without bundling a TOML parser.

## Web UI

> **Source of truth:** [`mununu-ui/src/components/extraction/ExtractionPanel.tsx`](../../mununu-ui/src/components/extraction/ExtractionPanel.tsx) — surface: UI.

The Extraction tab's domain selector exposes **Verify Project (verify.toml)** as a workflow. Drop the manifest, type the server-side base directory, click **Run Verify**. The UI POSTs `{ config_toml, base_dir }` to `/api/v1/verify` and renders the response through [`VerdictTable`](../../mununu-ui/src/components/extraction/VerdictTable.tsx).

## Examples

> **Source of truth:** [`examples/verify/`](../examples/verify/) — surface: CLI.

Each example ships a `verify.toml`, source files, a `validate.sh` script, and a byte-deterministic `transcript.txt`:

| Example | Shape |
|---|---|
| [`xstate_pair/`](../examples/verify/xstate_pair/) | Two XState machines composed asynchronously; direct alphabet binding |
| [`microprogram_plus_sv/`](../examples/verify/microprogram_plus_sv/) | Microcode listing + SV peripheral via direct binding |
| [`uart_codesign_chaotic/`](../examples/verify/uart_codesign_chaotic/) | C firmware + chaotic-stub peripheral via register-map binding |
| [`uart_codesign_protocol_spec/`](../examples/verify/uart_codesign_protocol_spec/) | C firmware + hand-authored CTXDSL protocol-spec peripheral |
| [`crewai_handoff/`](../examples/verify/crewai_handoff/) | Sequential 2-agent CrewAI crew |
| [`langgraph_workflow/`](../examples/verify/langgraph_workflow/) | Conditional-branching LangGraph StateGraph |

Reproduce any of them:

```bash
bash examples/verify/<example>/validate.sh
```

## See also

- [Agentic Adapters](Agentic-Adapters) — CrewAI + LangGraph adapter details
- [Adapter Formats](Adapter-Formats) — every adapter, its detection rules, and emitted CTXDSL shape
- [Property Templates](Property-Templates) — the template catalog used by the `template = "..."` form
