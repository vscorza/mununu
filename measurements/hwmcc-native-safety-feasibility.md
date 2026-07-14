# Can mununu's native engines decide HWMCC safety? — real cases, native vs external feasibility, and a path

> Companion to [`hwmcc-owned-engine-gaps.md`](hwmcc-owned-engine-gaps.md). That doc
> *categorizes* where the owned engines fail on the HWMCC'20 bv suite. This one
> *instantiates* each category with a real design, asks whether a mununu-**native**
> approach is feasible at all, and where it is not, says so plainly; where it is, it
> proposes the concrete path. Grounded in the `--owned-only` measurement (2026-07-13,
> commit `84185f1`) and the shipped engine behaviors.

> Method: "native" = mununu owns the search loop — the exact 3-valued BDD engine
> (`symbolic_bitblast::exact_bad_reachable`), native BMC + k-induction
> (`native_bmc::decide_bad_safety`), the deep counterexample search
> (`native_bmc::bmc_cex_until`), and native McMillan interpolation
> (`native_interp::verify_safety_interp`). "External" = the search runs in someone
> else's engine — Z3 SPACER (`native_spacer`, in-process), btormc, Pono. Every verdict
> below is either **measured** with `mununu btor2 verify --owned-only` on the named
> real design, or a **theorem** about the representation, or an **argument** — each is
> labeled.

---

## 0. The one property class, and the one honest question

Every HWMCC'20 bv obligation is **bit-level safety**: `AG ¬bad` — is a `bad` node
reachable over the concrete (bit-precise) transition relation? There is no liveness,
no branching, no abstraction in the benchmark itself. So the question "can a native
engine decide it" is **not** about property class; it is about whether a native engine
can, on the *concrete bit-level model*, either (a) find a counterexample or (b) prove
an inductive safety invariant, within a budget.

The honest answer, established below: **for two categories it is provably infeasible
for a native engine to stay exact; for one it is feasible and now shipped; for two it
is feasible only by building (or reusing) a SPACER-class engine — which mununu already
links in-process.** And the natural follow-up — *do the **external** engines (btormc,
Pono, SPACER) succeed where native can't?* — is measured in §7 with a load-bearing
result: external wins one category cleanly and the *shallow* tier of the rest, but on
the **hard instance of every category** btormc/Pono/SPACER abstain too — the frontier is
undecided-by-everyone, not native-vs-external. The KMTS 3-valued cube — mununu's
headline native technique — is **not** a bit-level-safety engine at all (§6), and
understanding why is the key to not chasing the wrong lever.

---

## 1. The exact 3-valued engine: infeasible above ~40 bits (theorem)

**Real case — `arbitrated_top_n2_w8_d16_e0`:** 45 state cells, datapath widths up to
16 bits; the *narrowest* arbiter instance is already **313 total state bits**, and the
family runs to **8,378** (`arbitrated_top_n2_w128_d32_e0`). The exact engine's cap is
40 register+input bits (`MAX_BITBLAST_BITS`); it `Skip`s and abstains.

**Measured:** `--owned-only` reports `unknown` with the exact engine contributing
nothing — it never enters the merge on any arbiter.

