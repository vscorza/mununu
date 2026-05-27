# Strategy Extraction & Synthesis Modes

> Source of truth: [`crates/mununu-core/src/context/synthesis.rs`](../crates/mununu-core/src/context/synthesis.rs) and [`crates/mununu-core/src/api/handlers.rs`](../crates/mununu-core/src/api/handlers.rs) — surface: CLI+API+UI

## ControllerMode

Three modes control how synthesis extracts a strategy from the winning region:

- **Projection** (default). Keeps **all** transitions between winning states. Not a strategy — just the winning region as a sub-CLTS.
- **Functional** (`--extract-strategy`). Picks **one** controllable transition per state — the one whose target has the lexicographically smallest signature (best mu-progress). Deterministic, correct for all formulas.
- **Permissive**. Keeps **all** controllable transitions whose target signature is ≤ the source's. Maximally permissive supervisor (Ramadge-Wonham canonical). Nondeterministic, composable with other supervisors.

## Signatures

The **signature** of a state is its tuple of iteration ranks per fixpoint variable (outermost first). For mu-variables, smaller rank = closer to goal. The functional strategy picks the most progressive move; the permissive supervisor enables all non-regressive moves.

The winning-region / realizability verdict is always correct **for the model** at any alternation depth — the fixpoint engine evaluates the full modal-mu calculus exactly. Counterstrategies are also positional — both players have memoryless winning strategies (positional determinacy of parity games, Zielonka 1998). Memoryless on the model-checking product = finite-memory on the plant; the memory is the iteration-rank signature from fixpoint evaluation.

**Transfer to the concrete system depends on the abstraction.** For pure-safety (ν, depth-1) properties, an over-approximating 2-valued model is enough; sound verdicts transfer. For properties with alternating fixpoints (νμ in GR(1), nested obligations), the 2-valued model is *not* enough — see [`CLAUDE.md` → Soundness Guarantees](../CLAUDE.md#soundness-guarantees) and [`docs/abstraction.md`](abstraction.md). Use the KMTS + Kleene 3-valued path: definite (`KleeneT` / `KleeneF`) verdicts transfer at every alternation depth; `KleeneBot` triggers CEGAR refinement (R.5).

## Lasso traces

Counterexample traces for liveness use lasso format `prefix -> (cycle)^ω` with transition labels (`prefix_labels`, `cycle_labels`). The cycle detection uses DFS in the losing region. The last `cycle_labels` entry is the closing edge back to `cycle[0]`.

## Counterstrategy in synthesis response

The `/context/synthesize` endpoint automatically returns a `counterstrategy` field (with Cytoscape graph elements) for unrealizable cases. The graph is filtered to states reachable from initials via kept transitions (post-strategy-extraction BFS).

## Formula inversion

When inverting mu-calculus formulas, do **not** negate fixpoint variable references inside the body. Keep variables positive — the dual fixpoint's changed starting point (mu starts empty, nu starts full) handles the semantics. Negating variables causes infinite oscillation between all-true and all-false. This is also stated in CLAUDE.md's API & Endpoint Performance bullet because it bites at the handler layer.

## Nondeterminism and controllability (Skolem paradigm)

The controller chooses **which label** to trigger, but cannot choose **which outcome** occurs when multiple transitions share the same label (nondeterminism). Nondeterministic outcomes are always adversarial — **all** must satisfy — regardless of whether the label is controllable or uncontrollable.

Controllability only determines **who triggers** the label (controller vs environment), **not** the outcome.

This is the Skolem-paradigm rule and is the reason TLSF / AIGER use a turn-based encoding (see [`adapters/tlsf-aiger.md`](adapters/tlsf-aiger.md)).
