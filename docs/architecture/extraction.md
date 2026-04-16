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
