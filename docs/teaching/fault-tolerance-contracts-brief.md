# Sound GPU Verification Guidelines and Their Mu-Calculus Encodings

*A technical brief for a CS graduate student — what to verify on a GPU subsystem, how to know the verification is sound, and how to translate each guideline into a closed mu-calculus formula that a model checker can decide.*

Audience: a CS / EE grad student with a first course in model checking and SystemVerilog literacy. Reading time: 60–90 minutes plus the bibliography.

---

## 1. What "verifying a GPU" actually decomposes into

A GPU is many things at once: parallel arithmetic pipelines, a memory hierarchy, a scheduler, a power-management island, a debug fabric, and several DMA paths. The phrase *"formally verify a GPU"* is meaningless without specifying which of these subsystems and which of these properties. The first job of any verification guideline is to be precise about its scope.

Four property families recur across every GPU subsystem and each has a different verification economics:

| Family | Example property | What is tractable formally today |
|---|---|---|
| **Control correctness** | Arbiters are deadlock-free; FSMs have no unreachable error state. | Routinely tractable on production designs. |
| **Data integrity** | DMR comparator gates commit; ECC scrub completes; tensor-core output not corrupted. | Tractable on narrow widths (≤ 4–8 bits) or symbolic / BMC. |
| **Isolation & ordering** | Secret never leaves a security domain; memory orderings under a fence; privilege transitions. | Tractable on bounded state; coherence at scale needs symbolic. |
| **Bounded response & RAS** | Voltage-droop sentinel triggers shutdown within N cycles; uncorrectable error reaches the error register. | Tractable when N is small and the responder is finite-state. |

