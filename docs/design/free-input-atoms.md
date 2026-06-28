# Admitting free-input (and combinational-signal) atoms in verify-auto — H.B design

> Status: planning

This is the design + feasibility investigation for **H.B** of the Track-H
verify-auto arc: making SVA atoms over **primary inputs** and **combinational
outputs** verifiable, not SKIPPED. It is the dominant remaining blocker for a
*definite* automated verdict on real RTL — H.A (state-alias binding) and H.C
(flop-primitive survival) are shipped, and the measured residue on both
real-OpenTitan fixtures is exactly this: every still-SKIPPED property SKIPs on
an input/config/combinational atom (`cfg_enable_i`, `trigger_i`, `cnt_clr`,
`main_sm_err_o`).

## 1. The atom taxonomy

A translated mu-calculus atom references one or more BTOR2 signals. Four cases,
by how the signal's value is determined:

| # | Kind | Example | Determined by | Status |
|---|---|---|---|---|
| 1 | State register | `state_q == 5` | the cube state | ✅ bound directly |
| 2 | Value-alias of state | `state_q` (a `uext`-0 of the reset mux) | the cube state | ✅ H.A (`resolve_state_alias`) |
| 3 | **Free input** | `cfg_enable_i`, `trigger_i` | the environment, per cycle | ❌ **H.B** |
| 4 | **Combinational function** of state (+input) | `main_sm_err_o = f(state_q)` | state (+ inputs) | ❌ **H.B** |

The cube abstraction is **state-based**: each abstract state is a valuation of
predicates `{p_0 … p_{n-1}}`, and each concrete state gives every state predicate
a definite truth value. Cases 3 and 4 break that: a free input has no value at
the registered state (the environment chooses it *on the transition*), and a
combinational signal's value is a *function*, not equal to any single register.

The shipped strict resolver (`parser::resolve_state_alias`) deliberately
**rejects** cases 3 and 4 — binding `main_sm_err_o` to the state register it is a
function of produced a *spurious* VIOLATED in an intermediate run; the rejection
is the soundness gate. H.B is how we admit them *correctly*.

## 2. Approach A — a free input is a cube dimension

Model the input value chosen at cycle *t* as an extra **dimension of the cube at
cycle t**. Add `cfg_enable_i == 1` as predicate `p_k`; the abstract state becomes
`(register-cube, input-cube)`, and the input dimension is **unconstrained across
transitions** (the environment may pick any value each cycle).

### Transition semantics (the SMT hook)

The may/must SMT (`adapter/btor2/smt_must_edge.rs`) already encodes the one-step
relation `transition(state, inputs, next)` with the inputs as quantified
variables (`view.inputs`; the must form is `∀ state ⊨ src. ∃ input. next ⊨ tgt`).
Today `build_register_nid_map` maps **only `SignalKind::State` symbols** → NIDs,
so an input atom's register is absent → the per-target check returns
`Unknown` → the property SKIPs. The change:

- **Map inputs too.** Extend `build_register_nid_map` to also map
  `SignalKind::Input` symbols to their NID, behind a flag so the state-only
  callers are unchanged.
- **Source cube → pin the current input.** When a predicate's register is an
  input, its *source* constraint (`is_next = false`) pins the **current-cycle
  input BV** to the cube's value. This is exact: the transition out of cube
  `(s, cfg_enable_i = 0)` is driven by `cfg_enable_i = 0`.
- **Target cube → the input dimension is FREE.** The *next-cycle* input is not a
  variable in the one-step relation, so an input predicate has no `is_next` BV.
  Rather than return `Unknown`, the edge enumeration treats the target's input
  dimension as **unconstrained**: for every register-successor `s'` reachable
  from `(s, ai)`, emit may-edges to **both** input-dimension flavours of the
  target (`(s', a'=0)` and `(s', a'=1)`). That is the faithful "environment picks
  any next input" model.
- **Initial cube set → all input-dimension values are initial.** The environment
  is free at cycle 0, so the initial cube set is the product of the design's reset
  register cube with *every* input-dimension combination.

### Soundness