**Why this is a theorem, not a tuning knob.** BDD image computation is worst-case
exponential in the number of BDD variables; on a few-hundred-to-few-thousand-bit
sequential circuit the ROBDD for the reachable-state set blows up before a fixpoint.
The 40-bit cap is a *deliberate sound abstain*, not an OOM crash. There is **no native
fix that preserves exactness** — raising the cap trades a sound abstain for an OOM.
The only escape is to stop being exact (abstract the datapath — which is the KMTS
cube's job, §6, and changes the property you are deciding).

**External:** btormc/Pono have no BDD cap and decide the *shallow* arbiter instances
(e.g. `arbitrated_top_*_d16` via a btormc counterexample), but the **deep proof is open
even for them** — measured, `arbitrated_top_n2_w128_d64_e0 → unknown` under the full
portfolio (btormc + Pono + SPACER) at 60 s. So external rescues the shallow tier, not
the deep-proof tier.

**Feasibility verdict: INFEASIBLE for a native *exact* engine; external decides the
shallow instances, the deep proofs are open for everyone.** Correct dispositions: route
wide designs to a bit-level SAT/BMC engine (native BMC for CEX, external IC3 for
proofs), or abstract (KMTS) when the property is branching.

---

## 2. Deep / slow counterexamples: feasible, and shipped (measured)

**Real cases — `circular_pointer_top_*`, `shift_register_top_*`:** violated designs
with 64-/32-bit datapaths (17 and 14 state cells) whose counterexample the *default*
portfolio missed. In `hwmcc-owned-engine-gaps.md` these are Category C — "deep
counterexamples btormc finds, owned engines miss" — because native BMC in the full
portfolio runs `decide_bad_safety(max_k=40, timeout=5s)`, and one unrolling step of a
wide design does not fit in the 5-second per-query budget.

**Why native CEX-finding is feasible where native proof is not.** Finding a
counterexample is a *satisfiability* question over a bounded unrolling — no inductive
invariant to synthesize. It scales with SAT/SMT throughput and unrolling depth, both of
which are engineering, not representation, limits.

**Shipped fix — `native_bmc::bmc_cex_until` + `decide_reach_owned_only`.** A dedicated
wall-bounded, cancellable, **CEX-only** unrolling (no k-induction step queries to slow
or abort it) to depth 128, run as a separate "counterstrategy" thread alongside the
safety-proof engines (first-definite wins). Given a fair budget it decides the Category-C
designs:

| Design | verdict | decider | time (@240s owned-only) |
|---|---|---|---|
| `circular_pointer_top_w64_d8_e0` | violated | native + cex | 10 s |
| `circular_pointer_top_w128_d8_e0` | violated | native + cex | 13 s |
| `shift_register_top_w16/w32/w64_d8_e0` | violated | native + cex | 17–23 s |

**Precision — depth vs throughput (measured, §2.1).** These `d8` designs have a
*shallow* counterexample; the win came from the *fair time budget* (native BMC also
decides them once its per-query budget is 240 s instead of 5 s), with `bmc_cex_until`
corroborating. The CEX search's distinct value — reaching a counterexample at depth
41–128 that `max_k=40` cannot — is a separate, sound guarantee. Its **boundary** is
established empirically next.

### 2.1 The deep-CEX boundary (measured on the `d8 / d128` depth ladder)

The `circular_pointer_top_w*_d*` family is parameterized by datapath width `w` and a
counterexample-depth knob `d`. Running `--owned-only` across the ladder isolates
*throughput* from *depth*:

| Design | states × width | verdict @ budget | decided by |
|---|---|---|---|
| `circular_pointer_top_w64_d8_e0` | 17 × 64 b | **violated in 10 s** | native + cex |
| `circular_pointer_top_w16_d128_e0` | 137 × 16 b | **abstain (not decided in ~60–75 s)** | — |
| `circular_pointer_top_w64_d128_e0` | 137 × 64 b | **abstain (not decided in ~60–75 s)** | — |

**Reading:** the *shallow* case (`d8`) decides fast — a **throughput** win (native BMC
alone decides it once its per-query budget is 240 s not 5 s; `cex` corroborates). The
*deep* cases (`d128`, 137 state cells) do **not** decide, narrow or wide. The binding
constraint is **not** the overall budget but the CEX search's **per-query timeout**
(`OWNED_CEX_QUERY_MS = 15 s`): a single monolithic z3 query over ~128 unrolled frames of
a 137-cell design exceeds 15 s before the depth-128 `bad` frame is reached, so
`bmc_cex_until` abstains — and enlarging the *overall* budget cannot lift a *per-query*
wall. Reaching a genuinely deep counterexample on a state-heavy design is exactly what
**incremental SAT** (btormc/Boolector — assert one frame at a time, keep the learned
clauses) is built for, and what a from-scratch monolithic-unroll native BMC is not —
**though the gap is shared at the portfolio budget:** the full portfolio (btormc + Pono
+ SPACER) *also* returns `unknown` on both `w64_d128` and `w16_d128` at 60 s (§7), so
even external incremental SAT needs more than 60 s here. This is a throughput/budget
wall for everyone, not a native-only deficiency.

**Feasibility verdict: FEASIBLE and SHIPPED for the CEX direction up to the width×depth
product that fits one per-query SMT solve; beyond that (deep CEX on a state-heavy
design) even the owned CEX search — and even btormc at 60 s — soundly abstains.**
Closing that last gap natively means an **incremental** native BMC (persistent solver,
frame-at-a-time assertion, clause reuse) — a feasible, well-scoped engineering increment
(§8) that would extend
the owned CEX reach toward btormc's, without any external tool.

---

## 3. Auxiliary-invariant safety proofs: feasible only as a SPACER-class engine

**Real cases — the `cal*` family (`cal159`, `cal161`, `cal162`, `cal21`, `cal35`, …,
200+ instances):** *safe* designs whose property is **not k-inductive for small k** —
they need an auxiliary strengthening invariant. Pono proves them; in the full portfolio
they show `unreach=[pono]`, and at the 60 s per-engine budget even Pono times out on the
harder ones.

**What each native engine can do here:**

- **native k-induction** (`decide_bad_safety` step queries): decides only when the
  property is genuinely k-inductive at a reachable `k`. It does **not** synthesize the
  missing invariant. On the `cal*` family it abstains — correctly, soundly, uselessly.
- **native McMillan interpolation** (`native_interp`): *does* synthesize an inductive
  invariant, from Craig interpolants. **Measured win — `vis_arrays_am2910_p2`
  → safe via interpolation in 94 s** (`--owned-only @240s`), a design the exact engine
  can't touch (over cap) and k-induction can't prove. This is the one native engine
  that *closes* invariant-needing safe proofs. But its ceiling is **cvc5's interpolant
  search speed**: on `gen43` (a 256-bit design) the required interpolant
  `(or (= s32 s17) (= s19 (bvurem s19 s26)))` took cvc5's SyGuS enumeration **105 s** to
  reach — past any per-query budget — so `gen43` abstains not because no invariant
  exists but because *finding* it is too slow.

**Why "feasible" here means "SPACER".** Matching Pono/SPACER on the `cal*` class means
strong inductive-invariant synthesis over bit-level transition systems — i.e.
re-implementing IC3/PDR or a fast interpolating model checker. mununu's own IC3ia
foundation (`abs_safety::verify_safety_ic3`, frames + MIC + refinement) exists but,
**measured, abstains on real designs** — e.g. `paper_v3 → unknown` with `detail =
"IC3 refinement stalled (grammar ceiling)"` — because the completeness lever it needs
(targeted per-trace sequence interpolation) is the same interpolant-search problem, at
the same cvc5 ceiling. And z3-SPACER — a mature IC3/PDR — is **already linked
in-process** (`native_spacer`), so the "native" version competes with something mununu
already ships.

**External:** this is external's *cleanest* win — measured, `cal159 → holds via pono`
(60 s) under the full portfolio, where every native engine abstains. Pono's IC3/PDR (and
in-process SPACER) synthesize exactly the strengthening invariant native k-induction
cannot. (The interpolation-hard `gen43` is the exception even here: `unknown` for
external too at the portfolio budget — its invariant *exists* but needs the 105 s
`bvurem` interpolant no engine reaches in time.)

