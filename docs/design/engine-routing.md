# Engine routing — which engine decides which property, and why

> Status: reference. The map from a verification request to the engine that actually
> decides it, plus the soundness class each engine covers. Written for the agent/CI
> differentiation program (P0.1) so investment targets the *defensible* paths.

mununu has **two distinct portfolios** and one exact oracle. Getting a property to the
right one is the whole game: safety at scale wants the word-level reachability engines;
branching (νμ) properties can only go through the 3-valued engines.

## The engines

| Engine | Decides | Soundness | Scale ceiling |
|---|---|---|---|
| **exact-symbolic** (BDD bit-blast) | full modal-μ incl. νμ, at the *initial state* | definite (exact, no abstraction) | ~40-bit cone (enumeration) |
| **cube CEGAR** (+ `smt-hyper-must`) | full modal-μ incl. νμ, over a **predicate abstraction** | Bruns–Godefroid 3-valued (definite verdicts transfer; ⊥ ⇒ refine) | predicate-count bound; needs the right predicates |
| **reach portfolio** (exact ⊕ native BMC/k-induction ⊕ SPACER ⊕ btormc ⊕ Pono) | **`bad`-reachability only** (safety) | each member sound; first-definite wins; disagreement ⇒ contradiction alarm | word-level, **no bit cap** (native/SPACER/btormc/Pono) |

> Source of truth: [`exact_symbolic_verdict`](../../crates/mununu-core/src/adapter/btor2/symbolic_bitblast.rs#L2095) · [`cegar_refine_loop`](../../crates/mununu-core/src/adapter/btor2/cegar.rs) · [`decide_reach_portfolio_parallel`](../../crates/mununu-core/src/adapter/reach_portfolio.rs#L202) — surface: (CLI+API+UI)

## Per-surface routing

| Surface / property | Path → engine | Predicates? |
|---|---|---|
| `btor2/sv verify` (safety) | `decide_reach_portfolio` → **reach portfolio** | none |
| `btor2/sv verify-liveness` (`AG(a→AF b)`) | Biere l2s → `bad`-monitor → **reach portfolio** | none |
| `btor2/sv verify-recoverability` (`AG EF good`) | `exact_symbolic_verdict` → **exact-symbolic** | **none** |
| `btor2 check-fsm` (auto `AG EF (reg==idle)` per FSM reg) | `fsm_recoverability_scan` → **exact-symbolic** per register | **none** (idle auto-derived from init / reset-mux) |
| `sv verify-auto` (default `portfolio-sequential`) | **exact-symbolic → symbolic → explicit**, early-exit | exact leg none; cube legs **auto-seeded** |
| `sv verify-auto` safety ⊥ | escalate → reduce → **reach portfolio** | none |
| `btor2 cegar` (explicit) | `cegar_refine_loop` → **cube** | hand-`--predicate` unless auto-seeded upstream |

> Source of truth: [`engine_selection`](../../crates/mununu-core/src/adapter/slang/verify_auto.rs#L587) + [`verify_auto_portfolio`](../../crates/mununu-core/src/adapter/slang/verify_auto.rs#L1329) (the 3-engine sequential/parallel portfolio) — surface: CLI+API.

**The predicate-avoidance rule (P0.2), confirmed by the 2026-07-08 csrng spike:**

1. `verify-auto`'s default (`portfolio-sequential`) tries **exact-symbolic first** — which needs **no predicates** and, on a real OpenTitan FSM, decides recoverability directly (the FSM cone fits under the 40-bit cap).
2. Only when exact-symbolic is **over-cap** does it fall to the cube legs, which **auto-seed** predicates from the formula's atoms (`seed_from_formula`) — still no hand-written predicates.
3. Hand-written predicates / `config_values` / `counter_bounds` are the **residual** — needed only for the cube's ⊥ ceiling (combinational-input FSM control signals whose cone explodes). That residual is the target of P0.2's follow-up (auto-derive the missing concretizations).

So: **hand-written predicates are already the exception, not the rule** — the default path is auto.

## Defensibility (which paths to lead with)

- **`AG EF` recoverability via exact-symbolic / cube+hyper-must — DEFENSIBLE.** A branching νμ property SVA/LTL cannot express, decided soundly, on real RTL, with no predicates at FSM scale. No mainstream tool offers it. **This is the wedge — invest here (P1).** The first P1 slice ships `btor2 check-fsm` — the zero-input auto-scan that discovers FSM registers, derives idle from init / reset-mux, and reports unrecoverable traps (validated end-to-end on the real csrng `state_q`, idle=55 auto-derived).
- **Response-liveness via l2s — partly defensible.** Sound concrete reduction, but liveness-to-safety is known art; keep, don't lead.
- **Safety via the reach portfolio — TABLE-STAKES.** SymbiYosys/commercial do BMC/PDR safety at scale, often better. Keep for completeness + the ⊥ escalation; don't position as the differentiator.

The investment thesis that falls out: **branching properties on the FSM cone** (where mununu is unique *and* under the cap *and* predicate-free) is the defensible core — exactly the P1 auto-property-synthesis + bug-finder target.
