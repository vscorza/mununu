# KMTS Theory — Kripke Modal Transition Systems and 3-Valued Mu-Calculus

> **Concept: Kripke Modal Transition Systems and 3-valued mu-calculus — theoretical foundations for sound abstraction over the full mu-calculus.** Companion to [`native-sv-abstraction.md`](native-sv-abstraction.md) (the architecture doc) and [`predicate-abstraction-recipe.md`](predicate-abstraction-recipe.md) (the practical recipe). This doc anchors the architecture's central design choice (KMTS + 3-valued mu-calculus) in the published literature; it does not anchor live code and is exempt from `> Source of truth:` per [`CLAUDE.md`](../../CLAUDE.md) §Documentation Traceability.

## §1 Motivation — why 2-valued abstraction fails for liveness

Model checking with abstraction follows a familiar shape: replace a concrete transition system `M = (S, R, V)` with an abstract `M^# = (S^#, R^#, V^#)` over a coarser state space, evaluate a temporal formula φ on `M^#`, and infer something about `φ`'s value on `M`. The discipline of *sound* abstraction is what makes the inference legitimate. Two-valued model checking with a single abstract relation `R^#` admits exactly one direction of inference well: when `R^#` is an *over-approximation* (every concrete transition has an abstract counterpart, i.e. `R ⊆ γ(R^#)` under a Galois connection γ), `M^# ⊨ φ` implies `M ⊨ φ` for universal-fragment formulas — typical safety / invariance. The dual works for `R^#` an *under-approximation* (`γ(R^#) ⊆ R`) and existential-fragment formulas — typical reachability targets.

The mu-calculus mixes both fragments. A safety formula `νX. (φ ∧ □X)` is sound under over-approximation; a reachability formula `μX. (φ ∨ ◇X)` is sound under under-approximation; a `νX. (φ ∧ ◇X)` ("φ holds along some infinite path") is sound under *neither* alone. For full mu-calculus, a single-relation abstraction is fundamentally insufficient — *neither* over- nor under-approximation alone preserves the truth value of formulas with alternating fixpoints.

### §1.1 Worked counterexample

Concrete `M`:

```text
       a
      ┌─┐
      ▼ │
      s0 ──a──► s1
              ◯
              (deadlock)
```

`M` has two states. State `s0` self-loops on `a`; state `s1` is a deadlock with no successor. Atomic proposition `p` holds at `s1` only.

Property: `φ = μX. (p ∨ ⟨a⟩X)` — "p is reachable along some `a`-path." Concrete answer: `M, s0 ⊨ φ` (take the `a`-edge from `s0` to `s1`, where `p` holds).

Over-approximate abstraction `M^#` that merges `s0` and `s1` into a single abstract state `b`. Self-loop on `a` survives by over-approximation (`b` has an `a`-successor `b`). Atomic proposition `p` is now uncertain at `b` — concretely it depends on which concrete state `b` represents. A 2-valued abstraction must pick one valuation for `p` at `b`; the only sound choice for an over-approximation of a universal property would be `p = false`, since over-approximation gives "could be anywhere."

The abstract formula `μX. (p ∨ ⟨a⟩X)` at `b` evaluates to `false`: `p` is `false` at `b`, and the only `a`-successor of `b` is `b` itself, so the least-fixpoint stabilises at `false`. The abstraction tells us `M, s0 ⊭ φ`, which is *false on the concrete*. Over-approximation has flipped a true reachability verdict to false — unsound for `μ`.

Try under-approximation: drop the `a`-edge entirely if the abstraction cannot witness it on every concrete pair. Then `b` has no `a`-successor; the formula evaluates to `false` again, and we draw the same incorrect conclusion. Under-approximation is sound for `μX. (φ ∨ ⟨a⟩X)` *only when the witness edges are preserved* — and a coarse abstraction may not preserve them. The asymmetry is fundamental: the formula needs an existential witness from the under-approximation *and* a "no other behaviour" guarantee from the over-approximation, and a single-relation abstract model provides only one direction.

### §1.2 The KMTS resolution

A Kripke Modal Transition System carries *both* relations in one structure: a *may* relation `R_may` (over-approximation; `R ⊆ γ(R_may)`) and a *must* relation `R_must` (under-approximation; `γ(R_must) ⊆ R`), with the invariant `R_must ⊆ R_may`. The 3-valued evaluation reads both:

- Existential modalities (`⟨a⟩φ`) require a `must`-witness to claim `true` — guaranteeing the concrete behaviour exists.
- Universal modalities (`[a]φ`) require all `may`-successors to satisfy `φ` to claim `true` — covering every concrete behaviour.
- The third verdict, `⊥` ("unknown"), records the cases where the abstraction is too coarse to give a definite answer; refinement responds to `⊥` by tightening one of the two relations.

