# Combinational-of-input atoms in predicate-cube verification — a soundness analysis

> Status: planning — this document analyses the current verify-auto predicate-cube
> abstraction and proposes a sound treatment for *combinational-of-input* atoms.
> The core contribution (the nested-∀i′ hyper-must, §6.2) is a proposal, not yet
> implemented; the derived-⊥ label (§6.1) revives a mechanism removed in H.U.2b.
> Exempt from `Source of truth:` anchors per CLAUDE.md (planning doc); inline code
> references point at the shipped artifacts the proposal builds on.

> Audience: a reviewer evaluating whether the proposed treatments are *sound* (their
> definite verdicts transfer to the concrete RTL for all input sequences). The
> document is self-contained: §3 fixes the model, §4 the problem, §5/§6 the
> refutations and the sound construction.

---

## 1. Motivation

Mununu's no-sidecar SystemVerilog-assertion verifier (`verify-auto`) lifts an RTL
module to a word-level transition system (BTOR2), abstracts it into a predicate
cube, and evaluates each translated SVA over a 3-valued (Kleene) modal-mu
semantics. Two real OpenTitan modules anchor the regression suite:
`csrng_main_sm` and `sysrst_ctrl_detect`. Measured end-to-end (2026-06-30, in the
`mununu-sva` image):

| anchor | translated | HOLDS | SKIPPED | unsupported |
|---|---|---|---|---|
| `csrng_main_sm` | 2 | 1 (sva_0) | 1 (sva_1) | 0 |
| `sysrst_ctrl_detect` | 15 | 1 (sva_0) | 14 | 1 (sva_15, arithmetic) |

Fifteen of the sixteen SKIPs share one root cause. A cone analysis
(`cone_reaches_input`, [`adapter/btor2/parser.rs`](../../crates/mununu-core/src/adapter/btor2/parser.rs))
of every combinational signal the SVA reference shows them **all** to be functions
of a primary input, not of state alone:

```
main_sm_err_o          = f(state, enable_i, local_escalate_i)
event_detected_o       = f(state, cfg_*, trigger_i)
event_detected_pulse_o = f(state, cfg_*, trigger_i)
cnt_clr / trigger_active / trigger_event = f(state, trigger_i)
```

The genuinely state-only combinational outputs (`main_sm_state_o`, the lifted
`state_q` / `cnt_q` aliases) are either already bound as state aliases or
referenced by no assertion. So the dominant gap between "an SVA translates" and "an
SVA reaches a verdict" is the handling of **combinational-of-input atoms** — a
named combinational signal, appearing in a property, whose value depends on both
the current state and a free primary input.

Today these atoms are soundly **skipped** (the property reports `Skipped`, never a
wrong verdict). The goal of this analysis is to determine whether they can instead
be made to reach a *definite* verdict, or an honest `⊥`, **without sacrificing
soundness for all input sequences**.

---

## 2. Background

The pieces this analysis builds on, each already shipped:

- **Predicate-cube lift** ([`adapter/btor2/kmts_lift.rs`](../../crates/mununu-core/src/adapter/btor2/kmts_lift.rs)).
  Abstract states are cubes (Boolean valuations) over a predicate set `P`; the
  abstract transition relation carries *may* (over-approximating) and *must*
  (under-approximating) edges, forming a Kripke Modal Transition System (KMTS;
  Larsen–Thomsen 1988, Larsen 1989).
- **3-valued modal-mu evaluator.** Verdicts in `{KleeneT, KleeneF, KleeneBot}`
  over the information order. The Bruns–Godefroid preservation theorem (CONCUR
  2000) is the soundness backbone: a definite abstract verdict (`KleeneT` /
  `KleeneF`) transfers to the concrete system at every alternation depth.
- **Uniform predicate-image** (H.U; [`docs/design/predicate-image-soundness.md`](predicate-image-soundness.md)).
  A predicate is encoded as the value of an arbitrary BTOR2 *term* over the current
  cycle `(s,i)` (source) and the next cycle `(s′,i′)` (target, via a "primed"
  re-evaluation of the netlist). This replaced the per-atom-kind special cases with
  one rule.
- **Free-input atoms** (H.B; [`docs/design/free-input-atoms.md`](free-input-atoms.md)).
  A *raw* primary input is admitted as a free cube dimension: in the may/must SMT it
  is **source-pinned / target-free**. The soundness argument is that the
  may-relation then ranges over all environment choices ("for all input sequences"),
  the over-approximation the safety fragment needs.
