# UF-memory end-to-end: the may/must problem and the paths to a verdict

> **Status: planning** — the deferred "Option A" from the §Phase 10 stage-3.c fork (2026-06-12). The array *encoder* is shipped (commits `9c5fa09` must-side encoder, `3600cba` must-side routing); what remains is turning it into an end-to-end KMTS verdict for an internal-memory design (e.g. ibex `ibex_register_file_ff`). This doc states the problem, lays out four implementation options with pros/cons, and gives the soundness concern + validation path for each.

> **Concept:** lifting a BTOR2 design with an internal memory cell to a 3-valued KMTS that the modal-mu evaluator can verify. Builds on [`expression-interpretation-unification.md`](expression-interpretation-unification.md) (the interpreter-layer map) and [`phase10-uf-routing.md`](phase10-uf-routing.md) (the routing dispatch).

## The problem

The predicate-cube lift ([`kmts_lift.rs::predicate_cube_lift`](../../crates/mununu-core/src/adapter/btor2/kmts_lift.rs)) builds a KMTS over `2^|P|` predicate-cube states. Each cube is a conjunction of register-equality predicates. The lift computes two transition relations between cubes:

- **may-edges** (`R_may`) — an **over-approximation**: a may-edge `b → b'` exists if *some* concrete state in `b` can reach *some* concrete state in `b'`. Sound for safety (∀-properties): if the over-approximation has no bad path, neither does the concrete.
- **must-edges** (`R_must ⊆ R_may`) — an **under-approximation / witness**: a must-edge `b → b'` holds if the concrete relation *forces* the transition (the exact ∀∃ shape depends on the must form). Needed for liveness (∃-properties), for alternating fixpoints, and for any property that asserts a value *must* be produced — like **read-after-write** (`read(addr) must equal the last write`).

A 3-valued (KMTS) verdict needs **both** relations. Bruns–Godefroid: definite verdicts (`KleeneT`/`KleeneF`) transfer to the concrete iff the may side over-approximates *and* the must side under-approximates the same concrete relation.

### Where memory breaks the current machinery

The two relations are computed by **two different mechanisms** today:

| Relation | Mechanism | Memory-capable? |
|---|---|---|
| may | **concrete sampling** — [`simulate_one_step`](../../crates/mununu-core/src/adapter/btor2/bit_blast.rs) → `eval_op` over `BvValue`, sampling `(input, UF-representative)` pairs | **No** — `eval_op` errors on `Op::Read`/`Op::Write`; concrete `BvValue` has no array type |
| must | **SMT** — `encode_design_for_lift` → `smt_must_edge` Z3 query | **Yes** (since `9c5fa09`) — Z3 array theory `select`/`store` |

The must side is solved. The may side is the open problem: **concrete sampling cannot evaluate an array operation.** A design with an internal memory cell, fed to today's `predicate_cube_lift`, errors during may-edge sampling.

### The core tension

The two relations want **different versions of the design**:

- The may-sampling wants a **memory-free** design — so it can run concretely. The natural memory-free version is the **havoc rewrite** (shipped stage-1b: `Op::Read` → fresh nondeterministic input, `Op::Write` dropped), which is a sound over-approximation of reads.
- The must-SMT wants the **original array** design — so Z3 array theory can prove `read(store(mem,a,v),a) == v` precisely.

So `read(addr)` is **havoc on the may side** (returns anything → loose, sound) and **array-exact on the must side** (returns the written value → tight). That is exactly the KMTS shape we want: `R_must ⊆ R_concrete ⊆ R_may`, with the may side loose-but-sound and the must side precise. The implementation problem is **how to feed both design versions through one lift.**

## The four options

### Option 1 — Two-design threading (havoc-may + array-must)

Thread **both** design versions through `predicate_cube_lift`: the havoc-rewritten file drives the may-sampling; the original array file drives the must-edge SMT post-pass. Add `PredicateCubeLiftOptions::array_must_file: Option<Btor2File>` (or similar); when set, the must post-passes call `encode_design_for_lift(array_must_file)` instead of `lazy.file()`.

