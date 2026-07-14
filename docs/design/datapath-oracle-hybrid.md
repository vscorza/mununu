# Datapath-oracle hybrid for the KMTS 3-valued engine — design note + plan

> Status: planning

Companion to [`../../measurements/hwmcc-native-safety-feasibility.md`](../../measurements/hwmcc-native-safety-feasibility.md).
That doc measured the walls (§1–§5) and validated (§8.2) that grammar/budget tweaks
*confirm* them. This note proposes the structural way to actually cross the two that
matter — deep-CEX reach and datapath (nonlinear) reasoning — **without** discarding the
3-valued KMTS engine, and audits which libraries can be used given mununu's license.

---

## 1. The idea — the KMTS engine is already layered; the walls are in the query layer

The 3-valued KMTS engine separates cleanly into two layers:

- **Abstraction layer** — predicates → cubes → a KMTS (may/must) structure → Kleene
  3-valued modal-μ evaluation (`mu_calculus`). This is what buys branching-time +
  alternation soundness (Bruns–Godefroid: definite `KleeneT`/`KleeneF` transfer at every
  alternation depth).
- **Decision-procedure (query) layer** — the may/must edges
  (`kmts_lift` SMT post-image), the CEGAR refinement (`refine` / `native_interp`
  interpolation), and the verdict verification (`native_bmc::bmc_bad_reachable`, the
  inductive re-check in `abs_safety`) are **all SMT/SAT queries over the exact,
  bit-precise, word-level transition.**

Bruns–Godefroid depends only on **may edges over-approximating** and **must edges
under-approximating** — *not* on how a query is answered. So **any sound decision
procedure can be swapped into the query layer without touching the 3-valued soundness.**
The two feasibility walls both live in that layer:

- **Deep-CEX reach** (T3 / §2, §7): `bmc_bad_reachable` over z3 is too slow to reach
  depth 75–128 on wide designs (the *cumulative* cost of the accumulated unrolling).
- **Nonlinear datapath** (T1/T2 / §5): the may/must + refinement queries over `bvmul` /
  `bvurem` explode (cvc5 SyGuS interpolation; z3 per-edge multiplier-SAT).

Neither wall is in the abstraction layer. That is the whole compatibility argument.

## 2. There is already a precedent in the tree

mununu already ships one datapath-specialised oracle merged into the KMTS path: the
**ranking certificate** (`recoverability::ranking_certificate_holds`). It answers a query
the generic predicate cube cannot — "does every non-`good` transition strictly decrease a
measure δ?" — with a single Podelski–Rybalchenko SMT query over the **exact** transition
(including a 48-bit multiplier in the `data*data` recoverability example), and hands a
sound verdict back to the KMTS structure. That is precisely "merge a datapath reasoning
technique with the 3-valued engine," and it is sound-by-construction (it abstains, never a
wrong verdict). The hybrid proposed here generalises that one-off into a **pluggable oracle
interface**.

## 3. The oracle abstraction

A trait the query layer consults instead of calling z3 directly:

```rust
/// A sound decision procedure for the KMTS engine's concrete queries. Every method
/// has a SOUNDNESS DIRECTION; an oracle that cannot answer returns the conservative
/// value so 3-valued soundness is preserved.
pub trait DatapathOracle {
    /// EXACT reachability (concrete witness). `Violated{depth, trace}` is sound only
    /// with a real model; `NoCexWithin{k}` is a bounded fact; `Unknown` abstains.
    fn bad_reachable_within(&self, view: &Btor2SmtView, k: u32) -> Reach;

    /// MAY edge — OVER-approx. `∃` a concrete transition `cube_i → cube_j`?
    /// SOUND DIRECTION: a `false` must be a *proof* of no transition; on `Unknown`
    /// the caller KEEPS the may edge (over-approx stays sound).
    fn may_edge(&self, t: &Transition, ci: &Cube, cj: &Cube) -> Trit;

    /// MUST edge — UNDER-approx. does EVERY concrete transition from `cube_i` land in
    /// `cube_j`? SOUND DIRECTION: a `true` must be a *proof*; on `Unknown` the caller
    /// DROPS the must edge (under-approx stays sound).
    fn must_edge(&self, t: &Transition, ci: &Cube, cj: &Cube) -> Trit;

    /// REFINEMENT — a separating predicate for a spurious abstract step, or `None`.
    fn refine(&self, spurious: &AbstractStep) -> Option<PredicateExpr>;
}
```

