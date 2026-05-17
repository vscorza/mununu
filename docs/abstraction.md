# Abstraction guidelines for mununu

> Source of truth: [`crates/mununu-core/src/clts/`](../crates/mununu-core/src/clts/) (state-variable + per-state predicate primitives), [`crates/mununu-core/src/adapter/systemverilog/`](../crates/mununu-core/src/adapter/systemverilog/) (signal-level abstraction sidecar), [`crates/mununu-core/src/mu_calculus/`](../crates/mununu-core/src/mu_calculus/) (`hide` + bisimulation minimization at evaluation time) — surface: CLI+API+UI.

## Why this doc exists

Mununu's verdicts are correct **for the model**. Whether they transfer to the real system depends entirely on the abstraction the user (or adapter) picks. The [`Soundness Guarantees`](../CLAUDE.md#soundness-guarantees) section of CLAUDE.md states the three soundness directions; this doc gives the **per-subsystem recipe** — *which* mununu primitive to reach for, *when*, and what each primitive preserves.

Use this doc when:

- You are authoring an adapter and have to decide how to encode a multi-bit register or a memory region.
- You are hand-writing a CTXDSL source for a peripheral, microprogram, cache, bus, or protocol spec.
- You are reviewing a `// SOUNDNESS:` annotation and want a checklist.
- You are deciding whether an automated extraction is feasible for a new subsystem class or whether the user must hand-author it.

## The abstraction toolbox mununu already ships

| Primitive | What it abstracts | Surface where exposed | Soundness | Status |
|---|---|---|---|---|
| `AbstractionType::Boolean` | Multi-bit signal → `{ low, high }` | SV sidecar `domain.abstraction`; CTXDSL `boolean` variables | Sound iff property does not distinguish values within the same equivalence class | Shipped |
| `AbstractionType::Interval { min, max }` | Signal → finite set of integer intervals | SV sidecar | Same as above | Shipped |
| `AbstractionType::Symbols(Vec<String>)` | Signal → named representative values + "other" | SV sidecar; CTXDSL enum types | Same as above | Shipped |
| `AbstractionType::Ignored` | Drop signal from state space | SV sidecar | Sound for safety (model permits every concrete value); under-approximates liveness | Shipped |
| Per-state predicate (`state_variable_bitset`) | Lift state name → mu-calculus predicate | `clts/mod.rs:1173`; CTXDSL `predicates { … }` | Exact (no abstraction loss) | Shipped |
| Per-state structured valuation | Hand-write display metadata on state | CTXDSL `state S { valuations { … } }`; `ContextDoc.state_valuations` | Exact (display-only by default; formal when paired with predicates) | Shipped |
| Multi-label transitions | Collapse parallel edges → one transition w/ N labels | CTXDSL `transition s -> t on label a, label b;`; `SmallVec<[LabelId; 4]>` | Exact | Shipped |
| Rich modal guards | One `[…]` / `<…>` combining labels + current/next predicates + controllability class + step bound | CTXDSL `[(labels = {a}, req_next = {active}, ctrl = controllable)] φ`; `Guard` struct | Exact | Shipped, under-used |
| Chaotic stub on a label set | 1 state + self-loop per label for an unmodelled subsystem | hand-authored CTXDSL; future `codesign emit-chaotic-stub` generator | Sound for safety (over-approximation); **optimistic for liveness** | Pattern shipped; auto-gen pending |
| Hide / reclassify-as-internal | Hide labels from observable alphabet at evaluation time | `mu_calculus::evaluate_with_options` `hide: Vec<String>`; `/context/verify` API `hide` | Preserves safety; **can lose liveness** (hidden label cannot be required) | Shipped |
| Bisimulation minimization | Quotient composed automaton over observable behaviour | `evaluate_with_options` `minimize: true`; API `minimize` | Preserves all CLTS-observable behaviour (full bisimulation equivalence) | Shipped |
| Compositional stubs | Replace one source with `.espec.json` stub at verify time | `/context/verify` API `stubs`; matching CLI flag | Soundness depends on stub posture (chaotic vs. constrained) | Shipped |
| Domain profiles | Bias AST extractor to one of `software` / `rtl` / `agentic` / `synthesis` / `universal` | `mununu-extract --domain`; `extraction/ast_extract/domain.rs` | Profile chooses defaults but does not enforce soundness — must be declared per-extraction | Shipped |
| Mode filtering | One spec → multiple abstraction levels (e.g. `fixed` vs `vulnerable`) | `.espec.json` `mode` field | Per-mode posture declared inline | Shipped |

## The rule of thumb for automated extraction vs hand-authoring

> **Automated extraction is viable for a subsystem when the abstraction shape is** *uniform across instances* **and the source format carries enough structural information for an adapter to instantiate it mechanically.**

