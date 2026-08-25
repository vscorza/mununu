# P1 prototype — three-shot array-νμ recoverability (exploratory harness)

> Plan: `.claude/plans/three-axis-array-numu-recoverability.md` §5 (harness) + §6 (RTL set).
> This is the EXPLORATORY scaffold (scratchpad). Migrates to `mununu-private/experiments/array-numu/`
> once it stabilizes. All runs in the `mununu-sva-pono` image (`-w /work`, cargo on `/usr/local/cargo/bin`,
> yosys/slang/sv2v on `/opt/oss-cad-suite/bin`, pono at `/build/pono/build/pono`).

## Progress log

- **[2026-07-29] P1.step0 — shot-① substrate CONFIRMED WORKING.** pono ships `--ceg-prophecy-arrays` (Mann et al.
  TACAS'21 CEG-Prophecy) at `/build/pono/build/pono`. Smoke test (`smoke.btor2`, a read-after-write safety property,
  UNSAT): `pono --ceg-prophecy-arrays -e ic3ia` → **`unsat`** (proves it); plain `-e bmc` → `unknown`. So prophecy
  is strictly more capable on arrays than BMC, and the array axis has a real reference implementation to prototype
  against.

## The experiment ladder (measure-first order)

- **step0 — substrate works** ✅ (above).
- **step1 — does prophecy SCALE to a real 26-array design?** Lift RS_dec → array-bearing BTOR2 (`sv verify` /
  yosys `write_btor`, keep `$mem`), inject a safety sub-question (`bad = Valid_out==1` reachability; and a
  read-after-write invariant over an in-cone array), run pono `--ceg-prophecy-arrays` vs mununu's safety portfolio.
  DECISION: if pono-prophecy chokes on 26 arrays (timeout/memout), the array axis does NOT scale to RS_dec → the
  three-shot approach is blocked on the real anchor (record as a scale wall; retreat to synthetic + fragment).