The contract is the load-bearing part: **`Unknown` maps to the conservative side per
method** (keep may, drop must, abstain on reachability), so a fast-but-incomplete oracle
can only *widen* the ⊥ region, never produce a wrong `KleeneT`/`KleeneF`. z3 is the default
implementation; the two new ones are §5.

## 4. Worked application to the failing HWMCC cases

| Design | Class | Which oracle | Would it decide? |
|---|---|---|---|
| `vis_arrays_buf_bug` | deep-CEX (@18–28), **arrays** | **fast-SAT + arrays** | **Yes (measured)** — btormc/Boolector: **1 s** (owned z3-BMC: timeout @120 s) |
| `krebs.3` | deep-CEX (@75), non-array | **fast-SAT** | **Yes (measured)** — btormc/Boolector: **73 s** (owned z3-BMC: never reached it @240 s) |
| `brp2.2` | deep-CEX (@119) | fast-SAT | **No (measured)** — btormc/Boolector **timed out @300 s**; too deep even for the best BV-SAT in budget |
| `circular_pointer_top_w64_d128` | deep-CEX (@~128), wide | fast-SAT | **No (measured)** — btormc/Boolector **timed out @300 s** |
| `mul9` | multipliers present, but **control** property (`bad = and(51,52)`, *not* `out == a·b`) | nonlinear (mismatched) + invariant synthesis | **Partial at best** — mixed logic+arithmetic proof; not a clean multiplier-correctness target the algebraic oracle cracks (§4.2) |
| `gen43` | 256-bit **pure BV** (0 `mul`/`urem`/`add`) | fast-SAT (width) + **invariant synthesis** | **No by nonlinear oracle** — it has no arithmetic; needs §3 synthesis, not a datapath oracle |
| `arbitrated_top_*_d64` | wide **proof** (no nonlinearity) | fast-SAT (queries) + **invariant synthesis** | **No by oracle alone** — the wall is §3 invariant *discovery*, not the query speed |

> **Structure audit (2026-07-13).** A BTOR2-node inspection of the two "nonlinear" targets
> changed this table: **`gen43` has zero arithmetic operators** (the `bvurem` was a cvc5
> interpolant-search artifact, not a datapath op) — it is a wide pure-BV invariant-synthesis
> case, not a nonlinear one; and **`mul9`'s property is a control condition** that reads the
> product only indirectly, so it is a *mixed* proof, not the multiplier-correctness problem
> the algebraic oracle targets. **Consequence: no current failing design cleanly motivates
> the nonlinear oracle (§5.2/P2).** It remains a *potential* capability, but the concrete
> corpus points at the fast-SAT oracle (P1) + §3 invariant synthesis, not P2.

### 4.1 Deep-CEX cases — the clean win (fast-SAT oracle)

`krebs.3` violated at depth 75; §8.2 measured owned z3-BMC never reaching it (cumulative
query cost) *and* external btormc @60 s also abstaining. The fast-SAT oracle answers
`bad_reachable_within` with an **incremental** bit-vector SAT engine (assert one frame,
keep learned clauses, re-solve) instead of z3's grow-and-re-solve. This is exactly what
btormc does internally; doing it in-process closes the reach. The abstraction is
untouched — for a pure `AG ¬bad` the KMTS layer is a thin pass-through and the oracle does
the work; for a *branching* property over the same design the oracle answers the may/must
reach queries the Kleene evaluation needs. **This is the recommended first increment** and
is license-clean (§5). **Measured scope (2026-07-13):** it fixes **two** of the deep-CEX
cases — `vis_arrays` (btormc 1 s) and `krebs.3` (btormc 73 s) — *not* four: `brp2.2` (@119)
and `circular_pointer_d128` **time out even for btormc/Boolector at 300 s**, so no fast-SAT
engine (in-process or external) closes them in budget. P1's win is real but bounded to the
shallow/moderate deep-CEX class.

### 4.2 The nonlinear oracle in principle — and why the current corpus doesn't need it

*In principle*, on a **genuine multiplier design whose property is the arithmetic relation**
(`out == a·b`), a datapath predicate + a nonlinear oracle answering "does this product
relation hold across the transition?" makes the abstraction computable over the multiplier.
Even then, the CEGAR loop must still **discover** the right datapath predicate — interpolation
over `bvmul`, the §5 wall — so the oracle converts "cannot even try" into "tries, each edge
query now tractable," but only *decides* if a **bounded, discoverable** datapath invariant
exists. That is the honest ceiling of the mechanism.

*In practice, the two designs that looked like the motivation are not that problem* (structure
audit above):

