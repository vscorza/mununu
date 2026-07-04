# Post-R-F5 architecture — predicate abstraction, explicit & symbolic model checking

> Concept + design note — an architecture/theory overview, exempt from the
> `Source of truth:` anchor rule per CLAUDE.md §Documentation Traceability. Every
> named symbol below is a real, greppable artifact.
>
> Status (as-built): the R-F5 symbolic track has **shipped** — R-F5.0 (the OxiDD
> spike, `mu_calculus/symbolic.rs`) through R-F5.5 (symbolic CEGAR loop, `--engine
> symbolic` on `sv verify-auto`, and state-predicate-guarded modalities in the cube
> evaluator). The remaining item is **R-F5.6** — a cone-of-influence restriction for
> large designs; the symbolic engine is bit-count-capped until then. The few sections
> still tagged **(planned)** below name that residual gap, not the whole track.

## 0. TL;DR

- R-F5 does **not** replace mununu's automata foundation. The `Clts` (K)MTS model, the
  composition engine, the mu-calculus evaluator, and every non-SV adapter are unchanged.
- R-F5 adds a **third engine strategy** — *symbolic* (BDD) — for the **predicate-cube
  abstraction path** only, coexisting with the existing *explicit-enumerate* and
  *explicit-predicate-cube* strategies. All three sit behind the same **STS-IR seam** and
  feed the same 3-valued evaluator semantics.
- The BDD is used for **both** roles: (1) a succinct representation of *sets of
  predicate-cubes* (combinations of predicate truth values), and (2) the *engine* — a
  symbolic transition relation + fixpoint image/preimage. Role (2) is where the win is:
  it avoids materialising `2^|P|` cubes and the `O(2^2|P|)` SMT edge computation.

---

## 1. The three axes

Every mununu verification run is a choice on three orthogonal axes:

```mermaid
flowchart LR
  subgraph A["Axis 1 — Abstraction (what the states MEAN)"]
    A1["Concrete / explicit-state<br/>(FSM enum, bounded counter)"]
    A2["Predicate abstraction<br/>(states = predicate cubes)"]
  end
  subgraph B["Axis 2 — Engine (how the state space is REPRESENTED)"]
    B1["Explicit<br/>(enumerated states + adjacency)"]
    B2["Symbolic<br/>(BDD state sets + BDD relation)"]
  end
  subgraph C["Axis 3 — Domain (how VERDICTS are valued)"]
    C1["2-valued (BoolDom)<br/>Sharp CLTS, cheap path"]
    C2["3-valued (KleeneDom)<br/>KMTS may/must, sound abstraction"]
  end
```

- **Abstraction** is chosen by the adapter/sidecar (explicit enum vs predicate cube).
- **Engine** is chosen by scale: explicit is fine until `2^|P|` blows up, then symbolic.
- **Domain** is chosen by soundness need: 2-valued for exact/Sharp models, 3-valued
  (Kleene) whenever the abstraction carries a may/must split.

R-F5 fills in the **Symbolic** cell of Axis 2 — now shipped alongside the other cells,
selectable per run via `--engine symbolic`.

---

## 2. End-to-end pipeline

```mermaid
flowchart TD
  subgraph FE["Frontends"]
    SV["SystemVerilog + SVA"]
    OTH["XState / TLSF / AIGER / Promela /<br/>CTXDSL / microcode / agentic"]
  end

  SV -->|"slang --ast-json"| TR["SVA translator<br/>(adapter/slang/translate.rs)<br/>→ mu-calculus formula + predicate atoms"]
  SV -->|"sv2v + yosys (no flatten)"| B2["BTOR2 IR<br/>(Btor2File / Node)"]

  B2 --> STS["STS-IR seam<br/>StsVar + StepEval + SmtEncode<br/>(BtorSts wraps Btor2File; hides Z3/NID)"]

  STS -->|"StepEval::step"| ENUM["Explicit-Enumerate strategy<br/>(bit_blast) — Sharp CLTS"]
  STS -->|"SmtEncode::may_edges + must<br/>(--engine explicit, default)"| CUBE["Explicit predicate-cube lift<br/>(kmts_lift::predicate_cube_lift)"]
  STS -->|"BDD relation build (--engine symbolic)"| SYM["Symbolic predicate-cube (BDD)<br/>(symbolic_bitblast::BddBitBlaster<br/>→ AbstractRelation)"]

  ENUM --> CLTS["Clts (K)MTS<br/>states + may/must transitions + 3-valued labels"]
  CUBE --> CLTS

  CLTS --> EVX["Explicit evaluator<br/>evaluate / evaluate_tri<br/>(BoolDom / KleeneDom over BitVec)"]
  SYM --> EVS["Symbolic evaluator<br/>SymbolicKmts::evaluate — BDD image/preimage fixpoint<br/>(TritBdd = must/may BDD pair)"]

  OTH --> CLTS

  EVX --> V["Verdict"]
  EVS --> V

  V --> VA["3-valued per-state:<br/>True / False / ⊥ (Unknown)"]
```