**Feasibility verdict: FEASIBLE, and the feasible thing external already ships is a
SPACER-class engine — which mununu links in-process.** Two honest sub-paths: (i) *lean
on in-process z3-SPACER* for the proof direction on HWMCC — it is already there,
in-process, no subprocess; (ii) *raise native interpolation's ceiling* by replacing
cvc5's SyGuS search with a faster (word-level, IC3ia-integrated) interpolant procedure —
a real research lever (this is the paper track), not a quick win, and it only helps the
interpolation-tractable subset. Building a from-scratch native IC3/PDR to beat Pono on
`cal*` is **not** recommended: large, uncertain, and duplicative of the in-process
SPACER.

---

## 4. Arrays / memories: feasible with array-aware work (not yet built)

**Real case — `vcegar_arrays_itc99_b12_p2`:** pure bit-vector after flattening (0 array
ops survive), but memory-backed designs in the class (`vis_arrays_*`) put each memory
word in the state → wide (Category 1) *and* array-semantic. This is also where the one
**soundness-bug candidate** lives: native SPACER's btor2→CHC encoding returned a
spurious `violated` where ground truth is safe. That symptom is now **guarded**
(a sole-decider SPACER `reachable` is dropped to `unknown`, `reach_portfolio::collect`),
but the root-cause encoding is unfixed.

**External:** btormc/Pono carry mature BTOR2 array theory, but on this instance they
*also* abstain at the portfolio budget — measured, `vcegar_arrays_itc99_b12_p2 →
unknown` at 60 s (ground truth is safe; the design flattens to pure BV and needs more
budget or a better proof). So external is the right tool class here but not a free
decide at 60 s.

**Feasibility verdict: FEASIBLE with array abstraction / an array-aware native BMC
(unbuilt); external is the natural home but not free at the portfolio budget.** External
tools carry mature array theory; mununu's native array handling is the weakest link. A
scoped native increment (array-aware unrolling, or a sound array→UF abstraction with
CEGAR) is feasible but unbuilt. Priority is low unless an array-heavy corpus matters —
the guard already prevents the unsound outcome.

---

## 5. Nonlinear datapath arithmetic: infeasible for native (theorem + measured)

**Real case — `mul9`:** 29 state cells, 64-bit datapath, **2 multiplier nodes**;
`--owned-only @240s → unknown`. Also `mul7` (`unknown` in 0 s — instant abstain).

