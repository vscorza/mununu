# Counterstrategy

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change. We welcome feedback and bug reports via [GitHub Issues](https://github.com/vscorza/mununu/issues).

## What is a Counterstrategy?

When a property fails to hold -- when synthesis reports "unrealizable" -- the **environment has a winning strategy**. This counterstrategy is a concrete demonstration of how the environment can force the system into states where the property is violated, no matter what the controller does.

A counterstrategy consists of:
- A **winning region** for the environment: the set of states from which the environment can force violation
- **Transition choices**: for each state in the winning region, the uncontrollable actions the environment can take to stay within (or reach) the winning region

Understanding the counterstrategy is essential for debugging unrealizable specifications. It tells you exactly *why* the controller cannot win and *how* the environment exploits the design.

## Formula Inversion

Mununu computes the counterstrategy by **inverting** the original formula. Inversion applies De Morgan's laws and fixpoint duality throughout the formula AST, producing a new formula that computes the environment's winning region -- the exact complement of the controller's winning region.

The inverted formula answers: "From which states can the environment force the original property to be violated?"

## Duality Table

The inversion rules applied by Mununu:

| Original | Inverted |
|----------|----------|
| `A && B` | `!A \|\| !B` |
| `A \|\| B` | `!A && !B` |
| `!A` | `A` |
| `true` | `false` |
| `false` | `true` |
| `P` (predicate) | `!P` |
| `mu X. Phi(X)` | `nu X. ~Phi(~X)` |
| `nu X. Phi(X)` | `mu X. ~Phi(~X)` |
| `[] Phi` | `<> ~Phi` |
| `<> Phi` | `[] ~Phi` |
| `[ctrl=controllable] Phi` | `<ctrl=environment> ~Phi` |
| `<ctrl=environment> Phi` | `[ctrl=controllable] ~Phi` |

Key points:
- **Fixpoint duality**: `mu` (least, reachability) becomes `nu` (greatest, invariance) and vice versa. The starting point flips: mu starts empty and grows, nu starts full and shrinks.
- **Modal duality**: box `[]` (for-all-successors) becomes diamond `<>` (exists-a-successor) and vice versa.
- **Game duality**: `Controllable` becomes `Environment` and vice versa. The perspective flips -- what was the controller's choice becomes the environment's choice.

## Control::Environment Semantics

When the inverted formula contains `<ctrl=environment>`, it evaluates with **environment perspective** game semantics:

- There **exists** an uncontrollable transition leading to a state satisfying Phi, **OR**
- **All** controllable transitions lead to states satisfying Phi

This is the dual of `[ctrl=controllable]`, which requires all uncontrollable transitions to satisfy Phi AND at least one controllable transition to satisfy Phi. The environment wins if it can find any uncontrollable move that stays in its winning region, or if every controllable move the controller might try also stays in the environment's winning region.

## Example: Water Tank

Consider the water tank from Tutorial 05 with states: Empty, Filling, Full, Draining. The environment controls the `idle` label (which loops Empty back to Empty).

**Controller's formula** (controllable reachability of Full):

```
mu X. (Full || [ (ctrl = controllable) ] X)
```

This computes the set of states from which the controller can force reaching `Full` despite the environment. Result: **{Filling, Full}** -- 2 of 4 states. The controller wins from Filling (one more `fill` reaches Full) and from Full (already there). But from Empty, the environment can loop `idle` forever, and from Draining, the system reaches Empty where the environment traps it.

**Inverted formula** (environment's winning region):

```
nu X. (!Full && < (ctrl = environment) > X)
```

Result: **{Empty, Draining}** -- 2 of 4 states. This is the exact complement. From Empty, the environment can play `idle` indefinitely. From Draining, the next `drain` leads to Empty, where the environment takes over.

The two regions partition the state space: {Filling, Full} (controller wins) + {Empty, Draining} (environment wins) = all 4 states.

## Example: Traffic Light

Consider the traffic light controller with states: Green, Yellow, Red, RedWait. The environment controls `sensor_trigger` (which transitions RedWait to Green) and `timer_tick`.

**Controller's formula** -- force reaching Green:

```
mu X. (Green || [ (ctrl = controllable) ] X)
```

If the controller only controls `timer_expire`, it cannot force the system from RedWait to Green -- that requires `sensor_trigger`, which the environment controls. The winning region for the controller is just {Green}. The environment's winning region is {Yellow, Red, RedWait}.

**Inverted formula** (environment prevents Green):

```
nu X. (!Green && < (ctrl = environment) > X)
```

The counterstrategy shows: from RedWait, the environment can simply never trigger the sensor, keeping the system stuck in RedWait forever. From Yellow and Red, the timer eventually expires (controllable), but the system reaches RedWait where the environment takes over.

**GR(1) fix**: Add a fairness assumption. If we assume the environment eventually triggers the sensor (GF sensor), then under that assumption the controller can guarantee GF Green. This is the GR(1) pattern -- unrealizable under arbitrary environment behavior, but realizable under fair environment assumptions.

## Graph Visualization

The counterstrategy can be visualized as an automaton in **mununu-ui**. The graph shows:

- **States in the environment's winning region** highlighted
- **Transition edges** showing the environment's winning moves
- **Controllable transitions** that the controller might attempt (all leading back into the environment's winning region)

Use the `graph` command to generate visualizations:

```bash
mununu context graph spec.ctxdsl --output graph.html
```

The HTML output uses Cytoscape.js for interactive exploration. States are colored by their membership in the winning region, and transitions are labeled with their controllability status.

## Practical Use

When synthesis reports unrealizable, follow this diagnostic workflow:

1. **Enable diagnostics**: add `--counterexample --deadlock-traces` to the `synth` command
2. **Examine the winning regions**: the controller's winning region (from `eval`) and the environment's winning region (from the inverted formula) should partition the state space
3. **Trace the counterstrategy**: identify which uncontrollable actions the environment uses to escape the controller's winning region
4. **Fix the specification**: either strengthen the plant (add transitions), weaken the property, or add fairness assumptions (GR(1)) to rule out pathological environment behaviors

## See Also

- [Controller Synthesis](Controller-Synthesis.md) -- synthesis workflow and realizability
- [LTL Properties](LTL-Properties.md) -- writing temporal specifications
- [CLI Reference](CLI-Reference.md) -- diagnostic flags for the `synth` command