- **Shipped must-edge inference** (`MustEdgeInference`,
  [`kmts_lift.rs`](../../crates/mununu-core/src/adapter/btor2/kmts_lift.rs)):
  `Off` (may-only), `SmtPerTarget` (`∀s⊨src. ∀i. transition ⟹ next⊨tgt`),
  `SmtPerTargetStandard` (the canonical KMTS `∀s⊨src. ∃i. next⊨tgt`), and
  `SmtHyperMust` (the GKMTS hyper-must; Shoham–Grumberg LMCS 2007).
- **The H.O oracle** ([`adapter/btor2/concrete_oracle.rs`](../../crates/mununu-core/src/adapter/btor2/concrete_oracle.rs)
  + [`adapter/btormc/mod.rs`](../../crates/mununu-core/src/adapter/btormc/mod.rs)).
  An independent verdict — bounded concrete reachability (internal) plus an external
  symbolic model checker (`btormc --kind`) — used as a differential oracle. It is
  the validation instrument for any new verdict path proposed here.

---

## 3. Definitions

**Concrete system.** A deterministic word-level design is
`M = (S, I, δ, s₀)` with state space `S`, primary-input space `I`, next-state
function `δ : S × I → S`, and reset state `s₀`. A *run* is an infinite sequence
`s₀, i₀, s₁, i₁, …` with `s_{t+1} = δ(s_t, i_t)`. Inputs are **demonic**: a sound
verdict for a safety/liveness property must hold for *every* input sequence
`i₀ i₁ … ∈ Iᵒᵐᵉᵍᵃ`. This is the only environment model under which "the property
holds on the RTL" is a meaningful claim.

**Signals and predicates.** A *combinational signal* is a function `g : S × I → 𝔹`
(the value of a netlist node; in BTOR2 it is a node whose cone may reach `state`
cells and `input`s). A *predicate* `p ∈ P` is a comparison whose truth is a
function `p : S × I → 𝔹`. By the shape of that dependence we classify:

| kind | form | example |
|---|---|---|
| **state** | `p(s,i) = p̂(s)` | `state_q == 3` |
| **free-input** | `p(s,i) = (iⱼ ⋈ v)` | `cfg_enable_i` |
| **combinational-of-state** | `p(s,i) = ĝ(s)`, cone state-only | `main_sm_state_o == IDLE` |
| **combinational-of-input** | `p(s,i) = g(s,i)`, cone reaches an input | `trigger_active`, `main_sm_err_o` |

**Cubes.** A cube `c ∈ 𝔹^P` is a Boolean valuation of the predicates. Its
concretization is the set of consistent (state, input) pairs:
`γ(c) = { (s,i) ∈ S × I : ∀p∈P. p(s,i) = c(p) }`.
Write `cube(s,i)` for the unique `c` with `c(p) = p(s,i)`.

**Abstract edges.** Following [`kmts-theory.md`](kmts-theory.md), with the source/
target input distinction the uniform image makes explicit (`i` is the
transition/source input, `i′` the next-cycle/target input):

- **may** `c →◇ c′` ⟺ `∃ s,i,i′. (s,i)∈γ(c) ∧ (δ(s,i), i′)∈γ(c′)`.
- **must** `c →□ c′` ⟺ (standard KMTS) `∀ (s,i)∈γ(c). ∃ i′. (δ(s,i), i′)∈γ(c′)`.
- **hyper-must** `c →□ T`, `T ⊆ 𝔹^P` ⟺ `∀ (s,i)∈γ(c). ∀ i′. ∃ c′∈T. (δ(s,i), i′)∈γ(c′)`.

**Predicate position.** In a formula, an atom occurs in **source position** if it is
evaluated at the current cube (e.g. the antecedent `A` of `νX.((¬A ∨ □C) ∧ □X)`),
and in **target position** if it is evaluated at a successor cube (under a `□`/`◇`;
e.g. the `C` inside `□C`). The distinction is decisive: a source-position atom reads
`p(s,i)`; a target-position atom reads `p(s′,i′)` with `i′` the *next* input.

**Atom value at a cube (3-valued label).** For an atom `p` and cube `c`:
`L(c,p) = KleeneT` if `∀(s,i)∈γ(c). p(s,i)=1`; `KleeneF` if `∀(s,i)∈γ(c). p(s,i)=0`;
`KleeneBot` otherwise. This is the standard 3-valued labelling; a definite label
means `p` is constant across `γ(c)`.