- **Pros:** Minimal new infrastructure — reuses the shipped havoc rewrite (may) + the shipped array encoder (must). Directly produces the loose-may/tight-must KMTS. ~2–3 sessions.
- **Cons:** Two design representations in flight is a footgun — the cube indexing, register-NID maps, and predicate resolution must stay consistent across the havoc'd and original files (their state-cell NIDs match because havoc only rewrites array ops, but this is an invariant to test hard). The may-side stays *havoc-loose* — a read-after-write property's may side admits the violation, so the verdict relies entirely on the must side promoting the right edges; if must-promotion misses, the verdict is `KleeneBot` (refine) rather than `KleeneT`.
- **Soundness concern:** The havoc may-edge over-approximation is sound **only if** the havoc rewrite faithfully over-approximates every concrete read (it does — a fresh unconstrained input admits every value the concrete memory could hold). The risk is the *consistency* invariant: if the havoc'd file and the array file disagree on a non-memory NID, the may and must relations describe different designs and the KMTS is unsound. **Mitigation:** assert (test) that havoc rewriting changes *only* array-op NIDs, leaving every BV NID identical.
- **Validation path:** (a) the read-after-write UNSAT test on the array encoder (done); (b) a monotonicity test — every must-edge is also a may-edge (`R_must ⊆ R_may`) on a hand-built memory fixture; (c) end-to-end read-after-write on ibex `ibex_register_file_ff` scaled to 4×4 bits, verdict `KleeneT`, cross-checked against SBY bounded-check.

### Option 2 — Symbolic may-side (SMT may-image)

Replace the concrete may-sampling with an **SMT-based may-image**: `R_may(b,b') ⟺ ∃ s ⊨ b, ∃ input, ∃ s' ⊨ b'. (s, input, s') ∈ T`, encoded via the **same** array-aware `encode_design_for_lift`. Both relations then come from one encoder; no havoc, no two-design threading.

- **Pros:** Architecturally clean — one design, one encoder, both relations from SMT. The may side becomes *exact* (the existential image), tighter than havoc, so fewer `KleeneBot` verdicts and read-after-write's may side no longer admits the violation. No consistency-invariant footgun.
- **Cons:** Replaces the *entire* shipped sampling-based may-edge mechanism with SMT — a much bigger change than Option 1, touching the load-bearing R.2.5 may-edge construction that every existing predicate-cube fixture depends on. Per-cube-pair SMT cost (`2^|P| × 2^|P|` existential checks) is higher than sampling; needs the budget guards the R.2.5b SMT post-passes already use. Risk of regressing memory-free fixtures' may-edges if the SMT image disagrees with sampling on edge cases.
- **Soundness concern:** The existential SMT may-image is *exact* (neither over- nor under-approximating the predicate-abstraction image) — which is *more* precise than the sound over-approximation a may side strictly requires. That's fine (exact ⊆ over-approx). The concern flips to *completeness/cost*: an SMT timeout must fall back to a sound over-approximation (admit the edge), never silently drop it (which would be unsound under-approximation of may).
- **Validation path:** (a) on every memory-free R.2.5 fixture, the SMT may-image must produce a may-edge set that is a *superset-or-equal* of today's sampling may-edges (sampling can miss edges; SMT must not lose soundness) — a regression test across the fixture corpus; (b) the read-after-write KleeneT on ibex regfile; (c) timeout-fallback test (forced timeout → edge admitted, not dropped).

### Option 3 — Bounded bit-blast the memory (stage 4)

For *small* memories, enumerate the array concretely: a `N`-entry × `W`-bit memory becomes `N` ordinary `W`-bit bit-vector state cells (the `selected_addresses` ladder). No SMT, no havoc — the array disappears into the existing concrete bit-blast path.

- **Pros:** Reuses the entire concrete machinery unchanged; both may (sampling) and must work because there is no array left. Exact (no abstraction) within the cap. This is **already the planned stage 4** (`MemoryAbstraction::BoundedBitBlast` + `selected_addresses`).
- **Cons:** Only viable when `N × W` is tiny (the `MAX_STATE_BITS = 20` cap). ibex regfile is 32 × 32 = 1024 bits raw — needs the property to touch only a handful of addresses (`selected_addresses: [0, 1, 5, 31]`) for the rest to be dropped/havoc'd. Doesn't scale to real memories; it's a demonstration mode, not the general answer.
- **Soundness concern:** Exact for the *selected* addresses; the *unselected* addresses must be havoc'd (sound over-approximation) or the design is unsound (silently dropping them would under-approximate). The soundness hinges on the unselected-address policy being havoc, not drop.
- **Validation path:** (a) a fixture where `N × W` fits the cap entirely → exact verdict, SBY cross-check; (b) a fixture where only `selected_addresses` fit → the unselected addresses are havoc'd, verdict is sound-but-loose; assert it's weaker-or-equal to the full-enumeration verdict.

