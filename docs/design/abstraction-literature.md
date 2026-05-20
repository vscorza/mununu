# Abstraction Literature — Reading List for the Auto-Extraction Pipeline

> **Status: planning.** This doc anchors no live code; it grounds
> [`auto-extraction-architecture.md`](auto-extraction-architecture.md). Code
> citations point at existing files only to identify where each idea
> *would* land; the sketches are intentionally incomplete — they show
> shapes, not implementations.

## Preface

The Caliptra `soc_ifc_boot_fsm` proof-by-fire effort (see
[`caliptra-abstraction-analysis.md`](caliptra-abstraction-analysis.md))
walked into a 33.5 M-transition enumeration that the bit-blaster could
not complete in 22 minutes / 4.6 GB RSS on a release build. The Phase 1.6
work that cleared the input-bit cap surfaced a deeper truth: mununu's
`AbstractionType` + `FieldDomain` + `discovered_values` machinery is a
partial, ad-hoc realisation of techniques that the formal-verification
literature has formalised over 25+ years. This doc reads those papers
back into mununu's vocabulary — what does each contribute, what would
its core algorithm look like as Rust against today's `AbstractionType`
enum, and where in the existing module tree would the change land.

The list is curated, not exhaustive. The selection criterion is
*relevance to mununu's specific bottleneck*: extracting an
explicit-state CLTS from real-world RTL (and, less centrally, C and
agentic workflows) under per-signal abstraction declarations, then
evaluating mu-calculus over the result. Papers that improve IC3 itself
without an abstraction angle, or that target software-only domains, are
omitted. Tool papers (AVR, rIC3, BTOR2) appear when their pipeline
shape is the closest published comparator to mununu's.

Each entry follows the same skeleton:

1. **Theoretical core** — what the paper actually proves or proposes.
2. **Pseudocode** — load-bearing algorithm in 10–15 lines.
3. **mununu-shaped Rust sketch** — 10–20 lines of struct + signatures
   against the *current* mununu types ([`adapter/domain.rs`](../../crates/mununu-core/src/adapter/domain.rs),
   [`adapter/state_enum.rs`](../../crates/mununu-core/src/adapter/state_enum.rs),
   [`adapter/sidecar/mod.rs`](../../crates/mununu-core/src/adapter/sidecar/mod.rs),
   [`mu_calculus/mod.rs`](../../crates/mununu-core/src/mu_calculus/mod.rs)).
4. **Map to mununu** — one line: which primitive / module / stage this
   lands in.

The cross-reference matrix at the end is consumed verbatim by
[`auto-extraction-architecture.md`](auto-extraction-architecture.md) §2 and §5.

---

## 1. Kurshan, *Computer-Aided Verification of Coordinating Processes* (Princeton, 1994) — Localization Reduction

**Theoretical core.** Given a property over a subset of state variables,
*prune* the variables outside that subset along with their next-state
logic. The reduced model has fewer latches but admits more behaviours; any
universal property that holds on the reduction holds on the original
(over-approximation soundness). Refinement re-introduces pruned latches
one at a time until the property is decided or all variables are back.
Kurshan's framing predates CEGAR by six years; CEGAR's first paper
explicitly generalises it.

**Pseudocode.**

```text
input : circuit C with latches L = {l1..ln}, property φ over Lφ ⊆ L
output: verdict {holds, fails-with-cex} or refine-request

A := { l ∈ L | l reachable in dep-graph from Lφ ∩ free(φ) }   # initial keep-set
loop:
    M := build_model(C restricted to A; havoc everything outside A)
    r := check(M, φ)
    if r == holds                       : return holds
    if r == fails(cex) and cex feasible : return fails-with-cex(cex)
    A := A ∪ next_layer(cex)            # one pruned latch back per round
```

**mununu-shaped Rust sketch.**

```rust
// adapter/partition.rs (NEW)
use crate::adapter::domain::{AbstractionType, FieldDomain};

pub struct Partition {
    pub kept:    Vec<FieldDomain>,      // active fields
    pub dropped: Vec<FieldDomain>,      // → AbstractionType::Ignored
}

/// One pass of localization shrinkage: keep only fields transitively
/// referenced by the property + their data-flow ancestors. Iterative
/// refinement is driven by the CEGAR loop in `verify::orchestrator`.
pub fn localize(
    all_fields: Vec<FieldDomain>,
    property_atoms: &[String],          // predicate names from Formula::nodes
    deps: &DepGraph,                    // built from BTOR2 / SV AST
) -> Partition { todo!() }
```

**Map to mununu.** Pipeline Stage 2; parent of `AbstractionType::Ignored`.
mununu today applies localization *only* when a user hand-marks a signal
`"ignored"`; this paper makes it automatic.