---

## 4. Problem statement

A combinational-of-input atom `p(s,i) = g(s,i)` is, by definition, **not constant
over a state cube**: fixing the abstract state still leaves the input free, and
`g` depends on it. Two consequences make it the hard case.

**(P1) Source position.** At the current cube `c`, `L(c,p) = KleeneBot` whenever
`γ(c)` contains pairs with `g=0` and `g=1` — which is the generic situation, since
the cube constrains state but the input ranges freely. So a source-position
combinational-of-input atom is honestly `⊥` under the 3-valued labelling.

**(P2) Target position.** The target value `g(s′,i′)` depends on the *next* input
`i′`, which the environment chooses demonically. A target predicate that pins `g`'s
bit therefore constrains `i′`. Encoding this incorrectly is unsound; §5 proves two
naive encodings wrong.

The status quo (`verify-auto`'s seeder,
[`adapter/slang/verify_auto.rs`](../../crates/mununu-core/src/adapter/slang/verify_auto.rs))
routes a combinational-of-input atom to `unseedable` → the property is `Skipped`.
Skipping is sound (no claim is made) but leaves 15/16 anchor SKIPs unaddressed.

**Goal.** Make a combinational-of-input atom *bind* — reach a definite verdict
where the design and property admit one, or an honest `⊥` otherwise — while
preserving: a definite abstract verdict transfers to the concrete RTL for all input
sequences.

---

## 5. Refutations of the naive encodings

Two "obvious" encodings are unsound. Pinning down *why* is what the sound proposal
in §6 must respect.

### 5.1 Target-free is unsound for the must-relation

The free-input treatment (H.B) makes a target predicate **target-free**: its bit is
left unconstrained at the successor. For a *raw* input this is sound — `i′ⱼ` is
genuinely independent, so leaving it free models "the environment picks any next
input". The claim under refutation: *the same target-free shortcut is sound for a
combinational-of-input target predicate.*

> **Proposition 1.** Target-free is sound for the may-relation but **unsound for the
> must-relation** when the target predicate is combinational-of-input.

*Proof.* May: leaving a target bit free can only *add* may-edges (it drops a
conjunct from the existential), and the may-relation is an over-approximation, so
extra may-edges preserve soundness — they can only turn a definite verdict into
`⊥`, never flip it (Bruns–Godefroid). ∎(may)