**Reading the diagram.**
- Non-SV frontends build a `Clts` directly and always use the explicit evaluator.
- The SV frontend forks: the **SVA translator** produces the *property* (a mu-calculus
  formula); the **sv2v→yosys→BTOR2** path produces the *model*.
- The BTOR2 model is consumed through the **STS-IR seam**, never directly — that's the
  frontend-agnostic waist.
- Three engine strategies consume the seam. The first two produce an explicit `Clts`;
  the symbolic one (R-F5) builds BDDs and is evaluated by a symbolic fixpoint.
- All paths converge on the same 3-valued verdict vocabulary.

---

## 3. The IR structs — where they fall and what they look like

The IR is layered. Each layer hides the one below and is consumed by the one above.

```mermaid
flowchart TD
  L1["<b>L1 Source AST</b> — frontend-specific<br/>Btor2File / Node (BTOR2), slang JSON"]
  L2["<b>L2 STS-IR seam</b> — frontend-agnostic transition system<br/>StsVar, SymbolicTransitionSystem, StepEval, SmtEncode<br/>impl: BtorSts&lt;'a&gt;(&amp;'a Btor2File)"]
  L3["<b>L3 Predicate layer</b> — the abstraction<br/>PredicateSpec {name, register, value}<br/>PredicateExpr {Cmp, CmpReg, CmpRegAddend, And, Or, Not}"]
  L4A["<b>L4a Automata IR (explicit)</b><br/>Clts / Transition / TransitionModality / Tristate labels"]
  L4B["<b>L4b Symbolic IR (R-F5)</b><br/>SymbolicKmts / AbstractRelation {r_may, r_must : Bdd}<br/>state-AP labels as Bdd (TritBdd)"]
  L5A["<b>L5a Verdict (explicit)</b><br/>TritSet {must_true, may_true : BitVec}"]
  L5B["<b>L5b Verdict (symbolic, R-F5)</b><br/>TritBdd {must, may : Bdd}"]

  L1 --> L2 --> L3
  L3 --> L4A --> L5A
  L3 --> L4B --> L5B
```

### L2 — STS-IR seam (the narrow waist)

`adapter/sts_ir.rs`. Decouples the abstraction engines from BTOR2/Z3. See
[`docs/design/sts-ir.md`](sts-ir.md).

```rust
pub struct StsVar { pub name: String, pub width: u32 }

pub trait SymbolicTransitionSystem {
    fn state_vars(&self) -> Vec<StsVar>;
    fn input_vars(&self) -> Vec<StsVar>;
}

// Concrete one-step — the Explicit-Enumerate strategy needs only this.
pub trait StepEval: SymbolicTransitionSystem {
    fn step(&self, state: &HashMap<String,u128>, inputs: &HashMap<String,u128>)
        -> Result<HashMap<String,u128>, AdapterError>;
}

// SMT predicate-image — the predicate-cube strategy (and R-F5's relation builder) need this.
pub trait SmtEncode: SymbolicTransitionSystem {
    fn may_edges(&self, predicates: &[PredicateSpec], timeout_ms: u32) -> Vec<(usize,usize)>;
    // must-relation (∀∀ / ∀∃ / hyper-must) follows the same shape.
}
```

`BtorSts<'a>(&'a Btor2File)` implements all three by delegation; `Btor2File`, `Nid`, and
`z3::*` never escape it. **This is the seam R-F5's symbolic relation-builder plugs into
(R-F5.3) — it is not a new frontend.**

### L3 — Predicate layer

A predicate is a boolean function of the concrete registers. Two shapes:

```rust
// The simple cube-dimension form the lift and refinement pass around.
pub struct PredicateSpec { pub name: String, pub register: String, pub value: u64 } // register == value

// The full expression form (compounds / relational / arithmetic-addend).
pub enum PredicateExpr {
    Cmp        { register: String, op: CmpOp, value: u64 },          // reg <op> literal
    CmpReg     { lhs: String, op: CmpOp, rhs: String },              // reg <op> reg   (e.g. $stable, monotonicity)
    CmpRegAddend { lhs: String, op: CmpOp, rhs: String, addend: u64, width: u32 }, // reg <op> reg+const (mod 2^w)
    And(Box<PredicateExpr>, Box<PredicateExpr>),
    Or (Box<PredicateExpr>, Box<PredicateExpr>),
    Not(Box<PredicateExpr>),
}
```

Each predicate becomes **one cube dimension** (one bit). The abstract state space is the
set of predicate *cubes* = valuations of `P` = `{p_0, …, p_{|P|-1}}`.

### L4a — Automata IR (`Clts`, the (K)MTS)

The one data model the explicit evaluator reads. A **CLTS is a degenerate KMTS** where
every transition is `Sharp` and every AP label is definite.

```rust
pub enum TransitionModality<S> {
    Sharp,                                   // in BOTH may and must  (must ⊆ may)
    MayOnly,                                 // in may only  (over-approximation edge)
    MustHyperOnly(Box<SmallVec<[StateId<S>;4]>>), // GKMTS hyper-must: must reach SOME target in the set
}

pub enum Tristate { KleeneT, KleeneF, KleeneBot }  // per-(state, AP) 3-valued label
// Clts stores: state_names, outgoing/incoming adjacency of Transition{target, labels, modality},
//              and state_3valued_predicates: Option<BTreeMap<(StateId, String), Tristate>>.
```

### L4b / L5b — Symbolic IR (R-F5)

The BDD twin of L4a/L5a. State sets and the transition relation are OxiDD BDDs over the
predicate-bit variables (present `x` and next `x'`):

```rust
// A 3-valued state set (the symbolic TritSet): a pair of BDDs over present vars, must ⊑ may.
// (mu_calculus/symbolic.rs) — bridges losslessly to/from the explicit TritSet.
struct TritBdd { must: BDDFunction, may: BDDFunction }

// The symbolic KMTS built from a Clts: the may/must relation + AP labels as BDDs.
// (mu_calculus/symbolic.rs) — SymbolicKmts::from_clts / ::evaluate.
struct SymbolicKmts { /* r_may, r_must, per-label edges + state-var BDDs */ }

// The BTOR2-derived abstract relation, built directly by the BDD bit-blaster
// (adapter/btor2/symbolic_bitblast.rs::BddBitBlaster::abstract_relation).
struct AbstractRelation { /* r_may, r_must : BDDFunction over present+next preds */ }
```

`R_must ⊆ R_may` mirrors `Sharp ⊆ may`; a hyper-must is a disjunction over its target set.
`SymbolicKmts` is the general path (any `Clts`); `AbstractRelation` is the BTOR2
predicate-cube path that builds the relation once, without per-cube-pair SMT (§5).

### L5a — Verdict (`TritSet`)

```rust
pub struct TritSet { must_true: BitVec, may_true: BitVec } // invariant must_true ⊆ may_true
pub enum Trit { True, False, Unknown }                     // verdict_at(state)
```

`Trit` (evaluator output) and `Tristate` (CLTS label) are the same algebra under two
names; conversion is lossless. Post-R-F5, `TritBdd` is the symbolic counterpart of
`TritSet` and yields the same `Trit` at any state (by evaluating the BDD at that state's
minterm).

---

## 4. Predicate abstraction

Predicate abstraction replaces the concrete state (a valuation of the RTL registers) with
a **cube**: a valuation of a chosen predicate set `P`. `|P|` bits ⇒ up to `2^|P|` abstract
states, regardless of how wide the concrete registers are.

```mermaid
flowchart LR
  subgraph CON["Concrete states (huge)"]
    c0["cnt_q=0"]
    c1["cnt_q=1"]
    c7["cnt_q=7"]
    c8["cnt_q=8"]
    cbig["cnt_q=2^32-1"]
  end
  subgraph P["Predicates P"]
    p0["p0 = (cnt_q == 0)"]
    p1["p1 = (cnt_q >= 7)"]
  end
  subgraph ABS["Abstract cubes (2^P = 4)"]
    a00["¬p0 ¬p1"]
    a10["p0 ¬p1"]
    a01["¬p0 p1"]
    a11["p0 p1 (unsat)"]
  end
  c0 --> a10
  c1 --> a00
  c7 --> a01
  c8 --> a01
  cbig --> a01
  p0 -.-> ABS
  p1 -.-> ABS
```

