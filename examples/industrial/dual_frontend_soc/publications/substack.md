# Two RTL frontends, one IR: what to unify and what to keep separate

> **Draft for Substack publication.** Source: `examples/industrial/dual_frontend_soc/`. The transcript embedded below reproduces byte-for-byte by running `./examples/industrial/dual_frontend_soc/validate.sh` against the pinned commit. **Do not publish until the four-gate validation checklist in `publications/README.md` passes.**

---

Mununu, the open-source compositional model checker for reactive systems, has two SystemVerilog frontends. One is a Rust-native pipeline that parses a tightly-scoped subset of SV and builds explicit-state Kripke structures via SMT. The other shells out to yosys, runs `flatten`, and bit-blasts the resulting BTOR2 netlist. Same input language, two completely different extraction techniques.

People ask why. The instinct is "one of them must be the wrong call." It isn't. They serve two different verification audiences, and the right move is to **unify their seams and leave their cores alone**.

This post walks the design — Document B in mununu's four-document roadmap — and exercises it against a small SoC example. The transcript reproduces byte-for-byte.

## Why two pipelines

The two paths started at different times for different reasons.

The **custom-SV pipeline** is the original one. It parses SV directly and produces symbolic Kripke structures: registers as enums, counters with explicit bounds, combinational logic evaluated via z3. It was built for protocol-level verification — handshake correctness, request-grant fairness, FIFO occupancy bounds. When the abstraction holds, the state space stays tractable even on real-world RTL.

The **yosys frontend** came later. It hands the SV to yosys's mature parser, runs the standard `prep → proc → flatten → async2sync → chformal -lower` script, captures the BTOR2 output, and bit-blasts. It's the path for *bit-exact* properties: overflow, sign-extension, post-synthesis correctness on arbitrary SV.

Two precision tiers, two audiences. The custom-SV path is the one for "I want to know my AXI arbiter is fair." The yosys path is the one for "I want to know my carry chain doesn't overflow in this corner case."

People who haven't thought about it want to collapse them. The argument for collapsing is "one pipeline is simpler than two." The argument against is more interesting.

## The CIRCT pattern

