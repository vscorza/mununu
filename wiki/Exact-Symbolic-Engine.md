# Exact-Symbolic Engine (D1)

> **Source of truth:** [`adapter::btor2::symbolic_bitblast::exact_symbolic_verdict`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs#L1694), [`--engine exact-symbolic`](https://github.com/vscorza/mununu/blob/main/crates/mununu-cli/src/main.rs#L207), [`VerifyRequest.engine`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/api/models.rs#L1057), [`verifyAuto engine`](https://github.com/vscorza/mununu-ui/blob/main/src/api/endpoints.ts#L551) — surface: CLI+API+UI (`mununu sv verify-auto --engine exact-symbolic`, `POST /api/v1/sv/verify-auto` with `"engine": "exact-symbolic"`, the verify-auto UI engine selector).

The exact-symbolic engine is mununu's **third RTL verification engine**. Where the
[Predicate-Cube CEGAR](Predicate-Cube-CEGAR) path abstracts the design into a small
set of Boolean predicates and answers a **3-valued** verdict (`KleeneT` / `KleeneF` /
`KleeneBot`), this engine bit-blasts the **entire register state** into a ROBDD and
computes the mu-calculus fixpoints by image/preimage iteration over the **exact**
transition relation. There is no abstraction, so the verdict is **2-valued and
definite — there is no `⊥`**. Its cost is BDD size, not predicate count.

It exists for one reason: **liveness that predicate abstraction cannot decide.** An
`AF` (inevitability) needs a ranking function — a measure that strictly decreases
toward the target — and predicate abstraction does not synthesize one, so `AF`,
`AG AF`, and fair-cycle properties come back `⊥` on the cube path. The exact engine's
least-fixpoint iteration *is* the ranking: it converges in at most as many steps as
there are states, and that finite convergence bounds the distance to the target
directly. No ranking has to be guessed.

## When to use which engine

| | [Predicate-Cube CEGAR](Predicate-Cube-CEGAR) | Exact-Symbolic (this page) |
|---|---|---|
| Abstract state space | `2^(predicate count)` — independent of register width | `2^(register+input bits)`, ROBDD-compressed |
| Verdict | 3-valued (`T` / `F` / `⊥`) | **2-valued definite (`Holds` / `Violated`), never `⊥`** |
| Decides `AF` / `AG AF` / fair-cycle liveness | often `⊥` (no ranking) | **yes, definitely** |
| Counterexample | cube tally | a concrete **stall lasso** (for `AF`-shaped violations) |
| Limit | edge SMT cost at large `|P|` | **BDD size** (register+input bit count) |
| Best for | wide-datapath / large-register designs | small FSM-heavy control logic where a definite liveness answer is wanted |

## Invocation

The engine is reached through **`sv verify-auto`** only — it decides a property
directly over the bit-blasted state, so it needs no sidecar and no predicate seeding:

```bash
mununu sv verify-auto design.sv --top my_fsm --engine exact-symbolic
```

```jsonc
// POST /api/v1/sv/verify-auto
{ "sources": [{ "name": "design.sv", "content": "…" }],
  "top": "my_fsm", "engine": "exact-symbolic" }
```

Selecting `--engine exact-symbolic` on `btor2 cegar` / `sv cegar` is rejected with a
message pointing at `sv verify-auto` — the exact engine is not a predicate-cube
refinement mode.

## Pipeline

```
SystemVerilog ──sv2v──► Verilog ──Yosys (flatten + async2sync)──► BTOR2
                                                                    ▼
                    BddBitBlaster::build   (one BDD var per register + input bit)
                                                                    ▼
       ExactModel   (next-state substitution = the exact transition relation)
                                                                    ▼
   modal-μ fixpoints by ROBDD image/preimage   (⟨⟩ = ∃ inputs, [] = ∀ inputs)
                                                                    ▼
       Holds  iff  init ⊆ ⟦φ⟧      |      Violated  (+ stall lasso for AF shapes)
```

Inputs are **free**: a `⟨⟩` (diamond) existentially quantifies the input bits, a `[]`
(box) universally quantifies them. Registers keep their present variable and are
substituted by their next-state function to take one step. The verdict is checked at
the **initial state** — the reset state when the model carries an `init` line (see
[reset gating](#reset-gating-required)), or globally (every state) when it does not.

## What it decides

Every property the [Mu-Calculus Reference](Mu-Calculus-Reference) can express, exactly:

- **Safety** — `AG p` = `νX. (p ∧ [] X)`.
- **Reachability / recoverability** — `EF p`, `AG EF p` = `νY. ((μX. (p ∨ ⟨⟩X)) ∧ [] Y)`.
  The `EF` (there-exists-a-path) is the shape linear-time logic and SVA cannot phrase.
- **Inevitability / liveness** — `AF p`, `AG AF p` = `νX. ((μY. (p ∨ [] Y)) ∧ [] X)`.
  This is the class the cube path returns `⊥` for.
- **Assume-guarantee / GR(1) fair cycles** — via the Emerson–Lei `¬EF badcycle`
  construction (see [`gr1_response_formula`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/mu_calculus/mod.rs)).

All three liveness classes are **alternating fixpoints** (a `μ` nested inside a `ν`).
Alternation is exactly where naive over- or under-approximation collapses, because the
outer `ν` needs a *may* upper bound while the inner `μ` needs a *must* lower bound. The
exact engine sidesteps the choice by being exact — two-valued, no direction to pick.
(The cube path handles the same alternation soundly with the KMTS's separate may/must
edges; see [Predicate-Cube CEGAR](Predicate-Cube-CEGAR).)

## Definite `Violated` comes with a counterexample

For an `AF p`-shaped violation the engine returns a concrete
[`StallLasso`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs#L1869): a reset → prefix → `¬p` cycle — a real infinite run on which `p`
never holds. A liveness failure is a lasso, not a finite prefix, which is why bounded
model checking neither proves nor refutes these properties without separate fairness
machinery; the exact engine returns the lasso directly.

> Example: on OpenTitan `uart_tx`, `AG AF (bit_cnt_q == 0)` ("a transmission always
> completes") is decided **Violated** — a persistently-asserted `wr` or a stalled baud
> tick holds the counter non-zero forever — and the engine returns the stall lasso,
> where the predicate-cube path answers `⊥`. Companion verdicts on the same design are
> definite `Holds`: `AG EF (bit_cnt_q == 0)` (recoverability — reset always drains the
> counter) and `AG (bit_cnt_q < 12)` (a bounded-counter safety invariant). See
> [`e2e_d1_uart_tx_exact_liveness_verdict`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs).

## Reset gating (required)

`sv verify-auto` gates the reset by default: the recognized reset input is pinned
inactive and the initial state is the modeled post-reset state, so the verdict answers
"from its reset state, does the running design …". The exact engine is built for this
regime. Combining `--engine exact-symbolic` with `--no-gate-reset` is **rejected**
up front: with the reset freed the model starts from an unmodeled power-up state and
the async reset is not explored as a firing transition, which would produce a
spurious — but definite-looking — verdict. For free-reset reachability use the cube
engine (drop `--engine exact-symbolic`), whose over-approximating may-relation soundly
includes the reset edge.

## Soundness and limits

- **Sound and definite.** No abstraction ⇒ the 2-valued verdict is exact for the model
  at every alternation depth. A `Holds` and a `Violated` both transfer to the modeled
  design; there is no `⊥` to interpret.
- **Bounded by BDD size.** The bit-blaster builds BDDs over *every* register + input
  bit (no cone-of-influence restriction yet). A design whose register+input bit count
  exceeds the cap ([`MAX_BITBLAST_BITS = 40`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs#L195)) is rejected with a clean error *before* the
  BDD manager is allocated, and `verify-auto` degrades that property to `Skipped` — a
  wide datapath belongs on the [Predicate-Cube CEGAR](Predicate-Cube-CEGAR) path with
  its honest `⊥` and CEGAR refinement. The engineering choice, per property and per
  design, is between an exact answer where the state fits and a sound-but-possibly-`⊥`
  answer where it does not.
- **The model is what yosys emits.** The verdict is exact *for the bit-blasted BTOR2*;
  its transfer to the real system still depends on the extraction (black-boxed
  submodules, `setundef` discipline, the reset model). This is the standard
  [claims-integrity](https://github.com/vscorza/mununu/blob/main/docs/policies/claims-integrity.md) boundary, not specific to this engine.