The may-relation includes **all** environment choices (source-pinned over each
value, target-free, all-initial) — it is the standard over-approximation of "for
all input sequences." Definite verdicts (`KleeneT` / `KleeneF`) therefore
transfer to the concrete design at every alternation depth (Bruns–Godefroid
CONCUR 2000), exactly as for state predicates. The must form keeps the pinned
input fixed by the cube and leaves the *other* input bits existential — a sound
under-approximation. No new soundness regime over the shipped cube path.

### Cost

Each admitted input atom adds one cube dimension → the cube space grows by `2×`
per input atom. Real SVA antecedents reference a *handful* of inputs per property
(sysrst: ≤ 3; csrng: 0 input atoms after reset-gating), so the blow-up is small
and already bounded by `PredicateCubeLiftOptions::max_cube_count`. When an atom
set would exceed the cap, the property SKIPs with the existing cap diagnostic —
no silent truncation.

### Integration assessment ("plays well with existing systems?")

| System | Impact |
|---|---|
| `smt_must_edge` may/must | Inputs are already quantified; add input→NID mapping + source-pin/target-free. **Localized**, the core change. |
| `predicate_cube_lift` edge enumeration | Classify predicates (state vs input); expand target edges over input dimensions; product the initial set. **Moderate** — the cube-construction change. |
| `evaluate_tri` / `parity_game_3v` evaluator | **Unchanged.** Input dimensions are just more cube bits with a 3-valued verdict; the evaluator is generic over cube cells. |
| CEGAR (`cegar.rs`) | **Unaffected** in shape — input dims are predicates like any other; refinement still adds register predicates. |
| `config_values` / `r_s8_encoder` | Orthogonal but precedent-setting: that path admits a **held** (init-fixed) config constant via initial-cube expansion (`hyper_must_initial_cubes`). A *free* input differs (it varies every cycle), so it needs the per-cycle target-free treatment above, not just the initial expansion. The two compose (a config bit pinned at init + free inputs each cycle). |
| Reset-gating (shipped) | Synergistic: the reset input is *pinned* (not a free dimension), so it never inflates the cube; only the genuine environment inputs become dimensions. |

## 3. Approach B — SMT-quantified, no cube dimension

Keep only state predicates in the cube; when the formula references an input
atom, resolve its truth *inside the modal step* by quantifying the input in the
SMT (e.g. a `[a]φ`-style "for all inputs …").

**Rejected.** Our SVA→mu translation emits **propositional** atoms (`a`), not
modal labels, so an input atom would have no definite value per cube cell — the
3-valued evaluator (`evaluate_tri`, the parity-game solver) assumes each cube
cell gives every atom a definite trit. Making the input atom's truth
context-dependent (on the specific transition being explored) would entangle
input semantics into the **evaluator**, which is currently generic over cube
cells and shared with the non-SVA verify path. That is a larger, riskier change
to a load-bearing, soundness-critical component, for no expressiveness gain over
Approach A. Approach A confines the change to the **abstraction** (lift + SMT),
leaving the evaluator untouched.

## 4. Combinational outputs (case 4) unify under Approach A

