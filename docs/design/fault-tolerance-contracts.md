# Fault-tolerance contracts for hardware functional units

> **Status:** planning
> **Audience:** reviewers thinking about whether mununu's contract framework
> should host explicit fault-model assumptions, and what realistic scope a
> first example should cover.
> **Companion documents:** [A — Black-box modules in compositional extraction](black-box-modules.md), and the worked example at [`examples/hw/dmr_commit_4bit/`](../../examples/hw/dmr_commit_4bit/).

## 1. The question

In hardware verification, a recurring failure pattern is:

> Given the possibility of a bit-flip in a functional unit (FU), the accelerator
> must not commit a result corrupted by that flip.

The standard mitigation is *duplicate the compute, compare before commit*
(DMR — dual modular redundancy; or TMR — triple modular redundancy with
voter). Industry validation of these mitigations is almost exclusively
simulator-based (UVM constrained-random + fault-injection campaigns).
Public formal-method coverage of fault-tolerance proofs exists in academic
work (Braibant–Chlipala ITP 2013 for TMR voter correctness; Krstic et al.
TCAD 2008 for fault-tolerant FSM synthesis) but is not standard practice.

Two recent results from production silicon make the gap concrete:

- Dixit et al., *Silent Data Corruptions at Scale*, arXiv:2102.11245 (Meta, 2021).
- Hochschild et al., *Cores that don't count*, HotOS 2021 (Google).

Both report that real SDCs in datacenter CPUs are *input-pattern-dependent*
and *computation-correlated* — they hit both DMR replicas identically,
defeating the i.i.d. single-fault model behind classical DMR/TMR proofs. The
proof is sound for the stated assumption; the assumption is wrong for the
real world.

This is the same shape as the assume/guarantee discharge problem
[Document A](black-box-modules.md) was already solving for protocol
contracts. The contribution of this document is to argue that fault-model
contracts belong in the same framework, and to scope a first runnable
example.

## 2. Where the contract sits

The framework in [`crates/mununu-core/src/contract/mod.rs`](../../crates/mununu-core/src/contract/mod.rs)
already carries everything needed:

- A *user* (system integrator, safety engineer, certification authority)
  authors an `Assumption` clause declaring the fault model they expect.
- A *designer* (RTL author, IP vendor) authors `Guarantee` and `Invariant`
  clauses describing the mitigation structure.
- A `DischargeEdge` graph captures which guarantee discharges which
  assumption. The SCC check at [`contract/discharge.rs`](../../crates/mununu-core/src/contract/discharge.rs)
  rejects circular reasoning and unmet assumptions.

The gap [Document A §3.x](black-box-modules.md) names — *circular A/G is
unsound without an inductive condition* — is exactly the gap the Meta and
Google papers point at, viewed from the contract-discharge side: if the
"DMR works" guarantee secretly depends on the "faults are independent"
assumption, and that assumption depends on the same vendor's "replicas are
independent" guarantee, the cycle ships an unsound proof.

> Source of truth: [`crates/mununu-core/src/contract/discharge.rs`](../../crates/mununu-core/src/contract/discharge.rs) — surface: CLI (`mununu contract validate`)

## 3. What is tractable in mununu's SV pipeline today

The SystemVerilog adapter at [`crates/mununu-core/src/adapter/systemverilog/kripke.rs`](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs)
imposes hard limits that shape what can serve as a runnable example:

| Limit | Location | Consequence |
|---|---|---|
| 2^18 state cap | [kripke.rs:207](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs#L207) | A 32-bit datapath does not fit. |
| Auto-abstract ≤ 4 bits | [kripke.rs:918](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs#L918) | Wider widths need sidecar domains or are silently ignored. |
| Concat truncates to LSB | [kripke.rs:1551](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs#L1551) | Fault masks must use shift, not concat. |
| Default-false signal init | [kripke.rs:1405](../../crates/mununu-core/src/adapter/systemverilog/kripke.rs#L1405) | Every register must be explicitly initialised. |

Within these limits, **tractable** sub-properties for a fault-tolerance
example include:

- A 4-bit DMR comparator + commit gate with environment-controlled fault
  injection.
- A 3-of-N TMR voter FSM with a 1-of-N fault-location input.
- Multi-module composition of comparator + commit handshake.

**Not tractable** today:

- 32-bit ALU / FMA datapath (state explosion).
- Floating-point semantics.
- Multi-bit upsets (MBU) — encoding burst location alone is exponential.
- Liveness under chaotic vendor IP latency.

## 4. The worked example

The runnable example lives at [`examples/hw/dmr_commit_4bit/`](../../examples/hw/dmr_commit_4bit/).
It is a 4-bit DMR commit gate with:

- Two identity replicas of a 4-bit FU (`y_a = x`, `y_b = x`).
- A one-hot adversarial XOR on `y_a` only, gated by an environment input.
- A comparator producing `equal = (y_a_post == y_b)`.
- A registered commit stage that asserts `commit_valid` / `broadcast_valid`
  only when `equal` was true the previous cycle.
- An observer register `corrupted_broadcast` that latches if a commit ever
  broadcasts a value disagreeing with the pipelined golden reference
  `y_b_q`. The observer is the safety property expressed in observable form.

The contract set has one `A_env` assumption (single-bit flip on `y_a` per
cycle), three guarantees (`G_compare`, `G_commit`, `G_top`), and a linear
discharge chain `A_env ← G_compare ← G_commit ← G_top`. Tarjan SCC returns
`Acyclic`.

The mu-calculus property is `nu X. (!corrupted_broadcast_T && [] X)`.

The example's README is explicit about what is **not** verified — the
datapath, common-mode failures, MBU, persistent faults, faults in the
comparator or commit register, and liveness.

> Source of truth: [`examples/hw/dmr_commit_4bit/dmr_top.sv`](../../examples/hw/dmr_commit_4bit/dmr_top.sv) — surface: CLI

## 5. Why not extend `GapKind` yet

[`crates/mununu-core/src/contract/gap.rs`](../../crates/mununu-core/src/contract/gap.rs)
has a `GapKind::Other` slot. A future refinement could introduce
`GapKind::FaultModel` carrying named soundness implications for the three
boundaries fault assumptions usually drop on the floor — *multiplicity*
(single vs multi-bit), *location* (which signals are in-scope), *temporal
shape* (transient vs persistent).

The worked example does not require this variant; `GapKind::Other` with a
free-form description suffices for now. Adding the variant should be paired
with a CLAUDE.md note in §"Adapter / Emitter Capability Use" naming the
fault-injection pattern (env-controlled one-hot XOR mask, explicit register
init, no concat) so future contributors do not reinvent the SV-adapter
traps. Both additions are deferred until a second use case (TMR voter or
ECC scrubber) makes the variant pay for itself.

## 6. Honest scope statement

The proposed standard of formal contracts between fault-anticipating users
and mitigation-providing architects fills a real gap in published practice
(A/G contract frameworks — Benveniste et al. INRIA RR-8147; OCRA ASE 2013
— handle protocols and timing well but have no off-the-shelf instantiation
with explicit fault-model assumptions).

Within mununu, the verifiable scope is **narrow** — the commit gate that
the contract reduces to — and the datapath remains an act of trust. That is
the same boundary every published formal proof of DMR/TMR sits behind, and
stating it explicitly through an A/G contract is the proposal's actual
contribution. The worked example at [`examples/hw/dmr_commit_4bit/`](../../examples/hw/dmr_commit_4bit/)
is a faithful demonstration of the pattern; it is not a claim about any
real GPU.

## 7. What comes next

This document is planning-grade until:

1. The example transcript at [`examples/hw/dmr_commit_4bit/`](../../examples/hw/dmr_commit_4bit/)
   is reproduced under the pinned `mununu-dev` container with verdicts
   captured as evidence (the SAT case for the intact gate, the UNSAT case
   for the negative control).
2. A second use case (TMR voter, or an ECC scrubber for a small register
   file) is added to confirm the contract pattern generalises beyond DMR.
3. If both succeed, the `GapKind::FaultModel` variant and the CLAUDE.md
   adapter-pattern note become worth adding.

Until then this document sits under `docs/design/`, the example carries
the LTS-witness-only disclaimer in its README, and no public claim is made
about real silicon.
