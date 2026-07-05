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

15 distinct OpenTitan modules, 16 ledger properties.

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
| 13 | prim_arbiter_ppc | AG EF (mask==0) | ⊥ | atom does not bind — `mask` is not a post-synthesis register name |
| 14 | prim_arbiter_tree | AG EF (prio_mask_q==0) | ⊥ | atom does not bind — `prio_mask_q` not a post-synthesis register name |
| 15 | prim_packer_fifo | AG EF (depth_o==0) | ⊥ | drainability cone hits an unsupported `Mul` op |
| 16 | prim_fifo_sync | AG EF (depth_o==0) | ⊥ | drainability cone hits an unsupported `Mul` op |

**Tally: True = 5, False = 7, ⊥ = 4.**

## R-F5.6 cone-of-influence — the ⊥ story, before and after

Pre-COI the exact engine bit-blasted EVERY register+input bit and hit its 40-bit cap on **9**
designs (all ⊥). R-F5.6 (`dep_graph::cone_leaf_nids` → `BddBitBlaster::build_with_keep`, which
pins out-of-cone leaves to constant 0) restricts the bit-blast to the property's cone. Result:
**every bit-cap ⊥ is gone — 5 designs flipped to definite** (prim_esc_receiver, rom_ctrl,
usbdev, aes_cipher, otbn), and the monotone ledger recorded each ⊥→definite as an allowed
improvement (no locked verdict flipped — the soundness guardrail held). A hermetic regression
(`rf5_6_coi_lifts_bit_cap_on_out_of_cone_datapath`) locks the mechanism: a 47-bit design whose
property cone is 2 bits now decides.

The **4 remaining ⊥ have nothing to do with the bit cap** — two distinct, actionable causes:

- **Atom does not bind (arbiter_ppc, arbiter_tree).** `mask` / `prio_mask_q` are not the
  post-flatten register names (the round-robin priority state is optimized/renamed by yosys).
  Path forward: inspect the synthesized netlist and re-point the atom (a corpus-refinement TODO;
  even bound, the arbiters' N-wide `req_i` keeps the cone wide — the next COI limit is BDD
  variable ordering).
- **Unsupported operator (packer_fifo, fifo_sync).** The drainability cone reaches a `Mul` the
  R-F5.3a BDD bit-blaster does not implement → clean `Skipped`. Path forward: add `Mul` (and the
  shift ops) to the bit-blaster's `eval_op`.

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

## Automated, CI-ready e2e

- `diff_corpus_monotone_verdict_ledger` — the **gate**: reruns every design, panics on any
  definite-verdict flip/regress. Add a design = one `CorpusDesign` literal (untouched source
  paths + a ledger). Docker-gated (`mununu-sva`; `#[ignore]`), fast (exact-only, ~4s).
- `diff_corpus_verdict_census` — the **diagnostic**: prints the T/F/⊥ table + per-⊥ cause;
  optional `MUNUNU_CENSUS_EXPLICIT=1` adds the explicit-engine fallback column.
- Not prone to hallucination: the oracle is the engine + an independent cross-check (explicit
  engine / btormc / — for counterexamples — Verilator), never a model's self-report. The
  monotone ledger makes drift a hard test failure, not a silent edit.
