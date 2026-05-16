# Property Templates

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change.

Property templates provide parameterized mu-calculus formula patterns that can be instantiated across any domain — SystemVerilog RTL, MCP/agentic protocols, software extraction, and synthesis benchmarks. Templates hide formal logic behind named patterns with human-readable descriptions.

## Overview

Instead of writing raw mu-calculus:

```
nu X. (<> true && [] X)
```

Use a named template:

```bash
mununu context eval player.espec.json --template no_deadlock --automaton PlayerState
```

Templates resolve to standard `PropertyFormula::MuCalculus(String)` at instantiation time. The emitter, evaluator, and synthesis pipeline see no difference from a hand-written formula.

## Built-in Templates

| ID | Display Name | Kind | Parameters | Formula Pattern |
|----|-------------|------|------------|----------------|
| `no_deadlock` | No Deadlock | safety | — | `nu X. (<> true && [] X)` |
| `reachable` | Reachable State | liveness | `$TARGET` | `mu X. (${TARGET} \|\| <> X)` |
| `never` | Never (Invariant) | safety | `$BAD` | `nu X. (!${BAD} && [] X)` |
| `always_eventually` | Always Eventually | liveness | `$TARGET` | `nu Y. ((mu X. (${TARGET} \|\| <> X)) && [] Y)` |
| `bounded` | Bounded Resource | safety | `$OVERFLOW`, `$UNDERFLOW`* | `nu X. (!${OVERFLOW} && !${UNDERFLOW} && [] X)` |
| `response` | Response (Request-Grant) | liveness | `$TRIGGER`, `$RESPONSE` | `nu X. ((!${TRIGGER} \|\| mu Y. (${RESPONSE} \|\| <> Y)) && [] X)` |
| `mutual_exclusion` | Mutual Exclusion | safety | `$A`, `$B` | `nu X. (!(${A} && ${B}) && [] X)` |
| `label_blocked_in_state` | Label Blocked in State | safety | `$STATE`, `$LABEL` | `nu X. ((!${STATE} \|\| [${LABEL}] false) && [] X)` |

\* `$UNDERFLOW` is optional with default `false`.

## Using Templates

### CLI

```bash
# Zero-parameter template
mununu context eval model.espec.json --template no_deadlock --automaton FSM

# Template with arguments
mununu context eval model.espec.json \
    --template reachable --template-arg TARGET=GoalState --automaton FSM

# Multiple arguments
mununu context eval model.espec.json \
    --template mutual_exclusion --template-arg A=P1_Active --template-arg B=P2_Active \
    --automaton System

# List all templates
mununu templates

# Filter by domain
mununu templates --domain rtl

# Show template details
mununu templates --id reachable

# JSON output
mununu templates --json
```

`--template` and `--formula` are mutually exclusive. When `--template` is provided, the template is instantiated and injected as an ad-hoc formula.

### In Extraction Specs (`.espec.json`)

Properties can reference templates instead of raw formulas:

```json
{
  "properties": [
    {
      "id": "no_softlock",
      "template_ref": { "template": "no_deadlock" },
      "over": "PlayerState"
    },
    {
      "id": "can_return_idle",
      "template_ref": {
        "template": "always_eventually",
        "args": { "TARGET": "Idle" }
      },
      "over": "PlayerState"
    },
    {
      "id": "custom_formula",
      "formula": "nu X. ([] X)",
      "description": "Raw formula still works"
    }
  ]
}
```

When both `formula` and `template_ref` are present, `formula` takes precedence.

### In SV Sidecars (`.mununu.json`)

```json
{
  "properties": [
    {
      "id": "no_overflow",
      "template_ref": {
        "template": "bounded",
        "args": { "OVERFLOW": "fill_5" }
      }
    }
  ]
}
```

### In XState `__mununu` Block

```json
{
  "__mununu": {
    "properties": [
      {
        "name": "safe",
        "template_ref": { "template": "no_deadlock" },
        "role": "guarantee"
      }
    ]
  }
}
```

### API

```bash
# List templates
curl http://localhost:8080/api/v1/templates

# Filter by domain
curl http://localhost:8080/api/v1/templates?domain=rtl

# Verify with template
curl -X POST http://localhost:8080/api/v1/context/verify \
  -H "Content-Type: application/json" \
  -d '{
    "context": {"name": "model.ctxdsl", "content": "..."},
    "template_ref": {"template": "reachable", "args": {"TARGET": "Idle"}},
    "automaton": "FSM"
  }'
```

### Web UI

In the verification tab, check "Use Template" to switch from formula-name input to the template picker. Select a template, fill in parameters, preview the instantiated formula, and click "Apply Template" to verify.

## Domain Hints

Templates include domain-specific hints that appear in the UI parameter inputs:

| Template | RTL | Agentic | Software |
|----------|-----|---------|----------|
| `reachable($TARGET)` | state_IDLE, fill_0 | SessionClosed | Released, Disposed |
| `never($BAD)` | overflow, error_state | Unauthorized | NullState |
| `bounded($OVERFLOW)` | fill_5 | — | count_max |

## Adding Custom Templates

Templates are defined in `crates/mununu-core/src/adapter/templates/builtin_templates.json`. To add a new template:

1. Add an entry to the `templates` array with `id`, `display_name`, `description`, `kind`, `role`, `domains`, `params`, `formula_pattern`, `domain_hints`, and `tags`.
2. Ensure the `formula_pattern` uses `${PARAM_NAME}` placeholders matching the `params` entries.
3. The template is automatically available in CLI, API, UI, and spec files after rebuild.

### Parameter Types

| Type | Validation | Use Case |
|------|-----------|----------|
| `predicate` | Alphanumeric + underscore | State predicate names |
| `state` | Alphanumeric + underscore | Automaton state names |
| `integer` | Numeric, optional min/max bounds | Bounded values |
| `label` | Non-empty string | Transition labels |
| `expression` | Non-empty string | Free-form mu-calculus sub-expressions |

## Architecture

Templates are compiled into the binary via `include_str!` from the JSON catalog. The `TemplateRegistry` provides lookup, validation, and instantiation. Template resolution happens at the adapter layer (before the emitter/evaluator), producing standard `PropertyFormula::MuCalculus(String)` values.

See also: [CLI Reference](CLI-Reference), [API Reference](API-Reference)
