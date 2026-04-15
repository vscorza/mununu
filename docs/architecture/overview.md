# Architecture Overview

Mununu is a formal verification tool organized in three layers. Each layer has a distinct responsibility and a defined interface to the next.

```
┌──────────────────────────────────────────────────────────────────┐
│                  LAYER 1: EXTRACTION                             │
│                                                                  │
│  tree-sitter     LLVM IR+SVF      CIRCT         Manual          │
│  (TS/Python)     (C/C++/Rust)     (SV)          (.espec.json)   │
│       │               │              │               │          │
│       └───────────────┴──────────────┴───────────────┘          │
│                           │                                      │
│                    .espec.json                                   │
│               (Component IR format)                              │
└───────────────────────────┬──────────────────────────────────────┘
                            │
┌───────────────────────────┴──────────────────────────────────────┐
│                  LAYER 2: ADAPTATION                             │
│                                                                  │
│  External code summaries (3-tier)                                │
│  Action hiding                                                   │
│  Bisimulation minimization                                       │
│  Parallel composition (sync/async)                               │
│                                                                  │
│  Output: Adapted, composed system automaton (CTXDSL)             │
└───────────────────────────┬──────────────────────────────────────┘
                            │
┌───────────────────────────┴──────────────────────────────────────┐
│                  LAYER 3: VERIFICATION                           │
│                                                                  │
│  CTXDSL parsing → Realization → μ-calculus evaluation            │
│  Controller synthesis (strategy extraction)                      │
│  Guard partitions for performance                                │
└──────────────────────────────────────────────────────────────────┘
```

## Binaries

| Binary | Layer | Purpose |
|--------|-------|---------|
| `mununu-extract` | 1 | AST-based extraction: source + config → `.espec.json` |
| `mununu` | 2 + 3 | Adaptation + verification: `.espec.json` → verify/synthesize |

Both binaries share types via the `mununu-core` library crate.

## Interface: `.espec.json`

The `.espec.json` format is the **stable contract** between layers. It contains:
- Source provenance (repo, commit, file, class)
- State fields with line-anchored evidence
- Declarative automaton definitions (states, transitions, mode filters)
- Composition directives (synchronous/asynchronous)
- Properties (μ-calculus formulas with per-property "over" targets)

All extraction frontends produce `.espec.json`. The verification engine consumes it. The format is human-editable — users can inspect, override, and version control the intermediate artifact.

## Format Adapters

For inputs that are already formal specifications (not source code), format adapters translate directly to CTXDSL without going through `.espec.json`:

| Adapter | Input | Use case |
|---------|-------|----------|
| TLSF | GR(1) synthesis specs | SYNTCOMP benchmarks |
| AIGER | And-Inverter Graphs | Hardware model checking |
| Promela | Process algebra models | SPIN ecosystem |
| XState | Statechart JSON | State machine design |
| SystemVerilog | RTL designs | Hardware verification |

These bypass Layer 1 (no extraction needed) and go directly to Layer 3.

## Workspace Structure

```
mununu/
├── crates/
│   ├── mununu-core/      (lib: verification engine, adapters, composition, IR)
│   ├── mununu-cli/       (bin: `mununu` — CLI + HTTP API server)
│   └── mununu-extract/   (bin: `mununu-extract` — tree-sitter extraction)
├── docs/architecture/    (this documentation)
├── examples/
│   ├── ast_extract/      (extraction examples with source + config)
│   ├── ctxdsl/           (direct CTXDSL examples)
│   └── formats/          (TLSF, AIGER, Promela, XState examples)
└── tests/                (integration and system tests)
```
