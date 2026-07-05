# Track AR — Architecture review & consolidation gate

> Analysis + planning only. No edits land without the AR.5 GO verdict (per roadmap Track AR).
> Run 2026-07-05. Evidence gathered by three parallel `Explore` audits (AR.1 pipeline/seam,
> AR.2 coupling, AR.3 function inventory). AR.4/AR.5 synthesized from that evidence.

## Executive summary (AR.5 GO/NO-GO up front)

**The architecture is fundamentally sound — NO-GO on any consolidation *rewrite*; GO on four
surgical, high-confidence, low-blast-radius items.** The three-axis design (abstraction × engine ×
domain) holds in the code; the STS-IR seam has a single clean implementor; `Clts` is exemplary
encapsulation. The real debt is *targeted duplication and drift hazards*, not structural rot — and
one of them (the resolver family) is the exact class of bug that caused #242 this session. That
makes the case for the drift-guard refactors evidence-backed rather than speculative.

**GO (do these — ordered by value ÷ blast-radius):**
0. **Diagram/doc honesty — cheapest, do first.** Fix `docs/design/post-rf5-architecture.md` §2 to match
   the as-built: the model path *flattens* (drop "(no flatten)"); the live symbolic evaluator is
   `AbstractRelation::evaluate` (not the test-only `SymbolicKmts::evaluate`); add the 4th engine
   (`exact_symbolic_verdict`); and correct the stale `sts_ir.rs:8-13` module doc ("No existing call
   site is rewired yet" → the seam is live for the SmtAllPairs/resolve slice). Pure doc; near-zero risk.
1. **Resolver drift guard (#242 family) — HIGHEST value/effort.** Extract the `build()` cell-naming
   + `next_funcs` keying in `symbolic_bitblast.rs` into one internal helper so the two halves cannot
   drift again (VERY LOW blast radius, internal). Then introduce a single
   `resolve_to_canonical_name(file, name, Strictness)` behind `resolve_signal_symbol` (strict) and
   `BtorSts::resolve_register` (loose) so the strict-vs-loose divergence is an explicit parameter,
   not a silent duplication (LOW blast radius, ~9 callers). *Soundness-visible.*
2. **Subprocess-locator dedup — clearest mechanical DRY win.** `locate_slang`/`btormc`/`cvc5`/
   `verilator` are copy-paste-identical modulo (env var, default bin, version flag, hint, parser).
   Extract `locate_tool(...)` into `adapter/subprocess.rs`; the four become 3-line wrappers. Also
   the verbatim `locate_cmsis_stubs` dup between root `src/main.rs` and `crates/mununu-cli/src/main.rs`
   (a stale pre-crate-split copy). LOW–MEDIUM blast radius, public signatures unchanged.
3. **Ergonomic builders for the change-amplifiers.** `TransitionSpec` (IR, ~15 construct sites) and
   `OriginalTransition` (5 sites + doctests — the exact struct behind the CLAUDE.md pre-push CI pain,
   K.1b/K.2b) get a `Default` base / builder so a new field defaults everywhere instead of forcing a
   literal edit at every site (incl. doctests, which the pre-push check historically missed). Factor
   the shared `(modality, additional_targets)` tail of `OriginalTransition`/`TransitionSpec`/
   `TransitionDecl` into one `Modality` sub-struct.
4. **Trim dead option/echo fields (YAGNI).** `CegarOptions`: `lift_strategy::Lazy` "produces the same
   verdict as Eager" and `CegarTrace::lazy_lift_pending` is "always true"; `CegarTrace::approximant_reuse_enabled`
   merely echoes `CegarOptions`. Remove the no-op flags + echoes (common coupling → single source of truth).

**NO-GO (for now — but the top structural debt, named):** **full STS-IR seam adoption** — routing the
default sampling cube path + both BDD engines through `BtorSts`/`SmtEncode` to collapse the *three*
parallel predicate-image implementations into one. This is the seam's original goal and #242 was its
symptom, so it is genuinely load-bearing debt — but it is a **large, soundness-critical refactor of
the core edge-computation** (may/must inference is where verdict soundness is decided), high blast
radius, and the three impls are currently differential-cross-validated. Defer to a dedicated,
carefully-gated effort with a per-impl differential harness; do NOT fold it into an incidental
quality-session. Also NO-GO: a whole-pipeline rewrite; a second STS-IR *frontend* abstraction (one
implementor — YAGNI); collapsing the `Trit`/`Tristate` or `TritSet`/`TritBdd` layer splits
(deliberate, differential-guarded).

**Deciding factors (named, not fatigue framing):** (a) the GO items are soundness-adjacent
(resolver drift = #242) or pure mechanical dedup — high confidence, small blast radius; (b) a big
restructure has high soundness-critical-seam risk (the evaluator/lift/seam are the load-bearing
core) for low marginal value over the surgical items; (c) opportunity cost vs Track H/R-F5.6 is real
— these GO items are cheap enough to interleave, a rewrite is not.

---

## AR.2 — Interface-struct & coupling audit

Full table in the audit; the load-bearing findings:

**Top change-amplifiers (shape-change ripple widest):**
1. `TransitionSpec` (IR, `adapter/ir.rs:168`) — ~15 frontend-adapter construction sites; widest fan-in.
2. `OriginalTransition` (`abstraction/unrolling.rs:17`) — 5 prod sites across 3 crates + doctests; the
   #K.1b/K.2b CI-failure struct.
3. `PredicateSpec` (`kmts_lift.rs:325`) — 9+ sites, but stable 3-field value object (low risk).
4. `CegarOptions` (`cegar.rs:137`) — 7 behavior-switch flags, some no-op (control-coupling god-options).
5. `PredicateCubeLiftOptions` (`kmts_lift.rs:382`) — cross-field invariant (`compound_exprs` requires
   `may_edge_inference==SmtAllPairs`), enforced at runtime rather than by construction.

**Address verdicts:** builder/Default for #1/#2 (ergonomics); trim dead flags on `CegarOptions` +
`CegarTrace` echoes; enforce the `PredicateCubeLiftOptions` invariant via a constructor; encapsulate
`Btor2SmtView` (all-`pub` raw Z3 handles + an *unenforced drop-ordering temporal invariant* — bind
the Z3-scope lifetime into the type). **Leave (exemplary):** `Clts`, `Transition`, `TritSet`,
`WitnessCell` — private fields + accessors, the reference for how a seam type should look.

**Near-alias pairs:** `OriginalTransition`/`TransitionSpec`/`TransitionDecl` (shared modality tail →
one `Modality` sub-struct); `Trit`↔`Tristate` (two 3-valued enums + lossless `From` — leave, layer
split); `TritSet`↔`TritBdd` (perf reps, differential-guarded — leave); `ir::TransitionSpec` vs
`clts::TransitionSpec<S,L>` (**name collision** — rename the private builder-internal one); the
four-way `Guard`/`GuardExpr` family (separate consolidation pass).

## AR.3 — Function inventory & merge candidates

**Surface:** mununu-core = **889 `pub fn`**, concentrated in `clts` (95), `abstraction` (113),
`mu_calculus` (106), `adapter/btor2` (121). `mununu-cli` = 0 pub fn (binary), `mununu-extract` = 6.
Full per-module table in the AR.3 audit output.

**Merge-candidate clusters:**

| Cluster | Verdict | Blast radius | Priority |
|---|---|---|---|
| Resolver name→nid→name wrappers (`resolve_signal_symbol` strict vs `BtorSts::resolve_register` loose) | **REFACTOR** to a `Strictness`-parameterized helper | LOW | **HIGH** (soundness, #242 family) |
| #242 `build()` cell-name / `next_funcs` keying | **REFACTOR** to one internal helper (drift guard) | VERY LOW | **HIGH** (best value/effort) |
| Subprocess locators (`locate_{slang,btormc,cvc5,verilator}`) | **REFACTOR** to shared `locate_tool` | LOW–MED | **HIGH** (mechanical DRY win) |
| `locate_cmsis_stubs` root/cli verbatim dup | REFACTOR (stale copy) | LOW | Medium |
| BTOR2 text appenders (`augment/pin/inject` + `emit_*_monitor`) | REFACTOR internals to `Btor2Appender`; keep public API | MEDIUM | Medium |
| Predicate rewriters (`resolve_predicate_registers` vs `resolve_predicate_expr_registers`) | REFACTOR (tied to Spec/Expr duality) | MEDIUM | Medium |
| `spurious_verdict*` + production oracles | **LEAVE** (test-only + intentional cross-validation) | NIL | Low |
| strict/loose resolver *primitives* (`resolve_state_by_symbol` vs `resolve_state_alias`) | **LEAVE** (both correct; co-locate + doc) | — | — |

## AR.1 — As-built pipeline vs the intended mermaid (seam integrity)

**The macro pipeline is backed and faithful; the STS-IR seam is real but only PARTIALLY adopted.**
`BtorSts` is the canonical `Btor2File → StepEval/SmtEncode` waist, and the opt-in
`SmtAllPairs`/compound/derived slice + all register-name resolution correctly flow through it. But
the stated "single de-duplicated waist for the predicate image" goal (`sts_ir.rs:15-29`) is **not
met** — three engines reach `Btor2File`/z3 directly:

- **B1 — the DEFAULT explicit cube path (sampling `MayEdgeInference::Off`) HARD-BYPASSES the seam**
  (and it is the default). `cube_sampling_edges` calls `bit_blast::simulate_one_step` directly
  (`kmts_lift.rs:1587`); must-edges reach `smt_must_edge::*` + `z3::with_z3_config` directly
  (`kmts_lift.rs:1065-1068,1134-1139`). A second predicate-image implementation living beside the
  seam's — the exact class of drift that produced #242.
- **B2 — symbolic cube engine** (`BddBitBlaster::build`, `symbolic_bitblast.rs:156`) reads
  `Btor2File`/`Node`/`Nid` directly; never constructs `BtorSts`.
- **B3 — exact-symbolic engine** (`exact_symbolic_verdict`, a shipped CLI-reachable **4th engine
  absent from the §2 diagram**) builds `BddBitBlaster` directly; uses the seam only for
  `resolve_register`.
- B4/B5 — ENUM (`bit_blast`) and the verify-auto orchestrator share the seam *primitive*
  (`simulate_one_step_observe`) but not the seam *type* (soft bypass; behavior unified).

**Diagram / doc drift (cheap to fix, high-honesty value):** (i) the `SV→B2` "(no flatten)" label is
wrong — the shipped yosys model script *flattens* (`yosys/mod.rs:863`); "(no flatten)" describes only
the separate hierarchy-discovery pass. (ii) The `EVS` node names `SymbolicKmts::evaluate`, which has
**no production caller** (R-F5.0 spike, test-only); the live symbolic evaluator is
`AbstractRelation::evaluate`. (iii) `exact_symbolic_verdict` (4th engine) is undiagrammed. (iv)
`sts_ir.rs:8-13` module doc is **stale** ("No existing call site is rewired yet") — contradicted by
the live `BtorSts` calls in `kmts_lift.rs`.

**Responsibility-bleed:** none structural (translator does not build the model; the lift does not
evaluate). The only "bleed" is the *duplicated predicate-image logic* across the seam, the sampling
path, and the BDD engines — a drift hazard, not a layering violation.

## AR.4 — Clean-slate delta (target vs as-built)

The clean-slate architecture (from the README "three orthogonal choices: abstraction × engine ×
domain") **matches the as-built** at the macro level — the three axes, the seam, the two verdict
representations all exist as designed. The divergences are all *micro* and already captured above:
(i) the resolver family has no single canonical entry (drift hazard); (ii) the transition-record
triple duplicates a modality tail; (iii) subprocess-locator boilerplate; (iv) a name collision
(`TransitionSpec` ×2) and a four-way `Guard` family; (v) `Btor2SmtView` leaks the Z3 layer. None of
these is a structural redesign — they're the delta between "designed clean" and "grew organically,"
and every one is a bounded refactor. **The clean-slate exercise confirms: consolidate the seams, do
not redraw them.**
