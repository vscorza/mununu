# Differential-oracle corpus — verdict census

> Analysis + measurement. Generated from the P2 corpus in
> [`crates/mununu-core/tests/differential_oracle_e2e.rs`](../crates/mununu-core/tests/differential_oracle_e2e.rs)
> (the `diff_corpus_verdict_census` / `diff_corpus_monotone_verdict_ledger` tests). Run
> 2026-07-05 in the `mununu-sva` image, **with R-F5.6 cone-of-influence enabled**. Every design
> is BYTE-EXACT upstream OpenTitan (Apache-2.0) at commit `558921ccc8aeefab652b45c1402c10bceb63accc`;
> only the `prim_assert.sv` synthesis-macro shim is local. mununu adds ONE `@mununu_guarantee`
> liveness annotation per design (a recoverability property `AG EF <reset/idle state>` the
> design's own SVA fragment cannot express); the vendored source + its in-design SVA are never edited.

## What this validates

The corpus is the P2 slice of the differential-oracle e2e suite: **no single-engine definite
verdict is trusted on its own.** Each design's liveness verdict is calibrated by a monotone
ledger — a recorded definite (True/False) must be preserved on every rerun (a flip = a
soundness/precision bug that panics the strict test); a `⊥` may only move UP to a definite
verdict as the engine/abstraction improves. Verdicts are additionally cross-checked against an
independent engine (the explicit predicate-cube CEGAR) and, for reachability atoms, btormc (P1).

## Verdict census (exact-symbolic engine + R-F5.6 COI, reset-gated)

18 distinct designs (17 OpenTitan + the lowRISC ibex core), 19 ledger properties.

| # | Module | Liveness property | Verdict | Notes |
|---|---|---|---|---|
| 1 | uart_tx | AG AF (bit_cnt==0) | **False** | a stalled tx holds the counter ≠ 0 forever on some path |
| 2 | uart_tx | AG EF (bit_cnt==0) | **True** | always drains back to idle |
| 3 | csrng_main_sm | AG EF MainSmIdle(55) | **False** | terminal MainSmError trap (unsupported cmd / escalate) |
| 4 | edn_main_sm | AG EF Idle(193) | **False** | terminal Error + RejectCsrngEntropy traps |
| 5 | aes_ctr_fsm | AG EF CTR_IDLE(14) | **False** | terminal CTR_ERROR trap |
| 6 | prim_count | AG EF (cnt_o==0) | **True** | always clearable via clr_i |
| 7 | prim_esc_sender | AG EF Idle(0) | **True** | escalation-sender returns to Idle |
| 8 | prim_esc_receiver | AG EF Idle(0) | **True** *(COI)* | recovers to Idle (60-bit design, cone-pruned) |
| 9 | rom_ctrl_fsm | AG EF Done(518) | **False** *(COI)* | glitch-reachable Invalid trap (834-bit design, cone-pruned) |
| 10 | usbdev_linkstate | AG EF LinkDisconnected(0) | **True** *(COI)* | link always disconnectable (timers pruned) |
| 11 | aes_cipher_control_fsm | AG EF CIPHER_CTRL_IDLE(9) | **False** *(COI)* | cipher error trap (aes_reg_pkg datapath pruned) |
| 12 | otbn_start_stop_control | AG EF Halt(1) | **False** *(COI)* | terminal Locked trap (lc_ctrl/otp closure pruned) |
| 13 | prim_arbiter_ppc | AG EF (gnt_o==0) | **True** *(comb-atom)* | always returns to no-grant (`mask` register optimized away → binds the combinational `gnt_o`) |
| 14 | prim_arbiter_tree | AG EF (gnt_o==0) | **True** *(comb-atom)* | always returns to no-grant (combinational output) |
| 15 | prim_packer_fifo | AG EF (depth_o==0) | **True** *(Mul/shift)* | always drainable — decided once the bit-blaster gained Mul + shifts |
| 16 | prim_fifo_sync | AG EF (depth_o==0) | **True** *(comb-atom)* | always drainable (`depth_o` is combinational → binds via named-signal support) |
| 17 | prim_alert_sender | AG EF Idle(0) | **False** *(blackbox)* | alert-path sibling of esc_sender, but VIOLATED — the blackboxed diff_decode / sec_anchor / sigint environment can trap it out of Idle |
| 18 | ibex_controller | AG EF DECODE(5) | **True** *(exact)* | the lowRISC ibex core's main FSM — always returns to executing (no permanent non-DECODE trap). First CPU-scale, non-prim design; exposed + fixed the A.4 sampling-may unsoundness (below) |
| 19 | keymgr_ctrl | AG EF StCtrlReset(865) | **False** *(exact)* | OpenTitan key-manager sparse FSM — does NOT return to reset (terminal StCtrlDisabled/Invalid traps, the SEC_CM pattern). Deepest closure (10 packages); decidable only after the > 128-bit binary-constant bit-blast fix (256-bit key-state) |

**Tally: True = 10, False = 9, ⊥ = 0. Every design decides.**

## Are the False verdicts findings? No — expected violations (claims-integrity)

**None of the 8 `False` verdicts is a bug finding.** Each was checked against the design's own
source; every one is an *expected* violation, and for most the design's *own* SVA asserts the
behaviour mununu reports. This corpus is a **calibration + methodology testbed, not a findings
list**, and must be read as such.

**Category A — intentional terminal error/lock states (6).** `AG EF idle` is VIOLATED because the
FSM has a DELIBERATELY terminal error/escalate/lockdown state that only reset escapes — correct
SEC_CM sparse-FSM hardening, only reachable via a *fault* (escalation, glitch, unsupported
command). The modules assert this themselves:

| Design | Trap | Design's own evidence it is terminal |
|---|---|---|
| csrng_main_sm | MainSmError | `CsrngMainErrorStStable_A: state_q == MainSmError \|=> $stable(state_q)` |
| edn_main_sm | Error | comment "don't move out of Error as it's terminal" + `ErrorStStable_A: … \|=> $stable` |
| rom_ctrl_fsm | Invalid | comment "Invalid: Terminal and invalid state (only reachable by a glitch)" |
| aes_ctr_fsm | CTR_ERROR | `aes_ctr_ns = CTR_ERROR` self-loop + `alert_o=1`; gated behind `alert_o` by `AesCtrStateValid` |
| aes_cipher_control_fsm | alert/error state | cipher-core alert trap (same class) |
| otbn_start_stop_control | Locked | `OtbnStartStopStateLocked` self-loop — secure lockdown on fault/RMA |

The key reframing: **a `HOLDS` here would be the red flag, not the `VIOLATED`.** `HOLDS` would mean
the error state is escapable WITHOUT reset — a hardening bug contradicting the module's own
`$stable` FPV assertion. So these VIOLATED verdicts **cross-confirm the design intent** via a
completely independent (branching-time model-checking) route.

**Category B — blackbox-environment trap (prim_alert_sender).** Unlike Category A's intentional
terminal states, prim_alert_sender's `AG EF Idle` is VIOLATED as an artifact of the **abstraction
posture**: its `prim_diff_decode` / `prim_sec_anchor_*` submodules are blackboxed (chaotic-stub),
so the ack / ping / sigint signals they drive become a FREE adversarial environment that can hold
the sender out of Idle forever (a never-acking or perpetually-signalling environment). This is the
honest, sound reading of the *abstracted* model — an in-model expected violation, not a concrete-RTL
finding (the real submodules constrain that environment). It contrasts with its sibling
prim_esc_sender, whose `AG EF Idle` HOLDS: a useful calibration data point on how the blackbox
posture (not the FSM itself) shifts a recoverability verdict.

**RTL grounding (P3, claims-integrity Rule 9).** csrng's VIOLATED verdict is anchored to a
concrete RTL execution: Verilator (`hw-verif:latest`, native SV packages — no sv2v/yosys) on the
byte-exact upstream `csrng_main_sm` shows the trap reached and held — reset→MainSmIdle(55),
`local_escalate_i`→MainSmError(41) in one clock with `main_sm_err_o=1`, then 40 cycles never
returning to Idle after deassert. So `AG EF MainSmIdle = VIOLATED` is not a model-only claim:
`.claude/reviews/prospector/staging/RTL-003-csrng-main-sm/repro/sim-csrng.log`. **The exact engine
now emits a concrete counterexample for the `AG EF` recoverability shape too**
(`exact_reachable_trap_path` — a reset→trap path where the trap is the absorbing `¬EF p` region),
so all 7 `Violated` verdicts carry a replayable trace (the 6 `AG EF` traps + uart's `AG AF` stall
lasso). RTL replay is directed per design (drive the trap-inducing input, confirm the trap is
reached and held): csrng ✅ and edn are grounded; the remaining traps are the same mechanical
pattern.

**Category B — fairness-dependent must-liveness (1).** `uart_tx AG AF (bit_cnt==0)` is an `AF`
(all-paths) property: with a free environment the baud tick can stall forever, holding a
transmission incomplete on some path — the classic *liveness-needs-a-fairness-assumption* case,
expected under an adversarial environment. Its sibling `AG EF` (*can* complete) correctly HOLDS.

**What would a real finding look like?** One of these coming back `HOLDS` unexpectedly (error state
not terminal), or a NON-security FSM (uart, prim_count) failing recoverability with no error trap.
Neither happened. The value of the corpus is (a) mununu decides a branching-time recoverability
property the design's SVA cannot express (SVA asserts `$stable` *locally*; it cannot state "from
EVERY reachable state, idle is reachable"); (b) each verdict independently confirms the hardening;
(c) the definite verdicts are soundness tripwires — a future engine change that flipped one to
`HOLDS` would fire the monotone gate immediately.

## R-F5.6 cone-of-influence — the ⊥ story, before and after

Pre-COI the exact engine bit-blasted EVERY register+input bit and hit its 40-bit cap on **9**
designs (all ⊥). R-F5.6 (`dep_graph::cone_leaf_nids` → `BddBitBlaster::build_with_keep`, which
pins out-of-cone leaves to constant 0) restricts the bit-blast to the property's cone. Result:
**every bit-cap ⊥ is gone — 5 designs flipped to definite** (prim_esc_receiver, rom_ctrl,
usbdev, aes_cipher, otbn), and the monotone ledger recorded each ⊥→definite as an allowed
improvement (no locked verdict flipped — the soundness guardrail held). A hermetic regression
(`rf5_6_coi_lifts_bit_cap_on_out_of_cone_datapath`) locks the mechanism: a 47-bit design whose
property cone is 2 bits now decides.

Two follow-ups closed the remaining ⊥:

1. **Bit-blaster op completeness** — `Mul` (shift-and-add), variable shifts `Sll`/`Srl`/`Sra`
   (barrel shifter), signed comparisons `Slt`/`Sgt`/`Sgte`/`Slte`, each with a hermetic test. This
   decided prim_packer_fifo (**True**, always drainable).
2. **Named-combinational-signal binding** — a predicate may now bind to a combinational module
   output / wire (`depth_o`, `gnt_o`), not only a state register, using the BDD `walk_design`
   already computes; the atom's cone still seeds via the output's terminal fan-in. This decided
   the last three: the arbiters (`mask` is optimized away by yosys, but the combinational `gnt_o`
   binds — the arbiter always returns to no-grant, and gnt_o's cone is small) and fifo_sync
   (`depth_o` is combinational; its cone is just the pointers, so the wide data storage is pruned).

**Result: every design in the corpus now decides — ⊥ = 0.** The cone-of-influence restriction plus
op-completeness plus combinational-atom binding took the corpus from True=3/False=4/⊥=9 to
True=9/False=7/⊥=0, with the monotone gate green at every step (no locked verdict ever flipped).

## Spurious-results audit

**Two classes of near-spurious were caught and corrected; no residual spurious verdicts.**

- **Reset-triviality (systematic).** The `prim_assert` shim expands the design's SVA (incl.
  `disable iff (!rst_ni)`) to empty, so mununu's automatic reset-gating saw no reset signal and
  left `rst_ni` a FREE input — a free reset trivially "recovers" any FSM, so recoverability
  verdicts came back a false True. Pinning `rst_ni` inactive (`config: rst_ni=1`) restored
  meaningful verdicts (edn/aes_ctr/csrng → correctly VIOLATED). prim_esc_receiver re-exposed
  this when COI first made it decidable (it had no `rst_ni` pin yet) — a live demonstration the
  guard is still doing its job.
- **Vacuous atom (otbn).** A placeholder `state_q == 0` (0 is not a valid sparse state) gave a
  vacuously-False verdict once COI made it decidable; re-pointed to `Halt = 1` (the real idle
  state) for a meaningful VIOLATED (the Locked trap).
- **No definite verdict contradicts an independent oracle** after these fixes: the strict
  monotone gate preserves all 8 locked definites under COI (the COI soundness guardrail), the
  exact engine's reachability facts are self-consistent (verified on edn via EF-Error/EF(AG-Error)
  probes), and P1 cross-checks exact `EF` against btormc.