Industrial practice (Foster's biennial Mentor / Siemens surveys; AMD / NVIDIA / Intel public talks) confirms what falls in each cell: control verification is where formal methods earn their keep at scale; datapath verification stays in simulation almost everywhere. This is not a tool deficiency — it is the consequence of word width × functional complexity at the state-space cost of BDD / SAT.

The two empirical reports that reshape what *data integrity* even means in 2026:

- **Dixit et al., "Silent Data Corruptions at Scale," arXiv:2102.11245 (Meta, 2021).** Production datacenter cores produce SDC at parts-per-thousand. The fault is *input-pattern-dependent* and *stable per core*: a specific opcode + operand sequence reliably produces the wrong output on the same defective unit.
- **Hochschild et al., "Cores that don't count," HotOS 2021 (Google).** Same conclusion from a different fleet. The independent-faults assumption that classical DMR/TMR proofs rely on is empirically wrong at the input-dependent level.

This reframes the question: a sound GPU verification guideline does not just say "we verified DMR catches bit flips." It says "we verified DMR catches bit flips **assuming the fault model in §X**, and that assumption is **over-approximating** (so the verdict transfers), and the part we did **not** verify is **enumerated explicitly**."

## 2. What "sound" means — and the four-quadrant rule

A guideline `G` is *sound* against a real GPU when: the formal verdict for `G` on the abstract model implies the corresponding statement on the real silicon.

Whether soundness holds depends on **two** dimensions:

| Property class | Over-approximation (abstract admits ≥ real behaviors) | Under-approximation (abstract admits ≤ real behaviors) |
|---|---|---|
| **Safety** (`G` is "always P") | **SOUND.** Extra abstract behaviors can only add violations. If abstract says safe, real is safe. | **Unsound.** Missed real behaviors may include the violations. |
| **Liveness** (`G` is "eventually Q") | **Unsound.** Extra noop / havoc loops give spurious progress. | Conservatively sound but rarely useful in practice. |

Two operational consequences:

1. **Default verification mode for GPU guidelines is "safety + over-approximation."** The fault model is permissive (the adversary may do *anything* within the assumption alphabet), the property says "no bad state is reachable." If the model checker says SAT, the safety property holds on every concrete refinement.
2. **Liveness needs fairness assumptions.** "Every uncorrectable error eventually reaches the error register" requires a fairness assumption on the reporting path. Without it, an over-approximating model is free to never progress. Either pull liveness back to a *bounded* response ("within N cycles" — still safety, decidable on the Kripke) or document the fairness assumption as part of the guideline.

The hidden third axis is the **trusted compute base (TCB)**: whatever the model does not represent is, by definition, trusted. A guideline that says "DMR catches bit flips" while the comparator itself is unverified has *the comparator in the TCB*. A sound guideline names its TCB explicitly. This is the rule that catches sloppy verification claims more often than any formal soundness argument.

## 3. The mu-calculus toolkit — patterns you will reach for

Mununu's logic is the propositional modal mu-calculus over a finite CLTS (Compositional Labeled Transition System). The atoms in the formulas are **state predicates** synthesized from CLTS state valuations: a signal `commit_valid` with value `T` produces a predicate named `commit_valid_T`; a 4-bit register `broadcast_data` with value `15` produces `broadcast_data_15`. Modal operators range over labelled transitions.

The full grammar is small. The cheat sheet:

| Construct | Reading |
|---|---|
| `P` (a bare identifier) | atomic state predicate |
| `!P`, `P && Q`, `P \|\| Q` | Boolean composition |
| `[a] φ` | after every `a`-step, `φ` holds in the successor |
| `<a> φ` | some `a`-step leads to a state where `φ` holds |
| `[] φ`, `<> φ` | the modalities above quantified over any step |
| `nu X. φ(X)` | greatest fixpoint — "always," "is an invariant" |
| `mu X. φ(X)` | least fixpoint — "eventually," "is reachable" |
| `[(labels={a,b}, ctrl=controllable, req_next={ready}, forb_cur={fault}, steps=N)] φ` | rich modal guard — restrict the modal to the specified transitions only |

The rich modal guard is mununu's distinctive primitive (defined at `crates/mununu-core/src/mu_calculus/mod.rs`). A single `[...]` modality can constrain *five* axes at once — which labels are admissible, what predicates the current state must satisfy, what predicates the next state must satisfy, which controllability class is being talked about, and how many steps. This is what lets a single formula encode a guideline like "during a fault window, no controllable commit step may produce a broadcast" without auxiliary FSMs.

### 3.1 The five canonical patterns

Every GPU guideline I have seen reduces to one of these five shapes, possibly composed:

**(a) Pure safety invariant** — *"the bad combination is never reachable."*

```text
nu X. ( ! BAD && [] X )
```

Read: every reachable state has `BAD` false, and so does every successor. Use this for *every* data-integrity, isolation, and "never disable the watchdog" guideline.

**(b) Reachability** — *"the good state is reachable from here."*

```text
mu X. ( GOAL || <> X )
```

Use to demonstrate that a recovery path exists from an error state, or that a reset state is reachable from every operating mode. Mu and nu have dual roles: nu is "always," mu is "eventually."

**(c) Response (unbounded)** — *"whenever P, eventually Q."*

```text
nu X. ( ( !P || mu Y. ( Q || <> Y ) ) && [] X )
```

Use carefully — unbounded eventuality is liveness, so it is only sound under an over-approximating fault model if you have *also* added a fairness assumption to the environment. Better in GPU contexts is the bounded variant:

**(d) Bounded response** — *"whenever P, within N cycles Q."*

```text
nu X. ( ( !P || [(steps=N)] Q_reached_within_N ) && [] X )
```

The `steps=N` modal annotation in the rich guard bounds the modal exploration. This is the right shape for RAS guidelines ("uncorrectable error reaches the error register within 8 cycles"), watchdog timeouts, and voltage-droop sentinels. Bounded response is safety in disguise — it stays sound under over-approximation.

**(e) Controllability-scoped invariant** — *"controllable steps never cause BAD; the adversary may do as it pleases."*

```text
nu X. ( ! BAD && [(ctrl=controllable)] X )
```

Use when only the design's reaction is in scope and the environment is chaotic. For DMR / TMR with adversarial fault injection, you typically want `[(ctrl=environment)]` (only env steps quantified) for the fault-injection axis, with the safety property holding regardless.

### 3.2 Worked translation — English guideline to closed formula

**English:** *"If an uncorrectable ECC error is detected on any memory channel, the error register must reflect it within 4 cycles, and no commit may broadcast a value from the affected channel during those 4 cycles."*

**Decomposition** — there are two atomic guideline obligations bound by an AND:

1. *Reporting (bounded response):* `uce_detected → within 4 cycles, err_reg_set`.
2. *Quarantine (safety during the window):* `uce_detected → for 4 cycles, no commit_broadcast_from_affected_channel`.

**Formalization** — pick state-predicate names from valuations the SV adapter will produce: `uce_detected_T`, `err_reg_set_T`, `commit_broadcast_T`, `affected_channel_T`.

Reporting: pattern (d) with `P = uce_detected_T`, `Q = err_reg_set_T`, `N = 4`. Hand-translated for clarity:

```text
nu X. (
    ( !uce_detected_T
    || ( err_reg_set_T
       || [(steps=1)] ( err_reg_set_T
                      || [(steps=1)] ( err_reg_set_T
                                     || [(steps=1)] ( err_reg_set_T
                                                    || [(steps=1)] err_reg_set_T )))))
    && [] X
)
```

Quarantine: a nested safety check that says "during four steps following `uce_detected_T`, no `commit_broadcast_T` may co-occur with `affected_channel_T`."

```text
nu X. (
    ( !uce_detected_T
    || ( !(commit_broadcast_T && affected_channel_T)
       && [(steps=1)] !(commit_broadcast_T && affected_channel_T)
       && [(steps=1)] [(steps=1)] !(commit_broadcast_T && affected_channel_T)
       && [(steps=1)] [(steps=1)] [(steps=1)] !(commit_broadcast_T && affected_channel_T) ))
    && [] X
)
```

The full guideline is the AND of the two. Both clauses are nu-fixpoints over safety — they remain sound under over-approximating fault models.

This is the texture of formula encoding in practice: an English bullet becomes a single mu-calculus closed formula, usually a nested safety invariant. The mechanical part is the encoding; the judgment is in picking the right boundary (what's in scope, what's TCB) and the right time horizon (when does N stop being tractable).

### 3.3 The six failure modes when encoding a guideline as a formula

The patterns above hide failure modes. A grad student will hit each of these at least once:

1. **The atom doesn't exist.** You write `commit_valid_T` and the model checker says "predicate not found." Cause: the Kripke builder did not generate that valuation because the underlying signal is wider than the auto-abstract limit, or the signal is declared as an `output logic` (combinational) rather than an internal `logic` (registered). Fix: ensure the signal is an internal register and within abstract bounds, or declare a domain in the sidecar.
2. **Vacuous truth.** The formula says `[] P` and trivially passes because no transitions exist. Always run a deadlock-check `nu X. ([] X)` first; verify your Kripke is nontrivial.
3. **Wrong polarity.** "Never commit when corrupted" reads naturally as a `mu`-formula ("there is no path to bad state"). Don't write `mu`; you want the dual `nu X. (!BAD && [] X)`. The two are *not* the same in finite Kripke with `[]`-quantification.
4. **Liveness without fairness.** `mu X. (Q || <> X)` over a chaotic environment is unsound for liveness. Either bound it (`steps=N`) or add fairness to the environment specification.
5. **Implicit TCB.** The formula mentions `voter_output_T` but the voter is in the TCB (untrusted). The proof says "the voter never lies" — circularly. Fix: name the TCB in the guideline's README.
6. **Predicate name collision with state name.** State names get auto-promoted to predicates. If you name a state `commit_valid`, the predicate `commit_valid_T` collides. Use unambiguous names for both.

The first three are mechanical; the last three are judgment calls and the place where a grad student earns their fluency.

## 4. Encoding the fault model — five sound patterns

A fault model is *environment freedom*: which inputs the verifier may freely choose. The choice of fault-model encoding determines both the proof's strength and its scope.

| Pattern | SV idiom | Soundness | When to use |
|---|---|---|---|
| **Pure non-deterministic value** | `input logic [W-1:0] adv_value;` and use it directly | Strongest over-approx: covers SEU + MBU + stuck-at + persistent at the targeted port | When the targeted signal is at a clean abstraction boundary (interface, comparator input). |
| **One-hot XOR mask** | `signal ^ (mask << idx)` with env-chosen `idx`, `mask` constant | Strict single-bit-flip per cycle. MBU excluded by encoding. | When the guideline explicitly assumes SEU. |
| **Bounded-multiplicity mask** | Env-chosen `mask` constrained by `popcount(mask) ≤ K` (via env automaton) | K-bit flip per cycle. Burst SEU. | When DRAM / SRAM at advanced nodes (MBU regime). |
| **Stuck-at register** | `if (stuck_en) reg <= old; else reg <= new;` with env-chosen `stuck_en` | Persistent fault. | When modeling defective flop / latch. |
| **Bursty / interval-bounded** | Small FSM driving the fault-injection input with env-chosen interval ≥ K | Faults separated by at least K cycles. | When modeling MBU clustering with refractory period. |

**Rule of thumb.** Pick the *coarsest* fault model under which the guideline still holds. Coarseness = strength of the proof (your safety statement is conditional on a stronger assumption). Don't reach for the most precise fault encoding "because it's more realistic" — it tightens the assumption, so the conclusion transfers to fewer real systems.

The dual rule: state every fault-model exclusion in plain English at the top of the guideline. Examples from the worked DMR brief:

- "Single fault per cycle only; MBU excluded by the one-hot encoding."
- "Only replica A is faulted; correlated common-mode failures excluded."
- "The comparator, commit register, and observer are in the TCB."
- "No liveness — we verify safety only."

These four sentences are not boilerplate. They are the proof's claim shape.

## 5. One runnable instance — the 4-bit DMR commit gate

A complete working example lives at [`examples/hw/dmr_commit_4bit/`](../../examples/hw/dmr_commit_4bit/). It is *one* instance of the patterns in §3 and §4 above; it is not the only shape a fault-tolerance guideline can take.

What it encodes:

- **Fault model:** one-hot XOR mask on replica A's output, structurally encoded as `y_a_post = flip_en ? (y_a ^ (4'b0001 << flip_idx)) : y_a;`. Sound for SEU; excludes MBU.
- **Property:** *"No commit broadcasts a value disagreeing with the fault-free reference."* Encoded as pattern (a) — pure safety invariant:
  ```text
  nu X. (!corrupted_broadcast_r_T && [] X)
  ```
- **Verdicts (LTS witness only; not yet under simulation):**
  - Intact design: SAT, 81/81 reachable states.
  - Negative control (same SV with the commit gate removed): UNSAT, 0/161 states, initial state fails.
- **TCB enumerated** in the example README: datapath, common-mode failures, MBU, comparator, commit register, observer, liveness — all out of scope.

The point of citing this example here is to show the full traversal of §1–§4 against one concrete piece of RTL: property class (data integrity), abstraction direction (over-approximating), formula pattern (pure safety invariant), fault-model encoding (one-hot XOR), and TCB declaration. Every GPU guideline you write should be reducible to a similar five-line summary.

## 6. Where this stops being tractable, and what to do instead

Mununu's Kripke builder has a hard state cap of 2^18 states, and the SV adapter auto-abstracts signal widths > 4 bits. Three failure modes you will hit:

- **Datapath width.** A 32-bit FMA cannot be enumerated. Options: symbolic / BMC (BTOR2 + Yosys + sby), or a *property-driven slicing* where you verify a structural invariant on a small slice and argue informally about the rest.
- **Liveness over chaotic environment.** Over-approximated liveness is unsound. Options: bound it (response within N), pull the fairness assumption back into the design as a controllable label and prove conditional liveness, or move the question to a different verification class (e.g., probabilistic model checking for response-time guarantees).
- **Composition state explosion.** A four-component composition can blow past 2^18 even if each component is small. Options: compositional reasoning (each component's contract verified locally), partial-order reduction, or domain-specific abstractions in the sidecar.

A practical rule: **if the property is safety over a small modal horizon (say `steps ≤ 16`), aim for direct Kripke verification.** Otherwise reach for the bridge tools — BMC for large widths, probabilistic for response-time guarantees, simulation-driven assumption refinement (L\*) for environment models.

## 7. The library question — guideline-to-formula corpus

A practical research direction (and the natural deliverable a GPU vendor would ship alongside their RTL): a **corpus of canonical guidelines** in English plus their canonical mu-calculus formulas. Each corpus entry would carry:

- The English statement (the architectural rule).
- The property family from §1 (control / data / isolation / RAS).
- The canonical formula pattern from §3.
- The recommended fault-model encoding from §4.
- The tractability envelope (state-space bounds, width limits).
- The TCB statement.
- A small SV stub on which the formula is verifiable.

A starter list of GPU-relevant entries: FMA monotonicity, tensor-core commit gating, ECC scrub bounded response, watchdog liveness, voltage-droop sentinel, clock-gate safety, debug-fabric isolation, DMA boundary check, retire-stage commit monotonicity, fence-ordering, secure-boot signature.

Mununu's existing template registry at `crates/mununu-core/src/adapter/templates/` is the substrate for such a corpus; the work is in the curation, not the engine. A grad student could plausibly author 10–20 entries as a single thesis chapter with a clear empirical artifact and reuse argument.

## 8. Annotated reading list

Slimmed to the papers that directly support **formula encoding** and **soundness reasoning** for the GPU verification setting. Compositional A/G contract theory has its own deep literature but is secondary for this brief.

### 8.1 Mu-calculus and model checking — the encoding theory

1. **Dexter Kozen, "Results on the Propositional µ-Calculus," ICALP 1982 / TCS 1983.** The mu-calculus itself. Syntax, semantics, fixpoint characterization. Read first.
2. **E. Allen Emerson & Joseph Halpern, "Decision Procedures and Expressiveness in the Temporal Logic of Branching Time," STOC 1982 / JCSS 1985.** CTL vs LTL, branching-time semantics. Necessary to know which modal patterns belong to which logic.
3. **Edmund Clarke, E. Allen Emerson, A. Prasad Sistla, "Automatic Verification of Finite-State Concurrent Systems Using Temporal Logic Specifications," ACM TOPLAS 1986.** The algorithmic basis of CTL model checking; the foundation mununu builds on.
4. **Colin Stirling, *Modal and Temporal Properties of Processes*, Springer 2001.** Textbook. The cleanest pedagogical treatment of mu-calculus, parity games, and fixpoint depth.
5. **Wieslaw Zielonka, "Infinite Games on Finitely Coloured Graphs with Applications to Automata on Infinite Trees," TCS 1998.** Positional determinacy of parity games — the result that makes mu-calculus winning strategies finite-memory. Justifies mununu's strategy-extraction soundness.
6. **Zohar Manna & Amir Pnueli, *The Temporal Logic of Reactive and Concurrent Systems*, Springer 1992.** The canonical reference for what each temporal pattern means. Use as a lookup when you need to argue precisely what "always," "eventually," "response," and "bounded response" denote.

### 8.2 Abstraction and soundness — when does the formal verdict transfer?

7. **Patrick & Radhia Cousot, "Abstract Interpretation: A Unified Lattice Model for Static Analysis of Programs by Construction or Approximation of Fixpoints," POPL 1977.** Foundational. The soundness of over- and under-approximation, where to put the abstraction boundary.
8. **Edmund Clarke, Orna Grumberg, David Long, "Model Checking and Abstraction," ACM TOPLAS 1994.** The CTL-specific take: how abstraction interacts with universal vs existential modalities, when preservation holds.
9. **Glenn Bruns & Patrice Godefroid, "Generalized Model Checking: Reasoning about Partial State Spaces," CONCUR 2000.** The OOB-sink projection mununu uses; the formal justification of the masking rule.
10. **Dennis Dams, Rob Gerth, Orna Grumberg, "Abstract Interpretation of Reactive Systems," ACM TOPLAS 1997.** Modal abstraction with sound under- and over-approximation simultaneously (must / may transitions). Required if you want to combine safety and liveness in one model.

### 8.3 Soft errors, SDC, hardware fault models

11. **Robert Baumann, "Radiation-Induced Soft Errors in Advanced Semiconductor Technologies," IEEE TDMR 2005.** The physics. Required reading to know what fault model your process node actually exhibits.
12. **Subhasish Mitra & Edward J. McCluskey, "Common-Mode Failures in Redundant VLSI Systems: A Survey," IEEE Trans. Reliability 2000.** The mathematical decomposition of why DMR / TMR fail when fault-independence assumptions break.
13. **Harish Dattatraya Dixit, Sneha Pendharkar, Matt Beadon et al., "Silent Data Corruptions at Scale," arXiv:2102.11245 (Meta, 2021).** Production-scale SDC measurements. Reframes the fault model from "random SEU" to "input-pattern-dependent stable defect."
14. **Peter H. Hochschild et al., "Cores that don't count," HotOS 2021 (Google).** Companion to Dixit et al. Confirms independent-fault DMR proofs do not transfer to production cores.
15. **Iztok Krstic, Janak Patel et al., "Synthesis of Fault-Tolerant Finite-State Machines," IEEE TCAD 2008.** Constructive synthesis of fault-tolerant controllers. Pairs naturally with mununu's synthesis modes (`functional`, `permissive`).

### 8.4 GPU and accelerator verification practice

16. **Harry Foster (Mentor / Siemens), *Trends in Functional Verification Studies* (biennial, 2007–present).** Empirical surveys. The reference for "datapath formal is < 5% of total verification effort."
17. **OpenTitan formal verification documentation — `hw/formal/README.md` in the OpenTitan repository.** Closest public production-grade artifact to compare a GPU formal flow against; describes the JasperGold-driven flow for security-critical control logic.
18. **Vendor public talks (DAC, DVCon, Hot Chips) on AMD RDNA/CDNA, NVIDIA SM-level, Intel Xe-HPC verification flows.** Not single citations; track the most recent two years of these conferences for the current state of practice. Theme: UVM constrained-random dominates compute-unit verification; formal is surgical (arbiters, FSMs, CSR decode).

### 8.5 Optional — compositional contracts and learning

If you decide to scale guidelines across modules (one component's guarantee discharging another's assumption), these are the core sources:

19. **Amir Pnueli, "In Transition from Global to Modular Temporal Reasoning about Programs," NATO ASI Series 13, Springer 1985.** The Pnueli discharge rule.
20. **Kenneth L. McMillan, "Circular Compositional Reasoning about Liveness," CHARME 1999.** Step-indexed circular reasoning.
21. **Albert Benveniste et al., "Contracts for Systems Design," INRIA RR-8147 (2012).** Survey of contract-based design. Use as a map of the literature, not a starting point.
22. **Jamieson M. Cobleigh, Dimitra Giannakopoulou, Corina S. Păsăreanu, "Learning Assumptions for Compositional Verification," TACAS 2003.** L\* applied to assumption inference. Relevant if you want to bridge from fault-injection simulation traces to refined fault assumptions automatically.

## 9. A 4-week study plan

| Week | Theme | Papers |
|---|---|---|
| 1 | Mu-calculus + model-checking foundations | 1, 2, 3, 4 |
| 2 | Abstraction soundness (the **why** behind the four-quadrant rule) | 7, 8, 9, 10 |
| 3 | Soft-error fault models + GPU verification practice | 11, 12, 13, 14, 16 |
| 4 | Write your own guideline + encode it | Reproduce the §5 example end-to-end; write one new corpus entry following §7 |

By end of week 4 you should be able to:

- Pick the right formula pattern from §3 for any English guideline.
- Justify your fault-model choice from §4 in terms of soundness direction.
- Run `mununu context eval` against an SV file and read the verdict.
- Write the TCB declaration that closes the soundness argument.

That fluency is the actual goal. The example is a vehicle, not the destination.

## 10. Open directions

Each of these is plausible as a single-paper thesis chapter:

1. **Corpus of canonical GPU guidelines** (§7) — 10–20 entries with English, formula, fault model, TCB, tractability envelope, SV stub.
2. **L\* / assumption learning bridge from FI simulators** — automatically refine a fault-model assumption from simulation counterexamples and feed it back as a tighter env automaton.
3. **Probabilistic mu-calculus for SDC rate guarantees** — extend the encoding to express "the probability of corrupted commit is below ε under fault rate λ." Connects to Kwiatkowska et al.'s PRISM-style stochastic model checking.
4. **Bridge from BMC counterexamples to mu-calculus invariants** — given a SAT-found counterexample on a wide datapath, generate a closed mu-calculus invariant on a Kripke-tractable slice that excludes the trace.
5. **TCB shrinkage through recursive verification** — formally verify the comparator under its own contract; recursively until you hit a primitive you accept as trusted. How small does the TCB become?

Each has a measurable artifact and a falsifiable claim. Each is also independently useful even if the others fail.

---

*End of brief. Comments and corrections welcome — the patterns in §3 and §4 in particular benefit from being road-tested on new SV examples.*