The preservation theorem (§4.3) says that `true` and `false` verdicts on the abstract transfer to the concrete *for the full mu-calculus including alternating fixpoints* — a uniformly sound abstraction story, with refinement well-understood. Re-running the §1.1 example as a KMTS: `b` has a `may`-`a`-self-loop (over-approximation preserves it) but no `must`-`a`-edge (the abstraction cannot witness a concrete `a`-edge for every concretisation of `b`). The formula `μX. (p ∨ ⟨a⟩X)` at `b` evaluates to `⊥` — the `⟨a⟩` modality requires a `must`-successor that does not exist, but neither does it have all `may`-successors falsifying the formula. Refinement adds predicates that distinguish `s0` from `s1`, demoting `⊥` to `true` once the abstract structure separates the deadlock from the cycle.

This is the framework the rest of the doc develops.

## §2 KMTS — formal definition

### §2.1 Larsen–Thomsen modal transition systems

Larsen and Thomsen introduced *modal transition systems* (LICS 1988) as a refinement framework for process algebra. A modal transition system is a triple `M = (S, R_must, R_may)` over a labelled alphabet `Act`, where `R_must, R_may ⊆ S × Act × S` are *required* and *admitted* labelled-transition relations satisfying `R_must ⊆ R_may`. The intuition: a `must`-edge `(s, a, s')` is a transition every implementation of `M` *must* exhibit; a `may`-edge is a transition every implementation *may* exhibit; the inclusion `R_must ⊆ R_may` is the natural consistency condition.

Larsen (1989, *Modal Specifications*) developed the refinement notion built on this — what it means for one modal transition system to be a more concrete specification of another (§3 of this doc). The framework was originally an *under-specification* language: an implementer reading a modal transition system as a contract knows which behaviours are mandatory (must), which are optional (may), and which are forbidden (not in may).

### §2.2 KMTS — Kripke Modal Transition Systems

Huth, Jagadeesan, and Schmidt (TACAS 2001) extended Larsen–Thomsen MTS with Kripke-style state labelling to support full mu-calculus model checking. A **Kripke Modal Transition System** (KMTS) over an alphabet `Act` and an atomic-proposition set `AP` is a 5-tuple

```text
M = (S, S_0, R_must, R_may, L)
```

where