## Cross-engine soundness + the default-engine decision (2026-07-06)

The corpus verdicts above are the **exact** engine's (`--engine exact-symbolic`). The remaining
question for the roadmap was whether to make one of the two *cube* engines the `verify-auto`
default: `Cegar` (predicate-abstraction SMT all-pairs, today's default) or `SymbolicCube`
(predicate-cube BDD CEGAR, `--engine symbolic`). `diff_corpus_cegar_vs_symbolic_engine_parity`
answers it by running all 16 corpus properties through BOTH cube engines and cross-checking each
against the exact oracle (the `d.ledger`, all-definite). Full-corpus result:

| Anchor | Result |
|---|---|
| **Cube definite contradicts the exact oracle** | **0 / 16** — every cube definite matches exact |
| **Cegar vs SymbolicCube opposite definites** | **0** — the two cube engines never contradict |
| Properties decided (of 16): **exact** | **16** (the oracle) |
| … **SymbolicCube** | **5** — uart_tx AG-AF/AG-EF, prim_count, usbdev_linkstate, otbn_start_stop |
| … **Cegar** | **2** — aes_ctr_fsm, aes_cipher_control_fsm |
| Both cube engines ⊥ (exact still decides) | 9 |

**Soundness: proven on the corpus.** Zero oracle contradictions and zero cross-cube flips across
16 properties × 2 cube engines — every definite a cube engine emits is correct, and the cubes
never disagree with each other.

**The two cube engines are COMPLEMENTARY, not dominated.** SymbolicCube decides 5 (all liveness
that Cegar leaves ⊥); Cegar decides 2 (that SymbolicCube leaves ⊥ — its predicate-cube WP loop
saturates first). The two winner-sets are **disjoint**. So a blind default-SWAP to `symbolic`
would trade 2 regressions for 5 gains — that is *not* "verdict parity," which the gate required.

**Decision: exact-first PORTFOLIO, not a swap.** The exact engine (COI-pruned) decides all 16;
where even COI leaves the cone over the 40-bit cap (prim_esc_receiver 47 b, prim_fifo_sync 95 b —
SymbolicCube `Skipped` on both), the exact engine still decides them here, and the two cube engines
are a sound complementary fallback for designs beyond the exact cap. The differential proves the
portfolio safe: pick the definite verdict from whichever engine produces one (exact preferred),
because no two engines ever contradict. Wiring the portfolio as the `verify-auto` default is the
follow-up; the closeout precisions (BDD variable ordering; COI on the cube engine, which would lift
the 2 `Skipped`) raise each engine's individual hit-rate but are no longer *gates* — the portfolio
is sound today.

## A.4 honest-⊥ — the sampling-may unsoundness ibex_controller exposed (2026-07-06)

Adding the first CPU-scale design (`ibex_controller`) made the parity gate flag a real
**cross-engine soundness disagreement**: the exact engine said `AG EF DECODE` = **HOLDS** while the
default cube engine (`Cegar`) said **VIOLATED** — a definite contradiction. Root cause (confirmed
in code): the default may-relation is `MayEdgeInference::Off`, a **sampling** inference that
enumerates at most 8 boolean inputs per source cube. ibex has **more than 8** boolean inputs, so the
sampling was **incomplete** — it MISSED the real edge back to DECODE (`real ⊄ may`), so the cube
engine concluded "DECODE unreachable" → a **spurious VIOLATED**. The exact engine (full bit-blast,
no sampling) has the real edge → the sound HOLDS. This is a general hazard: an under-approximate may
makes a definite `KleeneT` on `[]φ` and a definite `KleeneF` on `<>`/`EF` both unsound.

**Fix — the A.4 honest-⊥ guard** ([`verify_auto`](../crates/mununu-core/src/adapter/slang/verify_auto.rs), `Formula::has_modality`): a
cube DEFINITE on a modal, pure-state (sampling-may) property is **downgraded to ⊥** when the design
has more boolean inputs than the sampling cap (incomplete enumeration). The engine now honestly
returns ⊥ instead of a possibly-wrong definite; the exact engine (or `--may-edge-inference
smt-all-pairs`) decides it soundly. Effect on the corpus: `ibex_controller` cube → ⊥ (exact True),
and exactly one prior design's under-sampled Cegar definite → ⊥ (honest; the exact oracle still
decides it). **Parity gate: 0 oracle-violations, 0 soundness-flips** — the disagreement is resolved
soundly, not papered over. This is the differential discipline finding + fixing a real soundness
bug on the first CPU-scale design.

## Automated, CI-ready e2e

- `diff_corpus_monotone_verdict_ledger` — the **gate**: reruns every design, panics on any
  definite-verdict flip/regress. Add a design = one `CorpusDesign` literal (untouched source
  paths + a ledger). Docker-gated (`mununu-sva`; `#[ignore]`), fast (exact-only, ~4s).
- `diff_corpus_cegar_vs_symbolic_engine_parity` — the **cross-engine soundness gate**: hard-fails
  on any cube definite that contradicts the exact oracle, or any Cegar↔SymbolicCube opposite
  definite; reports the per-engine precision (5 vs 2 above). `MUNUNU_PARITY_ONLY=<names>` subsets
  it. Docker-gated (`mununu-sva`; `#[ignore]`), full corpus ~42s.
- `diff_corpus_verdict_census` — the **diagnostic**: prints the T/F/⊥ table + per-⊥ cause;
  optional `MUNUNU_CENSUS_EXPLICIT=1` adds the explicit-engine fallback column.
- Not prone to hallucination: the oracle is the engine + an independent cross-check (explicit
  engine / btormc / — for counterexamples — Verilator), never a model's self-report. The
  monotone ledger makes drift a hard test failure, not a silent edit.