**Why this is the hardest wall, on *both* native proof routes:**

- **Exact BDD:** BDDs for integer multiplication are provably exponential in the operand
  width (Bryant, 1991) — the exact engine cannot even *build* the multiplier relation,
  independent of the 40-bit cap.
- **McMillan interpolation:** an interpolant separating the reachable set from `bad` on a
  multiplier/modulo design must speak `bvmul`/`bvurem`; cvc5's interpolant search
  explodes in grammar depth on nonlinear operators — **measured 105 s** for the
  `bvurem`-shaped `gen43` interpolant, and worse for genuine multipliers.
- **native BMC (CEX):** for a *violated* nonlinear design the deep CEX search can still
  find a witness (SAT over a bounded unrolling with a concrete multiplier is decidable);
  but for a *safe* nonlinear design there is no CEX, and both proof routes above wall.

**External:** the *violated* nonlinear case is external's to win — measured, `mul7 →
violated via btormc+pono` (19 s), because a bounded unrolling with concrete multipliers
bit-blasts to a SAT instance a mature engine cracks. But the *safe* nonlinear case is
hard **even for external** — measured, `mul9 → unknown` at 60 s (btormc finds no CEX;
proving a multiplier invariant is a known-hard problem for CDCL SAT). So external rescues
nonlinear *violations*, not nonlinear *proofs*.

**Feasibility verdict: INFEASIBLE for a native engine on the *proof* side; external
decides nonlinear *violations* (btormc) but not nonlinear *proofs* either.** BDD is a
theorem; interpolation is measured-hard; the safe-multiplier proof is hard for every
engine here. The correct disposition is (a) native BMC (or btormc) for the violated
cases, (b) accept `unknown` for the safe nonlinear cases. A multiplier-aware technique
(e.g. algebraic / Gröbner-basis reasoning as in modern arithmetic verifiers) is a
different tool than anything in mununu today and out of scope for the safety portfolio.

---

## 6. Why the KMTS 3-valued refinement does **not** rescue HWMCC safety

This is the crux the question asks about, and the answer is specific and measured.

The KMTS 3-valued predicate cube (`recoverability::verify_safety_scalable` + emergent-K
discovery) is built for a **different property class**: branching-time properties with
alternating fixpoints (`AG EF good` recoverability, νμ) over an **abstracted** datapath,
where the may/must distinction is load-bearing. On plain `AG ¬bad` bit-level safety it
has three problems, each real:

1. **Predicate abstraction here *inherits* §1/§5 — it does not escape them.** A predicate
   abstraction is **not** limited to `register==const` cubes: you *can* carry an arbitrary
   datapath predicate (e.g. `a*b == K`) and let interpolation-based CEGAR refine it — which
   is exactly what mununu's emergent-K loop (`discover_relational_predicates`) is meant to
   do. The problem on a bit-precise datapath obligation is not that a separating predicate
   *cannot exist*; it is that finding and using it **relocates the hard part** into the
   predicate's discovery and the abstraction's edge queries, landing back on §1 (scale) or
   §5 (nonlinear SMT):
   - **`mul9`** — `bad` is a condition on the exact 64-bit product `a*b` (2 multiplier
     nodes). A predicate over the product *could* separate `bad` in principle, but (i)
     mununu's predicate grammar (`PredicateExpr`: register vs const / register /
     register+const comparisons + boolean combinations — verified: **no multiplication
     atom**) cannot state it, so the emergent-K interpolant→predicate parser returns `None`
     on a `bvmul` interpolant and falls back; and (ii) even with the atom, *discovering* it
     is interpolation over `bvmul` — the §5 SyGuS explosion (a measured 105 s for the
     simpler `bvurem`) — and *using* it makes every may/must abstract edge a multiplier-SAT
     query. The cube inherits §5's nonlinear wall rather than abstracting past it. (The
     grammar gap is fixable; the interpolation/edge-SMT hardness is §5 and is not.)
   - **`gen43`** — the same shape with a `bvurem` (modular) relation over a **256-bit**
     datapath: the separating predicate is a modular-arithmetic fact the grammar cannot
     state and cvc5 cannot interpolate within budget. Inherits §5.
   - **`arbitrated_top_*`** — no nonlinearity, but a safety proof over a 313–8378-bit
     arbitration state needs *many* predicates to separate `bad`; the predicate count that
     actually decides it is §1's exact-analysis blow-up under another name. Inherits §1.

2. **Emergent-K finds a *relevant* predicate that is not an *invariant* here — the design
   is unsafe (measured).** The emergent-K interpolation loop
   (`discover_relational_predicates`) *does* find genuinely unique, *relevant* forms —
   register-to-constant bounds and orderings the mature pair-difference / eq-atom machinery
   structurally cannot express. These are **good** predicates (`count < 16` is *exactly*
   `¬bad` for a "buffer overflows at 16" design). **But on the real HWMCC corpus, every
   design where those forms appeared is `SAT`/UNSAFE at depth**, so the predicate is not an
   *invariant* — the design genuinely violates it:
   - **`vis_arrays_buf_bug`** — discovered `count < 16`, but the design has a concrete
     counterexample at **depth 18–28**; `count` genuinely reaches 16 on the real trace.
   - **`krebs.3`** — discovered `v_energy < 8`, but the counterexample is at **depth 75**.
   - **`brp2.2`** — discovered the ordering `dve_invalid ≥ a_done`, but the counterexample
     is at **depth 119**.

   So seeding the predicate as `safe` is wrong, and the verdict-verified driver correctly
   *rejects* the non-inductive seed (sound abstain) — no false `safe`. **This is not a
   theoretical dead end, and not a bad-predicate problem — it is an implementation gap.**
   The failed inductiveness check (`p ∧ T ⊭ p'`) is itself the signal that `bad` is
   reachable, and the predicate says *where* (`count → 16`). So the discovered fact could
   **direct a counterexample search** — a targeted deeper BMC toward the escaping region,
   or an IC3-style backward obligation chain — and turn the abstain into a definite
   `violated` with a trace. Today the loop only tries the predicate as a *safe* seed; the
   CEX-direction wiring is a concrete task (§8.1, T3). The simplest lever remains **deeper
   BMC** (§2) — for these moderate depths (18–119) it finds the counterexample with or
   without the hint; the predicate's unique value is on *very* deep counterexamples, where
   it enables acceleration rather than blind unrolling.

