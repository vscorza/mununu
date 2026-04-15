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
| **Transitions** | — | **Derived from method guards × effects** |
| **Labels** | — | **Derived from method names** |
| Composition | `composition` | — |
| Properties | `properties` | — |

## Domain Profiles

Domain profiles provide non-trivial defaults for controllability, abstraction, and composition based on the application domain. Available profiles:

| Profile | Language | Controllability default | Composition | Key heuristic |
|---------|----------|------------------------|-------------|---------------|
| `mcp_server` | TypeScript | Uncontrollable (client requests are nondeterministic) | Asynchronous | `handle*`, `on*` → uncontrollable; `start`, `close`, `send` → controllable |
| `protocol_implementation` | Rust | Controllable (caller controls API) | Synchronous | `pub fn` → controllable; `handle_*`, `on_*` → uncontrollable |
| `python_server` | Python | Uncontrollable | Asynchronous | `_*` prefix → internal; `handle_*` → uncontrollable |
| `hardware_rtl` | SystemVerilog | By port direction | Synchronous | Input ports → uncontrollable; output → controllable |

## State Space Derivation

Given fields `[f1: bool, f2: bool, f3: bounded_counter(0..2)]`:

1. **Enumerate**: cross-product of domains = 2 × 2 × 3 = 12 abstract states
2. **Name**: from field values — `f1_T_f2_F_f3_0`, `f1_T_f2_F_f3_1`, etc.
3. **Initial state**: from field initial values
4. **For each method**: parse guards (conditions on state fields) + effects (assignments). For each state where guards are satisfied, add transition to state after effects.
5. **Prune**: BFS from initial state — remove unreachable states
6. **Noop**: add self-loops for asynchronous composition interleaving

## Guard Extraction: Limitations

The tree-sitter extractor uses heuristics for guard detection:

- **Early-return pattern**: `if (this.field) { throw/return }` → guard is INVERTED (field must be false for method to proceed)
- **Simple boolean checks**: `if (this.field)` → `MustBeTrue`; `if (!this.field)` → `MustBeFalse`
- **Nested conditions**: Only the first field reference is captured. `if (this.a && !this.b)` captures `a` only.
- **Indirect guards**: `const x = this.field; if (x)` → NOT detected (no data flow analysis)
- **Method call guards**: `if (this.map.has(key))` → NOT detected (requires call summary integration)

Over-approximation: when guards cannot be determined, the method fires from all states. This is sound for safety properties.

## Call Summaries

External library calls (Map.set, Vec.push, dict.clear) need summaries to know their effect on model state. Built-in summaries cover:

- **TypeScript**: Map, Set, Array, console, crypto
- **Python**: dict, list, set
- **Rust**: HashMap, Vec, Option

Unknown calls default to over-approximation (nondeterministic effect on any state field passed as argument).

## Extraction Frontends

### Tree-sitter (current — TypeScript, Python, Rust)

```bash
mununu-extract config.extract.json --source server.ts --output spec.espec.json
```

Parses source AST, extracts fields/methods/guards/effects, derives automata. Lightweight, no external dependencies beyond the Rust toolchain.

### LLVM IR + SVF (planned — C, C++, Rust)

For compiled languages, LLVM IR preserves semantic information (SSA form, resolved types, inlined functions) that tree-sitter cannot access. SVF provides call graph + points-to analysis for resolving indirect calls.

### CIRCT (planned — SystemVerilog)

For hardware, CIRCT's `fsm` dialect provides explicit state machine representations extracted from RTL via Slang parsing and MLIR lowering.

### Manual (.espec.json)

For any language, a human can write the `.espec.json` directly. The existing extraction adapter (`mununu context eval spec.espec.json`) processes it.