### Option 4 — Full `BvTermBackend` unification (Move B + memory as a capability)

The long-term answer from [`expression-interpretation-unification.md`](expression-interpretation-unification.md): merge the concrete (`eval_op`) and SMT (`encode_op`) interpreters behind one `BvTermBackend` trait + a generic AST-walk driver. Memory becomes a *backend capability* — the `Z3Backend` does arrays via theory; the `Concrete` backend does them via bounded enumeration; a future `AbstractBackend` could do interval/octagon. The may/must split becomes "which backend you run," not "which mechanism you wrote."

- **Pros:** Eliminates the dual-maintenance of two interpreters *and* gives memory a uniform home. The may/must tension dissolves: run the `Concrete` backend (with bounded or havoc memory) for may, the `Z3Backend` (array theory) for must — same driver, same AST, different backend. The cleanest, most future-proof architecture.
- **Cons:** The largest change — a refactor of two load-bearing, soundness-critical, heavily-tested interpreters into a trait. ~5–8 sessions for the trait + driver + two backend impls, before any memory-specific work. High risk of subtle behavioral drift if any operator's two implementations weren't actually identical (they should be, but the merge is where you find out).
- **Soundness concern:** The merge itself is soundness-neutral *if* every operator's concrete and SMT implementations were semantically identical pre-merge (they walk the same AST with the same arity/width). The risk is discovering a latent disagreement during the merge — which is actually a *find*, not a regression, but must be handled carefully.
- **Validation path:** the merge is validated by the *union* of the existing concrete + SMT test suites passing against the unified driver (both backends produce their pre-merge results bit-for-bit). Memory then rides in as a capability test per Option 1/2/3's validation.

## Recommendation + sequencing

| Option | When | Risk | Delivers |
|---|---|---|---|
| 3 (bounded bit-blast) | **stage 4, soon** | low | small-memory demo; no SMT |
| 1 (two-design threading) | **the uf-memory milestone** | medium | havoc-may/array-must on ibex regfile |
| 2 (symbolic may-image) | after 1, if havoc-may is too loose | medium-high | exact may; fewer KleeneBot |
| 4 (`BvTermBackend`) | post-slot-4, own track | high | the unified architecture |

**Recommended path:** ship **Option 3** as stage 4 (it's the already-planned bounded mode and gives a concrete small-memory verdict with no new abstraction risk), then do **Option 1** as the uf-memory milestone on ibex regfile (havoc-may + the shipped array-must). Treat **Option 2** as the precision upgrade only if Option 1's havoc-may produces too many `KleeneBot` verdicts in practice (measure first). Keep **Option 4** as the post-slot-4 unification track — it subsumes all of the above but must not block the milestone.

The unifying soundness invariant across every option: **`R_must ⊆ R_concrete ⊆ R_may`**, with the may side a sound over-approximation (havoc, exact-existential, or havoc'd-unselected-addresses) and the must side a sound under-approximation (array theory). Every validation path above is ultimately a test that this chain holds on a fixture where the answer is independently known (read-after-write on ibex regfile, SBY-cross-checked).

## Cross-references

- Interpreter-layer map + the three unification moves: [`expression-interpretation-unification.md`](expression-interpretation-unification.md)
- Routing dispatch + corrected stage-3.c: [`phase10-uf-routing.md`](phase10-uf-routing.md)
- Stage-5 Option-B (Caliptra mbox black-box): `.claude/plans/measurements/Phase10-stage5-caliptra-mbox-blackbox-2026-06-12.md`
- Array encoder (shipped): commits `9c5fa09` (encoder), `3600cba` (must-side routing)
- KMTS soundness foundation: [`kmts-theory.md`](kmts-theory.md)