- `S` is a set of states;
- `S_0 ⊆ S` is a set of initial states (often a single `s_0`);
- `R_must ⊆ S × Act × S` is the *must* transition relation;
- `R_may ⊆ S × Act × S` is the *may* transition relation, with the invariant `R_must ⊆ R_may`;
- `L: S × AP → {T, F, ⊥}` is a 3-valued state labelling, with `T` and `F` the standard Boolean values and `⊥` the "unknown" value (Kleene's strong 3-valued logic).

The labelling generalises the standard Kripke-structure valuation `V: S × AP → {T, F}` in the same way `R_may / R_must` generalises a single `R`: `L(s, p) = T` asserts `p` holds in `s` in every concretisation; `L(s, p) = F` asserts `p` fails in `s` in every concretisation; `L(s, p) = ⊥` records that the abstraction has merged concretisations where `p` differs. A standard Kripke structure is the KMTS where `R_must = R_may` (every transition is *sharp*) and `L` never returns `⊥` (every AP is two-valued).

### §2.3 Mixed transition systems (Dams–Gerth–Grumberg generalisation)

Dams, Gerth, and Grumberg (TOPLAS 1997) considered a generalisation that drops the `R_must ⊆ R_may` invariant. A *mixed transition system* is a 4-tuple `(S, R_must, R_may, L)` over alphabet `Act` with `R_must, R_may ⊆ S × Act × S` *not* necessarily satisfying inclusion. The motivation: some abstractions produce *under-approximation-only* edges — transitions the abstraction can witness existentially but cannot guarantee are admitted in every concretisation. These edges would be `must`-without-`may` under standard KMTS, violating the invariant.

The mixed-transition framework is strictly more expressive than standard KMTS — every KMTS is a mixed transition system, but not vice versa. For most practical abstraction applications (predicate abstraction of bitvector transition systems, the §6.6 setting in the architecture doc), the standard-KMTS invariant holds by construction: any concrete witness for `(b, b') ∈ R_must` is also a concretisation of `(b, b') ∈ R_may`, because the predicate-image construction populates both relations from the same concrete `R`. The mununu data model adopts the standard-KMTS shape (`TransitionModality { Sharp, MayOnly }`, two variants, the `MustOnly` variant is structurally excluded) — see §6.

### §2.4 Notational conventions used in this doc

- `M, M_1, M_2` denote KMTSes. `M.S`, `M.R_must`, etc. denote their components.
- `s, t, s_0, s_1` denote states.
- `a, b, c` denote alphabet symbols (action labels in the Act sense, not propositions).
- `p, q, r` denote atomic propositions in `AP`.
- `φ, ψ` denote mu-calculus formulas.
- `T, F, ⊥` denote Kleene tristate values (when discussing mununu's Rust type the variants are `KleeneT`, `KleeneF`, `KleeneBot`; see §6).
- `⟦φ⟧_M : S → {T, F, ⊥}` denotes the 3-valued semantics of `φ` on `M`.
- The information order is `⊑_i` with `⊥ ⊑_i F` and `⊥ ⊑_i T`; `F` and `T` are incomparable. The truth order is `⊑_t` with `F ⊑_t T`; `⊥` is incomparable to both. See §4.1 for the lattice structures and their dichotomy.

### §2.5 The atomic-3-valued labelling, in practice

Where does `L(s, p) = ⊥` come from? In predicate abstraction (the §6.6 case): given a predicate `p_i(x)` over concrete state, an abstract state `b` corresponds to a set of concrete states `γ(b)`. We set

```text
L(b, p_i) = T   if  ∀ s ∈ γ(b). p_i(s)         (the predicate holds on every concretisation)
L(b, p_i) = F   if  ∀ s ∈ γ(b). ¬p_i(s)        (the predicate fails on every concretisation)
L(b, p_i) = ⊥   otherwise                       (concretisations disagree)
```

This is the canonical 3-valued lifting of a Boolean predicate over a coarsened state space. When `b` corresponds to a single concrete state (or a set where the predicate has a uniform value), `L(b, p_i) ∈ {T, F}` — the abstraction is sharp on this predicate at this state. When `b` merges concretisations that disagree on `p_i`, `L(b, p_i) = ⊥` — the abstraction has lost the distinction this predicate would draw. Refinement (CEGAR-style) responds to `⊥` valuations or `⊥` formula verdicts by adding predicates that split `b` into sub-states where the disagreement is resolved.

## §3 Modal refinement

### §3.1 What refinement means

Larsen (1989) defined modal refinement as a simulation-style game between two MTSes. KMTS refinement extends it with 3-valued AP labelling.

A **modal refinement** of KMTS `M_1` by KMTS `M_2` is a relation `≼ ⊆ M_1.S × M_2.S` satisfying, for all `(s_1, s_2) ∈ ≼`:

1. **AP-consistency.** For every `p ∈ AP`:
   - if `L_1(s_1, p) = T` then `L_2(s_2, p) ∈ {T}` — `T` valuations are preserved (`M_2` may sharpen `⊥` to `T` but not contradict `T`);
   - if `L_1(s_1, p) = F` then `L_2(s_2, p) ∈ {F}` — symmetric.
   - `L_1(s_1, p) = ⊥` admits any `L_2(s_2, p) ∈ {T, F, ⊥}` — refinement may sharpen `⊥` to `T` or `F`, or keep it as `⊥`.

2. **May-step accommodation.** For every may-step `(s_1, a, t_1) ∈ M_1.R_may`, there exists `t_2 ∈ M_1.S` with `(s_2, a, t_2) ∈ M_2.R_may` and `(t_1, t_2) ∈ ≼`. Reading: every behaviour `M_1` admits, `M_2` also admits.

3. **Must-step preservation.** For every must-step `(s_2, a, t_2) ∈ M_2.R_must`, there exists `t_1 ∈ M_1.S` with `(s_1, a, t_1) ∈ M_1.R_must` and `(t_1, t_2) ∈ ≼`. Reading: every behaviour `M_2` requires, `M_1` also requires.

`M_2 ≼ M_1` (read "`M_2` refines `M_1`") iff there is a refinement relation `≼` with `M_2.S_0 × M_1.S_0 ⊆ ≼`.

### §3.2 The intuition

Refinement narrows the gap between abstract and concrete. `M_1 = (R_must, R_may, L)` describes a *space* of concrete systems — every system whose behaviours land in `R_may` and whose mandatory behaviours include `R_must` and whose AP valuations sharpen the `⊥`s of `L` consistently. Refining `M_1` to `M_2` picks a subset of that space; refining all the way to a concrete Kripke structure `M_c` (where `R_must = R_may` and `L` never returns `⊥`) lands on a single concrete model.

The directional asymmetry between may and must is the key. May-edges can be *removed* on refinement (a refined model can be more restrictive about admitted behaviours); must-edges can be *added* on refinement (a refined model can require more behaviours). `R_must ⊆ R_may` is preserved by refinement because the may-set never shrinks below the must-set: refining either tightens may inward or grows must outward, but the inclusion is always maintained.

### §3.3 Soundness target — what refinement preserves

The central theorem (Larsen 1989, extended to KMTS by Huth–Jagadeesan–Schmidt TACAS 2001): mu-calculus formula evaluation is *monotone* in the refinement order, in the following sense. For `M_2 ≼ M_1` and any mu-calculus formula `φ`:

- `⟦φ⟧_{M_1}(s_1) = T` implies `⟦φ⟧_{M_2}(s_2) = T` for every `(s_1, s_2) ∈ ≼`.
- `⟦φ⟧_{M_1}(s_1) = F` implies `⟦φ⟧_{M_2}(s_2) = F` for every `(s_1, s_2) ∈ ≼`.
- `⟦φ⟧_{M_1}(s_1) = ⊥` admits any `⟦φ⟧_{M_2}(s_2) ∈ {T, F, ⊥}` — refinement may sharpen `⊥` either direction.

Reading: definite verdicts on a more-abstract model survive refinement to a less-abstract one; only `⊥` verdicts are unstable. Concretisation is just refinement all the way to a Kripke structure, so a `T`/`F` verdict on the abstract transfers to the concrete. The `⊥` direction is what CEGAR responds to: a `⊥` verdict signals that refinement *might* resolve it (or might not — the abstraction may genuinely be unable to distinguish concretisations that flip the formula's truth value).

### §3.4 Why refinement is the right notion for "abstract less informatively"

Compared to bisimulation (the natural equivalence for 2-valued model checking), refinement is asymmetric — it captures the directional intuition that an abstract model is a *coarser approximation* of a concrete one, not an equivalent description. Bisimulation requires both sides to match step-for-step in both directions; refinement requires only that may-behaviours upper-bound and must-behaviours lower-bound. This asymmetry is what allows non-trivial abstractions to refine to their concretisations — bisimulation is too strong for the predicate-abstraction use case.

The refinement notion is *transitive* and *reflexive*: `M ≼ M`, and `M_3 ≼ M_2 ∧ M_2 ≼ M_1 ⇒ M_3 ≼ M_1`. It is *not* a partial order — anti-symmetry fails because two KMTSes can refine each other without being syntactically identical (they may have different state structure but the same may/must/AP behaviour up to refinement). The right algebraic object is a *preorder*, sometimes called the *modal preorder*.

## §4 3-valued mu-calculus semantics

### §4.1 Two lattice structures

3-valued model checking involves **two distinct lattice structures over the same underlying set `{T, F, ⊥}`**. Confusing them is the most common implementation pitfall and (per the architecture doc §6.4) was the load-bearing reason for splitting mununu's `TruthDomain` trait into truth-order and information-order operations.

**Truth lattice** `(⊑_t, ⊥_t, ⊤_t)`:

- `F ⊑_t T`.
- `⊥` is incomparable to both.
- `⊥_t = F`, `⊤_t = T`.
- Operations: `∨` (join), `∧` (meet), `¬` (negation).
- Semantics of the propositional fragment: `T ∨ ⊥ = T`, `F ∨ ⊥ = ⊥`, `T ∧ ⊥ = ⊥`, `F ∧ ⊥ = F`, `¬T = F`, `¬F = T`, `¬⊥ = ⊥`. (Kleene's strong 3-valued connectives.)

**Information lattice** `(⊑_i, ⊥_i, ⊤_i)`:

- `⊥ ⊑_i F`.
- `⊥ ⊑_i T`.
- `F` and `T` are incomparable.
- `⊥_i = ⊥` (the least informative element); the lattice has no single greatest element — it's a tri-lattice with `T` and `F` as the two maximal points.
- Operation: `⊔_i` (information join), which combines two values toward more-definedness. `⊥ ⊔_i F = F`, `⊥ ⊔_i T = T`, `F ⊔_i F = F`, `T ⊔_i T = T`, `F ⊔_i T = top_i` (undefined or "inconsistent" — not a problem in monotone fixpoint iteration because the formula semantics never produces this combination).

**The dichotomy.** Formula connectives (`∧`, `∨`, `¬`, modal `[a]`, `⟨a⟩`) operate in the **truth order**. Fixpoint iteration operates in the **information order**. In a 2-valued setting these two orders coincide (the Boolean lattice is its own information lattice), which is why 2-valued evaluators do not need to distinguish them. In 3-valued evaluation they must be distinguished: the formula `μX. φ(X)` starts iteration at `⊥` (the information-order least element, the most-uncertain valuation) and increases toward `T` or `F`; the formula `νX. φ(X)` starts iteration at `T` *and* `F` *jointly* (the information-order maximal elements) and decreases toward `⊥`.

The information order is sometimes called the "Scott order" or "knowledge order" in the literature; the truth order is the "Boolean order." Both are well-defined; Kleene (1952) constructed the truth order; Scott (1976) constructed the information order in his domain-theoretic work; the explicit pairing on the same Kleene tristate set is the contribution of the 3-valued model checking community (Bruns–Godefroid 1999; Huth–Jagadeesan–Schmidt 2001; Godefroid–Jagadeesan 2003).

### §4.2 Mu-calculus syntax (recap)

Modal mu-calculus over an alphabet `Act` and atomic propositions `AP`:

```text
φ, ψ ::= p | ¬p | X | φ ∨ ψ | φ ∧ ψ | ⟨a⟩φ | [a]φ | μX. φ | νX. φ
```

with `p ∈ AP`, `a ∈ Act`, `X` a propositional variable, and the standard syntactic positivity restriction on `μ`/`ν` (formula variables appear only under an even number of negations — guaranteed here because negation is only on atomic propositions, the "positive normal form"). Note `μ`/`ν` are dual: `μX. φ = ¬νX. ¬φ[¬X/X]`.

### §4.3 3-valued semantics — modal operators

The semantics `⟦φ⟧_M : S → {T, F, ⊥}` on a KMTS `M = (S, S_0, R_must, R_may, L)` is defined inductively. Boolean and AP cases are direct (`⟦p⟧_M(s) = L(s, p)`; `⟦¬p⟧_M(s) = ¬L(s, p)` under Kleene negation; `⟦φ ∨ ψ⟧_M(s) = ⟦φ⟧_M(s) ∨ ⟦ψ⟧_M(s)` under Kleene `∨`; similarly for `∧`). The modal operators are the interesting cases — they read both relations:

```text
⟦[a]φ⟧_M(s) = T   iff   for every s' with (s, a, s') ∈ R_may : ⟦φ⟧_M(s') = T
              F   iff   exists s' with (s, a, s') ∈ R_must : ⟦φ⟧_M(s') = F
              ⊥   otherwise

⟦⟨a⟩φ⟧_M(s) = T   iff   exists s' with (s, a, s') ∈ R_must : ⟦φ⟧_M(s') = T
              F   iff   for every s' with (s, a, s') ∈ R_may : ⟦φ⟧_M(s') = F
              ⊥   otherwise
```

The asymmetry has a clean reading:

- `T` claims on `[a]φ` require *all* may-successors to satisfy `φ` as `T` — over-approximation must cover every concrete `a`-successor.
- `F` claims on `[a]φ` require *some* must-successor with `F` — under-approximation must witness a concrete `a`-successor where `φ` fails.
- `T` claims on `⟨a⟩φ` require *some* must-successor with `T` — under-approximation must witness.
- `F` claims on `⟨a⟩φ` require *all* may-successors to fail `φ` — over-approximation must rule out every concrete `a`-successor.

Everywhere else: `⊥`. The `⊥` cases are exactly when the abstraction is too coarse to give a definite answer with the data it has — the predicate set distinguishes the abstract states well enough to admit may/must-edges but not well enough to determine the formula's truth value.

### §4.4 Fixpoint semantics — Kleene iteration over the information order

For `⟦μX. φ⟧_M`, define `f_φ : (S → {T, F, ⊥}) → (S → {T, F, ⊥})` by `f_φ(V)(s) = ⟦φ⟧_{M[X ↦ V]}(s)`, where `M[X ↦ V]` extends the environment with `V` as the interpretation of `X`. Then

```text
⟦μX. φ⟧_M = ⊔_i { f_φ^n(V_⊥) : n ∈ ℕ }
⟦νX. φ⟧_M = ⊓_i { f_φ^n(V_max) : n ∈ ℕ }   (information meet, dual)
```

where `V_⊥(s) = ⊥` for all `s` (the information-order least valuation) and `V_max(s) = ?` — the dual is more subtle because the information order has *two* maximal valuations (everywhere-`T` and everywhere-`F`), not one. The standard resolution (Bruns–Godefroid 2000 §4): `ν` iterates *from both sides simultaneously*, computing the truth and falsity sets independently and taking their information-order combination. Concretely, `νX. φ` decomposes as a pair `(T-set, F-set)` of state subsets such that `T-set ⊓ F-set = ∅` (consistency), iterated until both stabilise; the third set (states in neither) carries `⊥`.

**Monotonicity in the information order.** `f_φ` is monotone in `⊑_i` (the lifted pointwise order on `S → {T, F, ⊥}`) for every syntactically positive φ — this is the structural lemma that makes Kleene iteration converge. Proof sketch: each Boolean connective (`∧`, `∨`) is monotone in `⊑_i` (information increases on both arguments imply information increases on the result); each modal operator is monotone in `⊑_i` of `φ` because tightening the formula's valuation can only tighten the modal verdict; fixpoints inherit monotonicity from their bodies. The lattice `(S → {T, F, ⊥}, ⊑_i)` is finite (since `S` is finite for the model-checking setting), so iteration terminates at a fixpoint in at most `2 * |S|` steps (each state can transition `⊥ → T` or `⊥ → F` at most once).

**Note on the truth order.** Naïve Kleene iteration over the truth order `⊑_t` would oscillate: a single iteration of `μX. ¬X` (an ill-formed but instructive non-example) flips `T → F → T` indefinitely. The positivity restriction on `μ`/`ν` rules this out for the truth order in the 2-valued case, but in the 3-valued case the additional `⊥` element gives a stable least starting point (`⊥`) that the truth order does not. The information order is the *right* order for 3-valued fixpoint computation; the truth order is the *right* order for formula evaluation. Confusing them produces an evaluator that either oscillates (truth-order fixpoint) or computes the wrong thing (information-order connectives).

### §4.5 The preservation theorem

Bruns–Godefroid (CONCUR 2000); Huth–Jagadeesan–Schmidt (TACAS 2001). For any KMTS `M`, concrete Kripke structure `M_c`, and refinement `M_c ≼ M`:

```text
For every mu-calculus formula φ and every (s_c, s) ∈ ≼ :
    ⟦φ⟧_M(s) = T   ⇒   ⟦φ⟧_{M_c}(s_c) = T
    ⟦φ⟧_M(s) = F   ⇒   ⟦φ⟧_{M_c}(s_c) = F
    ⟦φ⟧_M(s) = ⊥   ⇒   ⟦φ⟧_{M_c}(s_c) ∈ {T, F}   (the concrete is two-valued)
```

Proof sketch (structural induction on φ):

- **Atomic propositions:** by AP-consistency of refinement (§3.1) — `L(s, p) = T ⇒ L_c(s_c, p) = T`; similarly for `F`.
- **Boolean connectives:** by Kleene-3-valued semantics being a conservative extension of 2-valued (`T ∧ T = T`, etc.; the `⊥` cases never produce a stronger result than the concrete would).
- **Modal `[a]`:** suppose `⟦[a]φ⟧_M(s) = T`. By definition, every may-successor of `s` satisfies `φ` as `T`. By the may-step-accommodation clause of refinement, every concrete `a`-successor `s_c'` corresponds to some abstract may-successor `s'`. By the IH on `φ`, `⟦φ⟧_{M_c}(s_c') = T`. Therefore all concrete `a`-successors satisfy `φ`, so `⟦[a]φ⟧_{M_c}(s_c) = T`. For `F`: by must-step-preservation, the witness must-successor in `M` lifts to a concrete `a`-successor with `⟦φ⟧_{M_c} = F`; therefore some concrete successor falsifies `φ`, so `⟦[a]φ⟧_{M_c}(s_c) = F`.
- **Modal `⟨a⟩`:** dual.
- **Fixpoints:** monotonicity in `⊑_i` lifts to monotonicity in refinement; the preservation extends to the limit by continuity of `f_φ` on the finite lattice. Detailed argument in Bruns–Godefroid 2000 §5.

Reading: definite verdicts (`T` or `F`) on the abstract transfer to the concrete *for the entire mu-calculus, including alternating fixpoints*. The `⊥` case is the explicit "refinement needed" signal — refinement either sharpens to `T`/`F` (sometimes; depends on whether the predicate set can distinguish the concrete cases that differ) or stays `⊥` (the abstraction is genuinely unable to decide, often because the property is concretely undecidable for the predicate language at hand).

This is what makes KMTS the right framework for full-mu-calculus abstraction: a *single* abstract model that is uniformly sound for safety, reachability, liveness, and nested fixpoints, with a clean refinement-on-`⊥` recipe.

## §5 Compositional KMTS

### §5.1 Parallel composition — pointwise on may and must

KMTS composition (Larsen 1989 for MTS; Huth–Jagadeesan–Schmidt 2001 for KMTS; Larsen–Larsen–Wąsowski FoSSaCS 2007 for modal I/O automata) is *pointwise on may and must*. Given KMTSes `M_1, M_2` over a shared alphabet `Act` with synchronisation set `Sync ⊆ Act`, the composition `M_1 ∥ M_2 = (S, S_0, R_must, R_may, L)` is constructed as:

- `S = M_1.S × M_2.S`, the Cartesian product.
- `S_0 = M_1.S_0 × M_2.S_0`.
- For each capability `c ∈ {must, may}`:
  - **Synchronising step.** For `a ∈ Sync`: `((s_1, s_2), a, (s_1', s_2')) ∈ R_c` iff `(s_1, a, s_1') ∈ M_1.R_c` AND `(s_2, a, s_2') ∈ M_2.R_c`. Both sides must have a `c`-edge for the composed `c`-edge to exist.
  - **Interleaving step.** For `a ∉ Sync`: `((s_1, s_2), a, (s_1', s_2)) ∈ R_c` iff `(s_1, a, s_1') ∈ M_1.R_c` (left side moves; right side stable). Symmetric for the right side.
- `L((s_1, s_2), p) = L_1(s_1, p) ⊓_i L_2(s_2, p)` for shared APs, where `⊓_i` is the information-order meet ("agreement" — `T ⊓_i T = T`, `F ⊓_i F = F`, `T ⊓_i F = ⊥` if both could observe but disagree, `⊥` for any `⊥` input). For per-component APs (not shared), `L` projects directly from the component.

The capability conjunction on synchronising steps is the central rule: a composed `must`-edge requires *both* sides to have a `must`-edge (an existential witness must be witnessed on both sides); a composed `may`-edge requires *both* sides to have a `may`-edge. This is *set-intersection per capability axis*, mirroring the §6.5 of the architecture doc's `has_may ∧ has_may` and `has_must ∧ has_must` rule.

### §5.2 Why this is the structural free lunch

Composition is *purely structural* — no SMT, no abstraction-time computation, no inter-module discharge. Refinement is *congruential* (Larsen–Larsen–Wąsowski 2007 Theorem 4.4): `M_1 ≼ M_1' ⇒ M_1 ∥ M_2 ≼ M_1' ∥ M_2`. In particular, refining one module's KMTS (e.g. by CEGAR) refines the composed KMTS without recomposition — the predicate refinement step inside one module's lifter does not require re-running composition.

The preservation theorem (§4.5) lifts to composition by congruentiality: a `T`/`F` verdict on the composed abstract KMTS transfers to the corresponding verdict on the composed concrete Kripke structure. **The compositional verification problem reduces to the per-module abstraction problem, with no inter-module proof obligation** — provided each module's KMTS is independently a sound abstraction of its concrete implementation (which it is, by construction of the predicate-image lifter, §6.6 of the architecture doc).

This is what the architecture doc §7 calls the "structural free lunch": the cost of compositional abstraction in the AGR (Assume-Guarantee Reasoning) framework — circular discharge, assumption synthesis, L*-learning — is *zero* under KMTS because the compositional soundness is structural, not derived from per-module proofs that depend on assumed environment behaviour.

### §5.3 The tightness trade-off

What KMTS composition does *not* preserve is *tightness*: the composed abstraction may have more `⊥` verdicts than a monolithic predicate abstraction over the flattened design. The architecture doc §7.2 walks through a worked counterexample — a producer/consumer pair where the composed KMTS returns `⊥` on a safety property that monolithic abstraction returns `T` for, because the per-module predicate sets do not include the cross-module port-equality `producer.data_out == consumer.data_in` that the property implicitly relies on. Refinement closes the gap by adding the missing port-equality predicate; the heuristic of auto-emitting port-equality predicates for every declared multi-module connection handles the common case structurally.

Tightness loss is fundamental to compositional abstraction — *any* compositional framework will pay it. The KMTS advantage is that the loss is *visible* (as `⊥` verdicts rather than incorrect `T`/`F` verdicts) and *refinable* via the standard CEGAR loop, rather than requiring re-architecting an AGR proof.

### §5.4 Modal I/O automata and contracts

Larsen, Nyman, and Wąsowski (FoSSaCS 2007) extended modal transition systems to *modal I/O automata* — adding directionality (input vs output actions) to make refinement and composition asymmetric in the appropriate ways for interface theories. Antonik, Huth, Larsen, Nyman, and Wąsowski (FMCO 2008) further developed *modal contracts* — pairs of assumption / guarantee KMTSes with their own refinement and composition operators, supporting interface synthesis and product-line modelling.

For mununu's purposes, the basic KMTS composition (§5.1) is sufficient — the architecture doc's §4.5 driver-side controllability convention encodes the I/O directionality implicitly (driver = output, consumer = input) without needing the modal I/O automata machinery. Modal contracts may become useful if assume-guarantee-style assumptions become structurally important (e.g. for very large multi-IP SoCs); the architecture doc lists this under deferred work.

## §6 Where KMTS lands in mununu

The architecture doc §6.3 defines the additive extensions to mununu's existing `Clts<S, L>` type. Recap:

```rust
// crates/mununu-core/src/clts/mod.rs (EXTENSION)
enum TransitionModality { Sharp, MayOnly }  // standard KMTS: must ⊆ may
struct Transition {
    ...existing fields...
    modality: TransitionModality,  // default Sharp on construction
}

enum Tristate { KleeneT, KleeneF, KleeneBot }
struct Clts<S, L> {
    ...existing fields...
    state_3valued_predicates: Option<BTreeMap<(StateId, PredId), Tristate>>,
}
```

The mapping from this doc's notation to the mununu types:

- `R_may`: union of `Sharp` and `MayOnly` transitions.
- `R_must`: `Sharp` transitions only.
- The invariant `R_must ⊆ R_may` is structural — `Sharp` is in both, `MayOnly` is in may only.
- The forbidden `MustOnly` variant (must-without-may, the mixed-transition extension of §2.3) is structurally excluded from the enum. The decision matches the predicate-image construction in §6.6 of the architecture doc, where every must-witness lifts to a may-witness by construction.
- `L: S × AP → Tristate`: the new `state_3valued_predicates` field. Sharp-KMTS-equivalent labelling (the existing 2-valued `state_variable_bitset`) is preserved as the fast path; the 3-valued field is populated by the KMTS-aware BTOR2 lifter.
- The evaluator's truth-order operations (`∨`, `∧`, `¬`) and information-order operations (`⊥_i`, `⊔_i`, `⊑_i`) are separated in the `TruthDomain` trait (§6.4 of the architecture doc) — `BoolDomain` aliases the two lattices; `KleeneDomain` distinguishes them per §4.1 of this doc.
- Composition is the pointwise meet on the capability lattice (§5.1), implemented in [`composition/mod.rs`](../../crates/mununu-core/src/composition/mod.rs) as the modality merge `has_may ∧ has_may` and `has_must ∧ has_must` (architecture doc §6.5).

A standard (Sharp-only) Kripke structure is the special case where every transition is `Sharp` and `state_3valued_predicates` is `None` (the 2-valued `state_variable_bitset` is used directly). The KleeneDomain evaluator on such a CLTS returns verdicts in `{KleeneT, KleeneF}` only — `KleeneBot` never appears, because the lattice has no `⊥` content to produce. The BoolDomain monomorphisation of the evaluator computes the same verdicts more cheaply (one Boolean per state instead of a tri-state); for adapters that produce only Sharp KMTSes (XState, microcode, agentic), BoolDomain is the default and KleeneDomain is unused.

## §7 Reading list

This list cites primary sources for every load-bearing claim in this doc. Read in roughly the order presented to build the framework from first principles; skip to §7.2/§7.3 if you already have the MTS background.

### §7.1 Foundations of modal / mixed transition systems

1. **K. G. Larsen and B. Thomsen, *A Modal Process Logic*** (LICS 1988). Original modal transition systems. Defines `(S, R_must, R_may)` over an action alphabet with `R_must ⊆ R_may`; introduces the refinement preorder.
2. **K. G. Larsen, *Modal Specifications*** (CAV 1989 / 1990 — published as part of the *Automatic Verification Methods for Finite State Systems* proceedings). The refinement-based specification language built on MTSes; foundational for treating MTSes as under-specified contracts.
3. **D. Dams, R. Gerth, and O. Grumberg, *Abstract Interpretation of Reactive Systems*** (TOPLAS 1997, Vol. 19 No. 2, pp. 253–291). The *mixed transition system* generalisation dropping the `R_must ⊆ R_may` invariant; the foundational treatment of abstraction-as-interpretation for branching-time temporal logic. Mununu's standard-KMTS shape (§2.2) is the restricted-to-`R_must ⊆ R_may` case.

### §7.2 KMTS and 3-valued mu-calculus

4. **G. Bruns and P. Godefroid, *Model Checking Partial State Spaces with 3-Valued Temporal Logics*** (CAV 1999) — original paper; **G. Bruns and P. Godefroid, *Generalized Model Checking: Reasoning about Partial State Spaces*** (CONCUR 2000) — extended results. Together establish the 3-valued modal mu-calculus semantics (§4.3) and the preservation theorem (§4.5). The CONCUR 2000 paper is the canonical citation for the preservation result for full mu-calculus including alternating fixpoints.
5. **M. Huth, R. Jagadeesan, and D. A. Schmidt, *Modal Transition Systems: A Foundation for Three-Valued Program Analysis*** (TACAS 2001, LNCS 2028). The KMTS definition (§2.2) — the explicit 3-valued AP labelling extension of Larsen–Thomsen MTS — and the proof that 3-valued mu-calculus model checking on KMTSes is sound, complete (up to the lattice's expressiveness), and decidable for finite KMTSes. The companion to Bruns–Godefroid for the *finite-state* model-checking setting; mununu's KMTS is in this class.
6. **P. Godefroid and R. Jagadeesan, *Automatic Abstraction Using Generalized Model Checking*** (TACAS 2003, LNCS 2619). The CEGAR-style refinement loop for KMTS — how to respond to `⊥` verdicts by extracting refinement predicates from spurious abstract counterexamples. Foundational for the mununu architecture doc §6.8 CEGAR loop and the [`predicate-abstraction-recipe.md`](predicate-abstraction-recipe.md) §4 algorithm.

### §7.3 Compositional KMTS and modal contracts

7. **K. G. Larsen, U. Nyman, and A. Wąsowski, *Modal I/O Automata for Interface and Product Line Theories*** (FoSSaCS 2007, LNCS 4421). The compositional theory of KMTSes with input/output asymmetry. The congruentiality of refinement under composition (§5.2) and the structural soundness of compositional abstraction are proved here.
8. **A. Antonik, M. Huth, K. G. Larsen, U. Nyman, and A. Wąsowski, *20 Years of Modal and Mixed Specifications*** (Bulletin of the EATCS 95, 2008; also in FMCO 2008 proceedings). Survey of the modal-specification literature 1988–2008; useful both for historical context and for surveying extensions (parametric modal specifications, disjunctive modal transition systems, branching-time modal contracts) that mununu may want to revisit if AGR-style work becomes load-bearing.

### §7.4 Adjacent — background on 3-valued logic and fixpoint theory

For readers unfamiliar with Kleene's 3-valued logic or Tarski's fixpoint theorem:

- **S. C. Kleene, *Introduction to Metamathematics*** (North-Holland 1952), Chapter XII §64. The original strong 3-valued logic with truth tables for `∨`, `∧`, `¬` extended to `{T, F, ⊥}` (Kleene's "undefined" value). The truth-lattice operations of §4.1 are these connectives.
- **A. Tarski, *A Lattice-Theoretical Fixpoint Theorem and Its Applications*** (Pacific J. Math. 5, 1955, pp. 285–309). The least-fixpoint and greatest-fixpoint existence theorem for monotone functions on complete lattices. The mu-calculus fixpoint semantics is an instance of Tarski's framework lifted to the lattice of 3-valued state valuations under the information order (§4.4).

### §7.5 Mununu cross-references

- Architecture: [`native-sv-abstraction.md`](native-sv-abstraction.md) §6.
- Practical recipe (predicate seeding, image, CEGAR): [`predicate-abstraction-recipe.md`](predicate-abstraction-recipe.md).
- Broader abstraction-literature catalog (Phase A.1 deliverable; covers the 18-paper grounding from the pillow plan plus the KMTS / AGR additions from this work): [`abstraction-literature.md`](abstraction-literature.md).
- Mununu's `Clts<S, L>` extension (§6 of this doc): [`crates/mununu-core/src/clts/mod.rs`](../../crates/mununu-core/src/clts/mod.rs) (post-R.1).
- Composition modality merge: [`crates/mununu-core/src/composition/mod.rs`](../../crates/mununu-core/src/composition/mod.rs) (post-R.1).
- KMTS-aware BTOR2 lifter: [`crates/mununu-core/src/adapter/btor2/kmts_lift.rs`](../../crates/mununu-core/src/adapter/btor2/kmts_lift.rs) (post-R.2).
- KleeneDomain evaluator instantiation: [`crates/mununu-core/src/mu_calculus/truth_domain.rs`](../../crates/mununu-core/src/mu_calculus/truth_domain.rs) (post-R.3).