---

## 2. Clarke, Grumberg, Jha, Lu, Veith, *Counterexample-Guided Abstraction Refinement* (CAV 2000, JACM 2003)

**Theoretical core.** The canonical CEGAR loop. Build a (sound) abstraction,
model-check it; if the abstract counterexample is *feasible* on the
concrete system, return it; otherwise the counterexample is *spurious*
and contains information sufficient to refine the abstraction — typically
by partitioning a previously-collapsed equivalence class along a
predicate that explains the spurious step. Termination is not guaranteed
in general but holds for finite-state systems where the predicate set is
bounded.

**Pseudocode.**

```text
input : concrete system S, property φ
output: verdict {holds, fails-with-cex}

A := initial_abstraction(S)                # coarse: localization or boolean preds
loop:
    r := check(A, φ)
    if r == holds            : return holds
    cex := simulate(r.cex on S)
    if cex.feasible          : return fails-with-cex(cex)
    π   := analyze_spurious(cex)           # find separating predicate(s)
    A   := refine(A, π)                    # split one class along π
```

**mununu-shaped Rust sketch.**

```rust
// verify/refine.rs (NEW)
use crate::mu_calculus::Formula;
use crate::verify::VerifyReport;

pub enum CegarStep {
    Decided(VerifyReport),
    Spurious { new_predicates: Vec<String> },   // feed into sidecar discovered_values
}

pub fn refine_once(
    ctxdsl: &str,
    sidecar_path: &Path,
    formula: &Formula,
    report: &VerifyReport,                 // result of previous round
) -> CegarStep { todo!() }
```

**Map to mununu.** Stage 6 — wraps `verify::orchestrator::verify_project`.
Refinement output is written back to the sidecar's `discovered_values`,
re-realized, and re-evaluated; the loop terminates when the verdict is
True or a feasible counterexample is found.

---

## 3. Graf & Saidi, *Construction of Abstract State Graphs with PVS* (CAV 1997) — Predicate Abstraction

**Theoretical core.** Replace each concrete state `s` with the tuple
`(p1(s), …, pk(s))` where the `pi` are user- or tool-supplied
predicates. The abstract transition relation is the *best
over-approximation* under this map: there is an abstract edge `â → b̂`
iff some concrete state in the `â` class has a concrete successor in
the `b̂` class. Computing this image is the hot inner loop of every
predicate-abstraction tool; in the original paper, PVS was used as the
decision procedure (slow). The 25 years of follow-up work is mostly
about speeding this step up.

**Pseudocode.**

```text
input : transition relation T(s, s'), predicate set P = {p1..pk}
output: abstract transition relation T̂(â, b̂) ⊆ 2^P × 2^P

for each â ∈ 2^P:
    for each b̂ ∈ 2^P:
        query := ∃ s, s'. T(s, s') ∧ ⋀_i pi(s)  = â_i ∧ ⋀_i pi(s') = b̂_i
        if solver.is_sat(query):  T̂.add((â, b̂))
```

**mununu-shaped Rust sketch.**

```rust
// adapter/sidecar/predicate_image.rs (NEW)
pub struct Predicate {
    pub name: String,                  // becomes a FieldDomain variant name
    pub witness: SmtExpr,              // typed against the source's theory
}

pub struct AbstractTransition {
    pub from: Vec<bool>,               // truth values of predicates in `from`
    pub to:   Vec<bool>,
}

pub fn compute_abstract_transitions(
    t_relation: &SmtExpr,              // T(s, s') in the adapter's theory
    predicates: &[Predicate],
) -> Vec<AbstractTransition> { todo!() }
```