- Concrete states collapse into the cube whose predicate valuation they satisfy
  (`cnt_q=7,8,…` all fall into `¬p0 ∧ p1`).
- The abstraction is **exactly as fine as the predicate set**. Coarse `P` ⇒ few cubes but
  many `⊥`; adding a predicate (a CEGAR refinement, or a seeded bound / threshold) splits
  cubes and can turn `⊥` into a definite verdict.
- Predicates come from: the property's atoms, sidecar declarations, config concretization
  (H.J), counter bounds/thresholds (H.H), and CEGAR (`PredicateSource::{WeakestPrecondition,
  CraigInterpolation}`).

The abstract **transition relation** over cubes is where the two engines differ (§5), and
where the may/must split lives (§6).

---

## 5. Explicit vs symbolic model checking

Both engines evaluate the *same* mu-calculus formula over the *same* abstract semantics.
They differ only in how the state space + relation are represented and how the fixpoint is
computed.

```mermaid
flowchart TB
  subgraph EX["Explicit engine (shipped)"]
    ex1["Materialise 2^P cube states (Clts)"]
    ex2["Build may/must edges:<br/>SmtEncode — O(2^2P) Z3 queries"]
    ex3["Fixpoint: for each state, walk outgoing<br/>transitions (modal_bits_from_target)"]
    ex4["Verdict: TritSet (must/may BitVec)"]
    ex1 --> ex2 --> ex3 --> ex4
  end
  subgraph SY["Symbolic engine (R-F5, shipped)"]
    sy1["No cube enumeration — vars = predicate bits"]
    sy2["Build R_may / R_must as BDDs<br/>(BddBitBlaster, from BTOR2, once — not per pair)"]
    sy3["Fixpoint: BDD image/preimage<br/>∃x'. R(x,x') ∧ φ(x')  via apply_exists(And,…)"]
    sy4["Verdict: TritBdd (must/may BDD)"]
    sy1 --> sy2 --> sy3 --> sy4
  end
```

| | Explicit | Symbolic (R-F5) |
|---|---|---|
| Abstract state | one `Clts` state per cube (index `0..2^|P|`) | a minterm over `|P|` BDD vars |
| State *set* | `BitVec` of `2^|P|` bits | one BDD |
| Transition relation | adjacency lists (`Transition{target,modality}`) | `R_may` / `R_must` BDDs over `x`,`x'` |
| Edge construction | **`O(2^2|P|)` SMT** (`SmtAllPairs`) ← the bottleneck | build R once (R-F5.3) |
| Modal step | `for state … for transition …` | `apply_exists(And, φ[x'], x'-cube)` |
| Verdict | `TritSet` | `TritBdd` |
| Best when | small `|P|`, or explicit enum models | large `|P|` (predicate cube blows up) |

**Why symbolic is not "just a smaller verdict."** The verdict `BitVec` was never the
problem — a 4096-cube verdict is ~512 bytes. The cost is (a) enumerating `2^|P|` cube
states and (b) the `O(2^2|P|)` per-cube-pair SMT to build the relation. The symbolic
engine removes both: the relation is one BDD, and the fixpoint operates on BDD-encoded
sets without touching individual cubes. The `evaluate_tri` evaluator stays the ground
truth — the symbolic path is validated cell-for-cell against it (R-F5.0 spike).

### The shared modal semantics (both engines)

The 3-valued box/diamond is the Bruns–Godefroid semantics
(`evaluator::modal_trit_core`, and the symbolic `box_pre` in `mu_calculus/symbolic.rs`):

```text
[a]φ  must  =  ∀ may-successors.  φ.must      ( box.must = ¬∃x'. R_may  ∧ ¬φ.must[x'] )
[a]φ  may   =  ∀ must-successors. φ.may       ( box.may  = ¬∃x'. R_must ∧ ¬φ.may[x']  )
<a>φ  must  =  ∃ must-successors. φ.must      ( dia.must =  ∃x'. R_must ∧  φ.must[x']  )
<a>φ  may   =  ∃ may-successors.  φ.may       ( dia.may  =  ∃x'. R_may  ∧  φ.may[x']   )
```