- **`mul9`** has multipliers but `bad = and(51, 52)` is a **control** condition reading the
  product only indirectly. Proving it safe needs an invariant relating that control condition
  *through* the multiplier — a **mixed logic+arithmetic** proof, the worst case for a pure
  algebraic reasoner (which proves `out == a·b`, not "control condition C holds given the
  product"). The nonlinear oracle would help the arithmetic *sub-queries*, but the proof
  is not a clean multiplier-verification task.
- **`gen43`** has **no arithmetic at all** (0 `mul`/`urem`/`add`) — it is 256-bit pure logic;
  a nonlinear oracle has nothing to reason about. Its `unknown` is a §1 width + §3 synthesis
  problem, served by the fast-SAT oracle for the width and invariant discovery for the proof.

So the datapath oracle stays a *sound, principled* extension point, but **P2 is not motivated
by any design currently failing** — the corpus points at P1 (fast-SAT) + §3 synthesis.

### 4.3 Wide-proof case — outside the oracle's reach

`arbitrated_top_*_d64` has no nonlinearity; §7 measured it `unknown` for everyone. The
wall is synthesising a compact inductive invariant over an ~8 k-bit state — the §3
SPACER-class problem. A faster query oracle speeds each IC3/interpolation query but does
**not** synthesise the certificate; this case needs the invariant-discovery engine, not a
datapath oracle. Listed to keep the scope honest: the hybrid is not a universal solvent.

## 5. Library & license audit — the poisoning question

**Mununu's constraint is unusually strict.** The workspace is under the **Mununu
Non-Commercial License** (proprietary, source-available — *not* open-source), and
`deny.toml` allows only genuinely-permissive licenses on **linked** crates (MIT, Apache-2.0,
BSD-2/3, ISC, Zlib, MPL-2.0, …), enforced by `cargo deny check licenses` in CI. Two
consequences:

1. **Copyleft is hard poison for *linked* code.** GPL/LGPL/AGPL require the combined work
   be (L)GPL — impossible for a non-commercial proprietary work, and blocked by `deny.toml`
   anyway. So an in-process (statically-linked) solver **must** be permissive.
2. **Subprocess, not-bundled = no poisoning.** mununu already shells to btormc / Pono /
   cvc5 / yosys / sv2v / slang at arm's length (separate process, invoked if present, never
   bundled). A separate process communicating over files/pipes is not a derivative work, so
   a *GPL* tool used this way does not contaminate mununu — this is the existing escape
   hatch and the only clean route for the copyleft-only algebraic tools.

> Licenses below are stated to the best of current knowledge; **re-verify the exact SPDX at
> integration time** (upstreams relicense). The rule to apply is mechanical: *linked ⇒ must
> be in the `deny.toml` allow-list; copyleft ⇒ subprocess-only, never bundled.*

### 5.1 Fast bit-vector SAT / SMT-BV (for §4.1 — the deep-CEX oracle)

| Candidate | Kind | License | Verdict for mununu |
|---|---|---|---|
| **Bitwuzla** (`bitwuzla-sys`) | word-level BV+array SMT, incremental | **MIT** | ✅ **link** — SOTA on BV, has array theory (covers `vis_arrays`), MIT is allow-listed. Preferred. |
| **CaDiCaL** (`cadical` crate) | CNF SAT, incremental (IPASIR) | **MIT** | ✅ **link** — pair with mununu's own bit-blaster (`symbolic_bitblast`); cleanest "fast SAT + own encoding" path |
| **Kissat** | CNF SAT, one-shot | **MIT** | ⚠️ link-OK but **non-incremental** → poor fit for incremental BMC; prefer CaDiCaL |
| **Varisat** | pure-Rust SAT, incremental | **MIT/Apache-2.0** | ✅ **link** — zero FFI/build risk, easiest integration; slower than CaDiCaL |
| **splr** | pure-Rust SAT | **MPL-2.0** | ✅ link — MPL is allow-listed (file-level copyleft, non-viral) |
| **CryptoMiniSat** | CNF SAT | **MIT** (relicensed) | ✅ link — heavier; verify the linked version is the MIT one |
| **Boolector 3.x** | word-level BV SMT | MIT *(verify)* | ⚠️ prefer **Bitwuzla** (its actively-maintained MIT successor) |
| **btormc / Boolector** *(as today)* | model checker | *(subprocess)* | ✅ already subprocess — no poisoning; but in-process is the point here |

**Conclusion: the fast-SAT oracle is license-clean.** Bitwuzla (MIT, in-process,
BV+arrays) is the strongest single choice; CaDiCaL+own-bitblaster or pure-Rust Varisat are
clean fallbacks. No copyleft is required for this mechanism.