3. **The residual is BMC-depth, not missing-invariant.** The portfolio's remaining HWMCC
   abstentions are dominated by deep-CEX-unsafe cases and the §1/§3/§5 walls — where the
   fix is a deeper/faster reachability query, not a new predicate:
   - **`circular_pointer_top_w64_d128_e0`** — measured `unknown` for native *and* external
     (btormc/Pono/SPACER) at 60 s (§7); it is *violated* at depth ≈128, so **no predicate
     helps** — only deeper BMC finds the concrete trace.
   - **`arbitrated_top_n2_w128_d64_e0`** — measured `unknown` for everyone; a deep safety
     proof over an ~8 k-bit state, a scale/depth problem, not a missing cube.
   - **the item-2 designs** (`vis_arrays_buf_bug` @18–28, `krebs.3` @75, `brp2.2` @119) —
     the same story from the discovery side: `SAT` at depth, where a predicate is the
     wrong instrument.

   So predicate discovery is a sound, tested **hint generator** for the paper's emergent-K
   direction, not a bit-level-safety decider.

### 6.1 Does adding predicate-generation capability make these feasible?

The natural follow-up: if the *only* stated obstacle for `mul9` is a missing grammar
atom, does *adding* multiplication / modular predicate generation make it — and the other
cases — decidable? **No, not in general.** Adding a predicate atom expands what invariants
are *representable*; it does not reduce the *complexity* of finding or checking one. The
complexity relocates, it does not vanish. Everything in this suite is *decidable* (finite
state); the walls are complexity, and predicate abstraction wins **exactly when a compact,
discoverable inductive certificate exists in the chosen vocabulary.** Whether adding
capability helps therefore depends on *why* the case is hard:

- **Violated cases (§2, e.g. `circular_pointer_d128`) — predicates are irrelevant.** There
  is no inductive invariant to find; the task is to exhibit a counterexample. No
  predicate-generation helps at all; the only lever is deeper/faster BMC (§2's
  incremental-BMC increment). This is the majority of the hard residual, and it is the
  clearest "adding predicates does nothing" case.

- **Nonlinear-safe cases (§5, `mul9`/`gen43`) — representable, still §5-hard.** Adding a
  `bvmul`/`bvurem` atom lets the cube *state* a datapath predicate, and **if** the design
  has a *coarse* safety proof (one not needing the exact product — a range, sign, or
  parity fact) it becomes feasible. That coarse-proof class is exactly predicate
  abstraction's real edge and the emergent-K "unique form" wins. **But if the proof
  genuinely needs the exact multiplier**, both *discovering* the predicate (interpolation
  over `bvmul`) and *validating* the abstraction (per-edge multiplier-SAT) inherit §5's
  hardness — the same wall that leaves `mul9` `unknown` for btormc/Pono/SPACER too (§7). So
  it is feasible **iff a coarse discoverable proof exists**, not by grammar alone; the
  grammar atom is the cheap, fixable half.