The right column is the symbolic form; the left is the explicit form. They compute the
same function — the spike proves it.

---

## 6. Over / under / bottom approximation and may/must edges

### The two lattices

The 3-valued Kleene domain carries **two** orders over `{True, ⊥, False}`:

```mermaid
flowchart TB
  subgraph TRUTH["Truth order (formula semantics: ∧, ∨, ¬)"]
    tF["False"] --> tB["⊥"] --> tT["True"]
  end
  subgraph INFO["Information order (fixpoint convergence)"]
    iBot["⊥ (least defined)"] --> iT["True"]
    iBot --> iF["False"]
  end
```

- **Truth order** `False < ⊥ < True` drives `∧`/`∨`/`¬` (`⊥ ∨ True = True`, `⊥ ∧ False =
  False`, `⊥ ∧ True = ⊥`).
- **Information order** `⊥ < {True, False}` (True/False incomparable) drives fixpoint
  convergence — iterating makes the verdict *more defined*. In `BoolDom` these two orders
  coincide (which is why the 2-valued path never notices the distinction).

### may / must edges = over / under approximation

```mermaid
stateDiagram-v2
  direction LR
  s0: s0 p=True
  s1: s1 p=True
  s2: s2 p=False
  s0 --> s1: Sharp may+must
  s1 --> s0: Sharp may+must
  s2 --> s2: Sharp
  s0 --> s2: MayOnly over-approx
```

- **`R_may` (over-approximation).** Every concrete transition is admitted, plus possibly
  spurious ones. `(b,b') ∈ R_may ⟺ ∃ s⊨b, s'⊨b'. (s,s')∈R`. Used for the **upper** bound
  of a verdict (the `may` side).
- **`R_must` (under-approximation).** Only transitions guaranteed to exist. `(b,b') ∈
  R_must ⟺ ∀ s⊨b. ∃ s'⊨b'. (s,s')∈R`, with `R_must ⊆ R_may`. Used for the **lower** bound
  (the `must` side).
- **`MayOnly`** = a may-edge with no must-witness (a *pure over-approximation* edge). It
  can only push a verdict toward `⊥`, never fabricate a definite one.
- **`MustHyperOnly {targets}`** = GKMTS hyper-must: the transition must reach *some* target
  in the set (needed for sound refinement of alternating fixpoints).
- **`⊥` (bottom)** is the *verdict*, not an edge kind: it is what the evaluator returns
  when the may-relation admits a behaviour the must-relation cannot confirm — "the
  abstraction is too coarse to decide," never "violated."

In the example above, `νX.(p ∧ []X)` (= `AG p`) gives:
- **Sharp-only** (drop the `MayOnly` edge): `s0,s1 = True`, `s2 = False` — a definite AGp on
  the p-true cycle.
- **With the `MayOnly` edge** `s0→s2`: the box at `s0` sees a may-successor (`s2`) with
  `p=False` and no must-witness → `s0 = ⊥`; the cycle propagates `⊥` to `s1`; `s2 = False`.
  This is the may/must split — the reason the domain is 3-valued.

### Which verdicts transfer to the concrete system (soundness)

```mermaid
flowchart LR
  T["Abstract verdict True"] -->|"transfers"| CT["Concrete: property holds"]
  F["Abstract verdict False"] -->|"transfers"| CF["Concrete: property violated"]
  B["Abstract verdict ⊥"] -->|"does NOT transfer"| R["Refine (add predicate) or report honest ⊥"]
```

By the Bruns–Godefroid preservation theorem, **definite** abstract verdicts (`True`,
`False`) transfer to the concrete system at every alternation depth — *because* the
KMTS carries both `R_may` and `R_must`. A single over-approximation (may only) would be
sound for safety/`ν` but not for liveness/`μ`; the may/must pair is what makes definite
verdicts sound for the full mu-calculus. `⊥` is the only non-transferring verdict, and it
is honest — it drives CEGAR refinement, never a false claim.

This soundness argument is **representation-independent**: it is a property of the may/must
semantics, so it holds identically for the explicit (`TritSet`) and symbolic (`TritBdd`)
engines. Swapping the engine changes cost, not correctness.

---

## 7. What is shipped vs planned