- **step2 — the composition seam.** IF step1 scales: does prophecy produce an array-free system mununu's KMTS can
  consume for the BRANCHING νμ (not just pono's safety verdict)? Two integration shapes to prototype:
    (①a-blackbox) pono-prophecy decides the safety sub-questions; compose into νμ via the may/must edges — NON-trivial
       (a reachability verdict is not a must-edge); OR
    (①b-owned) extract prophecy vars / array-content atoms and feed as mununu `--predicate` seeds → the cube gets an
       array-content predicate → the νμ can split. This is the owned path the paper would ship.
- **step3 — the ablation matrix** (plan §4): `class × (①a|①b) × ③ranking-on/off × must-∀∃-fix-on/off → verdict`,
  over the §6 set (RS_dec/sdspi/ecg + synthetics), cross-checked by the small-array exact oracle.

- **[2026-07-29] path-1 REGISTERIZATION lever — REFUTED for RS_dec (clear limitation).** Hypothesis: registerize
  small in-cone `$mem` → array-free → exact decides (registerization is EXACT, sound). Measured: yosys
  `memory_nordff; memory` fully registerizes RS_dec → array-free BTOR2 but **41,699 lines / 1232 states / ~21k
  state bits** (widths incl. 3478, 1184, 292×11, 4×1096). `btor2 verify-recoverability Valid_out==0/==1` → **⊥/⊥**.
  RS_dec's in-cone arrays are LARGE (188-byte data buffer + syndrome banks), so registerization converts the
  array-SKIP into a **width/register-dominated wall** — no help. The 183-line array-free probe earlier was a
  safety-reachability cone (over-pruned), NOT the recoverability cone. ⇒ **registerization is a bounded lever for
  the `small-in-cone-array` sub-class only; RS_dec is `large-in-cone-array` → needs P1 (prophecy/three-shot).**
  **Wall taxonomy refined:** `array-content-dependent νμ recoverability` splits into `small-in-cone-array`
  (→ registerization, bounded, sound; corpus applicability UNVALIDATED — no confirmed small-array wall design yet)
  vs `large-in-cone-array` (→ P1 research). Per the user directive, **P1 is now the go-to for RS_dec.**
  **NEXT (P1.step1, resumed):** get RS_dec's FAITHFUL array-BTOR2 via mununu's own `build_script` lift (hand-yosys
  can't — it registerizes/`memory_nordff`-blocks, the P1.1 lesson), then run the pono-prophecy scale test.

- **[2026-07-29] P1-a TEST BED ESTABLISHED — clean single-array-wall, oracle-backed.** `array_gates_recovery.sv`:
  recovery of `busy` gated on array content (`mem[key]==all-ones`), no register-dominated datapath, parameterized
  size. Measured (engine-tagged):
    · **Oracle** (small AW=2/DW=2, yosys-registerized → array-free 6 states) → **`exact-symbolic` ROBDD = HOLDS**
      (busy==0) + **HOLDS** (busy==1) → property TRUE + non-vacuous (ground truth).
    · **Baseline small** (mununu keeps `$mem`) → **`symbolic` cube KMTS (Z3 QF_AUFBV) = ⊥** (the isolated wall).
    · **Scale** (large AW=8/DW=8, registerized → 258 states / ~2057 bits) → **`exact-symbolic` ROBDD = ⊥**
      (register-dominated; registerization does NOT scale).
    · **Baseline large** (mununu) → **⊥**.
  ⇒ a controlled case where the ONLY hard axis is the in-cone array, the true verdict is known (HOLDS), the cube
  ⊥s, and registerization dies on the large instance — so the symbolic-cube + array-content composition is the
  sole path for the large size. This is the clean bed to prove shot ① on (unlike RS_dec, which is multiply-hard).
  **NEXT (P1-a composition — the real work):** two shot-① options, engine-tagged, to make the cube decide the
  large instance (matching the oracle HOLDS):
    · **①b owned array-content predicate** — add a `Select{arr,idx,val}` leaf to `PredicateExpr`
      (`predicate_expr.rs:57`), teach CEGAR (`cegar.rs`/`refine.rs`) to propose `mem[key]==all-ones`, and the
      `symbolic` cube KMTS (encoder already does `select`/`store` on the must-side, `btor2_encode.rs`) evaluates
      it. Bounded code change; the owned/ship path. **← preferred.**
    · **①a pono-prophecy** — `pono --ceg-prophecy-arrays -e ic3ia` eliminates the array (safety); compose the
      array-free result into the νμ via `symbolic` cube may/must — the black-box-oracle path (integration is the
      hard part: a safety verdict is not a must-edge).

- **[2026-07-29] P1-a shot-①b Select-leaf FOUNDATION landed + compiling (`cargo check -p mununu-core` green).**
  `predicate_expr.rs`: new `Select { array, index, op, value }` variant — an array-content atom
  `array[index] <op> value`; SMT-only (`has_select()` gates it to `SmtAllPairs`, like `CmpReg`/`CmpRegAddend`);
  `eval` is a never-reached stub; `build_constraint_arr(bv_lookup, arr_lookup)` realises it as
  `select(arr, idx) <op> value` over Z3's **exact** array theory (mirrors the encoder's `Op::Read`,
  `btor2_encode.rs:762`); `build_constraint` delegates with an empty array lookup. Parser extended
  (`[`/`]` tokens + a `parse_atom` branch) so `mem[key]==255` parses to `Select`. Match ripple fixed at 4 sites
  (`predicate_bdd` → ERROR: ROBDD can't bit-blast a Select, it's SMT-only; `resolve_predicate_expr_registers` +
  `collect_predicate_registers` → resolve/keep both array + index; the recoverability display fn).
  **Engines:** the seed rides the `symbolic` predicate-cube KMTS (Z3 QF_AUFBV, `SmtHyperMust`); `exact-symbolic`
  ROBDD abstains on a Select (correct — arrays aren't bit-blastable) and stays the differential oracle.
  **NEXT (the wiring, then validation):**
    1. `smt_must_edge.rs` — at the 3 `build_constraint` sites (304-313 `build_pred_constraint`, 415 uniform, 1173)
       add an `arr_lookup: |name| view.state_{curr,next}_arr.get(nid_map[name])` and call `build_constraint_arr`,
       so a seeded Select actually drives the cube's may/must edges (confirm `nid_map` resolves array-cell names;
       else build a name→array-nid map from `view.signals`).
    2. gate a Select-bearing seed SMT-only in the lift/seed path (via `has_select()`, mirroring `has_addend()`).
    3. VALIDATE: `--predicate p_unlock:mem[key]=255` on the large `array_gates_recovery` → `symbolic` cube
       ⊥ → HOLDS, cross-checked vs the ROBDD oracle (HOLDS). If a seeded Select still ⊥s, that is a deeper
       finding to report before building CEGAR auto-discovery.

- **[2026-07-29] P1-a shot-①b — mechanism BUILT + compiled; validation blocked on INDEX RESOLUTION (diagnosed).**
  Wiring done: `smt_must_edge` 3 sites → `build_constraint_arr` (array lookup from `view.state_{curr,next}_arr`
  via `nid_map`); `select_guard_atoms` discovers `Eq(Read(mem,idx),K)` → `Select` compound seed into the
  recoverability `compound_seeds`. Build green, binary built. **Result: `array_gates_recovery` still ⊥ (both
  sizes), oracle=HOLDS.** Root cause (BTOR2-verified, NOT a mechanism failure): the lifted read INDEX is a
  **combinational reset-mux over an UNNAMED state** — `24 read 5 23 16`, `16 ite 5 3 15 14` = `rst_n ? key : 0`;
  the `key` state (`15 state 5`) is unnamed, the symbol `u.key` sits on `17 uext … u.key` (combinational). So
  `select_guard_atoms` (requires a symboled index) **skips it → the Select is never seeded** → cube stays ⊥.
  The cube-WITH-a-Select is thus still UNTESTED. **NEXT (index resolution — the real remaining piece):**
    · make discovery robust to a combinational/reset-mux index: either (a) resolve the read-index node to a
      value-equal NAMED node (e.g. the `uext` output `u.key`) and seed `mem[u.key]==K`, OR (b) carry the index by
      NID and resolve it via the view's per-node BV cache (`signal_bvs`) — the more robust design; AND
    · ensure the cube's predicate lookup resolves it: `build_pred_constraint` must use a combinational-aware
      `nid_map` (`build_register_nid_map_with_inputs`, which adds inputs+combinational) so `u.key`/the index node
      binds — the states-only `build_register_nid_map` won't.
    Then re-run: expect `symbolic` cube ⊥ → HOLDS on both sizes, oracle-cross-checked. This finally tests whether
    a Select predicate lets the cube decide the content-gated recovery (the actual P1-a question).
  **Engines unchanged:** `symbolic` predicate-cube KMTS (Z3 QF_AUFBV, `SmtHyperMust`) is the decider; `exact-symbolic`
  ROBDD abstains on Select + is the small-instance oracle.

- **[2026-07-29] P1-a shot-①b — MECHANISM COMPLETE; the array-MUST wall is DEFINITIVE (the core P1-a result).**
  Drove the Select seed end-to-end on the clean hand-authored fixture (`agr_clean.btor2`: named state FFs
  + direct `mem[key]` read, so the reset-cube gate is well-defined and the ONLY hard axis is the array). Found
  and fixed FOUR issues, each confirmed by instrumented docker runs:
    1. **Discovery see-through** (`select_guard_atoms`): the read index is a reset-mux `ite(rst,key,0)` over an
       unnamed FF, and `collect_symbols` attaches the visible name to the STATE (not the ite/uext). Fixed by
       walking the index cone to the first symboled STATE. ✅ unit-test `p1a_..._sees_through_resetmux_uext` PASSES.
    2. **Reset-cube free-array** (`free_select_bits`): a Select's reset truth depends on array CONTENT (not in the
       BV-only `init_values`) → treat the array as FREE at reset, enumerate the Select bit, trust only a unanimous
       Holds (sound over-approx of the initial set).
    3. **`array_name_nid` resolution (REAL BUG FIX)**: array cells are deliberately absent from `view.signals`
       (not BV cube dims), so the BV `nid_map` couldn't resolve the array name → `arr_lookup` returned `None` →
       the Select constraint was `None` → the may-check conservatively returned `May` for EVERY pair. Added an
       array-name→nid map to the view. **Measured: may_edges 256 (complete/degenerate) → 12 (the Select now
       genuinely constrains the abstraction).**
    4. **`universal_bound_arrays` must-check (REAL BUG FIX)**: the ∀∃ must-edge check universally bound only BV
       inputs + BV next-states, NOT the next-state ARRAY → the array's next-state was free under the ∀, letting
       the solver trivially break the transition → **spurious `NotMust` for every array source**. Added the
       next-state arrays to the ∀ bound set (sound: it can only turn spurious NotMust into a Z3-proven Must).
  **THE WALL (result):** with (4) fixed, the must verdict goes `NotMust → Unknown`. The ∀∃ must-query now
  correctly universally-quantifies the next-state array — which puts it in **quantified array logic (AUFBV + ∀
  over an array), undecidable for Z3 → `Unknown` → 0 must-edges → the νμ abstains (`Unknown`)**. The MAY side
  (`∃`, QF_AUFBV) is decidable and works (256→12). So:
    · **An array-content cube predicate is NECESSARY but NOT SUFFICIENT.** It fixes the may-relation but the
      recoverability diamond `<>X` needs a MUST edge, and the must-edge ∀∃ over a universally-bound array is
      beyond the decidable fragment. This is a PRECISE, mechanized validation of the three-axis plan's premise:
      **the array axis needs a dedicated abstraction (prophecy à la Mann TACAS'21 / array-aware IC3-CEG) that
      ELIMINATES the ∀-quantified array from the must-query — not just a predicate in the cube.** (Small arrays
      remain decidable via registerization; the wall is intrinsic to arrays too large to registerize.)
  **Engines:** `symbolic` predicate-cube KMTS (Z3 QF_AUFBV may / **AUFBV+∀ must ← the undecidable point**,
  `SmtHyperMust`); `exact-symbolic` ROBDD abstains (in-cone array) + registerized-oracle differential.
  Regression test `p1a_array_gated_recovery_hits_must_edge_quantifier_wall` locks in the honest `Unknown`; a
  future array engine that decides it flips this to `Holds`. Bugs (3)+(4) are independently mergeable (they make
  array-content predicates correctly participate in the cube's may-side and make the must-check sound for arrays).

- **[2026-07-29] SPCR P-A1 BUILT + the wall test FLIPPED ⊥→Holds (the owned decider works).**
  `crates/mununu-core/src/adapter/btor2/array_prophecy.rs` (`spcr`): owned BTOR2→BTOR2 pre-pass that
  registerizes the property-ACCESSED array cells (prophecy reg `pv=mem[key]` + exact frame
  `pv'=ite(waddr==key',wdata,pv)`) and DROPS the array → array-free → the must-query is pure QF_BV
  (decidable). Wired into `verify_recoverability_scalable` (replaces the parsed file with the array-free
  version after parse, retries exact). **`p1a_spcr_decides_array_gated_recovery_holds` = ⊥→Holds** — the
  array-gated recovery the raw cube left `Unknown` (the mechanized AUFBV+∀-array wall) now DECIDES via the
  array-free exact ROBDD. Ladder-2 SCALING proven: `spcr_scales_array_size_independent_one_pv_...` — an
  8-bit/256-cell array yields **1** prophecy register (O(#accessed-cells)) and exact decides, where
  whole-array registerization's 2048 bits would SKIP. 0 regressions (full lib suite: 2352 pass; the 3
  fails are the pre-existing env trio — refine cvc5-spelling, sv2v-present, interp cvc5-flake — confirmed
  by name on my tree; recoverability module 41/41). **P-A1 scope = single unconditional full-width write;
  P-A1b (write-ENABLE mux `ite(we,write,mem)` + RMW cell) is the gate to the real SV lifts** (which yosys-
  lift `array_gates_recovery.sv` to a `we`-mux/RMW, not a bare write → P-A1 soundly abstains today).
  Fixtures: `agr_spcr.btor2` (embedded `AGR_SPCR`/`AGR_SPCR_LARGE` in the module tests). Plan §7 updated.

- **[2026-07-29] SPCR P-A1b/c/d + P-A3 measured — SPCR decides real async-reset lifts; RS_dec is the P-B frontier.**
  Shipped (branch `feat/array-content-select-predicate`): P-A1b (write-enable mux + soundness gate), P-A1c
  (`fold_and_dce` collapses the yosys write-mask RMW), **P-A1d (RECURSIVE index-expression prophecy —
  `Cell`/`Elim`: one pv per index leaf, `read_at` reconstruction, `mem_next_at` recursive frame)** which
  decides the REAL async-reset lift `agr_small_mem.btor2` (`holds`, differential-oracle-confirmed vs the
  registerized ROBDD). 10 array_prophecy tests, 0 regressions across all steps.
  **P-A3 (full SV e2e + RS_dec):**
    · `sv verify-recoverability array_gates_recovery.sv --top array_gates_recovery_{small,large}` → both
      `holds` end-to-end (SV source → slang/yosys keep-$mem lift → SPCR → decide), incl. the 256-cell scaling.
    · SPCR now logs `SPCR: eliminated arrays=N prophecy_registers=M` (attribution).
    · **RS_dec HONEST NEGATIVE:** `AG EF(state==1)` = `holds` but the `SPCR:` log does NOT fire → SPCR
      ABSTAINED; the holds is the FSM ranking (state recovery is counter-driven, DP_RAM data out-of-cone).
      RS_dec's DP_RAM = dual-port streaming RAM (conditional write + moving address) = the **P-B residual**.
      ⇒ Phase-A does NOT close RS_dec's array-content recovery. Sources staged in `rsdec_src/`.
  ⇒ SPCR mechanism proven on real async-reset lifts + large-array scaling; RS_dec-class streaming RAM = P-B.

## Files
- `smoke.btor2` — the substrate smoke test (read-after-write, UNSAT under prophecy).
- (next) `lift_rsdec.sh`, `oracle_pono.sh`, `compose_proto.py`, `matrix.py`, `oracle_small.py`.

## Honesty gate (plan §6.4)
step1 is the make-or-break: if prophecy does not scale to a real array core, the primary contribution becomes the
**decidable-fragment characterization** on synthetics, not a real-core decide. That is a valid CAV result; do not
force a real-core claim that isn't there.
