# Differential-oracle corpus — verdict census

> Analysis + measurement. Generated from the P2 corpus in
> [`crates/mununu-core/tests/differential_oracle_e2e.rs`](../crates/mununu-core/tests/differential_oracle_e2e.rs)
> (the `diff_corpus_verdict_census` / `diff_corpus_monotone_verdict_ledger` tests). Run
> 2026-07-05 in the `mununu-sva` image. Every design is BYTE-EXACT upstream OpenTitan
> (Apache-2.0) at commit `558921ccc8aeefab652b45c1402c10bceb63accc`; only the `prim_assert.sv`
> synthesis-macro shim is local. mununu adds ONE `@mununu_guarantee` liveness annotation per
> design (a recoverability property `AG EF <reset/idle state>` the design's own SVA fragment
> cannot express); the vendored source + its in-design SVA are never edited.

## What this validates

The corpus is the P2 slice of the differential-oracle e2e suite: **no single-engine definite
verdict is trusted on its own.** Each design's liveness verdict is calibrated by a monotone
ledger — a recorded definite (True/False) must be preserved on every rerun (a flip = a
soundness/precision bug that panics the strict test); a `⊥` may only move UP to a definite
verdict as the engine/abstraction improves. Verdicts are additionally cross-checked against an
independent engine (the explicit predicate-cube CEGAR) and, for reachability atoms, btormc (P1).

## Verdict census (exact-symbolic engine, reset-gated)

15 distinct OpenTitan modules, 16 ledger properties.

| # | Module | Liveness property | Exact verdict | Notes |
|---|---|---|---|---|
| 1 | uart_tx | AG AF (bit_cnt==0) | **False** | a stalled tx holds the counter ≠ 0 forever on some path |
| 2 | uart_tx | AG EF (bit_cnt==0) | **True** | always drains back to idle |
| 3 | csrng_main_sm | AG EF MainSmIdle(55) | **False** | terminal MainSmError trap (unsupported cmd / escalate) |
| 4 | edn_main_sm | AG EF Idle(193) | **False** | terminal Error + RejectCsrngEntropy traps |
| 5 | aes_ctr_fsm | AG EF CTR_IDLE(14) | **False** | terminal CTR_ERROR trap |
| 6 | prim_count | AG EF (cnt_o==0) | **True** | always clearable via clr_i |
| 7 | prim_esc_sender | AG EF Idle(0) | **True** | escalation-sender returns to Idle |
| 8 | prim_esc_receiver | AG EF Idle(0) | ⊥ | bit-cap (60 > 40) |
| 9 | rom_ctrl_fsm | AG EF Done(518) | ⊥ | bit-cap (834) — ROM datapath in the cone |
| 10 | prim_arbiter_ppc | AG EF (mask==0) | ⊥ | bit-cap (294) + blackboxed prim_leading_one_ppc |
| 11 | prim_arbiter_tree | AG EF (prio_mask_q==0) | ⊥ | bit-cap (275) |
| 12 | prim_packer_fifo | AG EF (depth_o==0) | ⊥ | bit-cap (75) |
| 13 | usbdev_linkstate | AG EF LinkDisconnected(0) | ⊥ | bit-cap (48) |
| 14 | aes_cipher_control_fsm | AG EF CIPHER_CTRL_IDLE(9) | ⊥ | bit-cap (45); **explicit ⇒ False** |
| 15 | otbn_start_stop_control | AG EF (state==0) | ⊥ | bit-cap (41) |
| 16 | prim_fifo_sync | AG EF (depth_o==0) | ⊥ | bit-cap |

**Tally (exact): True = 3, False = 4, ⊥ = 9.**

## Why the ⊥s — and the path forward

**Every ⊥ has the same root cause: the exact-symbolic engine's 40-bit blast cap**
(`MAX_BITBLAST_BITS = 40` in `symbolic_bitblast.rs`) — it bit-blasts a BDD over EVERY
register+input bit, with no cone-of-influence restriction yet. This is the open roadmap slice
**R-F5.6**. Two mitigations, one demonstrated:

- **Explicit predicate-cube CEGAR (`--engine explicit`)** — no hard bit cap; abstraction-based.
  It **rescues aes_cipher_control_fsm (⊥ → False)**, but returns Unknown on the harder liveness
  cases (esc_receiver, rom_ctrl, packer, usbdev) where auto-seeded predicates are too weak.
  (Run the census with `MUNUNU_CENSUS_EXPLICIT=1` for the full fallback comparison.)
- **Cone-of-influence pruning (R-F5.6)** — only bit-blast the property's cone. The machinery
  exists (`adapter/partition/coi.rs` + `adapter/btor2/dep_graph.rs`); wiring it into the
  bit-blaster shrinks designs whose big datapath is OUTSIDE the property cone (candidates:
  otbn 41, aes_cipher 45, usbdev 48). It helps LESS where the cone is inherently wide (the
  arbiters' N-wide `req_i` feeds `mask`; rom_ctrl's FSM reads the counter). **The corpus is the
  ready-made trigger + validation harness for R-F5.6** — rerun the census, and the monotone
  ledger records each ⊥→definite as an allowed improvement.
- **arbiter_ppc** additionally needs its `prim_leading_one_ppc` submodule vendored (else it is a
  chaotic blackbox — orthogonal to the bit cap).

## Spurious-results audit

**One systematic near-spurious was caught and corrected; no residual spurious verdicts.**

- **Reset-triviality (caught + fixed).** The `prim_assert` shim expands the design's SVA
  (including `disable iff (!rst_ni)`) to empty, so mununu's automatic reset-gating — which keys
  off that SVA — saw no reset signal and left `rst_ni` a FREE input. A free reset trivially
  "recovers" any FSM to its reset state, so **every recoverability property came back a false
  True**. Pinning `rst_ni` to its inactive value (`config: rst_ni=1`) restored meaningful
  verdicts: edn/aes_ctr/csrng correctly flip to VIOLATED. This is exactly the class of
  false-positive the differential suite exists to surface.
- **No definite verdict contradicts an independent oracle** after reset-gating: the exact
  engine's reachability facts are self-consistent (verified on edn via EF Error / EF(AG Error)
  probes: Error is reachable AND terminal ⇒ VIOLATED, as the engine reports), and P1 cross-checks
  exact `EF` against btormc.
- **Not spurious, but scoped:** the ⊥ verdicts are honest "don't knows" (bit-cap), never a
  misleading definite. The two-valued exact engine emits no ⊥ of its own — a ⊥ here is a
  `Skipped`, always with a reason.

## Automated, CI-ready e2e

The census IS the automated module-validation the request asked for:

- `diff_corpus_monotone_verdict_ledger` — the **gate**: reruns every design, panics on any
  definite-verdict flip/regress. Add a design = one `CorpusDesign` literal (untouched source
  paths + a ledger). Docker-gated (`mununu-sva`; `#[ignore]`), fast (exact-only).
- `diff_corpus_verdict_census` — the **diagnostic**: prints the T/F/⊥ table + per-⊥ cause;
  optional `MUNUNU_CENSUS_EXPLICIT=1` adds the explicit-engine fallback column.
- Not prone to hallucination: the oracle is the engine + an independent cross-check (explicit
  engine / btormc / — for counterexamples — Verilator), never a model's self-report of a
  planted answer. The monotone ledger makes drift a hard test failure, not a silent edit.
