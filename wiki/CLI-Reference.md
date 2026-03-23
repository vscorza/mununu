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
| `--formula <NAME>` | Name of the formula to evaluate (as declared in `mu_formulas`) |
| `--automaton <NAME>` | Name of the automaton or composition to evaluate over |

**Optional flags**:

| Flag | Description |
|------|-------------|
| `--sidecar <FILE>` | Additional CTXDSL sidecar files to merge (repeatable) |
| `--no-partitions` | Disable guard partitioning during evaluation |
| `--print-structure [FILE]` | Print internal context structure to stdout or a file |

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
| `--no-proof-obligations` | Skip proof obligation emission for violating initial states |
| `--dump-json <FILE>` | Write a JSON summary of the synthesis result to a file |
| `--emit-dsl <FILE>` | Write the synthesized controller as a CTXDSL file |
| `--dump-diagnostics <FILE>` | Export diagnostics as a DSL sidecar file |
| `--print-structure [FILE]` | Print internal context structure to stdout or a file |

**Example -- realizable**:

```bash
# Synthesize a safety controller for the robot arm
mununu context synth tutorial/examples/06_controllability.ctxdsl \
    --formula safety_invariant --automaton RobotArm

# Synthesize with minimization and JSON output
mununu context synth examples/hw/arbiter.ctxdsl \
    --formula safety_invariant --automaton Arbiter \
    --minimize --dump-json result.json
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
```

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

## See Also

- [LTL Properties](LTL-Properties.md) -- writing temporal specifications
- [Controller Synthesis](Controller-Synthesis.md) -- synthesis concepts and examples
- [Hardware Verification Patterns](Hardware-Verification-Patterns.md) -- example models and properties