This holds for:

- **Firmware drivers** — `c-codesign` adapter. The register-map sidecar carries the structural information (which signals are MMIO, what their direction is).
- **Restricted microprograms** — a planned `microcode` adapter (see `docs/adapters/microcode.md` when shipped). The `regs` / `mem` / `interrupts` declarations carry the structural information.
- **Word-level arithmetic inside firmware / microcode** — subsumed by the host adapter's register-abstraction infrastructure; no separate extraction path.
- **Shared memory regions referenced by tracked addresses** — a chaotic-stub generator parameterised by an address list is mechanical.

It does **not** hold for:

- **Pipelines** — abstraction shape is uniform across CPU classes (5-stage in-order, 6-stage with forwarding, …) but no source format carries the structural information. Hand-authored CTXDSL or a **parameterised library template** is the right pattern.
- **Cache controllers** (MESI / MSI / MOSI / dragon, …) — same as pipelines.
- **Coherence buses** — same.
- **Interrupt controllers (PLIC, NVIC, …)** — same.
- **Bespoke critical subsystems** (watchdog, DMA, MMU, debug unit) — narrow utility per template; hand-authored CTXDSL is realistic.

This distinction matters because it tells you where the next investment goes: "build an extractor" versus "ship a parameterised template library". They cost different orders of magnitude — adapters are ~1.5k LOC each plus a per-format learning curve; CTXDSL library templates are ~100-300 LOC plus a small instantiation tool.

## Per-subsystem recipe

For each concrete subsystem class, the **minimum** abstraction that keeps the model tractable plus the primitive that supplies it.

### Word operations (32/64-bit arithmetic, shifts, comparisons)

- **Concrete content.** Arbitrary integer operations on register-resident values.
- **What to abstract.** Treat each operand as the abstraction class of its source register. Operations have no observable label — they update the abstract value of the destination register. Track status flags (carry / overflow / zero) only when a property reads them.
- **Primitive.** `AbstractionType::Symbols` for register values. Operations are subsumed by the host source's transition function.
- **Automated extraction?** Yes via `c-codesign` (LLVM SSA collapses arithmetic). Yes via the planned `microcode` adapter (no transitions emitted for pure computation).
- **Soundness.** Sound iff property does not distinguish values within the same symbol class.

### Memory (general-address, multi-GB)