Must (refutation by counterexample). Take `S = {s₀}` (one state, self-looping),
`I = {0,1}`, `δ(s₀,i)=s₀`, and a combinational-of-input signal `g(s,i) = i`. Let the
predicate be `p ≡ (g == 1)`, so `P = {p}`, and consider the two cubes `c₀ = {p↦0}`,
`c₁ = {p↦1}`. The concrete next state is always `s₀`; whether the *successor cube*
is `c₀` or `c₁` is decided by the next input `i′`. A target-free must check for the
edge `c₁ →□ c₁` drops `p`'s target conjunct, so it succeeds (`∀(s,i)∈γ(c₁). ∃ s′.
δ=s′` holds trivially), claiming `c₁` *definitely* re-reaches `c₁`. But the
environment can pick `i′=0`, sending the system to `c₀`. The must-edge is fabricated.
A diamond `⟨a⟩p` would then read `KleeneT` from this fabricated must-witness while the
concrete system, under the input sequence `i′=0`, refutes it — an unsound definite
verdict. ∎(must)

This is exactly the failure H.E observed empirically: treating
combinational-of-input targets as free dimensions produced spurious `VIOLATED`s.

### 5.2 Existential i′ ("same ∀-block") fabricates must-edges

The second naive fix keeps `p`'s target conjunct but quantifies the next input `i′`
in the *same block* as the transition's source input and next-state — i.e. it
reuses the standard must form `∀(s,i)∈γ(c). ∃(i, s′, i′). transition ∧ (s′,i′)∈γ(c′)`,
letting `i′` be existential.

> **Proposition 2.** With demonic inputs, an existential `i′` yields *angelic*
> must-edges and is unsound.

*Proof.* The standard KMTS must-edge `c →□ c′ ⟺ ∀(s,i)∈γ(c). ∃ i′. (δ(s,i),i′)∈γ(c′)`
is sound only when `c′`'s satisfaction does **not** hinge on the environment's
choice — i.e. when the target predicates are input-independent, so the `∃ i′` is
vacuous. When `c′` contains an input-dependent predicate, `∃ i′` lets the *prover*
choose a favourable next input to land in `c′`. That is an **angelic** environment:
"there exists a next input under which the successor is in `c′`." A must-edge is a
guarantee used to establish `⟨a⟩φ` (a reachable witness) and to refute `□φ`; under
demonic inputs the guarantee must hold for the inputs the environment *actually*
picks, i.e. for *all* `i′`, not for a convenient one. Concretely, reusing the
Proposition 1 system: `∃ i′. (s₀,i′)∈γ(c₁)` holds (pick `i′=1`), so `c₁ →□ c₁` is
claimed, and again the input sequence `i′=0` refutes it. ∎

The contrapositive is the design constraint: **the next input `i′` must be
universally quantified, nested inside the existential transition choice.**

---

## 6. Proposed approach

Two complementary, individually-sound treatments. They differ in which formula
positions they handle and in how much precision they recover.

### 6.1 Derived 3-valued ⊥-label (source position)

Treat a combinational-of-input atom as a **derived label**, not a cube dimension:
it does not partition cubes or participate in edge construction; instead, per cube
`c`, an SMT pass assigns `L(c,p)` by the rule in §3 — `KleeneT`/`KleeneF` if `p` is
constant over `γ(c)`, else `KleeneBot`. The evaluator already consumes 3-valued
state labels (`state_3valued_predicates`); this is the mechanism removed in H.U.2b
(`smt_combinational_label`), re-targeted from combinational-of-*state* (where it was
dead — those bind as cube dimensions) to combinational-of-*input* (where the label
is genuinely `⊥`).

> **Proposition 3 (soundness).** The derived ⊥-label preserves the Bruns–Godefroid
> guarantee: any definite verdict of the evaluator transfers to the concrete system.

*Proof.* A derived label is a *pure observation*: it adds no may- or must-edge and
removes none, so the modal transition structure — over which preservation is
proved — is unchanged. The label itself is sound by construction: `KleeneT`
(resp. `KleeneF`) is emitted only when `p` is *proved* constant `1` (resp. `0`)
across all of `γ(c)` (an UNSAT result), so it matches every concretization; `⊥`
claims nothing. Sound labels on an unchanged sound KMTS preserve the 3-valued
preservation theorem. ∎

**Why this still yields *definite* verdicts.** A `⊥` atom does not poison a safety
implication. The sysrst safety SVA have the shape `AG(A → AX C)`, i.e.
`νX.((¬A ∨ □C) ∧ □X)`, with the combinational-of-input atom only inside the
antecedent `A` (source position). In Kleene logic `⊥ ∨ KleeneT = KleeneT`. So at any
cube where the consequent `□C` is *definitely* true, `¬A ∨ □C = KleeneT`
irrespective of `A`'s `⊥`; where `A` is *definitely* false (e.g. a state mismatch),
`¬A = KleeneT` directly. The property is `KleeneT` iff the consequent carries it at
every cube the antecedent does not already exclude — a genuinely useful definite
verdict that needs *no* information about the input-dependent antecedent. (On the
anchors this is sva_6/8/9/13/14.)

**Boundary.** For a *target-position* combinational-of-input atom (under a `□`/`◇`;
sysrst sva_12, `event_detected_pulse_o` inside `□`), the label at each successor cube
is `⊥`, so `□⊥` evaluates to `⊥` (no may-successor is definitely T, no must-successor
definitely F). The derived label is **sound but indefinite** here — an honest `⊥`,
not a verdict. Recovering a definite verdict for target position is §6.2.

**The H.E refutation, resolved.** H.E reported the derived label unsound. The fault
was emitting a *definite* label for a combinational-of-input atom by reading it off
the state-cube while ignoring the free input — i.e. computing `KleeneT`/`KleeneF`
where the sound answer is `⊥`. Proposition 3 holds precisely because the label is
computed by the two-sided UNSAT check that returns `⊥` whenever the input can swing
`p`; the H.O oracle (§2) gates this empirically (a derived-label `KleeneT` that the
oracle refutes is a soundness bug, caught before merge).

### 6.2 Nested-∀i′ hyper-must (target position)

To give a *definite* verdict for target-position combinational-of-input atoms, the
must-relation must quantify the next input correctly. Per §5.2 the sound form nests
`∀ i′` inside the existential transition choice. Because, for a fixed source, the
demonic `i′` can land the successor in different cubes, the sound object is a
**hyper-must edge to a target *set*** (GKMTS; Shoham–Grumberg LMCS 2007), the
`MustHyperOnly { targets }` modality mununu already carries:

```text
c →□ T   ⟺   ∀ (s,i) ∈ γ(c).  ∀ i′.  ∃ c′ ∈ T.  (δ(s,i), i′) ∈ γ(c′)
```

Operationally (the SMT shape, extending `smt_hyper_must_check` in
[`adapter/btor2/smt_must_edge.rs`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs)):
for a candidate target set `T`, the source state+input is universally quantified
(it ranges over `γ(c)`), the transition is functional (`s′ = δ(s,i)`), and the
**next input `i′` is universally quantified in its own block**, asserting that for
every `i′` the resulting successor cube lies in `T`. The combinational-of-input
target predicate `g(s′,i′)` is evaluated over `(state_next, i′)` via the uniform
image's primed cache — already the term-resolution H.U built; the missing piece is
the `∀ i′` quantifier placement and the target-*set* membership.

> **Proposition 4 (soundness).** A hyper-must edge `c →□ T` established by the
> nested-∀i′ query is a sound under-approximation: every concrete pair in `γ(c)`
> has, for every next input, a successor in `⋃_{c′∈T} γ(c′)`.

*Proof.* The query is logically `∀(s,i)∈γ(c). ∀ i′. ∃ c′∈T. (δ(s,i),i′)∈γ(c′)`
(an UNSAT on its negation). This is exactly the GKMTS hyper-must concretization
condition (Shoham–Grumberg LMCS 2007, §3): the successor `δ(s,i)` paired with *any*
demonic `i′` is covered by the target set. By the GKMTS preservation theorem,
hyper-must edges soundly witness `⟨a⟩`/refute `□` at every alternation depth,
including the alternating fixpoints (νμ) where standard KMTS must-edges are
non-monotone under refinement. ∎

> **Proposition 5 (refutation of the singleton must).** A *singleton* target
> (`T = {c′}`) hyper-must with a combinational-of-input target predicate is, in
> general, unsatisfiable, which is why the *set* is essential.

*Proof.* `∀ i′. (δ(s,i), i′) ∈ γ(c′)` forces `g(s′, i′)` to equal `c′(p)` for *all*
`i′`. If `g` genuinely depends on `i′` at `s′` (the defining property of
combinational-of-input), some `i′` makes `g` differ, so no singleton `c′` is
reached for all `i′`. The demonic `∀ i′` is satisfiable only against a target *set*
that includes both polarities of `p` — recovering the same indefiniteness the
⊥-label expresses, but now *inside* a sound must-witness, so the surrounding
formula can still resolve to a definite verdict when its other operands carry it. ∎

**Cost and scope.** The nested-∀i′ query adds a quantifier alternation (`∀i′` under
`∃` transition under `∀` source) to the must check — Z3 handles this for the
bit-vector fragment but it is heavier than the shipped `∀∃` forms, and the target
*set* enumeration is the GKMTS cost. It is the genuine, complete treatment; it is
also the largest and most soundness-delicate change, and should land behind the H.O
oracle differential on both anchors.

### 6.3 Relation to the relational/arithmetic atoms (H.F/H.G)

A relational atom with an input operand (`cnt_q >= cfg_detect_timer_i`, H.F) is a
combinational-of-input predicate whose term is the comparison; the §6.1/§6.2
treatments subsume it once the seeder admits an input operand. An arithmetic operand
(`cnt_q == cnt_q__past + 1`, H.G) is orthogonal: it needs an arithmetic term in the
predicate layer (today `PredicateExpr` is arithmetic-free), independent of the
input-dependence question analysed here.

---

## 7. Complexity and computational cost

The treatments differ not only in soundness reach (§6) but in how they scale. This
section fixes a cost model and places each approach in it, so the decision weighs
precision against compute.

### 7.1 Parameters

| symbol | meaning | typical anchor value |
|---|---|---|
| `n = \|P\|` | number of cube predicates (dimensions) | ~3–8 |
| `K` | number of *reachable* abstract cubes (≤ `2ⁿ`, usually far fewer) | ~tens |
| `E` | candidate (source,label,target) edge triples examined | `O(K² · \|labels\|)` |
| `m` | combinational-of-input atoms in the property | ~1–3 |
| `W` | total primary-input bit-width (the `i′` the env controls) | tens–hundreds |
| `R` | state-register bit-width (the BTOR2 `state` cone) | tens |

The unit cost is a single SMT solve over the lifted BTOR2 transition. The shipped
edge inference is quantifier-free bit-vector (`QF_BV`): NP-complete, but the solves
are over one transition unrolling and are individually cheap in practice (the
anchors' must passes complete in well under a second per
[`smt_must_edge.rs`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs)).
The cost question is therefore (a) how many solves, and (b) whether any solve leaves
`QF_BV` for the quantified fragment.

### 7.2 Baseline (shipped) costs

- **may** `c →◇ c′` — one `∃` solve (a `QF_BV` SAT) per examined cube pair:
  `O(E)` solves.
- **must** per-target — `SmtPerTarget` is a `QF_BV` validity check
  (`∀s∀i. transition ⟹ next⊨tgt`, discharged as one UNSAT); `SmtPerTargetStandard`
  is the `∀∃` form, discharged with a single quantifier instantiation over the
  source. Both are `O(E)` solves and stay effectively in `QF_BV` because the only
  universal is the source state, eliminated by the UNSAT framing.

So the abstraction's baseline is `O(E)` `QF_BV` solves, with `E = O(K² · \|labels\|)`
and `K ≤ 2ⁿ`. The dominant blow-up axis is `n` (the cube dimension count): each
added predicate at most doubles `K`, hence quadruples `E`.

### 7.3 Derived ⊥-label (§6.1) — cheap, additive, no dimension growth

The label is computed **per cube, per combinational atom**, by two `QF_BV` UNSAT
checks (`is it constant 1?` / `is it constant 0?`):

- **Solves added:** `2 · K · m` `QF_BV` solves — *additive*, run once after the
  cubes are built. No new edge queries.
- **Cube count:** **unchanged.** A derived label is *not* a cube dimension, so `n`
  and `K` (and therefore the `O(E)` edge cost) are untouched. This is the key
  scaling property: the ⊥-label adds precision at the source position for the price
  of `O(K·m)` extra cheap solves and **zero** exponential growth.
- **Fragment:** stays in `QF_BV` (each check is a one-cycle constancy query over the
  cube's state constraint plus the free input — no quantifier alternation).

Cost class: **linear in `K·m`, no change to the exponential base.** This is the
inexpensive treatment.

### 7.4 Nested-∀i′ hyper-must (§6.2) — quantifier alternation + target-set search

Two cost sources compound:

1. **Quantifier alternation.** The query `∀(s,i)∈γ(c). ∀i′. ∃c′∈T. (δ(s,i),i′)∈γ(c′)`
   has a genuine `∀i′` *inside* the existential transition choice that cannot be
   removed by the UNSAT framing (unlike the source `∀s`, which can). It leaves
   `QF_BV` for **quantified BV (BV with one alternation)**. For finite bit-vectors
   this is decidable, but Z3 discharges it by bit-blasting / instantiating the `∀i′`
   — worst case `2ᵂ` instantiations in `W` input bits, or a symbolic QBF-style
   search. In theory quantified BV is far harder than `QF_BV` (the alternation is
   the source of the jump); in practice cost is governed by `W` (the input
   cone of the combinational signal, often a handful of bits, not the full `W`) and
   Z3's quantifier engine.
2. **Target-set enumeration.** By Proposition 5 a singleton target fails; the sound
   object is a *set* `T`. Enumerating/searching candidate `T ⊆ 𝔹ⁿ` is the GKMTS
   cost — bounded by the cubes actually reachable from `c` under any `i′`
   (`\|T\| ≤ K`), but each candidate `T` is itself a hyper-must solve.

If, instead of a derived label, the combinational atom is admitted as a **cube
dimension** (so target position is decided by the cube structure), `n` grows by `m`
and `K` (hence `E`) grows by up to `2ᵐ` — the exponential axis the ⊥-label avoided.

Cost class: **a quantifier-alternation solve per source cube (governed by the
combinational signal's input-cone width), plus up to `2ᵐ` cube blow-up if realised
as dimensions.** This is the expensive, complete treatment.

### 7.5 Summary table

| treatment | solves added | solve fragment | cube-count (`K`) impact | reach |
|---|---|---|---|---|
| skip (status quo) | 0 | — | none | none |
| derived ⊥-label (§6.1) | `2·K·m` (`QF_BV`) | `QF_BV` | **none** | source position; definite via Kleene `⊥∨T=T` |
| nested-∀i′ hyper-must (§6.2) | `O(K·\|T\|)` per atom | **quantified BV** (∀i′) | `×2ᵐ` if dimensions | source **and** target position |

### 7.6 What to measure before committing to §6.2

The §6.2 cost is the one with a phase change (`QF_BV` → quantified BV). Before
implementing it, measure on the anchors: (a) the input-cone width of each target
combinational signal (the effective `W` the `∀i′` bit-blasts over — `event_detected_pulse_o`'s
cone, not all of `sysrst`'s inputs); (b) Z3 wall-clock for one nested-∀i′ solve at
that width; (c) the reachable target-set size `\|T\|`. If the input cone is small
(a few bits) the `2ᵂ` instantiation is negligible and the hyper-must is affordable;
if it is wide, the quantified-BV solve, not the cube count, becomes the bottleneck —
and the ⊥-label (which never leaves `QF_BV`) is the pragmatic floor. The H.O oracle
bounds correctness independently of which is chosen.

---

## 8. Conclusion

The dominant remaining gap on both real-RTL anchors is the **combinational-of-input
atom**, not the relational or arithmetic forms the roadmap originally flagged. The
analysis yields a clear, layered picture:

1. **Skipping is sound but maximally imprecise** (the status quo).
2. **The derived ⊥-label (§6.1) is sound and, for the prevalent
   `AG(A→AX C)` safety shape, frequently *definite*** — via Kleene `⊥∨T=T`, the
   verdict rides on the consequent and ignores the indefinite antecedent. It is the
   smaller change (reviving a 3-valued labeller, gated by the H.O oracle) and
   addresses every source-position combinational-of-input atom (anchor
   sva_6/8/9/13/14). Its boundary is target-position atoms, where it yields an
   honest `⊥`.
3. **The nested-∀i′ hyper-must (§6.2) is the complete sound treatment**, definite
   for target-position atoms too (sva_12), at the cost of a quantifier-alternation
   SMT query and GKMTS target-set enumeration. It is sound precisely because the
   next input is universally quantified (§5.2) against a target *set* (§6.1
   refutation / Proposition 5).

Both treatments are sound by the same backbone — a definite verdict over an
unchanged (⊥-label) or soundly-extended (hyper-must) KMTS transfers to the concrete
RTL for all input sequences (Bruns–Godefroid; Shoham–Grumberg for the GKMTS case).
The two naive shortcuts are refuted: target-free fabricates must-edges
(Proposition 1), and an existential next input makes them angelic (Proposition 2).

**Open questions for the decision point.**
- Is the ⊥-label's reach (source-position, definite-when-consequent-carries)
  sufficient value to justify reviving the H.U.2b-deleted machinery, or should the
  effort go straight to the nested-∀i′ hyper-must?
- For the nested-∀i′ query, what is the practical Z3 cost on the anchors, and does
  the GKMTS target-set enumeration stay bounded at the cube sizes verify-auto
  produces?
- Should H.F (relational-with-input) land first as the seeder-admission half, so
  that whichever combinational treatment ships immediately flips sva_5/7/10/11 as
  well?

These are the inputs to the scope decision; this document deliberately stops short
of recommending one, per the request to lay out the science first.

---

## References

- G. Bruns, P. Godefroid. *Model checking partial state spaces with 3-valued
  temporal logics.* CONCUR 2000. (3-valued preservation.)
- K. G. Larsen, B. Thomsen. *A modal process logic.* LICS 1988; K. G. Larsen,
  *Modal specifications*, 1989. (KMTS.)
- D. Dams, R. Gerth, O. Grumberg. *Abstract interpretation of reactive systems.*
  TOPLAS 1997. (Mixed transition systems; must vs may.)
- S. Shoham, O. Grumberg. *A game-based framework for CTL counterexamples and
  3-valued abstraction-refinement.* LMCS 2007. (GKMTS, hyper-must, monotone
  refinement on alternating fixpoints.)
- Internal: [`docs/design/kmts-theory.md`](kmts-theory.md),
  [`docs/design/predicate-image-soundness.md`](predicate-image-soundness.md),
  [`docs/design/free-input-atoms.md`](free-input-atoms.md).
