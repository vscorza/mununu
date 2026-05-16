# TLSF / AIGER Adapter Encoding

> Source of truth: [`crates/mununu-core/src/adapter/emit.rs`](../../crates/mununu-core/src/adapter/emit.rs) (turn-based emit path) and the TLSF / AIGER adapter modules under [`crates/mununu-core/src/adapter/tlsf/`](../../crates/mununu-core/src/adapter/tlsf/) and [`crates/mununu-core/src/adapter/aiger/`](../../crates/mununu-core/src/adapter/aiger/) — surface: CLI+API+UI

The TLSF and AIGER adapters use a **turn-based compound-label encoding** rather than the older per-signal `set_` / `clr_` labels. This page explains the encoding and the LTL-to-mu-calculus translation that depends on it.

## Compound labels

- `env_{bits}` — uncontrollable, one per input assignment.
- `ctrl_{bits}` — controllable, one per output assignment.

These replace the per-signal `set_` / `clr_` labels used by older adapters.

## Turn bit

States include a **turn bit** (LSB):

- `turn=0` = env's turn (round boundary).
- `turn=1` = ctrl's turn (intermediate).

State count is `2^(N+1)` where N = inputs + outputs.

## Turn-based routing

- From `turn=0` states, only **env** transitions exist (→ `turn=1`).
- From `turn=1` states, only **ctrl** transitions exist (→ `turn=0`).

This ensures the evaluator's Skolem paradigm naturally alternates ∀ env / ∃ ctrl. See [`../synthesis.md`](../synthesis.md) for the broader Skolem rule.

## Game-aware formulas

Formulas are emitted as **mu-calculus** (not LTL) with `[(ctrl=Controllable)]` modals. Propositional checks use `(turn || φ)` to skip at intermediate states — without this, formulas would be checked at intermediate states where the controller hasn't responded yet.

## LTL-to-mu-calculus translation in the emitter

`ltl_to_game_mu_inner` bypasses the standard LTL translator (which uses `[ctrl=All]`) and emits mu-calculus directly with turn guards. Key patterns (with `[c]` = `[(ctrl=Controllable)]`):

| LTL | mu-calculus |
|---|---|
| `G φ` | `ν X. ((turn ∨ φ) ∧ [c] X)` — skip check at ctrl-turn |
| `F φ` | `μ X. ((¬turn ∧ φ) ∨ [c] X)` — only count at env-turn (round boundary) |
| `X φ` | `[c] [c] φ` — two steps = one round (turn alternates each step) |

## SYNTCOMP verdict differences

`lilydemo15` and `lilydemo16` are **realizable** under our Mealy encoding (a valid alternating-grant strategy exists) but the SYNTCOMP reference says unrealizable. This is a semantic difference, not a bug — record it as such if a comparison is published.
