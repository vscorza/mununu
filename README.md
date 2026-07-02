# Mununu

**Formal verification and controller synthesis for reactive systems**

[![CI](https://github.com/vscorza/mununu/actions/workflows/ci.yml/badge.svg)](https://github.com/vscorza/mununu/actions/workflows/ci.yml)
[![Rust 1.91+](https://img.shields.io/badge/rust-1.91%2B-orange.svg)](https://www.rust-lang.org/)
[![License: Non-Commercial](https://img.shields.io/badge/license-Non--Commercial-blue.svg)](LICENSE)

**[Video Tutorials](https://www.youtube.com/watch?v=PovNx1rWOy8&list=PL8lIyan4cdjWOUZy32IKu4Yc3Ivi1_YLZ)** &mdash; Watch the Mununu tutorial series on YouTube

## What is Mununu?

Mununu is a verification tool for analyzing and synthesizing controllers for reactive systems modeled as Compositional Labeled Transition Systems (CLTS). It evaluates mu-calculus and LTL properties, performs controller synthesis, and supports compositional modeling of concurrent systems through a dedicated DSL.

## Features

- **CTXDSL** &mdash; A domain-specific language for defining automata, compositions, properties, and controllers with enum types, parameterized automata (process templates), and state groups/wildcards
- **Mu-calculus evaluation** &mdash; Fixpoint-based property verification with bitvec-backed state sets
- **LTL support** &mdash; Linear temporal logic formulas automatically translated to mu-calculus
- **GR(1) specifications** &mdash; Generalized Reactivity(1) properties for reactive synthesis
- **Composition** &mdash; Synchronous, asynchronous, and superset composition of CLTS components
- **Controller synthesis** &mdash; Automatic synthesis of controllers satisfying safety and liveness properties
- **State abstraction** &mdash; Multi-level variable abstraction (Boolean, integer intervals, symbol sets)
- **Format adapters** &mdash; Import from TLSF, AIGER, Promela, XState, SystemVerilog, **CrewAI** (`.crewai.json`), and **LangGraph** (`.langgraph.json`), plus extraction-spec inputs from C / Rust / TypeScript; export controllers to XState JSON or SystemVerilog
- **N-source verify framework** &mdash; `mununu verify <verify.toml>` composes any combination of the above sources, picks an alphabet-binding strategy (direct / explicit renamings / register-map-derived), declares a composition shape, and produces a structured `VerifyReport`. See [`wiki/Verify-Project-Flow.md`](wiki/Verify-Project-Flow.md).
- **REST API** &mdash; Built-in HTTP server for integration with web frontends
- **Web UI** &mdash; Interactive editor, graph visualization, and verification via [mununu-ui](https://github.com/vscorza/mununu-ui)

## Quick Start

```bash
# Build from source
git clone https://github.com/vscorza/mununu.git
cd mununu
cargo build --release

# Verify a handshake protocol
cargo run -- context eval examples/hw/handshake.ctxdsl \
  --formula ack_reachable --automaton Handshake

# Synthesize a controller (six extraction modes; see wiki/Controller-Modes.md)
cargo run -- context synth examples/hw/handshake.ctxdsl \
  --formula safety_invariant --automaton Handshake \
  --controller-mode parity-game

# Generate a graph visualization
cargo run -- context graph examples/hw/handshake.ctxdsl \
  --output handshake.html

# Start the API server (for use with mununu-ui)
cargo run --features api -- server --addr 127.0.0.1:8080
```

## Examples

The `examples/` directory contains ready-to-use CLTS specifications:

| Example | Description | States | Properties |
|---------|-------------|--------|------------|
| [Handshake Protocol](examples/hw/handshake.ctxdsl) | Req/Ack handshake with latency | 4 | Safety, reachability, liveness |
| [Round-Robin Arbiter](examples/hw/arbiter.ctxdsl) | 2-client mutual exclusion arbiter | 3 | Mutual exclusion, fairness |
| [Traffic Light](examples/hw/traffic_light.ctxdsl) | Timer-driven 4-phase controller | 4 | Cycle reachability, transition checks |
| [Ready/Valid Adapter](examples/hw/rv_adapter.ctxdsl) | Skid buffer for backpressure | 2 | Buffer reachability, drain/push |
| [AMBA Bus Arbiter](examples/amba_arbiter_gr1.ctxdsl) | 2-client GR(1) bus arbiter | 3+3+3 | GR(1) mutual exclusion, no starvation |
| [Fair Elevator](examples/elevator_gr1.ctxdsl) | 3-floor GR(1) elevator controller | 6+4 | Floor reachability, full traversal |
| [Sterile Batch Release](examples/sterile_batch_release.ctxdsl) | Pharmaceutical BPM pipeline | 14 | Ship-requires-release, disposition |
| [AMBA 4-Client Synthesis](examples/amba_arbiter_gr1_synthesis.ctxdsl) | 4-client arbiter with synthesis | 5+12 | 6 mutual exclusion pairs, GR(1) liveness |
| [Bus Arbiter Retry](examples/bus_arbiter_retry.ctxdsl) | Recurrence-after-stability (alt 4) | 4 | Demonstrates ParityGame mode beyond GR(1) — see [Controller Modes](https://github.com/your-org/mununu/wiki/Controller-Modes) |
| [Dual-Channel Arbiter](examples/dual_arbiter_alt4.ctxdsl) | Two channels, dual recurrence-after-stability (alt 4) | 16 | Memory + strategy selection — synthesis disables 17 controllable transitions; ParityGame keeps the parity-correct strategy |

### Agentic AI Orchestration

| Example | Description | Composition | Properties |
|---------|-------------|-------------|------------|
| [Customer Support Pipeline](examples/agentic/support_pipeline.xstate.json) | XState parallel: triage + budget tracking | 5+2 (parallel) | No tool over budget (safety) |
| [MCP Tool Authorization](examples/agentic/mcp_auth.ctxdsl) | Session + confirmation gate protocol | 3+4 (async) | Session-required, confirm-before-delete |
| [Multi-Agent Handoff](examples/agentic/handoff_protocol.ctxdsl) | Supervisor + 2 specialist agents | 4+3+3 (async) | Mutual exclusion, GR(1) liveness |

### Verify Framework (`mununu verify <verify.toml>`)

Each entry under [`examples/verify/`](examples/verify/) ships a `verify.toml`, the referenced source files, a `validate.sh` reproduction script, and a byte-deterministic `transcript.txt`:

| Example | Sources | Binding | What it demonstrates |
|---------|---------|---------|----------------------|
| [`xstate_pair/`](examples/verify/xstate_pair/) | XState × 2 | direct | Multiple sources via the same adapter; mixed inline + template properties |
| [`microprogram_plus_sv/`](examples/verify/microprogram_plus_sv/) | hand-authored CTXDSL + SystemVerilog | direct | Microcode + RTL pairing through the framework |
| [`uart_codesign_chaotic/`](examples/verify/uart_codesign_chaotic/) | C firmware + chaotic-stub peripheral | register-map | Codesign C+SV with register-map-derived rendezvous labels |
| [`uart_codesign_protocol_spec/`](examples/verify/uart_codesign_protocol_spec/) | C firmware + hand-authored protocol spec | direct | The "protocol-spec recipe" inside the framework |
| [`crewai_handoff/`](examples/verify/crewai_handoff/) | CrewAI 2-agent crew | direct | Native CrewAI adapter end-to-end |
| [`langgraph_workflow/`](examples/verify/langgraph_workflow/) | LangGraph triage workflow | direct | Native LangGraph adapter end-to-end |

Reproduce any of them: `bash examples/verify/<example>/validate.sh`

## CTXDSL at a Glance

```
context handshake {
    alphabet {
        label req_assert;
        label ack_assert;
        label ack_deassert;
    }

    automata {
        automaton Handshake {
            controllable { label ack_assert; label ack_deassert; }
            states {
                state Idle initial;
                state WaitAck;
                state Active;
            }
            transitions {
                transition Idle -> WaitAck on label req_assert;
                transition WaitAck -> Active on label ack_assert;
                transition Active -> Idle on label ack_deassert;
            }
        }
    }

    mu_formulas {
        // Reachability: Active is reachable from any state
        formula ack_reachable {
            over Handshake;
            body = mu X. (Active || <> X);
        }

        // GR(1): GF(request) -> GF(active)
        formula gr1_liveness {
            over Handshake;
            body = (! (nu X. (mu Y. (WaitAck || <> Y)) && ([] X)))
                || (nu Z. (mu W. (Active || <> W)) && ([] Z));
        }
    }

    controllers {
        controller safe_handshake {
            source Handshake;
            satisfying ack_reachable;
        }
    }
}
```

## Architecture

Mununu verifies by three orthogonal choices: **abstraction** (explicit-state vs
predicate cubes), **engine** (explicit enumeration vs symbolic BDD), and **domain**
(2-valued for exact models vs 3-valued Kleene may/must for sound abstraction). All
paths flow through a frontend-agnostic transition-system seam (STS-IR) into one
3-valued verdict vocabulary — `True` / `False` / `⊥` (abstraction too coarse to
decide, never a false claim).

```mermaid
flowchart TD
  subgraph FE["Frontends"]
    SV["SystemVerilog + SVA"]
    OTH["XState / TLSF / AIGER / Promela /<br/>CTXDSL / microcode / agentic"]
  end

  SV -->|"slang --ast-json"| TR["SVA translator<br/>→ mu-calculus formula + predicate atoms"]
  SV -->|"sv2v + yosys (no flatten)"| B2["BTOR2 IR"]

  B2 --> STS["STS-IR seam<br/>StsVar + StepEval + SmtEncode<br/>(hides BTOR2 / Z3)"]

  STS -->|"StepEval::step"| ENUM["Explicit-Enumerate<br/>Sharp CLTS"]
  STS -->|"SmtEncode::may_edges + must"| CUBE["Explicit predicate-cube lift"]
  STS -->|"symbolic relation build (planned, R-F5)"| SYM["Symbolic predicate-cube (BDD)"]

  ENUM --> CLTS["Clts (K)MTS<br/>states + may/must transitions + 3-valued labels"]
  CUBE --> CLTS
  OTH --> CLTS

  CLTS --> EVX["Explicit evaluator<br/>(BoolDom / KleeneDom over BitVec)"]
  SYM --> EVS["Symbolic evaluator (planned)<br/>BDD image/preimage fixpoint"]

  EVX --> V["3-valued verdict:<br/>True / False / ⊥"]
  EVS --> V
```

The full design — predicate abstraction, explicit & symbolic model checking, the IR
layering, and how over/under/⊥ approximation and may/must edges operate — is in
[`docs/design/post-rf5-architecture.md`](docs/design/post-rf5-architecture.md).

Source layout:

```
src/
├── adapter/        # Format adapters (TLSF, AIGER, Promela, XState, SystemVerilog, extraction)
├── clts/           # Core CLTS data structure (builder, label store, state management)
├── composition/    # Synchronous, asynchronous, and superset composition
├── context/        # CLTS registry, synthesis, mu-calculus evaluation engine
├── context_dsl/    # DSL lexer, parser, AST, realization, incremental loading
├── mu_calculus/    # Formula parsing, fixpoint evaluation, simplification
├── ltl/            # LTL to mu-calculus translation
├── guard/          # Guard expression parsing
├── abstraction/    # State variable abstraction (value domains, unrolling)
├── api/            # REST API server (axum-based, optional feature)
├── persistence/    # CLTS disk serialization
└── main.rs         # CLI entry point
```

## How It Compares

| Feature | Mununu | SLUGS | Strix | TLV |
|---------|--------|-------|-------|-----|
| Input language | CTXDSL | structured slugs | TLSF/HOA | SMV |
| Mu-calculus | Yes | No | No | Yes |
| LTL | Yes (translated) | No | Yes (native) | Yes |
| GR(1) synthesis | Yes | Yes (native) | Via LTL | Yes |
| Composition | Sync/Async/Superset | No | No | Yes |
| Web UI | Yes | No | No | No |
| Implementation | Rust | C++ | Rust/C++ | C |
| State abstraction | Yes | No | No | Partial |

## Building from Source

Requires **Rust 1.91+** (Edition 2024).

```bash
# Clone and build
git clone https://github.com/vscorza/mununu.git
cd mununu
cargo build --release

# Run tests
cargo test

# Install pre-commit hooks
./scripts/setup-hooks.sh

# Build with API server support
cargo build --release --features api
```

### Optional external tools

Several mununu pipelines invoke external tools via subprocess. **All are optional** — mununu functions without them and emits a structured warning when an invoked pipeline requires a missing tool. The currently-supported external tools are sv2v (SystemVerilog normalisation), Yosys (SV → BTOR2), SymbiYosys (verification oracle), and CVC5 (Craig interpolation for the R.5 CEGAR loop). See [`docs/external-tools.md`](docs/external-tools.md) for per-platform install instructions and discovery env vars.

## Web UI

The companion [mununu-ui](https://github.com/vscorza/mununu-ui) project provides:

- Monaco-based CTXDSL editor with syntax highlighting
- Interactive graph visualization (Cytoscape/Dagre) with controllable/uncontrollable edge styling
- Unified verification tab with counterstrategy graphs and counterexample traces
- Internationalization (English, Spanish, Portuguese)

```bash
# Start the API server
cargo run --features api -- server --addr 127.0.0.1:8080

# In another terminal, start the UI
cd /path/to/mununu-ui
npm install && npm run dev
# Open http://localhost:5173
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions and guidelines.

## License

[Mununu Non-Commercial License](LICENSE)

## References

- N. Piterman, A. Pnueli, and Y. Sa'ar. "Synthesis of Reactive(1) Designs." *VMCAI*, 2006.
- R. Bloem, B. Jobstmann, N. Piterman, A. Pnueli, and Y. Sa'ar. "Synthesis of Reactive(1) Designs." *JCSS*, 78(3), 2012.
