# CLI Reference

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change. We welcome feedback and bug reports via [GitHub Issues](https://github.com/vscorza/mununu/issues).

## Overview

Mununu provides a command-line interface for evaluating mu-calculus formulas, synthesizing controllers, generating graph visualizations, and managing Context DSL files. All commands are grouped under the `mununu context` subcommand (except `server`).

## Environment Variables

| Variable | Purpose | Example |
|----------|---------|---------|
| `RUST_LOG` | Control logging verbosity | `RUST_LOG=mununu=info mununu context eval ...` |

Set `RUST_LOG=mununu=debug` for detailed evaluation traces, or `RUST_LOG=mununu=trace` for full fixpoint iteration logging.

---

## `mununu context eval`

Evaluate a mu-calculus or LTL formula over a realized automaton.

```
mununu context eval <CONTEXT> [options]
```

**Description**: Parses the CTXDSL file, realizes all automata and formulas, then evaluates the specified formula over the specified automaton. Reports the set of states satisfying the formula.

**Required flags**:

| Flag | Description |
|------|-------------|
| `--formula <NAME>` | Name of the formula to evaluate (as declared in `mu_formulas`). Mutually exclusive with `--template`. |
| `--template <ID>` | Instantiate a [property template](Property-Templates) instead of selecting an existing formula. Mutually exclusive with `--formula`. |
| `--automaton <NAME>` | Name of the automaton or composition to evaluate over |

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--template-arg <KEY=VALUE>` | Template argument binding (repeatable). Requires `--template`. |
| `--sidecar <FILE>` | Additional CTXDSL sidecar files to merge (repeatable) |
| `--adapter <FORMAT>` | Translate from an external format before processing. Supported: `tlsf`, `aiger`, `btor2` (or `btor`), `promela`, `xstate`, `systemverilog` (or `sv`, hand-written parser), `sv-yosys` (or `yosys`, Yosys-driven elaboration), `extraction`, `auto` |
| `--no-partitions` | Disable guard partitioning during evaluation |
| `--print-structure [FILE]` | Print internal context structure to stdout or a file |
| `--print-ctxdsl [FILE]` | Print the intermediate CTXDSL (after adapter translation) to stdout or a file |

**Example**:

```bash
# Evaluate safety invariant over the handshake automaton
mununu context eval examples/hw/handshake.ctxdsl \
    --formula safety_invariant --automaton Handshake
```

```bash
# Evaluate with sidecar properties file
mununu context eval examples/counters/counters.ctxdsl \
    --sidecar examples/counters/counters_properties.ctxdsl \
    --formula safety_invariant --automaton Counter
```

```bash
# Evaluate using a property template (no need to write mu-calculus)
mununu context eval examples/game/player_fsm.espec.json \
    --adapter extraction \
    --template no_deadlock --automaton PlayerState

# Template with arguments
mununu context eval examples/game/quest_deadlock.espec.json \
    --adapter extraction \
    --template reachable --template-arg TARGET=AllComplete --automaton QuestProgress
```

```bash
# Evaluate a SystemVerilog design
mununu context eval design.sv --adapter sv \
    --formula safety --automaton handshake

# Evaluate an XState machine
mununu context eval machine.xstate --adapter xstate \
    --formula safety --automaton light
```

---

## `mununu context synth`

Synthesize a controller for a given automaton/formula pair.

```
mununu context synth <CONTEXT> [options]
```

**Description**: Computes the winning region for the specified formula using game-theoretic evaluation, then synthesizes a controller that restricts the automaton to states within the winning region. Reports whether the specification is realizable and outputs the controller if so.

**Required flags**:

| Flag | Description |
|------|-------------|
| `--formula <NAME>` | Name of the formula to synthesize a controller for |
| `--automaton <NAME>` | Name of the source automaton |

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--sidecar <FILE>` | Additional CTXDSL sidecar files to merge (repeatable) |
| `--no-partitions` | Disable guard partitioning during evaluation |
| `--minimize` | Apply bisimulation reduction to minimize the synthesized controller |
| `--counterexample` | Generate counterstrategy traces when unrealizable |
| `--deadlock-traces` | Capture traces leading to deadlock states |
| `--max-counter-traces <N>` | Cap the number of counterstrategy traces collected |
| `--extract-strategy` | Legacy flag — equivalent to `--controller-mode functional`. Kept for backwards compatibility. When `--controller-mode` is also provided, `--controller-mode` wins. |
| `--controller-mode <NAME>` | Controller extraction mode. One of `projection` (default), `functional`, `permissive`, `signature-memory`, `product-game`, `parity-game`. See [Controller Modes](Controller-Modes.md) for the full reference. |
| `--no-proof-obligations` | Skip proof obligation emission for violating initial states |
| `--adapter <FORMAT>` | Translate from an external format before processing. Supported: `tlsf`, `aiger`, `btor2` (or `btor`), `promela`, `xstate`, `systemverilog` (or `sv`, hand-written parser), `sv-yosys` (or `yosys`, Yosys-driven elaboration), `extraction`, `auto` |
| `--dump-json <FILE>` | Write a JSON summary of the synthesis result to a file |
| `--emit-dsl <FILE>` | Write the synthesized controller as a CTXDSL file |
| `--output-format <FORMAT>` | Output format for the synthesized controller: `ctxdsl` (default), `xstate`, `systemverilog`, `gdscript` |
| `--emit-native <FILE>` | Write the controller in the native format specified by `--output-format` |
| `--dump-diagnostics <FILE>` | Export diagnostics as a DSL sidecar file |
| `--print-structure [FILE]` | Print internal context structure to stdout or a file |
| `--print-ctxdsl [FILE]` | Print the intermediate CTXDSL (after adapter translation) to stdout or a file |

**Example -- realizable**:

```bash
# Synthesize a safety controller for the robot arm
mununu context synth tutorial/examples/06_controllability.ctxdsl \
    --formula safety_invariant --automaton RobotArm

# Synthesize with minimization and JSON output
mununu context synth examples/hw/arbiter.ctxdsl \
    --formula safety_invariant --automaton Arbiter \
    --minimize --dump-json result.json

# GR(1) / Buchi formula with memory-aware controller
mununu context synth examples/elevator_gr1.ctxdsl \
    --formula door_always_closes --automaton Elevator \
    --controller-mode product-game

# Full parity-game synthesis (correct for arbitrary alternation depth)
mununu context synth examples/elevator_gr1.ctxdsl \
    --formula door_always_closes --automaton Elevator \
    --controller-mode parity-game
```

**Example -- unrealizable with diagnostics**:

```bash
# Synthesize an impossible specification with full diagnostics
mununu context synth tutorial/examples/09_unrealizable.ctxdsl \
    --formula impossible_spec --automaton Valve \
    --counterexample --deadlock-traces

# Export the synthesized controller as CTXDSL
mununu context synth examples/hw/handshake.ctxdsl \
    --formula safety_invariant --automaton Handshake \
    --emit-dsl controller_output.ctxdsl

# Extract a positional strategy (one controllable transition per state)
mununu context synth examples/hw/arbiter.ctxdsl \
    --formula liveness_grant --automaton Arbiter \
    --extract-strategy
```

**Output format notes**: When `--counterexample` is used with liveness or GR(1) formulas, counterexample traces are reported in **lasso format** (e.g., `lasso trace #0: Red -> (PedWaiting)^ω`), showing a finite prefix followed by an infinitely repeating cycle.

---

## `mununu context graph`

Generate a Cytoscape.js HTML visualization of automata.

```
mununu context graph <CONTEXT> --output <FILE> [options]
```

**Description**: Parses the CTXDSL file and generates an interactive HTML graph visualization. The output uses Cytoscape.js for pan, zoom, and node inspection. States are shown as nodes, transitions as labeled edges.

**Required flags**:

| Flag | Description |
|------|-------------|
| `--output <FILE>` | Output file path for the HTML visualization |

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--sidecar <FILE>` | Additional CTXDSL sidecar files to merge (repeatable) |
| `--automaton <NAME>` | Restrict output to a single automaton |
| `--type <TYPE>` | Graph type: `dsl` (CTXDSL inferred, default), `unrolled` (internal after abstraction), or `both` |

**Example**:

```bash
# Generate graph for all automata
mununu context graph examples/amba_arbiter_gr1.ctxdsl \
    --output arbiter_graph.html

# Generate graph for a single automaton with unrolled view
mununu context graph examples/hw/traffic_light.ctxdsl \
    --output traffic.html --automaton TrafficLight --type both
```

---

## `mununu context summarize`

Emit a JSON summary of automata, predicates, controllers, and formulas.

```
mununu context summarize <CONTEXT> [options]
```

**Description**: Parses and realizes the CTXDSL file, then outputs a JSON summary listing all automata (with state and transition counts), formulas, controllers, and predicates.

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--sidecar <FILE>` | Additional CTXDSL sidecar files to merge (repeatable) |
| `--print-structure [FILE]` | Print internal context structure to stdout or a file |

**Example**:

```bash
mununu context summarize examples/hw/handshake.ctxdsl
```

---

## `mununu context merge`

Parse and validate multiple CTXDSL files, optionally copying them to an output directory.

```
mununu context merge <FILE...> --output <DIR> [options]
```

**Description**: Loads and validates one or more CTXDSL files (the first is treated as the main context, the rest as sidecars). Optionally copies the validated files to an output directory for deployment or archival.

**Required flags**:

| Flag | Description |
|------|-------------|
| `--output <DIR>` | Directory where validated files should be copied |

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--force` | Overwrite the output directory if it already exists |

**Example**:

```bash
mununu context merge main.ctxdsl properties.ctxdsl \
    --output build/ --force
```

---

## `mununu context predicates`

List guard predicates registered for the context.

```
mununu context predicates <CONTEXT> [options]
```

**Description**: Parses the CTXDSL file and lists all predicates (state-based guard expressions) defined in the context. Useful for inspecting which predicates are available for use in formulas.

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--sidecar <FILE>` | Additional CTXDSL sidecar files to merge (repeatable) |
| `--automaton <NAME>` | Restrict output to predicates for a single automaton |

**Example**:

```bash
mununu context predicates examples/hw/arbiter.ctxdsl --automaton Arbiter
```

---

## `mununu server`

Start the HTTP API server (requires the `api` feature).

```
mununu server [options]
```

**Description**: Starts a lightweight HTTP API server that exposes Mununu's verification and synthesis capabilities over REST endpoints. Useful for integration with web frontends (such as mununu-ui) and CI pipelines.

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--addr <ADDRESS>` | Server bind address (default: `127.0.0.1:8080`) |

**Example**:

```bash
# Start server on default port
mununu server

# Start on a custom address
mununu server --addr 0.0.0.0:9090
```

---

## Common Patterns

### Evaluate, then synthesize

```bash
# First check if the property holds
mununu context eval spec.ctxdsl --formula my_property --automaton MySystem

# If some states fail, synthesize a controller
mununu context synth spec.ctxdsl --formula my_property --automaton MySystem \
    --minimize --emit-dsl controller.ctxdsl
```

### Debug unrealizable specs

```bash
# Run synthesis with full diagnostics
RUST_LOG=mununu=info mununu context synth spec.ctxdsl \
    --formula failing_property --automaton MySystem \
    --counterexample --deadlock-traces --dump-json diag.json

# Visualize the automaton to understand the state space
mununu context graph spec.ctxdsl --output debug.html --automaton MySystem
```

### Sidecar workflow (separate properties from models)

```bash
# Model in one file, properties in another
mununu context eval model.ctxdsl --sidecar properties.ctxdsl \
    --formula my_property --automaton MySystem

# Merge for deployment
mununu context merge model.ctxdsl properties.ctxdsl --output deploy/
```

### Import from external formats and export controllers

```bash
# Import XState JSON, synthesize, export controller as XState JSON
mununu context synth machine.xstate --adapter xstate \
    --formula safety --automaton light \
    --output-format xstate --emit-native controller.json

# Import SystemVerilog RTL, synthesize, export as SV module
mununu context synth design.sv --adapter sv \
    --formula safety --automaton FSM \
    --output-format systemverilog --emit-native controller.sv

# Auto-detect format from extension
mununu context eval design.sv --formula safety --automaton FSM
```

See [Adapter Formats](Adapter-Formats.md) for supported formats and limitations.

---

## `mununu sv` — SystemVerilog Analysis Tools

### `mununu sv init`

Generate a skeleton `.mununu.json` annotation sidecar from a SystemVerilog module.

```bash
mununu sv init <FILE> [--output <FILE>] [--force]
```

| Flag | Description |
|------|-------------|
| `--output <FILE>` | Output path (default: `<stem>.mununu.json` next to the `.sv` file) |
| `--force` | Overwrite existing sidecar |

**Defaults:** 1-bit registers → `boolean`, enums → `enum` with variants, ≤4-bit → `bounded_counter`, >4-bit → `discover`. Includes a `safety` property placeholder and detected `localparam` values.

**Example:**
```bash
mununu sv init examples/systemverilog/fifo.sv
# → Generated sidecar: examples/systemverilog/fifo.mununu.json
#   3 signal(s), 3 input(s), 1 property/ies
```

### `mununu sv discover`

Discover significant register values via SMT analysis. Requires `--features smt` at build time.

```bash
mununu sv discover <FILE> [--annotation <FILE>] [--output <FILE>] [--max-values <N>]
```

| Flag | Description |
|------|-------------|
| `--annotation <FILE>` | Path to `.mununu.json` (default: auto-detected next to `.sv`) |
| `--output <FILE>` | Write updated sidecar to a different file |
| `--max-values <N>` | Max values per signal (default: 32) |

Finds concrete values that make guard conditions satisfiable — even through combinational logic (e.g., `assign y = x * 4; if (y == 12)` → discovers `x = 3`). Updates the sidecar's `discovered_values` section, preserving user-given variant names.

**Example:**
```bash
mununu sv discover examples/systemverilog/alu.sv
# → cmd — 5 value(s):
#     VAL_0 = 0 (SMT: guard (cmd == 0) at line 0)
#     VAL_1 = 1 (SMT: guard (cmd == 1) at line 0)
#     ...
# → Updated sidecar: examples/systemverilog/alu.mununu.json
```

See [RTL Verification Pipeline](RTL-Verification-Pipeline) for the full workflow.

## `mununu templates`

List available property templates. Templates provide parameterized mu-calculus formula patterns that can be used with `--template` in `eval` and `synth` commands.

```
mununu templates [options]
```

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--domain <DOMAIN>` | Filter templates by domain: `game`, `rtl`, `agentic`, `software`, `synthesis` |
| `--id <ID>` | Show details of a specific template |
| `--json` | Output as JSON |

**Examples**:

```bash
# List all templates
mununu templates

# Filter by domain
mununu templates --domain game

# Show template details
mununu templates --id reachable

# JSON output (for scripting)
mununu templates --json
```

See [Property Templates](Property-Templates) for the full catalog and usage guide.

---

## See Also

- [Adapter Formats](Adapter-Formats.md) -- supported external formats
- [Property Templates](Property-Templates.md) -- parameterized property patterns
- [Game Engine Integration](Game-Engine-Integration.md) -- game FSM verification
- [LTL Properties](LTL-Properties.md) -- writing temporal specifications
- [Controller Synthesis](Controller-Synthesis.md) -- synthesis concepts and examples
- [Hardware Verification Patterns](Hardware-Verification-Patterns.md) -- example models and properties
