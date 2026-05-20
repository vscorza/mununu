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

## Automatic cone-of-influence (Phase A.3)

> Source of truth: [`adapter/partition/mod.rs`](../crates/mununu-core/src/adapter/partition/mod.rs) — surface: CLI+API.

mununu's BTOR2 and SystemVerilog adapters now run an **automatic
cone-of-influence pass** during translation. The pass walks the
frontend IR's signal dependency graph from the seed atoms extracted
from the property formulas (intrinsic `bad`/`constraint`/`justice`
lines on BTOR2, atoms in `@mununu` property comments on SV), keeps
every transitively-reachable state cell and input, and pins everything
else to [`AbstractionType::Ignored`](../crates/mununu-core/src/adapter/domain.rs).

**User wins on collision.** Auto-COI only acts on signals the sidecar
does **not** mention. If a signal appears in
`.mununu.json`'s `signals[]` or `inputs[]` with any explicit
declaration, that declaration wins regardless of the auto-COI verdict.
The two layers compose:

1. The sidecar carries user-curated abstractions (`Boolean`,
   `BoundedCounter`, `EnumValues`, `Ignored`, `Discover`).
2. Auto-COI fills the gap for signals the user did not list, dropping
   them to `Ignored` when they are outside the property's reach.

**Soundness.** Cone-of-influence dropping is an over-approximation —
the abstract model admits *more* behaviours than the concrete model
(pinned signals are equivalent to havoc on their value). For safety
properties this is sound; for liveness see the soundness summary at
the bottom of this doc.

**Defensive default for unbindable seeds.** If the adapter cannot
extract any property atoms (e.g., a BTOR2 file whose `bad` line traces
only through anonymous compiler-synthesised state cells), the
partition keeps every signal rather than drop everything silently.
This avoids accidental under-approximation when the property
structure is opaque to the partition's syntactic scan.

**Observability.** Every adapter populates
[`AdapterOutput.partition_summary`](../crates/mununu-core/src/adapter/mod.rs)
with the counts and bit-widths. The verify orchestrator threads this
through `SourceSummary.partition_summary` in `VerifyReport`; CLI
warnings surface each `Dropped` signal (`"auto-partition: state cell
'X' dropped (outside-cone-of-influence); add an explicit sidecar entry
to override"`).

**Opt-out.** `PartitionOptions::disabled = true` returns every signal
as `Kept` — useful for regression triage but not exposed on the CLI
today.

**Predicate binding for `signal == const` atoms.** Phase A.3 also
wires per-state valuations from the BTOR2 bit-blaster into the
mu-calculus evaluator's `Environment::abstract_states` channel and
extends the parser to capture `signal == constant` (also `!=`, `<`,
`<=`, `>`, `>=`) as a single predicate atom. Formulas of the shape
`nu X. ((!(state == 5)) && [] X)` now discriminate over the model
instead of returning vacuous verdicts. Scope-guarded to BTOR2-style
numeric valuations to keep the SV adapter's variant-string predicate
path unchanged.

**Datapath UF substitution** (combinational arithmetic collapsed to
uninterpreted functions) is reserved for the follow-up plan
[`phase-a3-followup-datapath-uf.md`](../../.claude/plans/phase-a3-followup-datapath-uf.md);
the `Datapath { uf_symbol }` variant is reserved in
`PartitionClass` but never produced today.

## The rule of thumb for automated extraction vs hand-authoring

> **Automated extraction is viable for a subsystem when the abstraction shape is** *uniform across instances* **and the source format carries enough structural information for an adapter to instantiate it mechanically.**

This holds for:

- **Firmware drivers** — `c-codesign` adapter. The register-map sidecar carries the structural information (which signals are MMIO, what their direction is).
- **Restricted microprograms** — shipped via the [`microcode` adapter](../crates/mununu-core/src/adapter/microcode/) (plan Part 5 + Part 5.5). JSON input; `regs` / `mem` / `interrupts` declarations carry the structural information; ops emit canonical rendezvous labels (`wr_mem_<region>`, `rd_mem_<region>`, `fence_<order>`, `irq_ack_<source>`). See `examples/verify/rv5_2core_mesi_microcode_extracted/` (parity) and `examples/verify/dma_engine_microcode/` (industrial DMA demo).
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
- **Automated extraction?** Yes via `c-codesign` (LLVM SSA collapses arithmetic). Yes via the [`microcode` adapter](../crates/mununu-core/src/adapter/microcode/) (no transitions emitted for pure computation; explicit `regs` / `mem` declarations + sharing tags drive the alphabet).
- **Soundness.** Sound iff property does not distinguish values within the same symbol class.

### Memory (general-address, multi-GB)

- **Concrete content.** Byte-addressable RAM, infinitely many possible values per byte.
- **What to abstract.** Only tracked addresses (declared in microcode `mem { … }` or referenced by the cache's tracked-line set) are modelled. Per-address state is a small symbol set: `{ initial, written_by_<src>, observed_by_<sink> }` for provenance-tracking properties; `{ stale, fresh }` otherwise.
- **Primitive.** `AbstractionType::Symbols` per address. Chaotic stub on the untracked-address majority.
- **Automated extraction?** Yes via a chaotic-stub generator parameterised by the tracked-address list — pending implementation.
- **Soundness.** Tracked-address restriction is sound for safety properties referencing only tracked addresses. Chaotic stub over untracked addresses is sound for safety, optimistic for liveness.

See the **[Memory soundness matrix](#memory-soundness-matrix)** below for the per-posture × per-property-class breakdown and the declaration shape (`[sources.memory_abstraction]`) that records the choice in `verify.toml`.

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

## Memory soundness matrix

> Source of truth: [`MemoryAbstractionPosture`](../crates/mununu-core/src/verify/config.rs) (verify-framework declaration), [`docs/design/black-box-modules.md`](design/black-box-modules.md) (chaotic-stub foundations, Doc A §A.4), [`docs/design/hw-sw-codesign-extraction.md`](design/hw-sw-codesign-extraction.md) (HW/SW codesign chaotic-stub formulation, Doc C §C.5) — surface: CLI+API+UI (declared in `verify.toml`, consumed by the orchestrator and surfaced in summaries).

Memory abstraction is the single biggest soundness-relevant choice in any non-trivial verification target. This section defines the four canonical postures, how they interact with fence semantics, and which property classes each posture supports soundly. The choice is recorded in `verify.toml` via the `[sources.memory_abstraction]` block (see [Declaring the posture](#declaring-the-posture-in-verifytoml) below) so reviewers can audit the abstraction without re-reading the CTXDSL.

### The four postures

| `kind` | What is modelled | What is dropped | When to reach for it |
|---|---|---|---|
| `chaotic` | Self-loop on every memory label; no per-address state | All address discrimination; all value information; all ordering | Smoke test that the surrounding system terminates / does not deadlock regardless of memory behaviour. Always sound for safety; **never sound for any property that depends on what memory returns**. |
| `tracked_addresses` | Per-address presence/absence in a small declared set; chaotic over the rest | Per-address *values*; ordering across addresses unless fences enforce it | Safety properties that read **which** address was last touched but not **what value** it holds — e.g. "the watchdog kick happens before the deadline register write". |
| `tracked_with_values` | Per-tracked-address state from a declared symbol set (`stale`/`fresh`, `initial`/`written_by_X`/…); chaotic over untracked | Concrete numeric values within a symbol class; behaviour of untracked addresses | The default for memory-correctness properties — "did the microprogram's write to X become observable before the load on the other core?". The symbol set is the verifier's choice of equivalence class. |
| `full_concrete` | Every modelled address carries its concrete integer value | Nothing (within the declared address range) | Only for tiny address ranges (a handful of registers) where the property genuinely depends on numeric values — e.g. a checksum register accumulating a known sequence. State-space cost is exponential in the value width. |

Posture choice is **monotone in expressivity** from `chaotic` to `full_concrete`: each strictly extends what the model can distinguish, at strictly higher state-space cost.

### The three fence semantics

`fence_semantics`, when set on a posture, declares how a microcode `fence` op or an architectural barrier is interpreted in the model. The choice is **independent of `kind`** — any posture can pair with any fence semantics, but some combinations are pointless (chaotic + rvwmo gains nothing).

| `fence_semantics` | What the fence enforces | What it preserves | Soundness caveat |
|---|---|---|---|
| `global_barrier` | All pending writes globally ordered before all subsequent reads, across every source | "After fence, every tracked write is observable to every source" | **Strictly stronger than any real architecture**; sound *as a model assumption* but the user must justify the gap to RVWMO / TSO / SC if claims transfer to real hardware. |
| `release_acquire` | Per-source release-store and acquire-load semantics; happens-before chains via fences | Standard acquire/release reasoning ("if a release-store on core 0 is observed by an acquire-load on core 1, every prior write on core 0 is observable") | Sound for properties expressible in the release/acquire fragment; loses behaviours from non-RA atomics. |
| `rvwmo` | RISC-V Weak Memory Order, modelled per the [public spec](https://github.com/riscv/riscv-isa-manual) | Whatever the embedded RVWMO encoding preserves | **Not implemented in mununu today.** Declaring `rvwmo` is currently an **aspirational** annotation — the orchestrator does not yet enforce RVWMO semantics. The declaration is honoured by `mununu memory check` (when shipped) as a signal that the user understands the gap; it does not change verdicts. |

### Posture × property class — soundness matrix

The **claim transfer direction** is from model verdict → real system. ✓ = transfer is sound (within the modeled scope of declared tracked addresses). ⚠ = transfer is sound only with caveats spelt out below the table. ✗ = transfer is not sound — the verdict is true of the model but says nothing about the real system.

| Property class | `chaotic` | `tracked_addresses` | `tracked_with_values` | `full_concrete` |
|---|---|---|---|---|
| **Safety, no-memory mention** (e.g. "no double-DVFS") | ✓ | ✓ | ✓ | ✓ |
| **Safety, references tracked memory presence** (e.g. "address X written before Y") | ✗ | ✓ | ✓ | ✓ |
| **Safety, references tracked memory values** (e.g. "if X contains `fresh`, then …") | ✗ | ✗ | ✓ | ✓ |
| **Safety, references untracked memory** (any property that reads an address outside `tracked`) | ✗ | ✗ | ✗ | ✗ (use full_concrete only over the *declared* address range) |
| **Liveness, no-memory mention** (e.g. "every request eventually serviced") | ⚠ | ⚠ | ⚠ | ⚠ |
| **Liveness, references tracked memory** (e.g. "every write eventually observable") | ✗ | ⚠ | ⚠ | ✓ |

**Caveats.**

- **All liveness rows carry ⚠** because mununu's composition is asynchronous by default and chaotic / over-approximated memory **admits noop loops**. Without an explicit fairness constraint, the model can satisfy "eventually X" only by demonstrating the existence of a fair execution. Liveness verdicts are honest about the model but pessimistic for the real system: a real bus enforces forward progress that the chaotic-stub model does not. Use `tracked_with_values` + `release_acquire` (or `global_barrier`) + a fairness annotation on the bus source before drawing liveness conclusions.
- **The "references untracked memory" row is uniformly ✗** by construction. Reaching that row means the user named an address that is not in `tracked`. The orchestrator does not detect this today — `mununu memory check` (B2b, pending) will surface the mismatch.
- **`tracked_addresses` + value-mention property** is the most common authoring mistake: the property formula refers to a per-address value class (`mem.x.fresh`) but the posture does not encode values. The verdict on the model is meaningless. `mununu memory check` will flag this.
- **`full_concrete` + large value width** is **not** a soundness problem but a tractability one: the state space grows as `(value-domain-size)^(number-of-tracked-addresses)`. Stay under a few thousand combined states unless the verifier has memory budget for millions.

### Posture × fence-semantics — preservation matrix

For a memory-order property (e.g. "if core 0 stores X then fences, the store is visible to core 1's subsequent load"):

| Posture | `global_barrier` | `release_acquire` | `rvwmo` |
|---|---|---|---|
| `chaotic` | n/a (chaotic posture has no ordering to enforce) | n/a | n/a |
| `tracked_addresses` | Ordering enforced on presence-of-write only | Ordering enforced on per-source release/acquire labels | Aspirational; not enforced |
| `tracked_with_values` | Ordering enforced on value-class transitions across addresses | Per-source acquire/release ordering on value-class transitions | Aspirational; not enforced |
| `full_concrete` | Strongest possible ordering; equivalent to SC | Per-source acquire/release with concrete values | Aspirational; not enforced |

The honest path for v1 multicore verification: pair `tracked_with_values` with `release_acquire`, and **declare the gap to RVWMO in the `notes` field** of `[sources.memory_abstraction]`. The verdict is sound for what the model encodes; transfer to a real RISC-V system requires either a sound abstraction argument (e.g. "this property is in the SC subset that RVWMO preserves") or an external memory-model checker (Herd / RMEM) integrated as a future adapter.

### Declaring the posture in `verify.toml`

> Source of truth: [`MemoryAbstractionPosture`](../crates/mununu-core/src/verify/config.rs) — surface: CLI+API+UI.

```toml
[[sources]]
id = "memory"
adapter = "ctxdsl"
files = ["memory/shared.ctxdsl"]

[sources.memory_abstraction]
kind             = "tracked_with_values"
tracked          = ["X", "Y"]
value_symbol_set = ["stale", "fresh"]
fence_semantics  = "release_acquire"
notes            = "Sound for the SC subset; RVWMO gap analysed in design/rvwmo-gap.md."
```

The block is optional — omitting it is legacy-safe and equivalent to declaring `chaotic` posture. The four validator rules enforced at parse time:

1. `kind` must be one of `chaotic`, `tracked_addresses`, `tracked_with_values`, `full_concrete`.
2. `fence_semantics`, when set, must be one of `global_barrier`, `release_acquire`, `rvwmo`.
3. Non-empty `tracked` requires `kind = tracked_addresses` or `tracked_with_values`.
4. Non-empty `value_symbol_set` requires `kind = tracked_with_values`.

`mununu memory check` (B2b, pending) extends these to property-level cross-checks: every property formula mentioning `<source>.<address>.<value>` must reference a `(<address>, <value>)` pair declared in the source's posture block.

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
- **Cover weak-memory-model semantics** (RVWMO, TSO, sequential consistency) **as an enforcement layer**. The [memory soundness matrix](#memory-soundness-matrix) above documents which fence semantics mununu's orchestrator enforces today (`global_barrier`, `release_acquire`) and which are aspirational (`rvwmo`). Verifying memory-order intent at the architectural level still requires either an external checker integrated as an adapter (Herd / RMEM) or a heavy abstraction (TSO / SC) that ignores weak orderings.

## See also

- [Soundness Guarantees](../CLAUDE.md#soundness-guarantees) — the load-bearing rules.
- [`docs/policies/claims-integrity.md`](policies/claims-integrity.md) — full claims-integrity policy with the abstraction-soundness procedure.
- [`docs/adapters/extraction.md`](adapters/extraction.md) — `.espec.json` extraction adapter, mode filtering, property templates.
- [`docs/synthesis.md`](synthesis.md) — `ControllerMode`, signature-based extraction, Skolem-paradigm rules.
- [`wiki/Verify-Project-Flow.md`](../wiki/Verify-Project-Flow.md) — the verify framework that consumes all of the above.
