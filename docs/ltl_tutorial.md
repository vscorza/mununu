# LTL (Linear Temporal Logic) Tutorial for Context DSL

This tutorial introduces LTL (Linear Temporal Logic) support in the Context DSL. LTL provides an intuitive syntax for expressing temporal properties that are automatically translated to μ-calculus for evaluation.

---

## Table of Contents

1. [Introduction](#introduction)
2. [Basic Syntax](#basic-syntax)
3. [Temporal Operators](#temporal-operators)
4. [Common Patterns](#common-patterns)
5. [LTL vs μ-Calculus](#ltl-vs-μ-calculus)
6. [Examples](#examples)
7. [Migration Guide](#migration-guide)

---

## Introduction

LTL (Linear Temporal Logic) is a temporal logic for reasoning about sequences of states. In the Context DSL, LTL formulas are written using the `ltl` keyword and are automatically translated to μ-calculus during realization.

### Why LTL?

- **Intuitive syntax**: LTL operators (`G`, `F`, `X`, `U`) are more readable than raw μ-calculus
- **Standard patterns**: Common verification patterns (safety, liveness, reactiveness) map naturally to LTL
- **Automatic translation**: LTL formulas are translated to μ-calculus, so existing evaluation infrastructure works unchanged

### Basic Example

```clts
mu_formulas {
    formula safety {
        over System;
        body = ltl G !deadlock;
    }
}
```

This formula states: "Globally, deadlock never occurs" (`G !deadlock`).

---

## Basic Syntax

### Syntax Marker

LTL formulas must be prefixed with the `ltl` keyword:

```clts
body = ltl <LTL formula>;
```

### Atomic Formulas

- **Predicates**: `safe`, `completed`, `error`
- **Boolean constants**: `true`, `false`

### Propositional Operators

| Operator | Syntax | Description |
|----------|--------|-------------|
| Not | `!φ` or `not φ` | Negation |
| And | `φ && ψ` or `φ ∧ ψ` | Conjunction |
| Or | `φ \|\| ψ` or `φ ∨ ψ` | Disjunction |
| Implies | `φ -> ψ` or `φ → ψ` | Implication |

**Examples:**
```clts
body = ltl safe && bounded;              // Both safe and bounded
body = ltl !error;                       // Not error
body = ltl request -> grant;             // Request implies grant
```

---

## Temporal Operators

### Next (`X`)

**Syntax:** `X φ`

**Meaning:** In the next state, φ holds.

**Example:**
```clts
body = ltl X alarm;  // Alarm is active in the next state
```

**Translation:** `[] φ` (box modality)

### Always (`G`)

**Syntax:** `G φ` or `always φ`

**Meaning:** Globally, φ always holds (in all future states).

**Example:**
```clts
body = ltl G safe;  // Safe condition always holds
```

**Translation:** `ν X. (φ ∧ [] X)` (greatest fixpoint)

### Eventually (`F`)

**Syntax:** `F φ` or `eventually φ`

**Meaning:** Eventually, φ will hold (in some future state).

**Example:**
```clts
body = ltl F completed;  // Completion state is eventually reached
```

**Translation:** `μ X. (φ ∨ [] X)` (least fixpoint)

### Until (`U`)

**Syntax:** `φ U ψ`

**Meaning:** φ holds until ψ happens (and ψ eventually happens).

**Example:**
```clts
body = ltl request U grant;  // Request holds until grant is received
```

**Translation:** `μ X. (ψ ∨ (φ ∧ [] X))`

### Weak Until (`W`)

**Syntax:** `φ W ψ`

**Meaning:** φ holds until ψ happens, or φ always holds.

**Example:**
```clts
body = ltl request W grant;  // Request until grant, or request always
```

**Translation:** `(φ U ψ) ∨ G φ`

### Release (`R`)

**Syntax:** `φ R ψ`

**Meaning:** ψ holds until φ releases it (dual of until).

**Example:**
```clts
body = ltl error R recovery;  // Recovery holds until error releases it
```

**Translation:** `!(!φ U !ψ)`

---

## Common Patterns

### Safety Properties

**Pattern:** "Something bad never happens"

**Formula:** `G !bad`

**Example:**
```clts
formula mutual_exclusion {
    over System;
    body = ltl G !(in_critical_1 && in_critical_2);
}
```

### Liveness Properties

**Pattern:** "Something good eventually happens"

**Formula:** `F good`

**Example:**
```clts
formula termination {
    over Algorithm;
    body = ltl F terminated;
}
```

### Reactiveness (Request-Response)

**Pattern:** "Every request eventually gets a response"

**Formula:** `G (request -> F response)`

**Example:**
```clts
formula responsiveness {
    over Protocol;
    body = ltl G (request -> F grant);
}
```

### Recurrence (Infinitely Often)

**Pattern:** "Something happens infinitely often"

**Formula:** `GF φ` (equivalent to `G F φ`)

**Example:**
```clts
formula heartbeat {
    over System;
    body = ltl G F heartbeat;  // Heartbeat occurs infinitely often
}
```

### Stabilization

**Pattern:** "Eventually, something always holds"

**Formula:** `FG φ` (equivalent to `F G φ`)

**Example:**
```clts
formula stabilization {
    over System;
    body = ltl F G idle;  // System eventually stabilizes to idle
}
```

### Until Patterns

**Pattern:** "Phase transition"

**Example:**
```clts
formula initialization {
    over System;
    body = ltl initialization U operational;
}
```

---

## LTL vs μ-Calculus

### When to Use LTL

- **Temporal properties**: Safety, liveness, reactiveness
- **Standard patterns**: Request-response, mutual exclusion, termination
- **Readability**: When LTL syntax is more intuitive

### When to Use μ-Calculus

- **Complex fixpoints**: Nested fixpoints with custom variable names
- **Guards and labels**: When you need label/variable guards in modalities
- **Performance**: Direct μ-calculus may be slightly more efficient (no translation step)

### Comparison

| LTL | μ-Calculus Equivalent |
|-----|----------------------|
| `G φ` | `nu X. (φ && [] X)` |
| `F φ` | `mu X. (φ \|\| [] X)` |
| `X φ` | `[] φ` |
| `φ U ψ` | `mu X. (ψ \|\| (φ && [] X))` |

### Mixed Usage

You can use both LTL and μ-calculus in the same `mu_formulas` section:

```clts
mu_formulas {
    formula safety {
        over System;
        body = ltl G safe;  // LTL formula
    }
    
    formula complex {
        over System;
        body = nu X. (safe && [label step] X);  // μ-calculus with guards
    }
}
```

---

## Examples

### Example 1: Safety and Liveness

```clts
context example {
    automata {
        automaton System {
            states {
                state idle initial;
                state running;
                state completed;
            }
            transitions {
                transition idle -> running on epsilon;
                transition running -> completed on epsilon;
                transition completed -> completed on epsilon;
            }
        }
    }
    
    mu_formulas {
        // Safety: Never deadlock
        formula no_deadlock {
            over System;
            body = ltl G !deadlock;
        }
        
        // Liveness: Eventually complete
        formula eventually_complete {
            over System;
            body = ltl F completed;
        }
    }
}
```

### Example 2: Request-Response Protocol

```clts
context protocol {
    automata {
        automaton Client {
            states {
                state idle initial;
                state waiting;
                state satisfied;
            }
            transitions {
                transition idle -> waiting on request;
                transition waiting -> satisfied on grant;
                transition satisfied -> idle on epsilon;
            }
        }
    }
    
    mu_formulas {
        // Every request eventually gets a grant
        formula responsiveness {
            over Client;
            body = ltl G (request -> F grant);
        }
    }
}
```

### Example 3: GR(1) Pattern

```clts
context gr1_example {
    automata {
        automaton System {
            states {
                state s0 initial;
            }
            transitions {
                transition s0 -> s0 on epsilon;
            }
        }
    }
    
    mu_formulas {
        // Environment safety assumption
        formula env_safety {
            over System;
            body = ltl G env_assume;
        }
        
        // Environment justice (infinitely often)
        formula env_justice {
            over System;
            body = ltl G F env_justice;
        }
        
        // System safety guarantee
        formula sys_safety {
            over System;
            body = ltl G sys_guarantee;
        }
        
        // System justice (infinitely often)
        formula sys_justice {
            over System;
            body = ltl G F sys_justice;
        }
    }
}
```

---

## Migration Guide

### From μ-Calculus to LTL

If you have existing μ-calculus formulas, you can migrate them to LTL for better readability:

**Before (μ-calculus):**
```clts
formula safety {
    over System;
    body = nu X. (safe && [] X);
}
```

**After (LTL):**
```clts
formula safety {
    over System;
    body = ltl G safe;
}
```

### Common Migrations

| μ-Calculus | LTL |
|------------|-----|
| `nu X. (φ && [] X)` | `G φ` |
| `mu X. (φ \|\| [] X)` | `F φ` |
| `[] φ` | `X φ` |
| `mu X. (ψ \|\| (φ && [] X))` | `φ U ψ` |

### Backward Compatibility

Existing μ-calculus formulas continue to work without changes. The `ltl` keyword is optional for μ-calculus (you can use `mu` for clarity, or omit it):

```clts
// All of these are valid:
body = nu X. (safe && [] X);        // Default: μ-calculus
body = mu nu X. (safe && [] X);     // Explicit μ-calculus marker
body = ltl G safe;                  // LTL syntax
```

---

## Operator Precedence

LTL operators have the following precedence (highest to lowest):

1. **Unary operators**: `!`, `G`, `F`, `X`
2. **Until operators**: `U`, `W`, `R`
3. **And**: `&&`
4. **Or**: `||`
5. **Implies**: `->`

**Examples:**
- `G φ && ψ` = `(G φ) && ψ`
- `φ U ψ && χ` = `φ U (ψ && χ)`
- `φ -> ψ || χ` = `φ -> (ψ || χ)`

Use parentheses to clarify precedence when needed.

---

## References

- **[Temporal Logic Patterns](ltl_templates/temporal_logic_patterns.md)** - Common LTL/CTL patterns with use cases
- **[LTL Implementation Plan](ltl_templates/ltl_implementation_plan.md)** - Technical implementation details
- **[μ-Calculus Grammar](../archive/mu_calculus/mu_calculus_grammar_semantics.md)** - μ-calculus reference

---

## Summary

LTL support in Context DSL provides:

✅ **Intuitive syntax** for temporal properties  
✅ **Automatic translation** to μ-calculus  
✅ **Backward compatibility** with existing μ-calculus formulas  
✅ **Standard patterns** for safety, liveness, and reactiveness  
✅ **Mixed usage** of LTL and μ-calculus in the same context  

Start using LTL today to make your temporal properties more readable and maintainable!