- **Wide-scale cases (§1/§3, `arbitrated_top`) — representation is already fine.**
  Register-comparison predicates *can* express arbitration / counter invariants; the wall
  is *discovering* a compact inductive certificate at scale, which is the §3 SPACER-class
  problem — and even external Pono/SPACER do not crack the *deep* instances at budget (§7).
  Adding predicate generation here is not a grammar fix; it is building the IC3/PDR that
  mununu already links in-process.

**The unifying principle.** Predicate abstraction *discretizes* the state space by
predicate valuations and decides a design precisely when a *compact, discoverable*
inductive certificate lives in its vocabulary. Expanding the vocabulary helps only that
class. It does nothing for violated designs (no certificate to represent), and it does not
lower the complexity of *finding* or *checking* a certificate when the property genuinely
depends on the exact datapath — there it merely lets the cube fail the same way the
external engines already do. So "add predicate generation and discretize" makes more cases
*expressible*; it makes feasible only the ones with a compact certificate a bounded search
can reach — which is the §3 class in-process SPACER already serves.

**Feasibility verdict for KMTS-on-HWMCC-safety: not the right tool — and that is fine.**
The cube's real, defensible domain is **branching-time recoverability on abstracted
datapaths** (`AG EF good`), which *no external bv tool can even state*, decided at
scale via the ranking certificate and property-directed seeding. That is orthogonal to
the HWMCC bit-level-safety question, and it is where the native differentiation actually
lives.

---

## 7. Native vs external — the measured division of labor

The natural next question is: *where the native engines can't, do the external engines
(btormc, Pono, in-process SPACER) succeed?* Measured — full portfolio (`mununu btor2
verify`, exact + native + in-process SPACER + btormc + Pono, each at its ~60 s default
budget; mununu-sva, 2026-07-13):

| Design | Category | External verdict @~60 s | Owned-only @240 s |
|---|---|---|---|
| `circular_pointer_top_w64_d8_e0` | 2 shallow CEX | violated — **btormc** (60 s) | violated — native+cex (**10 s**) |
| `circular_pointer_top_w64_d128_e0` | 2 deep CEX | **unknown** | unknown |
| `circular_pointer_top_w16_d128_e0` | 2 deep CEX | **unknown** | unknown |
| `mul7` | 5 nonlinear, violated | violated — **btormc+pono** (19 s) | — |
| `mul9` | 5 nonlinear, safe | **unknown** | unknown |
| `gen43` | 3/5 · 256-bit, `bvurem` | **unknown** | unknown |
| `arbitrated_top_n2_w128_d64_e0` | 1 deep proof | **unknown** | unknown |
| `cal159` | 3 aux-invariant safe | holds — **pono** (60 s) | — |
| `vcegar_arrays_itc99_b12_p2` | 4 arrays | **unknown** (ground truth: safe) | unknown |

**The load-bearing finding: on the *hard* instance of every category, the external
engines abstain too.** At the portfolio budget, deep counterexamples (`d128`), the deep
arbiter proof (`d64`), nonlinear-safe (`mul9`), the 256-bit `gen43`, and the array proof
are undecided by btormc, Pono, *and* in-process SPACER — not just by the native engines.
So these categories are **not** "mununu is missing what external has"; for the hard
residual, **nobody decides them** at practical budgets. (Budget matters: FINAL.md at
120 s decides 41/136 where this ran at 60 s — a longer budget lifts some *middle-tier*
instances, but the deep/wide/nonlinear-safe frontier stays open regardless of *which*
engine.) Note too that on `circular_pointer_w64_d8` the owned deep-CEX search decided in
**10 s** vs btormc's 60 s — because the full portfolio runs native BMC at the 5 s default
while `--owned-only` gives it a fair budget; the owned path is not merely a fallback, it
is sometimes *faster* on the throughput cases.

### The feasibility matrix (native | external)

| Category | Native proof | Native CEX | External | Basis |
|---|---|---|---|---|
| 1. State ≫ 40 b | **No** (exact) | via native BMC | shallow: **btormc/Pono**; deep proof (`d64`): **No** even external | theorem + measured |
| 2. Deep/slow CEX | n/a (violated) | **Yes — shipped** (shallow) | shallow: **btormc**; deep (`d128`): **No** @60 s even external | measured |
| 3. Aux-invariant safe | Only as SPACER-class | n/a (safe) | **Yes — Pono / SPACER** (`cal159` holds) | measured |
| 4. Arrays | feasible, unbuilt | feasible, unbuilt | array theory — yes with budget (unknown @60 s) | argument + measured |
| 5. Nonlinear | **No** | via native BMC (if violated) | violated: **btormc** (`mul7`); safe: **No** (`mul9`) | theorem + measured |

