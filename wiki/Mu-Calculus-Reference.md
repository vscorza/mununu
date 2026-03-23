# Mu-Calculus Reference

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change. We welcome feedback and bug reports via [GitHub Issues](https://github.com/vscorza/mununu/issues).

Mununu uses the modal mu-calculus as its core property language. This page covers every operator, explains fixpoint iteration, and lists common formula patterns you can reuse.

## Boolean Connectives

| Syntax | Meaning |
|--------|---------|
| `true` | Satisfied by every state |
| `false` | Satisfied by no state |
| `phi && psi` | Conjunction -- both must hold |
| `phi \|\| psi` | Disjunction -- at least one must hold |
| `! phi` | Negation |

Parentheses control precedence: `(phi \|\| psi) && chi`.

## State Predicates

State names declared in your automaton serve as atomic propositions. A state predicate is satisfied exactly by the state(s) with that name.

```
// True only in the Lockout state
Lockout

// True in Locked, Strike1, or Strike2
Locked || Strike1 || Strike2
```

## Fixpoint Operators

### Greatest fixpoint: `nu`

```
nu X. phi
```

Computes the **largest** set of states satisfying `phi`, where `X` is a recursion variable that stands for "the set computed so far." Iteration starts from the **full state set** and shrinks.

Use `nu` for **safety and invariance** properties -- things that must hold forever.

### Least fixpoint: `mu`

```
mu X. phi
```

Computes the **smallest** set of states satisfying `phi`. Iteration starts from the **empty set** and grows.

Use `mu` for **reachability and liveness** properties -- things that must eventually happen.

### How fixpoint iteration works

The evaluator repeatedly applies the body `phi` until the result stabilizes:

- **`mu X. phi`**: start with `X = {}` (empty). On each round, compute `phi` with the current `X`, and let the result become the new `X`. Stop when `X` no longer changes. The final `X` is the smallest set closed under `phi`.
- **`nu X. phi`**: start with `X = S` (all states). Same iteration, but shrinking. The final `X` is the largest set closed under `phi`.

Internally, state sets are backed by bitvec for efficient iteration even on large composed systems.

## Modal Operators

### Box (universal): `[] phi`

```
[] phi
```

Satisfied by a state if **all** of its successors satisfy `phi`. If a state has no successors (deadlock), `[] phi` is vacuously true.

### Diamond (existential): `<> phi`

```
<> phi
```

Satisfied by a state if **at least one** successor satisfies `phi`. If a state has no successors, `<> phi` is false.

## Labeled Modalities

You can restrict modal operators to transitions carrying specific labels.

### Labeled box

```
[ labels = { ack_assert, ack_deassert } ] phi
```

All successors reachable via `ack_assert` or `ack_deassert` transitions must satisfy `phi`. Transitions with other labels are ignored.

### Labeled diamond

```
< labels = { req_assert } > phi
```

There exists a successor reachable via a `req_assert` transition that satisfies `phi`.

## Controllability-Aware Modalities

These operators encode a two-player game between the controller and the environment. They are the key ingredient for controller synthesis.

### System box: `[ (ctrl = controllable) ] phi`

```
[ (ctrl = controllable) ] phi
```

This is the game-theoretic "can the controller ensure `phi`?" operator. It requires:

- **All uncontrollable transitions** lead to states satisfying `phi` (the environment cannot escape).
- **At least one controllable transition** leads to a state satisfying `phi` (the controller has a move).

If there are no controllable transitions, the operator degenerates to requiring all (uncontrollable) successors to satisfy `phi`.

### Environment diamond: `< (ctrl = environment) > phi`

```
< (ctrl = environment) > phi
```

There exists an uncontrollable transition leading to a state satisfying `phi`. This models the environment's ability to force a particular outcome.

## Common Patterns

These patterns come up repeatedly in hardware and protocol verification. Copy and adapt them freely.

| Pattern | Formula | Description |
|---------|---------|-------------|
| Safety invariant | `nu X. ([] X)` | All reachable states are valid (no bad state is reachable) |
| Reachability | `mu X. (target \|\| <> X)` | `target` is reachable on some path |
| Controllable reachability | `mu X. (target \|\| [(ctrl=controllable)] X)` | The controller can force reaching `target` |
| GF (infinitely often) | `nu X. (mu Y. (p \|\| <> Y)) && ([] X)` | `p` is visited infinitely often on every path |
| No deadlock | `nu X. ((<> true) && ([] X))` | Every reachable state has at least one successor |
| Conditional reachability | `(! source) \|\| (mu X. (target \|\| <> X))` | From `source`, `target` is reachable |
| Labeled reachability | `mu X. (target \|\| < labels = {a} > X)` | `target` is reachable using only `a`-transitions |

### Worked example: door lock recovery

"From the Lockout state, the Locked state is always reachable (the admin can always recover)."

```
formula can_recover {
    over DoorLock;
    body = (! Lockout) || (mu Y. (Locked || <> Y));
}
```

This reads: either we are not in Lockout, or from Lockout there exists a path to Locked.

### Worked example: controller can force return home

"Despite uncontrollable sensor events, the robot arm controller can always force a return to the Home state."

```
formula ctrl_return_home {
    over RobotArm;
    body = mu X. (Home || [ (ctrl = controllable) ] X);
}
```

This computes the set of states from which the controller has a winning strategy to reach Home, regardless of what the environment does.

## Nesting Fixpoints

You can nest `mu` inside `nu` (and vice versa) to express properties that combine safety and liveness. The alternation depth determines the complexity of the property.

```
// GF Node0: on every infinite path, Node0 is visited infinitely often
nu X. ((mu Y. (Node0 || <> Y)) && ([] X))
```

The outer `nu` ensures the property holds forever. The inner `mu` ensures Node0 is reachable from every point along the way.

## Formula Declaration in CTXDSL

Formulas are declared in the `mu_formulas` block and bound to an automaton or composition via `over`:

```
mu_formulas {
    formula no_deadlock {
        over pipeline;
        body = nu X. ((<> true) && ([] X));
    }
}
```

Evaluate from the CLI:

```bash
mununu context eval my_system.ctxdsl --formula no_deadlock --automaton pipeline
```

Synthesize a controller:

```bash
mununu context synth my_system.ctxdsl --formula no_deadlock --automaton pipeline
```
