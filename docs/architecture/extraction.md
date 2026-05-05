# Layer 1: Extraction

Extraction converts source code into per-component automata. The output is `.espec.json` — a declarative specification of states, transitions, properties, and source provenance.

## The Two Inputs

Every extraction takes:
1. **Source file** — the actual code (TypeScript, Python, Rust, C, SystemVerilog)
2. **Extraction config** (`.extract.json`) — declares WHAT to extract, not HOW the automaton looks

The config is what the user (or agent) writes. The tool derives the automaton topology.

## What the Config Declares vs What the Tool Derives

| Aspect | Config (user writes) | Tool derives |
|--------|---------------------|--------------|
| Which class/struct to analyze | `targets[].class` | — |
| Which fields are state | `targets[].state_fields` | Field types, lines, initial values |
| Abstraction per field | `targets[].abstraction_overrides` | — (uses domain profile defaults) |
| Which methods to model | `targets[].methods.include` | Method line ranges, guards, effects |
| Controllability | `targets[].controllability_overrides` | — (uses domain profile defaults) |
| **States** | — | **Derived from field cross-product** |
| **Transitions** | — | **Derived from method guards x effects** |
| **Labels** | — | **Derived from method names** |
| Composition | `composition` | — |
| Properties | `properties` | — |

## Domain Profiles

Domain profiles provide non-trivial defaults for controllability, abstraction, and composition based on the application domain. Available profiles:

| Profile | Language | Controllability default | Composition | Key heuristic |
|---------|----------|------------------------|-------------|---------------|
| `mcp_server` | TypeScript | Uncontrollable (client requests are nondeterministic) | Asynchronous | `handle*`, `on*` -> uncontrollable; `start`, `close`, `send` -> controllable |
| `protocol_implementation` | Rust | Controllable (caller controls API) | Synchronous | `pub fn` -> controllable; `handle_*`, `on_*` -> uncontrollable |
| `python_server` | Python | Uncontrollable | Asynchronous | `_*` prefix -> internal; `handle_*` -> uncontrollable |
| `hardware_rtl` | SystemVerilog | By port direction | Synchronous | Input ports -> uncontrollable; output -> controllable |

## State Space Derivation

Given fields `[f1: bool, f2: bool, f3: bounded_counter(0..2)]`:

1. **Enumerate**: cross-product of domains = 2 x 2 x 3 = 12 abstract states
2. **Name**: from field values, separated by `_` — `f1_T_f2_F_f3_0`, `f1_T_f2_F_f3_1`, etc.
3. **Initial state**: from field initial values
4. **For each method**: parse guards (conditions on state fields) + effects (assignments). For each state where guards are satisfied, add transition to state after effects.
5. **Prune**: BFS from initial state — remove unreachable states
6. **Noop**: add self-loops for asynchronous composition interleaving

### State Naming Convention

State names are constructed from field names and their abstract values, separated by `_`:
- `field1_value1_field2_value2` (e.g., `started_T_closed_F`)
- Values: `T`/`F` for boolean, `Some`/`None` for presence, `0`/`1`/... for counter, variant name for enum

When field names start with `_` (Python convention), the natural concatenation produces double underscores: `_active_T__rate_limited_F`. This is expected behavior, not a bug. Formulas must reference the actual generated state names.

## Guard Extraction

The tree-sitter extractor detects guards from if-statements in method bodies:

### Supported Patterns

- **Simple boolean check**: `if (this.field)` / `if self.field:` / `if self.field` -> guard `MustBeTrue`
- **Negated check**: `if (!this.field)` / `if not self.field:` -> guard `MustBeFalse`
- **Early-return pattern**: `if (this.field) { throw/return }` -> guard is INVERTED. The method proceeds only when the field is false, because the early exit fires when it's true.

### Known Limitations

| Limitation | Status | Impact |
|-----------|--------|--------|
| Nested conditions: `if (a && !b)` captures only first field | To be fixed (Phase 1) | Under-approximation |
| Indirect guards: `const x = this.field; if (x)` | To be fixed (Phase 1) | Under-approximation |
| Method call guards: `if (this.map.has(key))` | To be fixed (Phase 1) | Missing collection guards |
| Over-approximation: undetected guards allow method from all states | By design | Sound for safety properties |
| Under-approximation: unknown effects keep current value | To be fixed (Phase 1) | Unsound for liveness |

