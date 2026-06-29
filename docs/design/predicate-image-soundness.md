# Predicate-image soundness — mapping mununu's cube/edge construction to the theorem

> Status: planning. The construction this doc argues sound is **evolving** toward
> the uniform predicate-image (H.U); §5 marks, per case, what is shipped-and-sound,
> what is shipped-as-foundation-only, and what is gap. This doc is the artifact the
> 2026-06-29 soundness review found missing: we cited the *evaluator's* preservation
> theorem but never argued our *abstraction* meets its hypotheses. Until H.O (the
> oracle harness) lands, every DEFINITE verify-auto verdict on real RTL is an
> internal claim — this doc is the soundness *argument*; the oracle is the
> soundness *evidence*.

## Contents

1. [Why this doc exists](#1-why-this-doc-exists)
2. [The objects](#2-the-objects)
3. [The theorem we rely on](#3-the-theorem-we-rely-on)
4. [The predicate-image construction](#4-the-predicate-image-construction)
5. [Per-case soundness ledger](#5-per-case-soundness-ledger)
6. [The must relation, inputs, and controllability](#6-the-must-relation-inputs-and-controllability)
7. [What is argued vs verified vs gap](#7-what-is-argued-vs-verified-vs-gap)
8. [References](#8-references)

---

## 1. Why this doc exists

> Concept: the soundness of a 3-valued predicate abstraction is *by construction*
> when the abstract may/must relations are, respectively, a sound over- and
> under-approximation of the concrete transition relation. The mis-steps the
> H.E arc hit (a derived label that the cube could not pin; a `target-free`
> encoding that fabricated must-edges for a combinational-of-input signal) were
> all cases where the *construction* silently violated that precondition. The
> fix is not another special case — it is to make the construction uniform
> (§4) so the precondition holds by inspection of one rule, and to write *this*
> argument so it is checkable rather than rediscovered via spurious verdicts.

mununu's verification core abstracts a symbolic transition system (a BTOR2
design, frontend-agnostic via the STS-IR seam) into a **Kripke Modal Transition
System (KMTS)** whose states are **predicate cubes**, then evaluates the modal
μ-calculus over it with a **3-valued** (`KleeneT` / `KleeneF` / `KleeneBot`)
evaluator. The whole point is: a *definite* verdict (`KleeneT` or `KleeneF`) on
the abstraction transfers to the concrete design at every alternation depth, so
we get sound safety **and** liveness/recoverability verdicts on designs too large
to enumerate. This doc states the construction precisely and argues the transfer.

## 2. The objects

**Concrete system.** A symbolic transition system `M = (S, I, T)` over state
variables (latches) and inputs:
- states `S` = valuations of the latches;
- a one-step relation `T(s, i, s')` relating current latch state `s` + input `i`
  to next latch state `s'` (the BTOR2 `next` functions; combinational signals are
  *terms* `g(s, i)`, not separate state);
- initial states `I`.

> Source of truth: [`Btor2SmtView`](../../crates/mununu-core/src/adapter/sidecar/predicate_image/btor2_encode.rs) — surface: API (the SMT encode of `T` + the per-node term BVs).

**Predicates.** `P = {p_0, …, p_{n-1}}`, each `p_k` a Boolean over a **term**
`t_k` of the design — `t_k` may be a latch (`state_q`), an input (`cfg_enable_i`),
or a combinational expression (`trigger_active = (trigger_i == 0)`), and the atom
is `t_k ⋈ v` for `⋈ ∈ {==, !=, <, ≤, >, ≥}` or a Boolean combination thereof.

> Source of truth: [`PredicateSpec`](../../crates/mununu-core/src/adapter/btor2/kmts_lift.rs) + [`PredicateExpr`](../../crates/mununu-core/src/adapter/btor2/predicate_expr.rs) — surface: API.

**Abstraction.** An abstract state is a **cube** `c ∈ {0,1}^n` (bit `k` = the
truth of `p_k`). The concretization `γ(c) = { s | ∀k. p_k(s) = c_k }`. The
abstract KMTS carries two transition relations with `R_must ⊆ R_may`:
- `R_may` — an **over**-approximation: `(c, c') ∈ R_may` whenever *some* concrete
  transition crosses from `γ(c)` to `γ(c')`;
- `R_must` — an **under**-approximation: `(c, c') ∈ R_must` only when *every*
  concrete state in `γ(c)` has a successor in `γ(c')` (∀∃; the GKMTS hyper-must
  generalises the target to a set).

> Source of truth: [`predicate_cube_lift`](../../crates/mununu-core/src/adapter/btor2/kmts_lift.rs) — surface: API (builds the cube `Clts` + edges).

## 3. The theorem we rely on

> Concept: 3-valued μ-calculus model checking over a KMTS (Bruns–Godefroid CONCUR
> 2000; Godefroid–Jagadeesan TACAS 2003; Shoham–Grumberg LMCS 2007 for the
> game/GKMTS extension and the alternating-fixpoint monotonicity).

**Preservation theorem.** Let `A` be a KMTS abstraction of `M` such that
- (over) `R_may ⊇ α(T)`: every concrete transition is covered by a may-edge;
- (under) `R_must ⊆ α(T)`: every must-edge is realized by every concrete state of
  its source cube;
- (labels) each AP's 3-valued cube label is `KleeneT`/`KleeneF` only when the AP
  is, respectively, definitely true / definitely false over **all** of `γ(c)`.

Then for every closed μ-calculus formula `φ` and abstract state `c`:
`⟦φ⟧_A(c) = KleeneT ⟹ ∀ s ∈ γ(c). M, s ⊨ φ`, and
`⟦φ⟧_A(c) = KleeneF ⟹ ∀ s ∈ γ(c). M, s ⊭ φ`.
`KleeneBot` carries no claim (refine, or report honest ⊥).

> Source of truth: [`evaluate_tri`](../../crates/mununu-core/src/mu_calculus/evaluator.rs) + [`parity_game_3v`](../../crates/mununu-core/src/mu_calculus/parity_game_3v.rs) — surface: API (the 3-valued evaluator; verdict-equivalence to the 2-valued path on Sharp-only inputs is a shipped done-criterion).

**The obligation this doc discharges.** The theorem is about the *evaluator*. Our
job is to show our **construction** satisfies the three preconditions (over,
under, labels). §4–§5 do that, per predicate kind.

## 4. The predicate-image construction

> Concept: implicit predicate abstraction — Cimatti–Griggio–Mover–Tonetta,
> "IC3 modulo theories via implicit predicate abstraction", TACAS 2014; the
> abstract image is a single SMT query, predicates over arbitrary terms.

The **uniform** abstract may-relation is one satisfiability query over the SMT
encoding of `T`:

```
(c, c') ∈ R_may   ⟺   ∃ s, i, i', s'.  T(s, i, s')  ∧  src@(s,i)  ∧  tgt@(s',i')
```

where `src@(s,i) = ⋀_k cube_bit(c,k)?  p_k(s,i)` and `tgt@(s',i')` is the
**primed** form of each predicate: `p_k` over term `t_k(state, input)` becomes
`t_k(s', i')` with `i'` a **fresh** next-cycle input. A pair is excluded only when
the solver proves the query UNSAT — so timeouts / unresolved terms conservatively
*keep* the edge (over-approximation preserved).

> Source of truth: [`smt_per_target_may_check`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs) — surface: API.

The must-relation is the dual ∀∃ obligation (UNSAT of its negation), so a pair is
*included* only when proved — failure conservatively *drops* it (never fabricates
a must-witness, preserving the under-approximation):

```
(c, c') ∈ R_must  ⟺   ∀ s ⊨ src.  ∃ i, i', s'.  T(s,i,s') ∧ tgt@(s',i')
```

> Source of truth: [`smt_per_target_must_check_standard`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs) + [`smt_hyper_must_check`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs) — surface: API.

**Why uniformity is the soundness lever.** With one rule, the over/under
preconditions of §3 hold for *every* predicate by the same argument — there is no
per-atom-kind encoding to get wrong. Every special case below is a *projection* of
this rule:
- **latch** `t_k = state_q`: `tgt` uses `s'` (`state_next`); no `i'`.
- **raw input** `t_k = in`: `src` pins `in`, `tgt` uses the *fresh* `i'` →
  existential in may (both flavours reachable — correct, the environment is free).
- **combinational** `t_k = g(state, input)`: `tgt` is `g(s', i')`.
- **relational / arithmetic**: `t_k` is the comparison/arith term itself.

The current code (pre-H.U) implements latch + raw-input directly and special-cases
combinational; §5 records exactly which projections are shipped-sound vs awaiting
H.U.

> Source of truth: [`build_pred_constraint`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs) — surface: API (today: per-kind branches; H.U: the one uniform rule above).

## 5. Per-case soundness ledger

| Predicate term kind | `src` encoding | `tgt` (primed) encoding | Status | Sound? |
|---|---|---|---|---|
| **latch** `state_q ⋈ v` | `state_curr` | `state_next` (`s'`) | shipped | **Yes** — the textbook case; over/under hold by the SMT image. |
| **free input** `in ⋈ v` (H.B) | pin `view.inputs[in]` | `true` (≡ fresh `i'` existential) | shipped | **Yes** — `tgt` free = "for all next-input flavours"; over-approx in may, and in must the next input is genuinely environment-free so both flavours are realizable. |
| **combinational-of-state** `g(state) ⋈ v` (H.E) | derived per-cube 3-valued **label** | the same label at the target cube (value determined by that cube's own state) | shipped (foundation) | **Yes** — `g(state)` is determined by `γ(c)`'s latch dimensions, so the label meets the §3 label precondition at every cube incl. targets. *Caveat:* no anchor exercises it end-to-end (sysrst's are output-line, §7). |
| **combinational-of-input** `g(state, input) ⋈ v` (H.E) | — | — | **deferred (SKIPPED)** | label is unsound (cube can't pin a free input → label not definite over `γ(c)`); `target-free` is unsound for must (`g(s',i')` is not freely both flavours → fabricates must-edges). The sound encoding is `tgt = g(s', i')` (§4) = **H.U.1**. Until then SKIP — never a misleading verdict. |
| **relational** `t1 ⋈ t2` (REL / H.F) | both terms at `(s,i)` | both terms at `(s',i')` | partial (state↔state shipped via `CmpReg`; input/comb operands deferred) | state↔state **Yes**; an input/comb operand needs the §4 primed form = H.U. |
| **arithmetic** `t == t' + k` (H.G) | term BV | primed term BV | deferred | a derived observer node (pure function of state, the `$past`-shadow regime) under H.U. |

**The empty-cube subtlety.** A cube whose dimension constraints are jointly UNSAT
(e.g. `state==0 ∧ state==1`) has `γ(c) = ∅`. A `KleeneT`/`KleeneF` label there is
*vacuously* consistent with §3 (the ∀ over an empty set), and such cubes are
unreachable from `I` so they cannot pollute the init verdict — but the may/must
*edge* checks must treat them correctly (an empty source has no must-obligation;
an empty target receives no must-edge). The SMT image handles this automatically
(an UNSAT `src`/`tgt` makes the query UNSAT). The H.E `smt_combinational_label`
guard against an empty cube was added under a *wrong* hypothesis (it was not the
cause of the spurious VIOLATED) — it is sound but is retired with the
derived-label machinery in H.U.2.

## 6. The must relation, inputs, and controllability

The ∀∃ must (§4) existentially chooses the input `i` and successor `s'` per source
state. For a **closed** system or one where inputs are demonic-for-safety this is
the standard KMTS must. For an **open** system the input partitions into
*environment* (uncontrollable) and *controller* (controllable) — the must-edge
quantifier alternation must respect that partition (∀ env ∃ ctrl for a controller
obligation). That is the controllability-aware KMTS of de Alfaro–Godefroid–
Jagadeesan (LICS 2004), tracked separately as the **R.6** arc; this doc's plain
∀∃ must is sound for the safety + non-controllability fragment the H-track
verify-auto path targets. A DEFINITE *controllability* verdict additionally
depends on R.6.8 (the per-player preservation audit) and is out of scope here.

> Source of truth: [`docs/design/kmts-theory.md`](kmts-theory.md) §7 — surface: (theory companion).

## 7. What is argued vs verified vs gap

- **Argued (this doc):** the over/under/label preconditions hold for the latch,
  free-input, and combinational-of-state cases; therefore definite verdicts on
  those transfer (Bruns–Godefroid).
- **Verified (tests):** the 3-valued evaluator equals the 2-valued path on
  Sharp-only inputs; the SMT may/must edge checks on small fixtures
  (`smt_must_edge` + `sts_ir` seam tests); the cone classifier; the
  combinational-label determinism on a state-cube. These check *components*, not
  the end-to-end transfer on real RTL.
- **Gap 1 — no end-to-end oracle.** No DEFINITE verify-auto verdict on real RTL
  has been cross-checked against an independent model checker. A spurious
  `HOLDS` (worse than a spurious `VIOLATED`) would be undetected. Closed by
  **H.O** (oracle harness): H.O.0 = differential vs mununu's own *exact*
  bit-blast path on small fixtures (catches spurious HOLDS on the shipped core);
  H.O.1 = external BTOR2 model checker on the real anchors.
- **Gap 2 — combinational-of-input not yet sound-and-bound.** Currently SKIPPED;
  closed by **H.U.1** (the primed term `g(s',i')`).
- **Gap 3 — the construction is argued, not mechanized.** The mapping in §5 is a
  pen-and-paper argument; the mis-steps show pen-and-paper alone is fallible.
  H.U makes the construction uniform so the argument reduces to inspecting one
  rule; H.O provides empirical evidence the rule is implemented correctly.

**Honest bottom line.** The state-predicate + free-input cases are on solid
footing and PO-backed; combinational-of-input is correctly *deferred*, not
wrongly bound; and "verify-auto produces a sound verdict on real RTL" is **not a
publishable claim until H.O lands**.

## 8. References

- E. M. Clarke, O. Grumberg, S. Jha, Y. Lu, H. Veith. *Counterexample-guided
  abstraction refinement.* CAV 2000.
- S. Graf, H. Saïdi. *Construction of abstract state graphs with PVS.* CAV 1997.
- G. Bruns, P. Godefroid. *Generalized model checking: reasoning about partial
  state spaces.* CONCUR 2000.
- P. Godefroid, R. Jagadeesan. *On the expressiveness of 3-valued models.*
  VMCAI/TACAS 2003.
- S. Shoham, O. Grumberg. *A game-based framework for CTL counterexamples and
  3-valued abstraction-refinement.* LMCS 2007.
- A. Cimatti, A. Griggio, S. Mover, S. Tonetta. *IC3 modulo theories via implicit
  predicate abstraction.* TACAS 2014.
- L. de Alfaro, P. Godefroid, R. Jagadeesan. *Three-valued abstractions of games.*
  LICS / CONCUR 2004. (controllability axis; R.6.)
- Cross-refs: [`kmts-theory.md`](kmts-theory.md),
  [`predicate-abstraction-recipe.md`](predicate-abstraction-recipe.md),
  [`abstraction-literature.md`](abstraction-literature.md).