**Where external cleanly beats native:** Category 3 — Pono/SPACER synthesize the
auxiliary strengthening invariant the native engines cannot — and the *middle tier* of
1/2/5, where btormc's incremental SAT finds a shallow counterexample the portfolio's
5 s native budget misses (though a fair native budget or the owned deep-CEX also
decides those). **Where external does *not* rescue the problem:** the deep/wide/
nonlinear-safe frontier — provably or empirically open for native and external alike.

---

## 8. The proposed path for native safety — prioritized and honest

1. **DONE — own the counterexample direction (Category 2, throughput half).**
   `bmc_cex_until` + `--owned-only` ship a sound, wall-bounded deep CEX search that
   decides the *shallow* Category-2 designs owned-standalone (`circular_pointer_d8` in
   10 s). This is the one place a *native* engine cleanly beats the portfolio's default
   budget, no external tool.

2. **NEXT feasible native increment — incremental BMC (Category 2, deep half).** The
   §2.1 boundary shows the monolithic-unroll CEX search abstains on deep (`d128`)
   counterexamples because one 128-frame query exceeds the per-query budget. Replacing
   the rebuild-per-depth unrolling with a **persistent solver that asserts one frame at a
   time and reuses learned clauses** (incremental SAT/SMT, the technique btormc uses)
   is a well-scoped, feasible engineering increment that would push the owned CEX reach
   toward btormc's — entirely within the native BMC engine, no external dependency. This
   is the highest-ROI native safety work remaining.

3. **Lean on in-process z3-SPACER for the proof direction (Category 3) — do not
   re-implement it.** SPACER is already linked in-process (`native_spacer`); for HWMCC
   safe-invariant proofs it is the pragmatic answer, no subprocess. Building a
   from-scratch native IC3/PDR to beat Pono here is large, uncertain, and duplicative.

4. **Raise native interpolation's ceiling as the *research* lever (Category 3, subset).**
   `native_interp` is the only owned engine that closes invariant-needing proofs
   (`vis_arrays` @94 s); its wall is cvc5's SyGuS interpolant search time. A faster,
   word-level, IC3ia-integrated interpolant procedure is the paper track
   (`cube-ic3ia-invariant-discovery.md`) — real, but slow, and only helps the
   interpolation-tractable subset. Not a HWMCC-numbers play.

5. **Accept `unknown` (or route external) for Categories 1 and 5.** Wide-exact and
   nonlinear-safe are theorems against native exactness; the sound move is a graceful
   abstain plus native BMC on the violated instances. Do not spend engineering trying to
   make the exact engine or interpolation cross a proven barrier.

6. **Keep the KMTS cube pointed at its real target.** Its differentiation is
   branching-time recoverability external bv tools cannot state — not bit-level HWMCC
   safety. Measuring or marketing it on HWMCC safety is the wrong axis.

### 8.1 Concrete engineering tasks

Two classes of increment fall out of §6. The first expands *representability* (helps the
coarse-invariant class only, §6.1); the second turns *abstentions into definite verdicts*
on the unsafe-at-depth class (§6.2). Each is scoped, and each carries an honest caveat.

**Grammar / discovery — missing predicate atoms** *(helps §5 coarse-invariant proofs;
measured caveat: does **not** lift the exact-datapath cases — the interpolation/edge-SMT
hardness is §5 and is unchanged, so gate behind a flag and measure, do not assume a lift):*

- **T1 — multiplication predicate atom.** Extend `PredicateExpr` (today: `Cmp` / `CmpReg`
  / `CmpRegAddend` + boolean — no product) with a `bvmul`-based comparison atom, and teach
  the interpolant→predicate parser to accept a `bvmul`-shaped interpolant (today it returns
  `None` → falls back). Unlocks the cube for `mul9`-class designs **iff** they have a coarse
  product-predicate proof.
- **T2 — modular / remainder predicate atom.** The same for `bvurem` / `bvudiv`
  (`gen43`-class 256-bit modular relations).
- **T1/T2 acceptance test.** A design with a *known* coarse product/modular invariant
  decides `holds` with the atom on and `unknown` with it off (the isolation pattern the
  existing `safety_cube_decides_constant_bound_via_interpolation_discovery` test uses), and
  a free/unsafe counterpart never decides a false `safe` (soundness half).

**Predicate-directed counterexample search — turn §6.2 abstentions into `violated`:**

