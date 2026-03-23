# CTXDSL Language Reference

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change. We welcome feedback and bug reports via [GitHub Issues](https://github.com/vscorza/mununu/issues).

CTXDSL is Mununu's domain-specific language for defining automata, compositions, properties, and controllers in a single file. This page is the complete syntax reference.

## File Structure

Every `.ctxdsl` file has one top-level `context` block:

```
context my_system {
    alphabet { ... }
    automata { ... }
    composition { ... }
    mu_formulas { ... }
    controllers { ... }
}
```

All sections are optional. Order does not matter.

## Alphabet

The `alphabet` block declares the signal/event labels used across automata. Labels declared here are available to all automata in the context.

```
alphabet {
    label req_assert;
    label req_deassert;
    label ack_assert;
    label ack_deassert;
}
```

You can also give a label a display name (used in graph visualization):

```
alphabet {
    label sclk = "SCLK";
}
```

Labels that appear only inside an automaton's `transitions` or `controllable` block are implicitly added to the alphabet -- you do not need to list them in `alphabet` unless you want to set a display name or make dependencies explicit.

## Constants and Ranges

Constants and ranges are declared at the context level, before or after automata.

```
const MAX_RETRY = 3;
range counter = 0 ..= MAX_RETRY;
```

Constants are `i64` values. Ranges define inclusive integer intervals and can reference constants in their bounds.

## Automaton

An automaton is a labeled transition system with states, transitions, and optional controllability and variable declarations.

```
automata {
    automaton SpiMaster {
        controllable { ... }
        variables { ... }
        states { ... }
        transitions { ... }
    }
}
```

### States

One state must be marked `initial`. State names are identifiers that double as atomic propositions in mu-calculus formulas.

```
states {
    state Idle initial;
    state Selected;
    state Shifting;
    state Complete;
}
```

### Controllable

The `controllable` block lists labels that the controller is allowed to enable or disable. Labels not listed here are treated as **uncontrollable** (environment actions that the controller cannot prevent).

```
controllable {
    label ack_assert;
    label ack_deassert;
    label latency_tick;
}
```

This distinction is central to controller synthesis. See the [Mu-Calculus Reference](Mu-Calculus-Reference) for controllability-aware modalities.

### Variables

Automata can declare typed state variables. Supported types are `i64` and `bool`.

```
variables {
    count: i64 = 0;
    ready: bool = false;
}
```

Variables are used in guard expressions and updated via effects on transitions.

### Transitions

Transitions connect states and carry one or more labels.

**Single label:**

```
transition Idle -> WaitAck on label req_assert;
```

**Multi-label** -- the transition fires when all listed labels occur simultaneously:

```
transition Idle -> Active on label req_assert, label ack_assert;
```

**Epsilon** -- an internal (silent) transition with no observable label:

```
transition Buffered -> Ready on epsilon;
```

**Guard** -- a Boolean condition on variables that must hold for the transition to be enabled:

```
transition Active -> Done on label tick
    guard count >= 3;
```

**Effects** -- variable updates that execute when the transition fires:

```
transition Active -> Active on label tick
    effects { count = count + 1; };
```

Guards and effects can be combined:

```
transition Active -> Done on label tick
    guard count >= 3
    effects { count = 0; };
```

## Composition

Compositions combine automata into larger systems. See the dedicated [Composition](Composition) page for full details.

```
composition {
    synchronous pipeline {
        members [Producer, Consumer];
    }

    asynchronous sensor_network {
        members [SensorA, SensorB, Monitor];
    }
}
```

## Mu-Calculus Formulas

Formulas are named mu-calculus expressions evaluated over a specific automaton or composition.

```
mu_formulas {
    formula safety_invariant {
        over SpiMaster;
        body = nu X. ([] X);
    }

    formula transfer_completes {
        over SpiMaster;
        body = mu X. (Complete || <> X);
    }
}
```

See the [Mu-Calculus Reference](Mu-Calculus-Reference) for the full expression language.

## Controllers

A controller block synthesizes a maximally permissive controller that restricts only controllable labels to enforce a given formula.

```
controllers {
    controller safe_handshake {
        source Handshake;
        satisfying safety_invariant;
    }
}
```

- `source` -- the automaton (or composition) to synthesize over.
- `satisfying` -- the mu-calculus formula the controller must enforce.

The synthesized controller preserves all uncontrollable transitions. If the property is unrealizable (no controller can enforce it), Mununu reports the result and can generate counterstrategy diagnostics via the CLI (`--counterexample`).

## Comments

Line comments start with `//`:

```
// This is a comment
transition Idle -> Active on label start; // inline comment
```