### 5.2 Word-level / algebraic nonlinear reasoning (for §4.2 — the datapath oracle)

| Candidate | Kind | License | Verdict for mununu |
|---|---|---|---|
| **Singular** | Gröbner bases (CAS) | **GPL** | ⛔ **no link** — subprocess-only if used at all |
| **msolve** | polynomial system solving | **GPL** | ⛔ no link — subprocess-only |
| **CoCoALib** | Gröbner bases | **GPL** | ⛔ no link |
| **FLINT** | number theory / polynomials | **LGPL** | ⛔ no link (LGPL still incompatible with a proprietary non-commercial work); subprocess-only |
| **AMulet2** | algebraic multiplier verification (mod 2ⁿ) | MIT *(verify)* | ⚠️ a *tool*, not a library → **subprocess** (AIG→certificate), like btormc; or port its algorithm |
| **z3 nlsat** *(already linked)* | nonlinear real arith | MIT | ✅ linked, but NRA — it **bit-blasts** BV multiplication (the hard path); not an algebraic-BV oracle |
| *native mod-2ⁿ polynomial reasoning* | roll-your-own | mununu's own | ✅ license-clean, but a real research/eng effort |

**Conclusion: the nonlinear oracle is doubly hard — technically *and* by license.** The
strong algebraic engines (Singular/msolve/CoCoA/FLINT) are **all copyleft**, so they are
**subprocess-only** for mununu (fine, same pattern as btormc — invoke if present, never
bundle). The only *linkable* clean routes are (a) AMulet2's algorithm reimplemented
natively (polynomial reasoning mod 2ⁿ — clean but substantial), or (b) staying with
z3/cvc5 and accepting the bit-blasting wall. So the honest recommendation is: if the
nonlinear class is pursued, do it as a **non-bundled subprocess oracle** (AMulet2 for
multipliers) mirroring the btormc pattern, not a linked dependency.

## 6. Phased plan

- **P0 — the oracle seam (no new dependency).** Extract the `DatapathOracle` trait and
  route the existing z3-backed queries (`bad_reachable_within`, may/must edges, refine)
  through it, with z3 as the sole implementation. Pure refactor; behaviour-identical;
  gated by the existing differential tests. This is the enabling step and carries no
  license risk.
- **P0.5 — cheap portfolio budget knob (no dependency) — SHIPPED.** A `--timeout-ms` flag
  on `btor2 verify` overriding the subprocess-member (btormc/Pono) budget (default 60 s).
  **Subtlety that a naive version misses (caught empirically):** raising the *time* is
  useless while btormc's *depth* stays capped at `DEFAULT_KMAX = 40` — `krebs.3`'s CEX is at
  depth 75 — so the raised-budget path also lifts btormc's `-kmax` (to 1000). Measured:
  `krebs.3` goes `unknown` @default → `violated via btormc` @`--timeout-ms 120000`.
  (`vis_arrays_buf_bug` is *already* decided by the default portfolio — its CEX @18–28 is
  within kmax 40 and btormc finds it in ~1 s — so P0.5's net new decide is the deep-CEX
  class beyond depth 40, e.g. `krebs.3`.) No new dependency, no build-time cost. It does
  *not* give the no-subprocess path — that is P1's job.
