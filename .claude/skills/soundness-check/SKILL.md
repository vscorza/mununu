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

## Search Patterns

Use these grep patterns to find potential violations:

```
# Missing SOUNDNESS comments near None fallbacks
eval_expr.*None|unwrap_or\(|unwrap_or_default

# Skip patterns without warnings
skip_to_semicolon|skip_braces|skip_unknown

# Guard failures
guard.*None|eval_guard.*false
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