- **Concrete content.** Byte-addressable RAM, infinitely many possible values per byte.
- **What to abstract.** Only tracked addresses (declared in microcode `mem { … }` or referenced by the cache's tracked-line set) are modelled. Per-address state is a small symbol set: `{ initial, written_by_<src>, observed_by_<sink> }` for provenance-tracking properties; `{ stale, fresh }` otherwise.
- **Primitive.** `AbstractionType::Symbols` per address. Chaotic stub on the untracked-address majority.
- **Automated extraction?** Yes via a chaotic-stub generator parameterised by the tracked-address list — pending implementation.
- **Soundness.** Tracked-address restriction is sound for safety properties referencing only tracked addresses. Chaotic stub over untracked addresses is sound for safety, optimistic for liveness.

### Pipelines (per CPU core)

- **Concrete content.** N pipeline registers carrying instruction + control fields, hazard logic, forwarding paths.
- **What to abstract.** Per-stage occupancy as Boolean `{ empty, busy }`. Forwarding encoded as multi-label transitions. Branch flush encoded via rich modal guards. No cycle-accurate timing.
- **Primitive.** `AbstractionType::Boolean` for occupancy; multi-label transitions; rich modal guards.
- **Automated extraction?** No — the abstraction is semantic. Library template feasible.
- **Soundness.** Sound iff the property does not require cycle-accurate timing. Document `// SOUNDNESS: pipeline occupancy is Boolean-abstracted; cycle-level hazards are not modelled.` inline.

### Caches (per core, per line)

- **Concrete content.** Cache memory (KB to MB), tag array, coherence state bits per line.
- **What to abstract.** Memory content not tracked. Tracked lines = small hand-picked set (1-4 typically). Per-line state = symbol set of the coherence protocol's states (e.g. `{ I, S, E, M }` for MESI).
- **Primitive.** `AbstractionType::Symbols` per line; per-state predicate to lift `M_lineX` into a mu-calculus-usable predicate.
- **Automated extraction?** Partial — library template parameterised by `<N>` cores × `<M>` lines.
- **Soundness.** Sound iff the property references only tracked lines and the protocol's symbol set is exhaustive.

### Coherence buses

- **Concrete content.** Arbitration, snoop requests, invalidations, write-backs.
- **What to abstract.** Bus cycles → discrete events. Arbitration fairness → fairness annotation only if liveness is being verified. At most one outstanding transaction per line in the simplest model.
- **Primitive.** Multi-label transitions on rendezvous events.
- **Automated extraction?** No. Hand-authored CTXDSL with a library-template path for common protocols (MESI bus, AXI ACE) as follow-up.
- **Soundness.** Sound when arbitration determinism is modelled accurately enough for the property; chaotic-stub variant sound for safety only.

### Interrupt controllers (PLIC / NVIC)

- **Concrete content.** Per-source pending + priority + enable; arbiter; per-hart claim/complete.
- **What to abstract.** Pending state Boolean per tracked source. Priority either dropped or symbolic (`{high, low}`). Claim/complete as discrete events.
- **Primitive.** `AbstractionType::Boolean` per source; per-state predicates.
- **Automated extraction?** Partial — library template feasible.
- **Soundness.** Same as caches.

### Critical coexisting subsystems (watchdog, DMA, MMU, debug)

- **Concrete content.** Subsystem-specific.
- **What to abstract.** A small CLTS automaton (3-10 states) modelling only the interaction with bus / interrupt interfaces. Internal state not visible at those interfaces is dropped.
- **Primitive.** Hand-authored CTXDSL; `sv-rtl` when RTL exists.
- **Automated extraction?** No for the general case.
- **Soundness.** Sound iff the externally-visible abstraction matches the actual interaction protocol.

### Firmware drivers

- **Concrete content.** Real C source.
- **What to abstract.** Memory-mapped accesses → rendezvous labels via the register-map sidecar. Non-MMIO code → internal events. Loops → bounded or under-approximated (with a documented soundness note).
- **Primitive.** `c-codesign` adapter (shipped).
- **Automated extraction?** Yes — most mature path mununu has.
- **Soundness.** Documented inline by the adapter; loop bounding is the main soundness-relevant choice.

## Soundness summary (one-line reference)

- **Boolean / interval / symbol-set on a variable** — sound when property doesn't distinguish values within an equivalence class.
- **Ignored variables** — sound for safety; model permits every concrete value.
- **Chaotic stub** — over-approximation; sound for safety, optimistic for liveness (Doc C §C.5 for the codesign formulation).
- **Hidden labels** — preserve safety; can lose liveness (hidden label cannot be required as a transition).
- **Bisimulation minimization** — preserves all CLTS-observable behaviour.
- **Tracked-address restriction on memory** — equivalent to ignoring untracked addresses; sound for properties that reference only tracked addresses.

## Authoring discipline

When introducing an abstraction decision — whether in an adapter, a CTXDSL source, or a sidecar — follow this checklist:

1. **Declare the abstraction posture explicitly.** Either inline (`AbstractionType::Symbols(["zero", "non_zero"])` in a sidecar; `mem { x : shared }` in microcode) or in a comment block at the top of a hand-authored CTXDSL file.
2. **Add a `// SOUNDNESS:` annotation** at every `eval_expr → None` choice and every adapter decision that drops information. State whether it is over-approximation or under-approximation and why it is sound for the relevant property class. CLAUDE.md § Soundness Guarantees is the enforcement point; `/soundness-check` is the audit skill.
3. **Add a regression test** for the abstraction decision when adding a new adapter or modifying the Kripke builder. The test must exercise both the abstracted case and at least one concrete case that maps into the same abstraction class, asserting the verdict agrees.
4. **Document the choice in the user-facing wiki page** for the affected adapter or workflow. If the abstraction is non-obvious (e.g. "the chaotic-stub peripheral over-approximates every register access"), state the soundness consequence inline.

## What this doc deliberately does not do

- **Quantify state-space cost** per abstraction choice. That depends on the user's specific model; a follow-up benchmark suite would help, but it does not exist today.
- **Prescribe one abstraction class as "the right one"** for any subsystem. The right class depends on the property — the recipe above gives the *minimum*; safety-only properties often tolerate coarser abstractions than liveness properties.
- **Cover memory-model semantics** (RVWMO, TSO, sequential consistency). Mununu does not encode any weak-memory model today; verifying memory-order intent at the architectural level requires either an external checker integrated as an adapter or a heavy abstraction (TSO / SC) that ignores weak orderings.

## See also

- [Soundness Guarantees](../CLAUDE.md#soundness-guarantees) — the load-bearing rules.
- [`docs/policies/claims-integrity.md`](policies/claims-integrity.md) — full claims-integrity policy with the abstraction-soundness procedure.
- [`docs/adapters/extraction.md`](adapters/extraction.md) — `.espec.json` extraction adapter, mode filtering, property templates.
- [`docs/synthesis.md`](synthesis.md) — `ControllerMode`, signature-based extraction, Skolem-paradigm rules.
- [`wiki/Verify-Project-Flow.md`](../wiki/Verify-Project-Flow.md) — the verify framework that consumes all of the above.