- **T3 — non-inductive discovery ⇒ CEX direction.** When a discovered candidate invariant
  fails its inductiveness check (`p ∧ T ⊭ p'`), treat the failure as a reachability signal
  rather than only a rejected safe-seed: run a *targeted* deeper BMC toward the escaping
  region (`p → ¬p`), or chain the counterexample-to-induction backward (IC3-style), and on
  success return `violated` with the trace instead of abstaining. Validates on
  `vis_arrays_buf_bug` (@18–28), `krebs.3` (@75), `brp2.2` (@119). This is the natural
  companion to the incremental-BMC increment (item 2): the predicate supplies the
  *direction*, incremental BMC supplies the *reach*; the predicate's unique value is on
  *very* deep counterexamples where blind unrolling can't get there but the escaping
  transition can be accelerated.

### 8.2 Validation of §8.1 (attempted 2026-07-13) — the cheap levers confirm the walls

The §8.1 tasks were probed against the exact examples before committing to the large
builds. **On these designs all three confirm the limitations rather than lift them** — an
empirical validation of §1–§6, not a fix:

- **T1 (`bvmul`) / T2 (`bvurem`) — confirmed §5, grammar build short-circuited.** A key
  fact makes the grammar atom moot for the *portfolio*: `native_interp` already
  round-trips *arbitrary* cvc5 interpolant shapes through z3 (including `bvmul`/`bvurem`) —
  the `PredicateExpr` atom is only needed by the *cube's* discovery parser, not the
  interpolation engine. So the decisive test is whether owned interpolation decides the
  designs given budget. **Measured: `gen43` and `mul9` are both `unknown` at owned-only
  @300 s** (300 s > `gen43`'s 105 s single-interpolant time). One interpolant does not
  close the proof and the search does not converge, so an atom that merely lets the *cube*
  hold the same interpolant cannot do better. §5's nonlinear-SMT wall stands; the grammar
  build was not undertaken because the interp probe proves it futile on these designs.
- **T3 (predicate-directed CEX) — confirmed §2/§7.** The natural first lever — scaling the
  deep-CEX per-query budget with the wall budget — was implemented and measured.
  **`krebs.3` stayed `unknown` at owned-only @240 s**: the search never reached depth 75,
  because the wall is the *cumulative* z3-BMC cost of ~75 deep queries, which a per-query
  bump cannot fix. And it is **not** an owned-vs-external gap — **`krebs.3` and `brp2.2`
  are `unknown` for the full external portfolio (btormc/Pono/SPACER) @60 s as well.** These
  deep-CEX-unsafe designs are hard for *everyone* at practical budgets; a predicate
  direction does not help when the bottleneck is BMC *reach*. The per-query change was
  reverted (a tweak that does not fix the target is not worth shipping).

**Net.** The §8.1 tasks are real *directions*, but on these specific examples the binding
constraint is the underlying wall — nonlinear SMT for T1/T2, deep-CEX BMC reach for T3 —
and the cheap levers do not cross it. The genuine levers remain the hard ones: a faster
bit-vector SAT backend (btormc-class) or a much larger budget for deep CEX, and word-level
nonlinear reasoning for multiplier proofs — none a quick owned change. This section is the
measured confirmation that the §1–§6 walls are real, not artifacts of a missing feature.

**Bottom line — the native/external division of labor.** The measured picture (§7) is
sharper than "external engines cover mununu's gaps." External wins **one category
cleanly (3, auxiliary-invariant safe proofs — Pono/SPACER)** and the **shallow tier of
1/2/5** (btormc's incremental SAT on shallow counterexamples the portfolio's 5 s native
budget misses). But on the **hard instance of every category** — deep counterexamples
(`d128`), the deep arbiter proof (`d64`), nonlinear-safe (`mul9`), the 256-bit `gen43`,
the array proof — **btormc, Pono, and in-process SPACER abstain too.** For that frontier
the choice is not native-vs-external; it is undecided-by-everyone at practical budgets.

So a *native* mununu engine can own the counterexample direction (shipped; sometimes
*faster* than btormc, §7) and close the interpolation-tractable safe proofs; it
**cannot**, by theorem, stay exact above ~40 bits or on nonlinear datapaths; it
**should not** re-build the IC3/PDR it already links in-process (that is external's clean
win, and it is already available); and it **need not chase** the hard residual as if
external had it — external does not. The native differentiation that is real — owned
deep-CEX search (with an incremental-BMC path to extend it), the exact/soundness
cross-check, and branching-time properties no external bv tool can even *state* — is
orthogonal to "beat btormc/Pono on the bv suite," which for two of five categories is
provably out of reach for everyone.