The canonical multi-frontend hardware compiler today is [CIRCT](https://circt.llvm.org/) — Circuit IR Compilers and Tools, built on MLIR. CIRCT supports multiple frontend dialects (FIRRTL, several SV dialects, Calyx) all lowering through a common middle (`hw`, `comb`, `seq`, `sv`). Different syntaxes, different abstraction tiers, but one IR everything funnels through and one set of passes that operate on the shared dialects.

The principle CIRCT made mainstream: **unify the seams, leave the cores free**. Each frontend can specialise — FIRRTL is parameterised hardware generation, the SV dialects are the SV-2017 surface, Calyx is software-style with a hardware semantics. They share the middle. The middle is where the leverage is.

Mununu's two pipelines should land in the same shape: one IR (`AdapterIR`), one composition primitive, one controllability rule, one set of property roles. The internal extraction techniques — bit-blasting vs SMT-backed Kripke — stay distinct because they serve different precision tiers.

## What can be unified

Concretely:

- **Output IR.** Both pipelines produce `AdapterIR`. Already aligned.
- **CTXDSL emission.** Shared via `crates/mununu-core/src/adapter/emit.rs`. Already aligned.
- **Property roles.** The IR has `PropertyRole::{Assumption, Guarantee, Invariant, Standalone}`. The custom-SV path uses them; the yosys path mostly produces `Standalone` today. Should be reconciled — `chformal -lower` knows the difference between `assert` and `assume`, the BTOR2 reader can keep the distinction.
- **Controllability rule.** Document A §4 says: classify labels by the port direction at the current scope's boundary. The shared classifier lives at `crates/mununu-core/src/controllability.rs`. The custom-SV path uses it; the yosys path defaults to "all inputs uncontrollable" because flatten throws away the port directions. **The directions are in the SV AST yosys parsed.** They're just being discarded at the BTOR2 emission boundary. Fix.
- **Black-box submodule handling.** When either pipeline encounters a `(* blackbox *)` module, it should emit `<module>.interface.json` + `<module>.gap_report.json` sidecars next to the CTXDSL output — the same JSON shape mununu's contract subsystem already consumes. This is the stage-1 integration between extraction and contracts.
- **Composition spec.** Both pipelines should emit the same `CompositionSpec` for the same shared design.

## What should not be unified

The list of things that *must* stay separate:

- **Abstraction tier.** Symbolic enums vs gate-level bit-exactness. They are different verification questions. Force one and you lose half the audience.
- **Parser.** Yosys has a mature SV-2017 frontend. Replicating it in Rust would burn engineering years for a tiny gain in homogeneity. Don't.
- **Clock semantics.** Yosys's `async2sync` is a synthesis-grade abstraction; custom SV preserves clock-domain detail. Different verification problems.
- **Bit-width.** Bounded counters in custom SV; native arbitrary width in BTOR2.
- **SV language coverage.** Yosys handles classes, interfaces, generates, packages. Custom SV is the opinionated path for SV-written-the-mununu-way.
- **External tool dependency.** Custom SV is pure Rust; yosys is a subprocess. Custom SV is the only path if you can't ship yosys (WASM builds, restricted CI).

The point of this list is to prevent the "we should collapse this" reflex. Each row has a real trade-off.

## The stage-1 integration

Until this week, mununu's contract subsystem and its extraction pipelines lived in parallel. The contract subsystem could validate discharge graphs and discover phase-1 contracts from `BlackBoxInterface` JSON. But the JSON was hand-authored — even though mununu's extractors had *just parsed the source* and knew exactly what ports each black-box module had.

That's a friction the discipline can't survive long-term. The whole point of the contract subsystem is to make black-box handling first-class, not to ask users to re-describe modules mununu already understands.

The fix is staged in three steps:

1. **Stage 1 (now, in M2):** when an adapter detects a black-box module, it auto-emits the `BlackBoxInterface.json` + `GapMarkerReport.json` sidecars alongside its CTXDSL output. The library helper is `contract::discover::build_blackbox_sidecars()`; the CLI exposes it as `mununu contract sidecars`.
2. **Stage 2 (M3, Document D):** source-comment annotations (`@mununu_guarantee` on the wrapper module) populate the discovered contract with vendor-supplied A/G clauses. The relocated task A6.
3. **Stage 3 (post-M3):** CTXDSL grammar extension for inline `contract { ... }` blocks. The discharge check becomes a precondition of every `mununu context eval / synth`. Contracts stop being a separate command and become part of the standard verify flow.

This commit lands the library + CLI layer of stage 1. The yosys-side auto-emission (the producer that calls `build_blackbox_sidecars` during extraction) is the explicit deferred follow-up. The hand-authored interface JSON + `mununu contract sidecars` stand in for the auto-emission today — the JSON shape is identical, only the producer is different.

## Walking the example

The repo has [`examples/industrial/dual_frontend_soc/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/dual_frontend_soc). Run it:

```bash
./examples/industrial/dual_frontend_soc/validate.sh
```

The example models a tiny SoC fragment: a host controller, a UART peripheral, and a closed-IP DDR3 PHY. The host and UART are open and verifiable; the DDR3 PHY is a 2-state chaotic stub with uncontrollable outputs, exactly the chaotic-stub default from [Document A](https://github.com/vscorza/mununu/blob/main/docs/design/black-box-modules.md) §2.

Step 1 calls `mununu contract sidecars` against a hand-authored `blackbox_interfaces.json`. The CLI produces two JSON files — `DDR3_PHY_V2.interface.json` and `DDR3_PHY_V2.gap_report.json` — in the output directory. Same JSON shape an auto-emitting adapter will produce once yosys integration lands.

Step 4 runs `mununu contract discover` against the auto-emitted interface. The discovery pipeline classifies each port (clock and reset as `Uncontrollable`, the three output signals as `Uncontrollable` from the host's perspective per the §4 rule), and emits one `OutputSequencing` gap covering the outputs:

```
WARN contract gap detected — chaotic stub default in effect
     module="DDR3_PHY_V2" kind=output_sequencing
     labels="rdata, ddr_ready, ddr_busy"
     soundness="safety verdicts hold; liveness verdicts depending on
                these labels are unsound (no progress assumption)."
phase-1 discovery: 8 label(s), 1 gap marker(s) for module `DDR3_PHY_V2`
```

Step 5 runs `mununu contract gaps --strict-contracts` against the auto-emitted gap report. Strict mode exits non-zero because the gap is unmet. In CI for a safety-critical product, this is the gate that prevents shipping a verification under chaotic-stub semantics without an authored contract.

Steps 6-8 verify three mu-calculus properties over the composed SoC. All hold:

```
soc_well_formed         →  SAT (1/1 reachable composed states)
burst_path_reachable    →  SAT (host can reach the DDR burst path)
uart_send_reachable     →  SAT (host can reach the UART send path)
```

Note the trade-off the chaotic stub forces. The safety properties hold *under chaotic DDR*. That's the substantive claim — the host controller is well-formed regardless of how the DDR PHY behaves internally. A liveness property like "every DDR burst eventually completes" would *not* hold under chaotic crypto and would need a vendor-supplied latency contract.

## What this example does not claim

Per [mununu's claims-integrity rules](https://github.com/vscorza/mununu/blob/main/CLAUDE.md):

- No claim mununu found a bug in any commercial DDR3 PHY or any specific SoC.
- The example uses `mununu contract sidecars` as a stand-in for adapter auto-emission. The JSON shape is identical to what the yosys-side integration will produce, but the producer is different until that integration lands.
- No vendor `@mununu_guarantee` source-comment annotations yet — those land in M3 (Document D, relocated task A6).
- The proof is conditional on the chaotic-stub contract; a vendor-supplied latency-bound contract would tighten it.

## Where this fits

The architectural reference is CIRCT and MLIR — multi-frontend hardware infrastructure built on the "unify the seams, leave the cores free" principle. The compositional foundations come from Alur & Henzinger's *Reactive Modules* (FMSD 1999) and de Alfaro & Henzinger's *Interface Automata* (ESEC/FSE 2001). The yosys primitives this work builds on (`hierarchy`, `(* blackbox *)`, `cutpoint -blackbox`) are mainstream, used in OpenTitan's formal flows and across SymbiYosys.

What mununu adds is the integration: a contract subsystem that connects to extraction at the adapter boundary, so the user never has to hand-author what the extractor already knows.

## What's next

This is M2 of a four-document roadmap.

- **Document D** — contract corpus + unified `.mununu/` config + the relocated A6/A7 (source-comment annotations + HITL UX). M3.
- **Document C** — HW/SW codesign extraction. The capstone post — a peripheral RTL + firmware C combined, with a register-map sidecar coupling the two sides for cross-boundary formal verification. M4.

The repo: [github.com/vscorza/mununu](https://github.com/vscorza/mununu).
The example: [`examples/industrial/dual_frontend_soc/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/dual_frontend_soc).
The design doc: [`docs/design/rtl-frontend-unification.md`](https://github.com/vscorza/mununu/blob/main/docs/design/rtl-frontend-unification.md).

— Mariano Cerrutti
