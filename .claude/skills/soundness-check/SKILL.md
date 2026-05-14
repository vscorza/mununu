---
name: soundness-check
description: >
  Checks adapter and Kripke builder code for soundness documentation compliance.
  Verifies that eval_expr fallbacks, guard failures, and abstraction decisions
  have nearby // SOUNDNESS: comments. Use when asked to audit soundness annotations.
---

Perform a soundness annotation audit of $ARGUMENTS (or changed files via `git diff --name-only HEAD~1 HEAD | grep '\.rs$'` if no args).

This project is a formal verification tool. Adapter code translates external formats into internal models. Every approximation decision (over-approx or under-approx) must be documented with a `// SOUNDNESS:` comment explaining the direction and impact.

## Audit Checklist

1. **eval_expr returning None/default**: Search for patterns like `eval_expr`, `evaluate`, or `eval_guard` that return `None`, `unwrap_or`, `unwrap_or_default`, or `unwrap_or_else`. Each must have a nearby `// SOUNDNESS:` comment within 3 lines explaining whether the fallback is over-approximation (allows transition) or under-approximation (blocks transition).

2. **Guard evaluation failures**: Search for `is_some_and`, `is_none_or`, `guard.*None`, or conditional paths where a guard cannot be evaluated. Document whether the fallback path is conservative.

3. **State abstraction defaults**: Search for `unwrap_or(` patterns in adapter/kripke code. Each default value (like `DEFAULT_COUNTER_BOUND`) must have a comment explaining why the default is sound for safety properties.

4. **Skip patterns in parsers**: Search for `skip_to_semicolon`, `skip_braces`, `skip_unknown`, or `advance()` in fallback/default match arms. Each should emit an `AdapterWarning` or have a `// SOUNDNESS:` comment noting what's being dropped.

5. **Nondeterministic transitions (havoc)**: Where a transition has multiple targets for the same label, verify there's a comment explaining this is intentional over-approximation.

6. **Capability under-use**: in adapter / emitter code, flag patterns that re-encode features the CLTS / CTXDSL layer already supports — emitting parallel single-label transitions where a multi-label edge fits, suffixing state names with predicate values where state predicates fit, folding controllability into label-name prefixes where `LabelControllability` fits, or emitting LTL where a `[(req_next = {...})]` guard would be more direct. Cross-reference CLAUDE.md `### Adapter / Emitter Capability Use`. Severity: LOW — a maintenance / review hint, not a soundness bug. If the under-use is intentional (e.g. AIGER inputs are single-bit), require a one-line comment in the adapter explaining why.

7. **Black-box / contract chaotic-stub discipline**: when an adapter encounters a module without a body (yosys `(* blackbox *)`, custom-SV instantiation without a sidecar entry, software call falling through to `CallEffect::Unknown`), it must call `contract::discover::build_blackbox_sidecars` (or equivalent) AND emit a structured `tracing::warn!` diagnostic naming the module, the gap kind, the labels affected, and the soundness consequence. Silently producing transitions on the un-bodied module (or worse, omitting it entirely from the IR) is a soundness violation — the user must see every gap. Cross-reference CLAUDE.md `### Black-Box Modules and Contracts`. Search patterns: any new code path under `crates/mununu-core/src/adapter/` that handles a black-box-like construct without invoking the contract discovery helper or without emitting a warning. Severity: HIGH (silent loss of behaviour) or MEDIUM (loud loss but no contract sidecar emitted).

8. **Codesign coupling soundness**: when `crates/mununu-core/src/codesign/coupling.rs` or `compose.rs` emits a composition spec for a HW/SW codesign workflow, the `CompositionKind` must be `Asynchronous`. Synchronous one-step rendezvous is unsound for racy access (Doc C §C.5 — bus arbitration is non-deterministic). Any code path that emits `Synchronous` or `Superset` for a codesign composition is a soundness violation. Cross-reference: the `coupling_fragment_uses_asynchronous_composition` test fixes this. Severity: HIGH.

9. **Discharge graph circular acceptance**: when `crates/mununu-core/src/contract/discharge.rs` finds a non-trivial SCC, the verdict must NOT be `Acyclic` and must NOT be silently dropped. Acceptable verdicts are `CircularWithRankWitness` (lightweight McMillan check passed), `Circular` (no rank witness — user-approved sign-off required), or `PotentiallyCircular` (some clause unresolved — conservative). Any change to this module that lets a cyclic discharge fall through to a positive verdict without one of these tags is a soundness violation. Severity: HIGH.

## Search Patterns

Use these grep patterns to find potential violations:

```
# Missing SOUNDNESS comments near None fallbacks
eval_expr.*None|unwrap_or\(|unwrap_or_default

# Skip patterns without warnings
skip_to_semicolon|skip_braces|skip_unknown

# Guard failures
guard.*None|eval_guard.*false

# Capability under-use (item 6) — adapter files only
labels:\s*vec!\[[^,\]]+\]      # single-element label vectors
controllable_labels:\s*vec!\[\] # hardcoded empty controllability
```

## Output Format

For each finding, report:
- **File:Line** — exact location
- **Pattern** — which checklist item it violates
- **Severity** — HIGH (no comment at all), MEDIUM (comment exists but doesn't state direction), LOW (comment exists but could be clearer)
- **Suggested fix** — what the SOUNDNESS comment should say

## Example Compliant Code

```rust
// SOUNDNESS: over-approx — when guard cannot be evaluated, allow
// the transition. This admits more behaviors than reality, which is
// conservative for safety properties but may produce spurious liveness.
let guard_holds = eval_guard(&expr).unwrap_or(true);
```

## Example Non-Compliant Code

```rust
// BAD: no soundness annotation
let bound = field.bound.unwrap_or(3);

// BAD: comment exists but doesn't state direction
// Default to 3 if not specified
let bound = field.bound.unwrap_or(3);
```
