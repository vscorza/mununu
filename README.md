# Mununu

**Formal verification and controller synthesis for reactive systems**

[![CI](https://github.com/vscorza/mununu/actions/workflows/ci.yml/badge.svg)](https://github.com/vscorza/mununu/actions/workflows/ci.yml)
[![Rust 1.91+](https://img.shields.io/badge/rust-1.91%2B-orange.svg)](https://www.rust-lang.org/)
[![License: Non-Commercial](https://img.shields.io/badge/license-Non--Commercial-blue.svg)](LICENSE)

<!-- TODO: Add GIF of mununu-ui showing graph visualization and verification -->

## What is Mununu?

Mununu is a verification tool for analyzing and synthesizing controllers for reactive systems modeled as Compositional Labeled Transition Systems (CLTS). It evaluates mu-calculus and LTL properties, performs controller synthesis, and supports compositional modeling of concurrent systems through a dedicated DSL.

## Features

- **CTXDSL** &mdash; A domain-specific language for defining automata, compositions, properties, and controllers
- **Mu-calculus evaluation** &mdash; Fixpoint-based property verification with bitvec-backed state sets
- **LTL support** &mdash; Linear temporal logic formulas automatically translated to mu-calculus
- **GR(1) specifications** &mdash; Generalized Reactivity(1) properties for reactive synthesis
- **Composition** &mdash; Synchronous, asynchronous, and superset composition of CLTS components
- **Controller synthesis** &mdash; Automatic synthesis of controllers satisfying safety and liveness properties
- **State abstraction** &mdash; Multi-level variable abstraction (Boolean, integer intervals, symbol sets)
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

# Synthesize a controller
cargo run -- context synth examples/hw/handshake.ctxdsl \
  --formula safety_invariant --automaton Handshake

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

```
src/
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

## Web UI

The companion [mununu-ui](https://github.com/vscorza/mununu-ui) project provides:

- Monaco-based CTXDSL editor with syntax highlighting
- Interactive graph visualization (Cytoscape/Dagre)
- One-click verification and controller synthesis
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
- [hw-verification-uba](https://github.com/vscorza/hw-verification-uba) &mdash; SystemVerilog labs from which hardware examples are derived.