## Call Summaries

External library calls (Map.set, Vec.push, dict.clear) need summaries to know their effect on model state. Built-in summaries cover:

- **TypeScript**: Map, Set, Array, console, crypto
- **Python**: dict, list, set
- **Rust**: HashMap, Vec, Option

Unknown calls default to over-approximation (nondeterministic effect on any state field passed as argument).

## Extraction Frontends

### Tree-sitter AST (TypeScript, Python, Rust)

```bash
mununu-extract config.extract.json --source server.ts --output spec.espec.json
```

Parses source AST, extracts fields/methods/guards/effects, derives automata. Lightweight, no external dependencies beyond the Rust toolchain.

**Language-specific field detection:**

| Language | Field source | Type inference |
|----------|-------------|----------------|
| TypeScript | Class property declarations (`public_field_definition`) | From type annotation (`: boolean`) |
| Python | `__init__` body assignments (`self.field = value`) | From RHS value (`True`/`False` -> bool, `{}` -> dict, `[]` -> list, `None` -> optional, `0` -> int) |
| Rust | Struct field declarations (`field_declaration`) | From type annotation (`: bool`, `: Option<T>`, `: HashMap<K,V>`) |

### LLVM IR (C, C++, Rust)

```bash
rustc --edition 2021 --crate-type=lib --emit=llvm-ir source.rs -o source.ll
python3 tools/llvm_extract.py source.ll --output spec.espec.json
```

For compiled languages, LLVM IR preserves semantic information (SSA form, resolved types, inlined functions) that tree-sitter cannot access. Current implementation identifies struct types and method functions but produces self-loop transitions (no dataflow analysis yet). SVF integration planned for call graph + points-to analysis.

### CIRCT (SystemVerilog)

```bash
circt-verilog design.sv | python3 tools/circt_extract.py --output spec.espec.json
```

For hardware, CIRCT provides MLIR representation of RTL designs. The extraction builds a reactive system model: every flip-flop (`seq.firreg`) is a state dimension, input signals are uncontrollable labels, and each clock step is a synchronous transition. Current implementation identifies state registers and comparisons but needs improvement for next-state function tracing through mux chains.

### Manual (.espec.json)

For any language, a human can write the `.espec.json` directly. The existing extraction adapter (`mununu context eval spec.espec.json`) processes it.

## Verification of Extraction Accuracy

Each language frontend has an end-to-end system test:

```bash
bash tests/system/extract_and_verify.sh
```

