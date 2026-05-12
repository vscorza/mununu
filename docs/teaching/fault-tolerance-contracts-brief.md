# Formal Fault-Tolerance Contracts for GPU Functional Units

*A technical brief for a CS graduate student — problem, model, properties, verification path, mununu architecture, and an annotated reading list.*

Author audience: a CS / EE grad student with a first course in model checking and SystemVerilog literacy. Reading time: 60–90 minutes plus the bibliography.

---

## 1. The problem

A GPU compute pipeline executes many parallel functional units (FUs) — integer ALUs, FMA pipelines, tensor cores. Each FU operates on registers and forwarding paths that are physically realized as transistors. Two phenomena routinely flip bits inside that compute:

- **Single-event upsets (SEUs)** caused by ionizing radiation, especially at advanced nodes and at altitude / orbit. A single neutron or alpha particle deposits enough charge to invert a storage node.
- **Silent data corruption (SDC)** from aging transistors, voltage droop, marginal timing, and pattern-dependent transistor weakness. Unlike SEUs, SDC is not random across the die — it is *computation-correlated*: certain input patterns trigger it deterministically.

The architectural mitigation common across high-reliability silicon (automotive ASIL-D, aerospace, datacenter) is **redundant compute followed by compare-before-commit**:

- *Dual modular redundancy (DMR):* two replicas compute the same function; a comparator gates the commit signal on equality.
- *Triple modular redundancy (TMR):* three replicas plus a voter; outvotes a faulty replica without aborting.

The framing of the user's proposal (rendered from Spanish):

> Given the possibility of a bit-flip in a functional unit, the accelerator must not commit (or broadcast) a result corrupted by that flip. The standard mitigation is to duplicate the compute and compare before commit — but this is validated almost exclusively in simulators. Could we instead build a **standard of formal contracts** between users (who anticipate these faults) and architects (who mitigate them)?

This is a real gap. Industry validation today is dominated by constrained-random simulation and fault-injection campaigns; formal proofs of DMR/TMR correctness exist in academic settings (Coq proofs of majority voters, etc.) but are not the verification artifact a customer receives from a vendor. There is no published instantiation of assume/guarantee contract theory whose *assumption alphabet* names a fault model explicitly.

## 2. Why this is hard in practice — the SDC failure mode

Two recent industrial reports from production silicon shape the constraint:

- **Dixit et al., "Silent Data Corruptions at Scale," arXiv:2102.11245 (Meta, 2021).** ALU-level SDC in Meta's datacenter is parts-per-thousand cores, input-pattern-dependent, and stable per core. A specific multiply / shift sequence reliably produces the wrong output on the same defective unit.
- **Hochschild et al., "Cores that don't count," HotOS 2021 (Google).** Same conclusion from a different fleet. The independent-faults assumption that classical DMR/TMR proofs rest on is empirically wrong.

The consequence: when two replicas execute the *same* program on the *same* input through the *same* type of defective transistor, they fail *identically*. The comparator agrees. The commit fires. The corrupted result broadcasts. The proof "DMR catches single bit-flips" is sound for its stated assumption — but the assumption never matched reality at the input-dependent fault level.

This is the failure mode an A/G contract must surface honestly: the **assumption clause** must precisely state what fault model the proof is conditional on, and the discharge graph must catch when that assumption has no real-world guarantor (e.g., when both replicas are claimed independent but share a clock and a power rail).

## 3. The contract framing — assume/guarantee around a fault model

Following Pnueli (1985) and Abadi & Lamport (1995), a component contract has the shape `(A, G)` meaning *the module guarantees G under environment assumption A*. The standard composition rule states `G_top` is provable from `⋀_m (A_m → G_m) ∧ (top-level env assumptions)`, with the side condition that every `A_m` consumed by a sibling must be guaranteed by either another module's `G_k` or by the top-level environment. McMillan (1999) addresses the soundness pitfall of circular `A_m ↔ G_k` cycles with step-indexed induction.

The proposal reframed: codify the fault hypothesis as a first-class **assumption clause** authored by the *user* (system integrator, safety engineer, certification authority), and the mitigation as **guarantee / invariant clauses** authored by the *architect* (RTL author, IP vendor).

A worked instance, for the bit-flip-in-FU problem:

- `A_env` (Assumption, owner = environment) — **"At most one bit of replica A's output flips per cycle; replica B is untouched."**
- `G_compare` (Guarantee, owner = comparator) — **"`equal = (y_a_post == y_b)`; structurally derived from the SV body."**
- `G_commit` (Guarantee, owner = commit stage) — **"`commit_valid` only asserts in a cycle where `equal` was true the previous cycle."**
- `G_top` (Invariant, owner = top) — **"`commit_valid → broadcast_data == y_b_q` (the fault-free reference)."**

The **discharge graph** is a directed graph: edges go from guarantor (`G_k`) to consumer (`A_m`). Run Tarjan SCC. Three outcomes:

- *Acyclic:* the standard non-circular Pnueli rule applies — safe to verify under topological order.
- *Circular with mu-rank witness:* the cycle admits a lightweight McMillan-style discharge if the alternation depths of fixpoints strictly decrease around the cycle. Auto-accepted with provenance `mu-rank`.
- *Circular without witness:* surface to a human reviewer; never silently accept.

For the DMR example above, the graph is the linear chain `G_top → G_commit → G_compare → A_env` and Tarjan returns acyclic. The contract structure carries proof content separate from the formal SV verdict.

## 4. Mununu's architecture, relevant to this proposal

Mununu is a model checker for the modal mu-calculus over **Compositional Labeled Transition Systems (CLTS)**. Three layers, each is a Rust crate:

```
external format → adapter (per-format) → CLTS + mu-calculus → verdict
```

### 4.1 The CLTS data model — `mununu-core/src/clts/`

A CLTS is a finite labeled transition system enriched with:

- *Per-state structured valuations* (`state_valuation`) — a key-value map per state, exposed to mu-calculus formulas as predicates of the form `<variable>_<value>` (e.g., `commit_valid_r_T`, `broadcast_data_r_15`).
- *Per-label controllability* — `Controllable | Internal | Uncontrollable`. Drives the synthesis problem: who chooses the label, the controller or the environment?
- *Multi-label transitions* — one edge can carry several semantically-related labels.

States are indexed by typed integers (`DefaultStateIdx`); transitions are stored compactly with `SmallVec<[LabelId; 4]>` per edge to amortize the common case.

### 4.2 The mu-calculus — `mununu-core/src/mu_calculus/`

The propositional modal mu-calculus, with explicit fixpoints `mu X. φ` (least) and `nu X. φ` (greatest), modalities `[a] φ` (after every `a`-step, `φ` holds) and `<a> φ` (some `a`-step leads to `φ`), and guarded fixpoint syntax. CTL and LTL safety patterns translate naturally; full LTL requires a Büchi product.

Evaluation is by **iterative fixpoint approximation** over `BitVec` state sets. The fixpoint engine returns iteration-rank signatures used downstream by strategy extraction (positional determinacy of parity games — Zielonka 1998).

Three synthesis modes:
- *Projection:* keep all transitions between winning states (not a strategy, the winning region).
- *Functional:* one controllable transition per state, the most "progressive" by rank signature.
- *Permissive:* all controllable transitions that do not regress in rank — Ramadge–Wonham canonical supervisor.

### 4.3 The SystemVerilog adapter — `mununu-core/src/adapter/systemverilog/`

Hand-written parser for a subset of SystemVerilog (`module`, `always_ff`, `always_comb`, `case`, `if-else`, `typedef enum`, `assign`). The Kripke builder enumerates reachable valuations of internal `logic` registers, applies sound abstraction to wider widths (≤ 4 bits is auto-bounded; wider widths need a sidecar `.mununu.json` domain), and emits a CLTS where each state's valuation captures every register value at that point.

Hard limit: **2^18 reachable states** (a model checker built on bitset evaluation must keep the state space tractable). The DMR example stays under 2^11 = 2048 states comfortably.

Soundness gaps to remember:
- Bitwise ops (`|`, `&`, `^`) operate on `i64` without source-width masking — caller responsibility.
- Output-port `logic` is treated as combinational; internal `logic` is treated as state. Mixing them confuses what counts as a register.
- `eval_expr → None` falls back to `Bool(false)` — explicit reset for every register is mandatory.

### 4.4 The contract framework — `mununu-core/src/contract/`

A `ContractClause` carries `{id, kind: Assumption | Guarantee | Invariant, owner, provenance, mu_rank?}`. A `ContractSet` bundles clauses, discharge edges, and top-level environment assumptions. The `discharge::validate` function builds the guarantor→consumer graph, runs iterative Tarjan SCC, and returns one of four verdicts:

- `Acyclic { topological, unmet_environment }`
- `CircularWithRankWitness { cycles, acyclic_remainder }`
- `Circular { cycles, acyclic_remainder }`
- `Unmet { missing_dischargers, partial }`

Gap markers (`gap::GapMarker`) name the kind of unknown left in a partially-discovered contract (`OutputSequencing`, `LatencyBound`, `InputAssumption`, `StatePredicate`, `Fairness`, `Other`) with structured soundness notes. A `--strict-contracts` CLI flag promotes any gap into a hard error for safety-critical CI.

The CLI surface is `mununu contract {validate, gaps, discover}`. Validation runs on a JSON contract set and is independent of any model — it catches dishonest contract structure (e.g., a fault assumption with no real guarantor) before verification even starts.

## 5. Worked example — 4-bit DMR commit gate

A runnable instance of the contract pattern lives at [`examples/hw/dmr_commit_4bit/`](https://github.com/vscorza/mununu/tree/main/examples/hw/dmr_commit_4bit). The fault model is encoded structurally as a one-hot XOR mask on replica A:

```systemverilog
assign y_a_post = flip_en ? (y_a ^ (4'b0001 << flip_idx)) : y_a;
```

Two replicas (`y_a = x`, `y_b = x`, both 4-bit), the comparator `equal = (y_a_post == y_b)`, a registered commit stage, and an observer register `corrupted_broadcast_r` that latches if a commit ever broadcasts a value disagreeing with the pipelined golden reference `y_b_q`. The observer expresses the safety property in observable form — translation of the contract into a finite-state predicate, not a "bug detector."

The mu-calculus safety property:

```
nu X. (!corrupted_broadcast_r_T && [] X)
```

reads as "every reachable state has `corrupted_broadcast_r` low, and so does every successor."

### Reproducing the verdicts (commands run against `mununu` on main)

```bash
# Contract discharge (independent of any model)
mununu contract validate examples/hw/dmr_commit_4bit/contracts.json
# → discharge: acyclic
#   topological order: G_top, G_commit, G_compare, A_env

# Safety on the intact design
mununu context eval examples/hw/dmr_commit_4bit/dmr_top.sv \
  --adapter systemverilog --formula no_corrupt_broadcast --automaton dmr_top
# → States satisfying: 81/81 (initial 1/1)

# Negative control — same property on a broken variant
# where commit fires unconditionally
mununu context eval examples/hw/dmr_commit_4bit/dmr_top_broken.sv \
  --adapter systemverilog --formula no_corrupt_broadcast --automaton dmr_top_broken
# → States satisfying: 0/161 (initial 0/1)
```

The negative control is essential: it demonstrates the formal verdict is meaningful. Without it, the SAT verdict on the intact design carries less proof content — a formula that holds vacuously is not a finding.

### What this example does NOT prove

1. **Datapath correctness is unverified.** Replicas are `y = x`. A bug in both replicas — the Meta/Google 2021 failure mode — passes the comparator silently.
2. **Single fault per cycle only.** MBU and persistent faults excluded by encoding.
3. **Comparator, commit register, and observer are in the TCB.** A flip inside them bypasses the proof.
4. **No liveness.** "Every valid result eventually broadcasts" needs a fairness assumption on `flip_en` that the chaotic fault model does not admit.

These caveats are not boilerplate. They are exactly the boundaries every published formal DMR/TMR proof sits behind, and stating them explicitly in the contract README is the *actual* contribution of the proposal.

## 6. Annotated reading list

### 6.1 Model checking and the mu-calculus

1. **E. Allen Emerson & Joseph Halpern, "Decision Procedures and Expressiveness in the Temporal Logic of Branching Time," STOC 1982 / JCSS 1985.** Origin of CTL, comparison with linear time. The branching-time intuition the modal mu-calculus generalizes.
2. **Dexter Kozen, "Results on the Propositional µ-Calculus," ICALP 1982 / TCS 1983.** The mu-calculus itself: syntax, semantics, expressive power. Read this before anything else on fixpoint logic.
3. **Edmund Clarke, E. Allen Emerson, A. Prasad Sistla, "Automatic Verification of Finite-State Concurrent Systems Using Temporal Logic Specifications," POPL 1983 / ACM TOPLAS 1986.** Founding paper of model checking (Turing Award 2007). The algorithmic basis for CTL model checking.
4. **Colin Stirling, *Modal and Temporal Properties of Processes*, Springer 2001.** Textbook. The cleanest pedagogical treatment of mu-calculus, parity games, and the modal-fragment hierarchy.
5. **Wieslaw Zielonka, "Infinite Games on Finitely Coloured Graphs with Applications to Automata on Infinite Trees," TCS 1998.** Positional determinacy of parity games. The reason mu-calculus winning strategies are finite-memory.
6. **Marta Kwiatkowska, Gethin Norman, David Parker, *Stochastic Model Checking* tutorial chapters (e.g., SFM 2007).** For when you graduate to probabilistic fault models.

### 6.2 Compositional / assume-guarantee verification

7. **Jayadev Misra & K. Mani Chandy, "Proofs of Networks of Processes," IEEE TSE 1981.** Earliest compositional proof system for concurrent processes.
8. **Cliff Jones, "Specification and Design of (Parallel) Programs," IFIP 1983.** Rely/guarantee for shared-state concurrency. The conceptual sibling of A/G.
9. **Amir Pnueli, "In Transition from Global to Modular Temporal Reasoning about Programs," NATO ASI Series 13, Springer 1985.** The Pnueli rule — the discharge schema mununu's `Acyclic` verdict implements.
10. **Martín Abadi & Leslie Lamport, "Conjoining Specifications," ACM TOPLAS 1995.** The canonical treatment of compositional specifications and circular reasoning pitfalls.
11. **Kenneth L. McMillan, "Circular Compositional Reasoning about Liveness," CHARME 1999.** The step-indexed rule. Mununu's mu-rank witness is a lightweight specialization.
12. **Luca de Alfaro & Thomas A. Henzinger, "Interface Automata," ESEC/FSE 2001.** The compatibility-checking view of components. Foundational for compositional extraction of contracts from interfaces.
13. **Rajeev Alur & Thomas A. Henzinger, "Reactive Modules," Formal Methods in System Design 1999.** The controlled/external variable distinction that underlies mununu's controllability rule.
14. **Albert Benveniste, Benoît Caillaud, Dejan Nickovic, Roberto Passerone, Alberto Sangiovanni-Vincentelli et al., "Contracts for Systems Design," INRIA RR-8147 (2012).** The canonical survey of contract-based design — the framework the user's proposal extends with fault-model assumptions.
15. **Alessandro Cimatti, Marco Dorigatti, Stefano Tonetta, "OCRA: A Tool for Checking the Refinement of Temporal Contracts," ASE 2013.** Closest practical sibling tool. Useful for comparing what mununu does vs the OCRA workflow.

### 6.3 Software / learned-assumption variants

16. **Cormac Flanagan & Shaz Qadeer, "Thread-Modular Model Checking," SPIN 2003.** The software analog of A/G — extends to method-level rely/guarantee.
17. **Jamieson M. Cobleigh, Dimitra Giannakopoulou, Corina S. Păsăreanu, "Learning Assumptions for Compositional Verification," TACAS 2003.** L*-based automatic assumption inference. The route to "mununu propose-and-review" UX for missing contracts.
18. **Orna Kupferman, Moshe Y. Vardi, Pierre Wolper, "Module Checking," J. ACM 2000.** Verification of open systems against environment moves. Underlies controllability semantics.

### 6.4 Hardware fault tolerance — theory

19. **Subhasish Mitra & Edward J. McCluskey, "Common-Mode Failures in Redundant VLSI Systems: A Survey," IEEE Trans. Reliability 2000 / ITC 2000.** The mathematical decomposition of why DMR / TMR fail when assumed-independent faults are correlated.
20. **Iztok Krstic, Janak Patel et al., "Synthesis of Fault-Tolerant Finite-State Machines," IEEE TCAD 2008.** Constructive procedure for synthesizing FT controllers; pairs well with mununu's synthesis modes.
21. **Robert Baumann, "Radiation-Induced Soft Errors in Advanced Semiconductor Technologies," IEEE TDMR 2005.** The physics. Required reading to know what fault model you should be assuming at your process node.
22. **Thomas Braibant & Adam Chlipala, "Formal Verification of Hardware Synthesis," ITP 2013.** A Coq proof of a majority voter circuit. Showcases what a *fully formal* TMR correctness proof looks like and how heavy it is.

### 6.5 Hardware fault tolerance — empirical / production

23. **Harish Dattatraya Dixit, Sneha Pendharkar, Matt Beadon, Chris Mason, Tejasvi Chakravarthy, Bharath Muthiah, Sriram Sankar, "Silent Data Corruptions at Scale," arXiv:2102.11245 (Meta, 2021).** Production-scale SDC measurements. Reframes the fault model from "random SEU" to "input-pattern-dependent stable defect."
24. **Peter H. Hochschild et al., "Cores that don't count," HotOS 2021.** Google's matching observation. Reinforces the case that independent-fault DMR proofs do not transfer to production cores.
25. **Cobham Gaisler / LEON3-FT verification reports** (public errata, particularly the registered-voter-output incident). Concrete case where the TCB boundary of a formal proof failed in deployment.

### 6.6 Industrial formal verification practice

26. **Harry Foster (Mentor / Siemens), *Trends in Functional Verification Studies* (biennial since 2007).** Empirical surveys of formal-vs-simulation usage in industry. Reference for "formal datapath verification is < 5% of effort."
27. **OpenTitan formal verification — `hw/formal/README.md` in the OpenTitan repository.** The reference open-source design for security-critical RTL with a documented formal flow. Closest production-grade artifact to compare mununu against.
28. **YosysHQ SymbiYosys (`sby`) documentation.** Open-source formal harness on top of Yosys + ABC. The toolchain mununu's BTOR2 frontend is meant to interoperate with.
29. **Cadence JasperGold "cutpoint" / "set_blackbox" methodology documentation (publicly available in product literature and conference tutorials).** Industrial precedent for black-box modeling in formal hardware verification.

### 6.7 Specific to the user's proposal

30. **The two source documents in the mununu repo itself**:
    - [`docs/design/black-box-modules.md`](https://github.com/vscorza/mununu/blob/main/docs/design/black-box-modules.md) — Document A. The compositional extraction framework that hosts fault-model contracts.
    - [`docs/design/fault-tolerance-contracts.md`](https://github.com/vscorza/mununu/blob/main/docs/design/fault-tolerance-contracts.md) — the planning note specific to this proposal.

## 7. How to use this list

For a grad student new to the area, a workable 4-week reading plan:

| Week | Theme | Papers |
|---|---|---|
| 1 | Mu-calculus + model checking foundations | 1, 2, 3, 4 |
| 2 | Compositional A/G theory | 7, 9, 10, 11, 12 |
| 3 | Practical contract frameworks + soft-error empirical | 14, 15, 19, 23, 24 |
| 4 | Mununu specifics + reproduce the example | 30 + run the DMR example |

By the end of week 4, the student should be able to:
- Write a `(A, G)` contract for a small RTL module of their own design.
- Run `mununu contract validate` and explain each verdict class.
- Express a safety property in mu-calculus, evaluate it on the SV → Kripke pipeline, and interpret a counterexample trace.
- Articulate the *specific* boundary every fault-tolerance proof sits behind: the assumption alphabet, the abstraction soundness direction (over- vs under-approximation), and the residual TCB.

That fluency is the actual goal. The example is a vehicle, not the destination.

## 8. Open research directions

A grad student looking for thesis-scale problems extending the proposal:

1. **Correlated-fault contracts.** Today the contract assumes independent single-bit-flip. Extend with a *correlation* operator on the assumption alphabet (e.g., "if replica A faults, replica B faults identically with probability ≥ p"). Probabilistic mu-calculus over CLTS-as-MDP is the natural target.
2. **L\* learning of fault-model assumptions from simulation traces.** Cobleigh-Giannakopoulou-Păsăreanu's loop, but the teacher is a fault-injection simulator and the learned automaton is the *assumption side* of the contract.
3. **TCB shrinkage via nested TMR.** Modelling the comparator itself as a redundant structure with its own contract, recursively. How small can the TCB get before model size dominates?
4. **Multi-cycle fault models.** Burst-error / persistent-stuck-at as automata over the assumption side rather than single-cycle nondeterminism. The Buchi product blows up; what abstraction preserves the safety verdict?
5. **Contract corpus** for common IP categories (AXI, AHB, OCP, DDR PHY). Mununu's Document D anticipates this. A grad-student-scale contribution: author 10–20 corpus entries with concrete `A` clauses, validate against vendor datasheets, and demonstrate cross-design reuse.

Each of these is plausible as a single-paper thesis chapter, with a clear empirical artifact (the corpus / extended contract IR / measurement campaign) and a clear theoretical statement.

---

*End of brief. Comments and corrections welcome — the framework is young and the mu-rank witness in particular has known limitations beyond its lightweight cases.*