```mermaid
flowchart LR
  subgraph SHIP["Shipped (R-F5.0 – .5)"]
    d1["STS-IR seam (StsVar / StepEval / SmtEncode / BtorSts)"]
    d2["Predicate layer (PredicateSpec / PredicateExpr)"]
    d3["Explicit engines: Enumerate + predicate-cube lift"]
    d4["Clts KMTS (Sharp / MayOnly / MustHyperOnly + Tristate)"]
    d5["Explicit evaluator (BoolDom / KleeneDom, TritSet)"]
    d6["R-F5.0–.2 symbolic engine: SymbolicContext / TritBdd /<br/>SymbolicKmts — BDD image/preimage νμ fixpoint,<br/>validated cell-for-cell vs evaluate_tri"]
    d7["R-F5.2b guarded modalities (label + current/next state)"]
    d8["R-F5.3 symbolic R_may/R_must from BTOR2<br/>(BddBitBlaster → AbstractRelation, built once — not per pair)"]
    d9["R-F5.4 --engine symbolic on btor2/sv cegar (CLI + API)"]
    d10["R-F5.5 symbolic CEGAR loop + verify-auto wiring +<br/>compound-predicate cube dims"]
  end
  subgraph PLAN["Remaining"]
    p1["R-F5.6 cone-of-influence restriction<br/>(symbolic engine is bit-count-capped today)"]
    p2["Out-of-fragment over cubes (honest errors, not planned):<br/>controllability (ctrl) + step-bounded (steps) modalities;<br/>MustHyperOnly edges on the symbolic path"]
  end
  SHIP --> PLAN
```

**What shipped (R-F5.0–.5).** The symbolic engine is selectable via `--engine symbolic`
on `btor2 cegar`, `sv cegar`, and `sv verify-auto` (CLI + API). It bit-blasts the BTOR2
design to BDDs (`BddBitBlaster`), builds `R_may`/`R_must` as an `AbstractRelation`
**once** — a single `substitute` + `apply_exists` per predicate, the
`O(2^2|P|)`-SMT-avoidance win, no per-cube-pair SMT — and evaluates the mu-calculus by
BDD image/preimage in `SymbolicKmts`. Every symbolic verdict is validated cell-for-cell
against the explicit `evaluate_tri` in the test suite, including nested νμ and guarded
modalities. This resolved the R-F5.0 spike's open question — the spike validated the
*evaluation* side (given a symbolic relation) and R-F5.3 delivered the *construction*
side via BDD bit-blasting.

**The remaining item is R-F5.6 (cone-of-influence).** `BddBitBlaster` currently
bit-blasts the whole design, so the symbolic engine is gated by a bit-count cap
(`MAX_SYMBOLIC_CUBE_BITS`) and skips designs above it (real sysrst RTL hits the cap).
Restricting the bit-blast to the predicate cone-of-influence is the scaling work that
lifts the cap. Independently, controllability (`ctrl`) and step-bounded (`steps`)
modalities, and `MustHyperOnly` edges on the symbolic path, are **honest errors** (out of
the predicate-cube fragment) rather than planned features — see `mu_calculus/symbolic.rs`.

---

## 8. Cross-references

- [`docs/design/sts-ir.md`](sts-ir.md) — the STS-IR seam (L2).
- [`docs/design/kmts-theory.md`](kmts-theory.md) — KMTS + 3-valued mu-calculus theory.
- [`docs/design/native-sv-abstraction.md`](native-sv-abstraction.md) — the SV predicate-
  abstraction pipeline (§6 KMTS recipe, §6.10 UF abstraction).
- [`docs/abstraction.md`](../abstraction.md) — per-subsystem abstraction recipe.
- `crates/mununu-core/src/mu_calculus/symbolic.rs` — `SymbolicContext` / `TritBdd` /
  `SymbolicKmts` (the BDD engine + symbolic mu-calculus evaluator).
- `crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs` — `BddBitBlaster` /
  `AbstractRelation` (symbolic `R_may`/`R_must` from BTOR2, built once).
- `crates/mununu-core/src/adapter/btor2/symbolic_engine.rs` — `symbolic_cube_verdicts`
  / `symbolic_cegar_refine` (the symbolic predicate-cube + CEGAR entry points).
- `crates/mununu-core/src/mu_calculus/trit.rs` — `Trit` / `TritSet`.
- `crates/mununu-core/src/clts/mod.rs` — `Clts` / `Transition` / `TransitionModality` /
  `Tristate`.
- `crates/mununu-core/src/adapter/btor2/kmts_lift.rs` — `predicate_cube_lift`,
  `MayEdgeInference` / `MustEdgeInference`.