This test covers:
1. **TypeScript** (`sample_server.ts`): 3 boolean fields -> 8 states, 28 transitions. Property `no_requests_after_close` FAILS (handler doesn't check `_closed`).
2. **Python** (`sample_handler.py`): 2 boolean fields -> 4 states, 20 transitions. Property `no_requests_when_rate_limited` FAILS (handler doesn't check rate limit).
3. **Rust** (`sample_protocol.rs`): 2 boolean fields -> 4 states, 16 transitions. Property `no_send_after_close` FAILS (send doesn't check `closed`).
4. **SystemVerilog** (`handshake.sv`): Native adapter. Property `safety` HOLDS.

All property violations are intentional — they demonstrate the tool's ability to detect missing guards in real code patterns.

## Compositional extraction

Single-class extraction produces one automaton from one source class. **Compositional extraction** stitches multiple instances of one or more classes into a multi-automaton model, applying per-instance label rewriting so the existing CLTS composition engine can interleave them correctly.

### When to use it

The canonical use case is **concurrency / race-condition modeling**: N workers contending for one shared resource. Most agentic and MCP-server bugs that show up in the prospector backlog have this shape — MCP-001 (LangGraph parallel-interrupt = 2 ToolCalls + 1 ToolNode), MCP-002 (langgraphjs AsyncLocalStorage = 2 Workers + 1 Provider), MCP-005 (mcp-server-memory file race = 2 Workers + 1 file). Pre-compositional-extraction these were hand-modeled.

### Configuration

Add an `instances` and (optionally) `shared` field to the `composition` block in the extract config:

```json
{
  "$schema": "extraction_config_v1",
  "domain": "mcp_server",
  "language": "typescript",
  "source": { "file": "src/memory/index.ts" },
  "targets": [
    { "class": "KnowledgeGraphManager", "state_fields": ["state"], "methods": { "include": ["load", "save"] } }
  ],
  "composition": {
    "type": "asynchronous",
    "name": "memory_write_race",
    "instances": [
      { "of": "KnowledgeGraphManager", "as": "worker_a" },
      { "of": "KnowledgeGraphManager", "as": "worker_b" }
    ],
    "shared": ["ev_save"]
  }
}
```

The `instances` array declares the topology. Each entry has `of` (class name to scan, must match a `target.class`) and `as` (instance name, becomes the automaton id). The `shared` array names the labels that synchronize across instances.

### Label rewriting semantics

For each instance `<name>` of class `<C>`:

1. Run single-class extraction on `<C>` (the existing post-GAP-005 pipeline, unchanged).
2. Set the resulting automaton's id to `<name>`.
3. Rewrite every label `L` to `<name>__<L>`, **except** labels listed in `composition.shared`. Shared labels are kept verbatim across all instances.

The composition engine at [crates/mununu-core/src/composition/mod.rs](../../crates/mununu-core/src/composition/mod.rs) already enforces the rest: shared labels (alphabet intersection) fire jointly across instances; disjoint labels interleave. **Prefix-by-default makes the safe choice (independence) the no-config behavior** — users opt in to synchronization by listing labels.

### Worked example: MCP-005 file race

For the `KnowledgeGraphManager` class with methods `load` and `save`, the config above produces:

| Instance | Labels |
|---|---|
| `worker_a` | `worker_a__ev_load`, `ev_save` (shared) |
| `worker_b` | `worker_b__ev_load`, `ev_save` (shared) |

The composition's alphabet intersection is `{ev_save}` — the synchronization point. The async-composition engine interleaves the prefixed `*__ev_load` labels (worker-internal work) and forces both workers' `ev_save` to fire jointly, producing the canonical race topology.

### Important notes

- **The label prefix from the domain profile applies first.** The `mcp_server` profile prefixes events with `ev_`, so `composition.shared` should list `ev_save` (not `save`). The label-rewriting comparison uses the post-prefix label name.
- **`noop` is never shared.** The state-space engine adds noop self-loops to keep states reachable; if they synchronized across instances, no instance could ever progress independently. The rewriter explicitly excludes `noop` from sharing regardless of the user's `shared` list.
- **Class-to-instance lookup is by name.** Each `instance.of` must match a `target.class` declared in the same config. An instance referencing an unknown class is a hard error, not a silent skip.
- **The `members` field of the resulting espec is auto-populated from the resolved instance names** — users don't write it manually when using `instances`.

### CLI / API access

```bash
# CLI: extract using a compositional config (no new flags needed)
mununu-extract ast manager_compose.extract.json --source src/memory/index.ts \
  --output composed.espec.json

# CLI: list supported composition modes inline
mununu-extract ast --list-composition-modes /dev/null --source /dev/null

# API: same shape as single-class extraction
curl -X POST http://localhost:8080/api/v1/extraction/extract \
  -H "Content-Type: application/json" \
  -d '{"config": "...", "source": "...", "language": "typescript"}'

# API: list composition modes
curl http://localhost:8080/api/v1/extraction/composition-modes
```

### Concurrency property templates

Five concurrency-specific property templates ship in the agentic / universal domains for use over compositional extractions:

| Template | Use |
|---|---|
| `no_clobber` | Safety: shared resource never enters a corrupt state. |
| `clobber_reachable` | Liveness witness: corruption state is reachable. Pair with `no_clobber` to confirm the safety property is non-vacuous. |
| `mutual_exclusion_3` | 3-way pairwise exclusion. (`mutual_exclusion` already covers 2-way.) |
| `bounded_handoff` | Handoff request is eventually completed. |
| `no_lost_update` | Every started write becomes externally visible. |

List them: `mununu templates --domain agentic`.

> **GAP-010 — template freshness caveat:** the template registry loads `crates/mununu-core/src/adapter/templates/builtin_templates.json` at compile time via `include_str!`. New templates added to that file only appear in the binary's catalog after `cargo build`. If `mununu templates` is missing a template you just added, rebuild the binary first (`cargo build --release -p mununu-cli`). This applies to API listings (`GET /api/v1/templates`) and UI template pickers as well — they all read from the same compile-time-embedded registry.

### Verification end-to-end

After extraction, the composition is verified with the existing toolchain:

```bash
mununu context eval composed.espec.json --formula no_clobber --automaton memory_write_race
mununu context eval composed.espec.json --formula clobber_reachable --automaton memory_write_race
```

The verdicts confirm the bug witness. For MCP-005, `no_clobber` fails (0/1 initials) and `clobber_reachable` holds (1/1) — the race is reachable.

### Phase B — pattern-based auto-detection

Phase B layers a **suggestion-grade** pre-pass on top of Phase A. The detector at `crates/mununu-core/src/adapter/extraction/ast_extract/concurrency_detect.rs` walks the tree-sitter AST and surfaces known concurrency idioms:

| Detector | Fires on | Output |
|----------|----------|--------|
| `python_asyncio_gather` | `asyncio.gather(a(), b(), …)` | branch count + `task_<i>` instance names |
| `typescript_promise_all` | `Promise.all([…])` / `Promise.allSettled([…])` | branch count + `task_<i>` instance names |

Each finding is a `DetectedConcurrency` record:

```json
{
  "detector_id": "python_asyncio_gather",
  "description": "asyncio.gather over 2 coroutine(s)",
  "line": 254,
  "branch_count": 2,
  "suggested_instance_names": ["task_0", "task_1"],
  "suggested_class_hint": null
}
```

**Three entry points:**

| Surface | Invocation |
|---------|-----------|
| CLI | `mununu-extract ast <config> --source <file> --propose-composition` |
| HTTP | `POST /api/v1/extraction/propose-composition` with `{source, language}` |
| Web UI | `Suggest from source (Phase B)` button on the compose-step `CompositionEditor` |

**Phase B is suggestion-only.** The output is a *list of starting points*, not a finished `composition.instances[]` block. The user reviews each finding, edits instance names to fit the domain, and adds the `shared[]` labels that name the resource the instances contend over. The Phase A engine still does the actual label-rewriting and composition.

**Validation against the prospector backlog** (2026-05-02 run on the staging fixtures):

| Fixture | Source | Expected | Detector output | Notes |
|---------|--------|----------|-----------------|-------|
| MCP-001 | `tool_node_prefix.py` | LangGraph parallel-interrupt; dynamic gather | `1 finding` (`asyncio.gather` at line 254, dynamic args) | Branch count `null` because the gather is over `*coros` — correct conservative output. |
| MCP-002 | `pregel_index.ts` | langgraphjs AsyncLocalStorage; `Promise.all` over mapped tasks | `2 findings` (lines 2144, 2155) | Both `null` branch counts — non-literal iterables. Detector flags both call sites. |
| MCP-005 | `index.ts` (mcp-server-memory) | File race via two unsynchronized `writeFile` calls | `0 findings` | **Correctly silent** — MCP-005's race is implicit (no `gather` / `Promise.all`); the contention is in the host that calls the manager twice in flight. Phase B does not produce false positives on this shape; the user must hand-author `composition.instances[]` for it. |

The MCP-005 result is the most informative: silence on a real race witness is the **expected** behavior at the current detector layer (B1, syntactic). Recovering this case requires shared-resource inference (B3) and resource-shape detection — explicitly out of scope for the initial Phase B layer per `~/.claude/plans/phase-b-auto-detection.md`.

### Out of scope (Phase B+ follow-ups)

- **B2 — shared-label inference**: today the suggested config has empty `shared`. A future layer can mine module-level mutable state (file paths, channels, registries) read or written by multiple instances and propose them as candidate sync points.
- **B3 — resource-shape detection**: the MCP-005-style implicit race (two unsynchronized writers) requires reasoning about the *call graph at the caller*, not the callee. Filed as a separate gap.
- **B4 — Query DSL / IR**: detectors currently use direct cursor walks. Migrating to a tree-sitter Query DSL or a small IR would simplify adding new idioms (`multiprocessing.Process`, `tokio::spawn`, `std::thread::spawn`).
- **B5 — Andersen-style points-to**: cross-file concurrency analysis requires a points-to layer the cursor-walk doesn't have. Out of scope for this iteration.