A combinational output (`main_sm_err_o = f(state, inputs)`) is a *determined*
function, not a free choice. Once (a) the state is a cube via the register
predicates and (b) any referenced inputs are cube dimensions (Approach A), the
output's value **per cube** is computed by the same predicate-image SMT that
already encodes the design — `build_register_nid_map` need only also resolve the
output **signal node** (not just state symbols), and `build_pred_constraint`
constrains that node in terms of state + input BVs. So case 4 is admitted by the
*same* mechanism as case 3 (map the signal's NID; the SMT determines its value),
with **no** extra cube dimension when the output depends only on already-tracked
state/inputs. Pure-state outputs (no input dependence) become determined cube
predicates directly.

## 5. The `|->` antecedent shortcut (an optimization, not the general path)

For an **overlapping** implication `a |-> b` where `a` is a free input and `b` a
state condition, `AG(a → b)` ≡ `AG b`: the environment can make `a` true at every
reachable state, so the obligation reduces to `b` everywhere. A free-input
conjunct in an overlapping antecedent can thus be replaced by `true` (sound and
exact). This is cheap and handles a slice of real properties **but does not
generalize**: it is wrong for **non-overlapping** `a |=> b` (the input *selects*
which transition's next-state the consequent constrains — sysrst's
`!cfg_enable_i |=> state==Idle` genuinely needs the input as a transition-driving
cube dimension), and it does not help free inputs in other positions. Treat it as
a translator-level optimization layered on Approach A, not a substitute.

## 6. Recommendation

Implement **Approach A**: free inputs become free cube dimensions, realized in the
may/must SMT as **source-pinned / target-free** input BVs with an
**all-input-initial** cube set, and unify **combinational outputs** by mapping
their signal node so the predicate-image determines their value. The evaluator and
CEGAR are untouched; the change is confined to `build_register_nid_map` +
`build_pred_constraint` (SMT) and the cube edge/initial enumeration
(`predicate_cube_lift`). Layer the §5 `|->` shortcut in the translator as a cheap
fast path.

This is the path to the first **definite** verify-auto verdict on real RTL: with
H.A + H.C shipped, sysrst's `!cfg_enable_i |=> state==Idle` and csrng's
`state==Error |-> main_sm_err_o` are exactly the case-3 / case-4 properties this
unblocks.

## 7. Implementation status

**Increment 1 — SMT-layer foundation (shipped, #176).**
[`build_register_nid_map_with_inputs`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs)
maps input symbols to their NID alongside state (state precedence on collision),
and [`build_pred_constraint`](../../crates/mununu-core/src/adapter/btor2/smt_must_edge.rs)
realises the source-pin / target-free shape for an input predicate (the source
pins the current-cycle input BV; the target returns `true`, leaving the next-cycle
input free). Dormant until increment 2 wired a caller.

**Increment 2 — cube-layer wiring (shipped).** Three seams:

1. The STS-IR SMT seam (`BtorSts::{may,must,hyper_must}_edges` in
   [`sts_ir.rs`](../../crates/mununu-core/src/adapter/sts_ir.rs)) consumes
   `build_register_nid_map_with_inputs`. Behaviour-preserving for every existing
   state-only predicate set (input map entries sit unused).
2. The verify-auto seeder
   ([`seed_from_formula`](../../crates/mununu-core/src/adapter/slang/verify_auto.rs))
   admits a **simple** `input == value` / bare-input atom as a free cube
   dimension (recorded in `Seeded::input_registers`), and forces
   `MayEdgeInference::SmtAllPairs` when any free input is present (the sampling
   may-path is state-oriented and cannot realise source-pin / target-free).
3. The verify-auto init/verdict read
   ([`free_input_init_cubes`](../../crates/mununu-core/src/adapter/slang/verify_auto.rs))
   leaves free-input dimensions unpinned at init and reads the verdict across
   **all** initial input flavours, combined conjunctively (a violation under some
   env input ⇒ Violated; an undecided flavour ⇒ Unknown; else Holds).

**Validated (mununu-sva docker image).**

- **sysrst_ctrl_detect** — `sva_0` reaches a sound **HOLDS** (pre-H.B: every
  translated property SKIPPED; this is the first real-OpenTitan IO-antecedent
  verdict). No spurious VIOLATED.
- **csrng_main_sm** — unchanged (its atoms are state + a combinational output;
  no free inputs), a clean no-regression anchor.
- **Reset-as-free-input** — an un-gated `disable iff (!rst_n) state != 3` property
  now HOLDS soundly without reset-gating: `rst_n` (a primary input) is admitted as
  a free dimension and the kept `!rst_n` disjunct makes reset cycles vacuous.

**Still skipped (the case-4 follow-up).** A predicate over a pure **combinational
output** — `is_input` is `false` for it (not a BTOR2 `Node::Input`) and the strict
alias resolver rejects it (not a value-alias of state) — is honestly SKIPPED, not
given a misleading verdict. sysrst's `trigger_active` / `cnt_clr` /
`event_detected_*` and csrng's `main_sm_err_o` are this case. Admitting them needs
the §4 signal-node mapping (map the output's BTOR2 node so the predicate-image
determines its value); not yet wired. An input inside a *compound* predicate is
also skipped (the compound SMT branch reads `state_curr`/`state_next` BVs, not
`view.inputs`).