- **P1 — fast-SAT reachability oracle (in-process Boolector, MIT — feature-gated) — SHIPPED.**
  `crates/mununu-core/src/adapter/btor2/native_boolector.rs`: an in-process Boolector BMC
  member of `decide_reach_owned_only`, behind the `boolector` cargo feature. The crate's
  `vendor-lgl` feature C-builds Boolector + Lingeling + btor2tools from the crate's
  bundled/`curl`-fetched sources via cmake — **no system Boolector**, self-contained given a
  C toolchain + cmake. The encoder threads the btor2 `next` expressions forward into
  Boolector BV nodes (structural sharing, no per-transition equality asserts), unrolls
  incrementally with `bad` as a one-shot assumption per depth, and only ever contributes a
  sound `Violated` — never a safety claim; it abstains (⇒ portfolio reads no verdict) on
  `rol`/`ror`, array `read`/`write`, or any array sort.
  - **Measured (`mununu-dev` + libz3):** standalone, the in-process engine finds
    `vis_arrays_buf_bug`'s CEX (depth 18) in **~3 s** and `krebs.3`'s in **~56 s** (via the
    frame-simplification below; was ~205 s). Both are decided `violated` **uniquely by the
    boolector member** in the owned-only portfolio — *no other owned engine (exact / k-induction
    / interp) reaches `krebs.3`*. The frame-simplification + engine swap (below) roughly HALVED
    the owned budget `krebs.3` needs — `--owned-timeout-ms 120000` now suffices (was 240000).
    (NB: the owned-only *wall* time can exceed the boolector member's — it is bounded by the
    slowest concurrent member, e.g. the synchronous, non-interruptible exact BDD engine on wide
    datapaths — a pre-existing owned-path property independent of this member.)
  - **Frame-simplification (bound striding) — SHIPPED.** The BMC maintains a running
    `reached_k = ⋁ⱼ≤ₖ bad_j` monitor ("a `bad` is reachable *within* k steps") and, past a
    shallow exact threshold, checks it at a STRIDE: one `reached_k` UNSAT proves the WHOLE
    `0..k` range clean at once, skipping the intermediate deep per-frame UNSAT proofs that
    dominate the runtime. Measured on `krebs.3`: **~205 s → ~56 s (3.6×)** — now *competitive
    with* the btormc subprocess (~73 s), where the naive per-frame version was ~3× slower.
    `vis_arrays` is unchanged (~3 s). Scoping was measurement-driven: cone-of-influence
    pruning was measured NOT to help here (`bad`'s cone covers ~97 % of the logic) and
    disabling model generation during the search was a net loss (the witness re-solve of the
    deep deciding formula dominates). **Trade:** for a deep CEX found via striding the reported
    depth is a *sound bound* (the frame where `bad` holds in the returned witness), not
    guaranteed minimal — shallow CEX (≤ threshold) stay exact, and the portfolio verdict is
    depth-independent. Differential-checked against the exact BDD engine + native z3 BMC (13
    feature-gated tests). The very-deep `brp2.2`/`circular_pointer` (>300 s even for btormc)
    stay out of reach. License-clean (boolector + boolector-sys both MIT).
  - **Infra note (resolved).** `mununu-dev:latest` had gone **stale** — missing `libz3` despite
    the Dockerfile's `libz3-dev` line — so *any* z3-linked build failed to link there. Rebuilding
    from `docker/Dockerfile.dev` restores `libz3` (verified: a default z3-linked test links +
    passes) and, since the image carries cmake+curl+cc, builds the `boolector` feature directly
    (no derived image needed). The default `make ci` never compiles the feature (optional dep,
    absent from the default tree); validate it in `mununu-dev` with `--features boolector`, per
    the SVA `#[ignore]` precedent.
- **P2 — nonlinear datapath oracle (subprocess, AMulet2-style).** **Not motivated by any
  currently-failing design** (the §4 structure audit: `gen43` is pure-BV, `mul9`'s property
  is control) — so this is *parked* until a genuine multiplier-*correctness* branching
  target appears. If one does: a non-bundled subprocess oracle for `bvmul` invariant
  validation, wired like btormc (subprocess-only for license reasons — §5.2). Higher cost,
  narrowest payoff; do the hour-0 property-fit check on the target first.

## 7. Honest limits

- **Answering ≠ discovering.** The oracle makes each concrete query sound + faster; it does
  not, by itself, discover *which* datapath predicates to add. Datapath-*dependent* safety
  proofs (mul9-class) still need the interpolation/synthesis loop to converge — the §5
  residual that §8.2 measured not converging at 300 s.
- **The hybrid earns its keep on branching-time properties over nonlinear/wide datapaths**
  (recoverability over a multiplier), where neither a pure bit-vector model checker (cannot
  *state* `AG EF`) nor pure predicate abstraction (chokes on the datapath) works alone. For
  plain bit-level `AG ¬bad`, the may/must machinery is overhead and a P1 fast-SAT engine is
  simply the point — that is what btormc already is, now in-process and license-clean.
- **P1 is the recommendation; P2 is optional and subprocess-only.** The deep-CEX reach is a
  clean, permissive-library win, now **SHIPPED** with a frame-simplification (bound striding)
  that cut the in-process engine's `krebs.3` time **~205 s → ~56 s (3.6×)** — competitive with
  the btormc subprocess (~73 s). The boolector member uniquely decides both `vis_arrays` (~3 s)
  and `krebs.3` owned-standalone (now at `--owned-timeout-ms 120000`, halved from 240000 — the
  member also replaces the redundant z3-deep-CEX in the owned path). Its value is the
  *no-subprocess* owned path; when a subprocess is fine, P0.5's `--timeout-ms` (btormc) remains
  a fast route. The very-deep `brp2.2`/`circular_pointer` time out even for btormc/Boolector at
  300 s; the nonlinear class is a copyleft-constrained, discovery-limited follow-up worth doing
  only if the multiplier-datapath branching class becomes a concrete target.
