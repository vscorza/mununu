# Layer 3: Verification

The verification layer evaluates temporal properties on composed automata and synthesizes controllers (winning strategies).

## Pipeline

```
CTXDSL text
    → Parse (lexer → parser → canonicalize)
    → Realize (expand parameters, build CLTS instances, compose, register predicates)
    → Evaluate (μ-calculus model checking with guard partitions)
    → Synthesize (strategy extraction from witness maps)
```

## CTXDSL

The Context DSL is mununu's native specification language. A CTXDSL document declares:
- **Alphabet**: labels (events) in the system
- **Automata**: named state machines with states, transitions, and controllability
- **Compositions**: parallel composition of automata (synchronous or asynchronous)
- **Formulas**: μ-calculus properties to verify
- **Controllers**: synthesis targets (automaton + formula)

## μ-Calculus Evaluation

Mununu evaluates alternation-free μ-calculus formulas over CLTS instances. The evaluator computes the set of states satisfying a formula via fixpoint iteration.

Key features:
- **Guard partitions**: pre-compiled state predicates as bitvectors for O(n/64) evaluation
- **Memoization**: subformula results cached
- **Skolem paradigm**: for synthesis, controllable labels are existentially quantified (∃ — the system chooses), uncontrollable labels are universally quantified (∀ — the environment chooses worst case)

## Controller Synthesis

Synthesis is strategy extraction from witness maps:
1. Evaluate formula with witness tracking enabled
2. For each diamond (∃) modality, record the chosen transition
3. Extract these witness transitions as a positional winning strategy
4. Encode as a controller CLTS

The synthesized controller, when composed with the plant, guarantees the property.

## Format Adapters

Format adapters translate formal specification formats into CTXDSL:

| Format | Source | Encoding |
|--------|--------|----------|
| TLSF | GR(1) synthesis specs | Signal-state (turn-based, 2^N states) |
| AIGER | Hardware circuits | Signal-state (turn-based) |
| Promela | Process algebra | Explicit automata |
| XState | Statecharts | Explicit automata (hierarchy flattened) |
| SystemVerilog | RTL FSMs | Explicit automata (enum → states, case → transitions) |
| Extraction (.espec.json) | Source code extractions | Explicit automata (from declarative spec) |

## API

The HTTP API (`mununu server`) exposes all verification capabilities:

```
POST /api/v1/context/verify      — evaluate formula
POST /api/v1/context/synthesize  — controller synthesis
POST /api/v1/context/summarize   — context inspection
POST /api/v1/context/graphs      — visualization
POST /api/v1/context/import      — adapter translation
```