**Map to mununu.** Stage 4 algorithmic ancestor. Today
`kripke_smt.rs:enumerate_values` (#L110) is a *constants* enumerator
keyed by one signal; Graf–Saidi generalises it to *predicate tuples*.

---

## 4. Jain, Kroening, Sharygina, Clarke, *Word-Level Predicate-Abstraction and Refinement Techniques for Verifying RTL Verilog* (IEEE TCAD 2008; DAC 2005 precursor)

**Theoretical core.** Three additions to Graf–Saidi for RTL: (a) predicates
are *word-level* — `x == 3`, `y < x + 1` — encoded as bit-vector formulas
to preserve precision; (b) the abstract image is computed by SMT with
careful clause enumeration to handle thousands of predicates; (c) new
predicates during refinement come from the *weakest precondition* of
Verilog `always_ff` statements along the spurious trace — a syntactic
rule, not a guess. The bit-vector handling is what mununu's
`kripke_smt.rs` already partly does; the WP rule is what we don't.

**Pseudocode.**

```text
input : spurious abstract counterexample π = â0 →â1 →…→ân, RTL statements
output: refined predicate set P'

for i in n .. 0:
    for each always_ff statement S that drives a signal in support(âi):
        wp_i := compute_wp(S, predicates_at(i+1))   # symbolic wp on bit-vectors
        candidates := atoms(wp_i) \ P              # new word-level predicates
        P := P ∪ rank_and_filter(candidates)
```

**mununu-shaped Rust sketch.**

```rust
// adapter/systemverilog/wp_refine.rs (NEW)
use crate::adapter::systemverilog::ast::{Module, AlwaysBlock};

pub struct WeakestPrecondition<'a> {
    pub stmt: &'a AlwaysBlock,
    pub post: SmtExpr,                  // predicate to be re-established
}

pub fn wp_atoms(blocks: &[AlwaysBlock], cex_trace: &[StateRef]) -> Vec<SmtExpr> {
    // walk the trace backwards; for each step, compute wp over the
    // assigning always_ff; collect atomic sub-expressions that mention
    // signals in the support of the failing modality.
    todo!()
}
```

**Map to mununu.** Stage 4 refinement source. The output atoms become new
entries in `SvAnnotation.discovered_values` and flow through
`resolve_to_field_domain` unchanged.

---

## 5. Jain, Kroening, Sharygina, Clarke, *VCEGAR: Verilog CounterExample Guided Abstraction Refinement* (TACAS 2007)

**Theoretical core.** Tool paper for paper #4. VCEGAR's value here is the
*architecture*: a single binary takes Verilog + a property, runs CEGAR
end-to-end, and emits either holds / a concrete counterexample. The
predicate set is seeded from syntactic Verilog atoms (case labels, `==`
guards) and refined by WP. The end-to-end pipeline is the closest 2007
analogue to what `mununu verify` aims to be.

**Pseudocode.**

```text
parse_verilog → AST
seed P with syntactic atoms (== constants, case labels)
build abstract model M̂ from (AST, P)
loop:
    r := bdd_check(M̂, φ)
    if r == holds          : print holds; exit
    if simulate(r.cex on AST) feasible : print cex; exit
    P := P ∪ wp_atoms(r.cex, AST)
    M̂ := rebuild(AST, P)
```

**mununu-shaped Rust sketch.**

```rust
// crates/mununu-cli/src/main.rs — extension to `mununu verify`
pub struct CegarOptions {
    pub max_iterations: u32,            // hard cap (terminate if no progress)
    pub seed_strategy: SeedStrategy,    // SyntacticAtoms | DiscoveredValues
    pub wp_refine: bool,                // enable refinement from cex trace
}

pub fn verify_with_cegar(
    config: &VerifyConfig,
    opts: CegarOptions,
) -> Result<VerifyReport, VerifyError> { todo!() }
```

**Map to mununu.** Stage 6 driver shape. The `mununu verify --cegar`
sub-flag is the natural CLI surface.

---

## 6. Goel & Sakallah, *Model Checking of Verilog RTL Using IC3 with Syntax-Guided Abstraction* (NFM 2019)

**Theoretical core.** AVR's distinguishing idea is to pick predicates *not
from WP* but from the **sub-terms already present in the design** — a
syntax-guided approach inspired by SyGuS. Every constant compared
against, every signal width slice, every arithmetic sub-expression that
appears in the RTL is a candidate predicate. The abstraction is
*implicit* — IC3 reasons over the predicate Boolean cube directly,
without materialising the abstract transition relation. The empirical
result: AVR wins HWMCC on word-level tracks against tools that *do*
materialise.

**Pseudocode.**

```text
input : design D (Verilog → btor2-like word-level IR), property φ
output: verdict + (optional) inductive invariant

P := collect_subterms(D)               # all sub-expressions in AST
                                       # filter by appearance in guards / compares
ic3_loop:
    F := initial frames F0 = init(D), Fi = ⊤ for i ≥ 1
    extend_with_blocking_clauses_over(P)
    if reach φ from Fk:  refine_predicates(syntax-guided choice)
    if invariant found:  return holds
```

**mununu-shaped Rust sketch.**

```rust
// adapter/sidecar/predicate_seed.rs (NEW)
use crate::adapter::systemverilog::ast::{Module, Expr};

pub struct PredicateSeed {
    pub signal: String,                 // FieldDomain.name
    pub witnesses: Vec<i64>,            // becomes DiscoveredValue list
    pub rationale: &'static str,        // "case label" | "== constant" | "width slice"
}

pub fn collect_syntactic_predicates(module: &Module) -> Vec<PredicateSeed> {
    // Pre-SMT pass: walk the AST, scrape (a) case labels, (b) RHS of
    // `signal == constant`, (c) constants appearing in width-aware
    // arithmetic. Cheaper than Stage 4; no SMT calls.
    todo!()
}
```

**Map to mununu.** Stage 3 (predicate seeding). Closest published
artefact to what mununu's `mununu sv discover` already does
*syntactically* (case-label scrape in
[`kripke.rs:scan_significant_constants`](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs));
this paper formalises the rule and shows it scales.

---

## 7. Goel & Sakallah, *AVR: Abstractly Verifying Reachability* (TACAS 2020)

**Theoretical core.** Tool paper for #6 with empirical results on HWMCC.
The novelty here over #6 is engineering: AVR ships an end-to-end binary
that takes BTOR2 in, produces verdicts + traces out, and exposes
configuration for predicate-seed strategy. Most relevant to mununu: AVR
demonstrates that the predicate-image-free IC3 approach beats explicit
materialisation on real industrial benchmarks. This is the strongest
argument for the Phase B parallel pipeline.

**Pseudocode.** (Same shape as #6; the contribution is system-level.)

**mununu-shaped Rust sketch.**

```rust
// adapter/btor2/avr_bridge.rs (PHASE B only — out of Phase A scope)
pub enum Verdict {
    Holds { invariant: Option<String> },
    Fails { cex_btor2_witness: PathBuf },
    Unknown,
}

/// Phase B trigger: shell out to AVR (or rIC3) on BTOR2; consume
/// counterexample witness, lift back to a CLTS trace for mununu's
/// existing report renderer.
pub fn check_with_external_avr(
    btor2_path: &Path,
    property_index: u32,
    timeout: Duration,
) -> Result<Verdict, IoError> { todo!() }
```

**Map to mununu.** Phase B reference. Mununu's explicit-state CLTS does
not benefit from AVR's algorithm directly; the bridge would *only* make
sense if Phase A's enumeration runtime fails the decision gate.

---

## 8. Goel & Sakallah, *Syntax-Guided Synthesis for Lemma Generation in Hardware Model Checking* (VMCAI 2021)

**Theoretical core.** Extends #6 by replacing hand-chosen predicate
grammars with *learned* sub-term grammars: SyGuS synthesises lemmas to
strengthen IC3 frames. Relevant to mununu only as a forward-looking
direction — once `mununu sv discover` produces stable significant-value
sets, a SyGuS layer could *propose* new predicates from cex traces
without WP.

**Pseudocode.**

```text
input : failing IC3 frame Fi with bad cube c, sub-term grammar G
output: lemma ℓ such that Fi ∧ ℓ blocks c and is inductive relative to G

candidates := enumerate(G, depth ≤ d)
for ℓ in candidates:
    if Fi ∧ ℓ blocks c and inductive_relative_to(Fi, ℓ):
        return ℓ
return ⊥                                # give up; widen G
```

**mununu-shaped Rust sketch.**

```rust
// adapter/sidecar/sygus_extend.rs (FUTURE WORK; not Phase A)
pub struct SubtermGrammar { pub operators: Vec<&'static str>, pub depth: u8 }

pub fn propose_predicates_from_cex(
    cex: &[AbstractState],              // from VerifyReport
    grammar: &SubtermGrammar,
) -> Vec<PredicateSeed> { todo!() }
```

**Map to mununu.** Out of Phase A scope. Cite as a Phase B+ extension to
Stage 3.

---

## 9. Andraus & Sakallah, *Automatic Abstraction and Verification of Verilog Models* (DAC 2004) and *Reveal* (LPAR 2008) — Datapath Abstraction

**Theoretical core.** Partition design signals into **control** (state
machines, narrow flags) and **datapath** (wide adders, multipliers,
shifters). Replace each datapath sub-circuit with an uninterpreted
function (UF) symbol; verify the control logic against UF-axiomatised
datapath. If the result holds, it holds for the concrete design (UF is
sound). Refinement re-instantiates a datapath component when the UF
abstraction is too coarse (e.g., the proof needs `mul(a, b) = mul(b, a)`).

**Pseudocode.**

```text
input : circuit C with signal set S
output: abstracted circuit C', UF mapping

control, datapath := partition_by_width_and_role(S)        # heuristic
for each connected datapath sub-circuit D in C:
    introduce UF symbol f_D with signature inputs(D) → outputs(D)
    replace D with f_D
C' := control ⊕ {f_D | D ∈ datapath}
```

**mununu-shaped Rust sketch.**

```rust
// adapter/partition.rs (NEW, shared with Stage 2)
use crate::adapter::domain::FieldDomain;

pub struct DatapathUf {
    pub symbol: String,                 // f_alu_0
    pub inputs: Vec<String>,            // signal names
    pub output: String,
}

pub struct Partition {
    pub kept: Vec<FieldDomain>,         // control signals
    pub dropped: Vec<FieldDomain>,      // → Ignored
    pub datapath_uf: Vec<DatapathUf>,   // NEW: feeds Stage 4 SMT theory selector
}
```

**Map to mununu.** Stage 2 (jointly with Kurshan). The `wait_count[7:0]`
collapse in `caliptra-abstraction-analysis.md` §2.2 — keep the
`== 0` predicate, drop the increment arithmetic — is exactly this paper's
prescription expressed as a hand-written sidecar entry. Stage 2 makes it
automatic.

---

## 10. Bryant, Kroening, Ouaknine, Seshia, Strichman, Brady, *Deciding Bit-Vector Arithmetic with Abstraction* (TACAS 2007; IJSTTT 2009)

**Theoretical core.** A decision procedure for bit-vector arithmetic that
*alternates under-* and *over-approximations*. Under-approximation: shrink
some bit-vector variables to fewer Boolean variables than their nominal
width; check for SAT. Over-approximation: replace bit-vector operations
with linear-arithmetic over-approximations; check for UNSAT. Tightens
each side until they meet. Crucial for mununu because the BTOR2 path's
runtime cost is *mostly* in solving the abstract-transition formula,
and the under/over alternation is the cheapest way to short-circuit
queries with obvious verdicts.

**Pseudocode.**

```text
input : bit-vector formula φ
output: SAT / UNSAT verdict

w := 1                                     # current under-approximation bit-width
loop:
    φ_under := restrict_widths(φ, w)
    if SAT(φ_under)        : return SAT
    φ_over := linear_overapprox(φ)
    if UNSAT(φ_over)       : return UNSAT
    w := w * 2
    if w > max_width(φ)    : fallback to full bit-blast
```

**mununu-shaped Rust sketch.**

```rust
// adapter/sidecar/bv_abstraction.rs (NEW)
pub enum BvApprox { Under(u32), Over, Concrete }

pub fn decide_bv_with_abstraction(
    formula: &SmtExpr,
    budget: Duration,
) -> Result<bool, TimeoutError> {
    // alternate Under(w) / Over / Concrete; cap each query at budget / 4
    todo!()
}
```

**Map to mununu.** Stage 4 inner loop. When the predicate-image SMT
query saturates Z3, drop into this scheme rather than time out.

---

## 11. Hoder, Bjørner, de Moura, *SMT Techniques for Fast Predicate Abstraction* (CAV 2006)

**Theoretical core.** Treat the predicate-image computation as a single
*all-SMT* problem: ask the solver to enumerate **all satisfying
assignments** of `T(s, s') ∧ assign(P, s) ∧ assign(P, s')` over the
predicate variables, then collect the projection. The DPLL(T) framework
allows this enumeration to share clause learning across the abstract
state space, beating per-state queries by 20×+. Direct fit for mununu's
Stage 4 hot loop.

**Pseudocode.**

```text
input : T(s, s'), predicates P
output: T̂ ⊆ 2^P × 2^P

solver.assert(T(s, s'))
solver.assert(p_i ↔ p_i(s) for each pi ∈ P)         # name predicate vars
solver.assert(p'_i ↔ p_i(s') for each pi ∈ P)
results := []
while solver.check_sat():
    m := solver.model()
    results.add((restrict(m, p_*), restrict(m, p'_*)))
    solver.assert(¬ same_assignment(m on p_* ∪ p'_*))  # block this abstract edge
return results
```

**mununu-shaped Rust sketch.**

```rust
// adapter/sidecar/predicate_image.rs (continuation of #3)
impl PredicateImage<'_> {
    /// All-SMT enumeration of abstract transitions.
    /// Bounded by `cap_edges` to keep enumeration bounded (default 4096).
    pub fn all_abstract_edges(&self, cap_edges: usize)
        -> Vec<AbstractTransition>
    {
        // see pseudocode above; uses z3::Solver's solver.check / model
        // / assert(¬model) idiom that already appears in kripke_smt.rs.
        todo!()
    }
}
```

**Map to mununu.** Stage 4 *algorithm*. Replaces the per-guard
constant-enumeration loop at
[`kripke_smt.rs:enumerate_values`](../../crates/mununu-core/src/adapter/systemverilog/kripke_smt.rs#L110)
with an all-SMT predicate-tuple enumeration. Single largest expected
runtime win at the Caliptra scale.

---

## 12. Cimatti, Griggio, Mover, Tonetta, *IC3 Modulo Theories via Implicit Predicate Abstraction* (TACAS 2014) — IC3-IA

**Theoretical core.** Combine IC3 with predicate abstraction *without
ever materialising* the abstract transition relation. Frames are
maintained over the predicate Boolean cube; concrete satisfiability
queries are pushed into the SMT solver lazily during blocking-clause
generation. The result is the runtime profile of IC3 with the precision
of word-level predicate abstraction. **This is the strongest published
alternative to mununu's explicit-state pipeline** — and the natural
target of Phase B.

**Pseudocode.**

```text
input : T(s, s'), property φ, predicate set P
output: verdict

F[0] := init(s) over s_abs := proj_P(s)
F[i] := ⊤ for i ≥ 1
loop:
    if F[k] ∧ ¬φ_abs unsat under T_abs(s_abs, s'_abs)    : return holds
    block bad cubes by generating clauses over P:
        c := generalize_cube(bad)                          # solver-side, lazy
        F[i..k].add(c)
    if blocking impossible : analyze_cex(s_abs trace) → refine P
```

**mununu-shaped Rust sketch.**

```rust
// (Phase B only — there is no Phase-A-portable shape.)
//
// IC3-IA cannot be expressed against mununu's explicit-state CLTS without
// re-enumerating the implicit relation, which defeats its purpose. The
// Phase B integration would be a new adapter that takes BTOR2 + sidecar,
// invokes an IC3-IA engine (own implementation or external — e.g. AVR,
// rIC3, nuXmv), and converts verdict + witness back into a VerifyReport.
// No Rust sketch fits in 20 lines here; the integration point is the
// external-process boundary.
```

**Map to mununu.** Phase B substrate candidate. Cited in Phase A only as
the architectural reason to **not** spend Phase A effort on materialising
larger predicate sets — the asymptotic ceiling is already known.

---

## 13. Lahiri & Bryant, *Predicate Abstraction with Indexed Predicates* (TOCL 2007; arXiv 2004)

**Theoretical core.** Extend predicate abstraction with *quantified*
predicates over arrays / memories: `∀i. mem[i] < threshold`. Without
this, predicate abstraction collapses any structured memory into either
"top" (too coarse, every read can be anything) or one predicate per
concrete index (combinatorial explosion). Relevant to mununu when the
extraction adapter handles C arrays or RTL register files.

**Pseudocode.**

```text
input : transition relation over arrays, candidate quantified predicates Q
output: refined predicate set P including indexed family

for each q = ∀i ∈ I. body(i) in Q:
    instantiate: p_i := body(i_witness) for representative i_witness ∈ I
    if p_i discriminates spurious cex:
        P.add(p_i family)               # one predicate per i, with index tagged
```

**mununu-shaped Rust sketch.**

```rust
// adapter/extraction/array_predicate.rs (NEW; FUTURE)
pub struct IndexedPredicate {
    pub array: String,                  // e.g. "mem" in C; "ram[]" in BTOR2
    pub index_role: IndexRole,          // representative | bounded-range | universal
    pub body: SmtExpr,
}
```

**Map to mununu.** Stage 4 extension for the C extraction path. **Not
Phase A** — gated on the Theory selector (see Risk in architecture doc).

---

## 14. Yang, Goel, Sakallah, *Leveraging Datapath Propagation in IC3 for Hardware Model Checking* (FMCAD 2023; arXiv 2309.14834)

**Theoretical core.** Within IC3, push *word-level constraints* directly
through datapath operators (adders, comparators) instead of bit-blasting
them inside generalize-cube. Each generalisation step retains word-level
structure, dramatically shrinking blocking-clause size on wide
datapaths. Like #12, this is an IC3-internal optimisation; **not
portable to mununu's explicit pipeline**.

**Pseudocode.** (Inside IC3's generalize_cube: replace bit-level pivot
selection with word-level pivot selection on the propagated constraint
graph.)

**mununu-shaped Rust sketch.** None — the algorithm lives below the
`Clts` abstraction. Cite for Phase B substrate selection (AVR-class or
rIC3-class engine).

**Map to mununu.** Phase B engine choice criterion.

---

## 15. Niemetz, Preiner, Wolf, Biere, *Btor2, BtorMC and Boolector 3.0* (CAV 2018)

**Theoretical core.** Spec for the BTOR2 word-level format: a typed,
sorted intermediate language for sequential hardware verification with
arrays and bit-vectors, designed as the successor of Btor and the
word-level peer of AIGER. mununu's BTOR2 reader (
[`adapter/btor2/parser.rs`](../../crates/mununu-core/src/adapter/btor2/parser.rs))
already consumes a subset; the paper defines the surface mununu must
keep honouring as Yosys evolves.

**Pseudocode.** (Format spec; no algorithm.)

**mununu-shaped Rust sketch.**

```rust
// adapter/btor2/parser.rs (EXISTING — cite, do not change)
pub struct Btor2Line { pub nid: u32, pub op: Op, pub sort: SortId, pub args: Vec<NodeId> }

/// Stage 1 ingest. Already implemented; predicate-image work in Stage 4
/// must not assume anything beyond what this parser preserves.
pub fn parse_btor2(text: &str) -> Result<Vec<Btor2Line>, ParseError> { todo!() }
```

**Map to mununu.** Stage 1 substrate. Anchor for "what does the BTOR2
side already promise downstream stages."

---

## 16. *The rIC3 Hardware Model Checker* (arXiv 2502.13605, 2025)

**Theoretical core.** Modern IC3 implementation for BTOR2 with the
post-2014 generalisations (PDR-CTG, IGR, etc.). Empirical baseline on
HWMCC 2024. mununu would not adopt its algorithm; rIC3 is the
**runtime baseline** the decision gate compares against — if Phase A's
explicit enumerator cannot solve Caliptra-class designs within 2× of
rIC3's runtime on the same BTOR2 input, Phase B is mandatory.

**Pseudocode.** (Standard IC3 + 2024-vintage generalisations; out of
scope.)

**mununu-shaped Rust sketch.** None — rIC3 is an external comparator
used in the decision gate.

**Map to mununu.** Phase A→B decision-gate runtime reference.

---

## 17. Hsiao, Trippel, Mulligan, Manerkar, *rtl2µspec: Synthesizing Formal Models of Hardware from RTL for Efficient Microarchitectural Security Verification* (MICRO 2021)

**Theoretical core.** A sibling pipeline: RTL → explicit-state model →
property check. The novelty is the *abstraction policy* — driven by
microarchitectural axioms (Spectre-class data-flow rules) rather than
per-signal user declarations. Same overall shape as mununu's BTOR2 path,
different decision about *what counts as a signal worth keeping*. Worth
citing as the closest non-mununu pipeline that produces an
explicit-state model for security verification, and as evidence that the
explicit-state surface (which mununu also commits to) has a track record
in this exact use case.

**Pseudocode.**

```text
input : Verilog RTL, microarchitectural axiom A
output: explicit-state model M with A-anchored predicates

P := predicates_from_axiom(A)               # e.g. "tainted signal reaches register"
COI := cone_of_influence(P, RTL)
M := build_explicit_model(restrict(RTL, COI), P)
return M
```

**mununu-shaped Rust sketch.** None — the paper's contribution is the
*axiom library*, not an algorithm. The mununu equivalent is the
[`adapter/templates/builtin_templates.json`](../../crates/mununu-core/src/adapter/templates/builtin_templates.json)
catalog plus the (future) microarchitectural-property templates.

**Map to mununu.** Validation that the explicit-state CLTS substrate is
defensible for security verification; informs the template registry's
domain taxonomy.

---

## 18. Caliptra 2.0 / 2.1 Architecture Specs (OCP 2022; Microsoft Tech Community 2024–2025)

**Theoretical core.** Not a paper; the architecture documents for the
empirical target. Defines the integrated root-of-trust IP, the FPV scope
that AMD/Google/Microsoft/NVIDIA contract out to third-party verification
firms, and the publicly-released RTL that mununu's Phase 1 work
exercises. Cite for: (a) the design is independently verified, so
mununu's contribution is a *complementary* abstraction-refinement view
on hand-bounded properties (CWE-1245 reachable UNDEF state, e.g.); (b)
the scope of "what is the modeled system" is constrained by Caliptra's
public release boundary.

**Pseudocode.** N/A.

**mununu-shaped Rust sketch.** N/A. The Caliptra-specific empirical
fixtures live under
[`.claude/reviews/prospector/staging/RTL-002/`](../../.claude/reviews/prospector/staging/RTL-002/)
and `/tmp/caliptra_retry/`.

**Map to mununu.** Empirical target for the decision-gate runtime
measurement.

---

## Cross-reference matrix

This matrix is the contract between this doc and
[`auto-extraction-architecture.md`](auto-extraction-architecture.md) §2 (stage
mapping) and §5 (current-vs-proposed comparison).

| # | Paper | mununu stage | Existing module touched | New module introduced | Phase |
|---|---|---|---|---|---|
| 1 | Kurshan 1994 — Localization | Stage 2 | [`adapter/domain.rs`](../../crates/mununu-core/src/adapter/domain.rs) (`Ignored` variant) | [`adapter/partition/`](../../crates/mununu-core/src/adapter/partition/) — **live (A.3)** | A.3 ✓ |
| 2 | Clarke et al. 2000 — CEGAR | Stage 6 | [`verify/orchestrator.rs`](../../crates/mununu-core/src/verify/orchestrator.rs) | `verify/refine.rs` | A.5 |
| 3 | Graf–Saidi 1997 — Predicate Abstraction | Stage 4 | [`kripke_smt.rs`](../../crates/mununu-core/src/adapter/systemverilog/kripke_smt.rs) | `adapter/sidecar/predicate_image.rs` | A.4 |
| 4 | Jain–Kroening TCAD 2008 — Word-Level | Stage 3 + 4 refinement | [`kripke_smt.rs`](../../crates/mununu-core/src/adapter/systemverilog/kripke_smt.rs) | `adapter/systemverilog/wp_refine.rs` | A.4–A.5 |
| 5 | VCEGAR TACAS 2007 | Stage 6 driver shape | [`crates/mununu-cli/src/main.rs`](../../crates/mununu-cli/src/main.rs) | `mununu verify --cegar` flag | A.5 |
| 6 | AVR NFM 2019 — SyGuS seed | Stage 3 | [`kripke.rs:scan_significant_constants`](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs) | `adapter/sidecar/predicate_seed.rs` | A.4 |
| 7 | AVR TACAS 2020 — tool | Phase B reference | n/a | `adapter/btor2/avr_bridge.rs` | B (conditional) |
| 8 | Goel–Sakallah VMCAI 2021 — SyGuS lemmas | Future | n/a | `adapter/sidecar/sygus_extend.rs` | post-B |
| 9 | Andraus–Sakallah Reveal 2008 — Datapath | Stage 2 | [`adapter/domain.rs`](../../crates/mununu-core/src/adapter/domain.rs) | [`adapter/partition/mod.rs::DatapathUf`](../../crates/mununu-core/src/adapter/partition/mod.rs) (type shipped, heuristic deferred) | A.3 follow-up — see [`phase-a3-followup-datapath-uf.md`](../../.claude/plans/phase-a3-followup-datapath-uf.md) |
| 10 | Bryant–Kroening TACAS 2007 — BV abstraction | Stage 4 inner | [`kripke_smt.rs`](../../crates/mununu-core/src/adapter/systemverilog/kripke_smt.rs) | `adapter/sidecar/bv_abstraction.rs` | A.4 |
| 11 | Hoder–Bjørner–de Moura CAV 2006 — Fast PA | Stage 4 algorithm | [`kripke_smt.rs:enumerate_values`](../../crates/mununu-core/src/adapter/systemverilog/kripke_smt.rs#L110) | `predicate_image.rs::all_abstract_edges` | A.4 |
| 12 | Cimatti IC3-IA TACAS 2014 | Phase B substrate | n/a | external | B (conditional) |
| 13 | Lahiri–Bryant TOCL 2007 — Indexed | Stage 4 extension (C) | n/a | `adapter/extraction/array_predicate.rs` | post-A |
| 14 | Yang–Goel–Sakallah FMCAD 2023 — Datapath in IC3 | Phase B engine choice | n/a | n/a | B (conditional) |
| 15 | BTOR2 / Boolector CAV 2018 | Stage 1 substrate | [`adapter/btor2/parser.rs`](../../crates/mununu-core/src/adapter/btor2/parser.rs) | n/a | A (cite) |
| 16 | rIC3 arXiv 2025 | Decision-gate baseline | n/a | n/a | A (gate) |
| 17 | rtl2µspec MICRO 2021 | Validation / template taxonomy | [`builtin_templates.json`](../../crates/mununu-core/src/adapter/templates/builtin_templates.json) | n/a | A (template doc) |
| 18 | Caliptra 2.0/2.1 spec | Empirical target | [`/tmp/caliptra_retry/`](file:///tmp/caliptra_retry/) | n/a | A.4–A.5 fixtures |

### Reading order if you only have time for three

1. **AVR NFM 2019 (#6)** — closest published comparator to mununu; sets
   the publication-positioning baseline.
2. **Hoder–Bjørner–de Moura CAV 2006 (#11)** — concrete algorithm for
   the Stage 4 hot loop; biggest expected runtime win at Caliptra scale.
3. **Andraus–Sakallah Reveal LPAR 2008 (#9)** — closest formalisation of
   the `wait_count`-collapse pattern that the Caliptra abstraction
   analysis hand-derived; parent of Stage 2.

### Out of Phase A scope (cite, do not adopt)

- IC3-IA (#12), Datapath-in-IC3 (#14), AVR-tool (#7), rIC3 (#16) — all
  live inside an IC3 prover, which mununu does not have and does not
  intend to build in Phase A.
- SyGuS lemma generation (#8) — gated on Stage 3 producing stable
  syntactic seeds first.
- Indexed predicates (#13) — gated on the Stage 4 SMT-theory selector
  (`Theory::BvUfArray`).
