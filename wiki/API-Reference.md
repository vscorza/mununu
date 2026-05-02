> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change. We welcome feedback and bug reports via [GitHub Issues](https://github.com/vscorza/mununu/issues).

# API Reference

Mununu exposes a REST API for programmatic access to context summarization, controller synthesis, graph generation, and formula verification. The server is built on [Axum](https://github.com/tokio-rs/axum) and listens on a configurable address (default `127.0.0.1:3000`).

Start the server with:

```bash
mununu serve --bind 127.0.0.1:3000
```

All request and response bodies use `application/json`. CORS is open by default (`Access-Control-Allow-Origin: *`).

---

## Table of Contents

- [GET /api/v1/health](#get-apiv1health)
- [POST /api/v1/context/summarize](#post-apiv1contextsummarize)
- [POST /api/v1/context/synthesize](#post-apiv1contextsynthesize)
- [POST /api/v1/context/graphs](#post-apiv1contextgraphs)
- [POST /api/v1/context/verify](#post-apiv1contextverify)
- [POST /api/v1/context/import](#post-apiv1contextimport)
- [GET /api/v1/templates](#get-apiv1templates)
- [Common Types](#common-types)
- [Error Responses](#error-responses)

---

## GET /api/v1/health

Returns the server health status. Use this for readiness probes and connectivity checks.

### Response

```json
{
  "status": "ok",
  "service": "mununu-api"
}
```

### Example

```bash
curl http://localhost:3000/api/v1/health
```

---

## POST /api/v1/context/summarize

Parse a CTXDSL context (with optional sidecar files) and return a summary of all automata, formulas, and declared controllers. Controllers are synthesized on the fly so the response includes realizability and size metrics.

> **Adapter formats:** this endpoint accepts CTXDSL only. To summarize an external format (`.xstate.json`, `.sv`, `.tlsf`, `.aag`/`.aig`, `.pml`, `.espec.json`), first translate via [`POST /api/v1/context/import`](#post-apiv1contextimport) and pass the resulting `ctxdsl` field to this endpoint. The CLI command `mununu context summarize <file> --adapter <format>` does this two-step internally; clients of the HTTP API must do it explicitly. The same convention applies to `/context/verify` and `/context/synthesize`.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `context` | `FileContent` | Yes | Main CTXDSL file. |
| `sidecars` | `SidecarFile[]` | No | Additional sidecar CTXDSL files merged into the context. Defaults to `[]`. |
| `format` | `"json" \| "table"` | No | Output format. Defaults to `"json"`. |

**FileContent / SidecarFile**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | File name (used for diagnostics). |
| `content` | `string` | Full CTXDSL source text. |

### Response Body

| Field | Type | Description |
|-------|------|-------------|
| `success` | `boolean` | `true` on success. |
| `summary.context_name` | `string` | Name declared in the context. |
| `summary.automata` | `AutomatonSummary[]` | One entry per automaton and composition. |
| `summary.formulas_count` | `number` | Total formulas defined in the context. |
| `summary.controllers_count` | `number` | Total controller declarations. |
| `summary.controllers` | `ControllerSummary[]` | Per-controller realizability and size. |

**AutomatonSummary**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Automaton identifier. |
| `states_count` | `number` | Number of states. |
| `transitions_count` | `number` | Number of transitions. |

**ControllerSummary**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Controller identifier. |
| `source` | `string` | Source automaton name. |
| `formula` | `string` | Formula used for synthesis. |
| `realizable` | `boolean` | Whether a winning strategy exists. |
| `states_count` | `number` | Controller state count (0 if unrealizable). |
| `transitions_count` | `number` | Controller transition count (0 if unrealizable). |

### Example

```bash
curl -X POST http://localhost:3000/api/v1/context/summarize \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "name": "traffic.ctxdsl",
      "content": "context traffic {\n  automata {\n    automaton light {\n      states { state red initial; state green; }\n      transitions { transition red -> green on go; transition green -> red on stop; }\n    }\n  }\n}"
    },
    "sidecars": []
  }'
```

### Example Response

```json
{
  "success": true,
  "summary": {
    "context_name": "traffic",
    "automata": [
      {
        "name": "light",
        "states_count": 2,
        "transitions_count": 2
      }
    ],
    "formulas_count": 0,
    "controllers_count": 0,
    "controllers": []
  }
}
```

---

## POST /api/v1/context/synthesize

Synthesize a controller for a given automaton and mu-calculus formula. The controller restricts the system's controllable transitions so that the formula is satisfied. When the specification is realizable, the response includes the controller serialized as CTXDSL source.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `context` | `FileContent` | Yes | Main CTXDSL file. |
| `sidecars` | `SidecarFile[]` | No | Sidecar files. Defaults to `[]`. |
| `automaton` | `string` | Yes | Target automaton name. |
| `formula` | `string` | Yes | Formula name defined in the context. |
| `options` | `SynthesisOptions` | No | Synthesis configuration. |

**SynthesisOptions**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `minimize` | `boolean` | `false` | Apply bisimulation minimization to the controller. |
| `diagnostics` | `DiagnosticsOptions` | `{}` | Control diagnostic output. |
| `extract_strategy` | `boolean` | `false` | **Legacy** — equivalent to `controller_mode: "functional"`. When `controller_mode` is set, that takes precedence. |
| `controller_mode` | `string \| null` | `null` (= `"projection"` or `"functional"` if `extract_strategy=true`) | Controller extraction mode. One of `"projection"`, `"functional"`, `"permissive"`, `"signature-memory"`, `"product-game"`, `"parity-game"`. Case-insensitive; dashes/underscores interchangeable. Unknown values return `400 Bad Request`. See [Controller Modes](Controller-Modes.md) for the full reference. |
| `output_format` | `string \| null` | `null` | Native controller export format: `"xstate"`, `"systemverilog"`/`"sv"`, or `"gdscript"`/`"gd"`. When set, the response includes a `controller_native` field. |

**DiagnosticsOptions**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `counterexample` | `boolean` | `false` | Compute a counterexample trace when unrealizable. |
| `counterstrategy` | `boolean` | `false` | Compute environment counterstrategy traces. |
| `deadlock_traces` | `boolean` | `false` | Include deadlock traces in diagnostics. |
| `max_counter_traces` | `number \| null` | `null` | Limit the number of counterstrategy traces. |

### Response Body

| Field | Type | Description |
|-------|------|-------------|
| `success` | `boolean` | `true` on success. |
| `realizable` | `boolean` | Whether a winning controller exists. |
| `controller` | `FileContent \| null` | Synthesized controller as CTXDSL. `null` when unrealizable. |
| `diagnostics` | `SynthesisDiagnostics` | Diagnostic information. |
| `counterstrategy` | `CounterstrategyResult \| null` | Environment counterstrategy graph for unrealizable cases. Automatically computed when synthesis fails. See [CounterstrategyResult](#counterstrategyresult) below. |

**SynthesisDiagnostics**

| Field | Type | Description |
|-------|------|-------------|
| `messages` | `string[]` | Human-readable diagnostic messages. |
| `violating_initials` | `string[]` | Initial states that violate the formula. |
| `counterexample_trace` | `string[] \| null` | A trace witnessing a violation (if requested). |
| `counterstrategy_traces` | `string[][]` | Environment counterstrategy traces. |
| `deadlock_traces` | `string[][]` | Traces leading to deadlock states. |
| `lasso_traces` | `LassoTrace[]` | Lasso-format infinite counterexample traces (prefix + cycle). Present for liveness violations. |
| `minimization` | `MinimizationReport \| null` | Minimization statistics (if `minimize` was `true`). |
| `proof_obligations` | `ProofObligation[]` | Per-state proof obligations. |

**LassoTrace**

| Field | Type | Description |
|-------|------|-------------|
| `prefix` | `string[]` | State names forming the finite prefix. |
| `cycle` | `string[]` | State names forming the infinitely repeating cycle. |
| `prefix_labels` | `string[]` | Transition labels between consecutive prefix states. `prefix_labels[i]` is the label from `prefix[i]` to `prefix[i+1]` (or `cycle[0]` for the last). |
| `cycle_labels` | `string[]` | Transition labels between consecutive cycle states. The last element is the label from the last cycle state back to `cycle[0]`. |

**MinimizationReport**

| Field | Type | Description |
|-------|------|-------------|
| `removed_states` | `number` | States removed by minimization. |
| `removed_transitions` | `number` | Transitions removed. |
| `merged_states` | `string[]` | Names of states that were merged. |

**ProofObligation**

| Field | Type | Description |
|-------|------|-------------|
| `state` | `string` | State name. |
| `detail` | `string \| null` | Additional detail about the obligation. |

### Example

```bash
curl -X POST http://localhost:3000/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "name": "arbiter.ctxdsl",
      "content": "... CTXDSL source ..."
    },
    "automaton": "arbiter",
    "formula": "mutual_exclusion",
    "options": {
      "minimize": true,
      "diagnostics": {
        "counterexample": true,
        "deadlock_traces": true,
        "max_counter_traces": 5
      }
    }
  }'
```

### Example Response (Realizable)

```json
{
  "success": true,
  "realizable": true,
  "controller": {
    "name": "arbiter_controller.ctxdsl",
    "content": "// Synthesised controller derived from automaton 'arbiter' and formula 'mutual_exclusion'\ncontext arbiter_mutual_exclusion_controller {\n  ...\n}"
  },
  "diagnostics": {
    "messages": [],
    "violating_initials": [],
    "counterexample_trace": null,
    "counterstrategy_traces": [],
    "deadlock_traces": [],
    "minimization": {
      "removed_states": 3,
      "removed_transitions": 7,
      "merged_states": ["s1_s3", "s2_s4"]
    },
    "proof_obligations": []
  }
}
```

### Example Response (Unrealizable)

```json
{
  "success": true,
  "realizable": false,
  "controller": null,
  "diagnostics": {
    "messages": ["No winning strategy exists from initial state idle"],
    "violating_initials": ["idle"],
    "counterexample_trace": ["idle", "req1", "grant1", "req2"],
    "counterstrategy_traces": [],
    "deadlock_traces": [],
    "lasso_traces": [],
    "minimization": null,
    "proof_obligations": []
  },
  "counterstrategy": {
    "environment_winning_states": ["idle", "req1"],
    "graph_elements": [
      { "data": { "type": "node", "id": "cs_idle", "label": "idle", "vars": [], "actions": [] }, "classes": "env-winning start" },
      { "data": { "type": "edge", "id": "cs_e0", "source": "cs_idle", "target": "cs_req1", "label": "request", "action_type": "uncontrollable" } }
    ],
    "inverted_formula": "nu X. (!goal && <ctrl=environment> X)",
    "minimized": true
  }
}
```

---

## POST /api/v1/context/graphs

Generate Cytoscape-compatible graph elements for automata, compositions, and (optionally) synthesized controllers. Returns nodes and edges that can be rendered directly in a Cytoscape.js visualization layer.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `context` | `FileContent` | Yes | Main CTXDSL file. |
| `sidecars` | `SidecarFile[]` | No | Sidecar files. Defaults to `[]`. |
| `automaton` | `string \| null` | No | Filter to a single automaton. `null` returns all. |
| `graph_types` | `("dsl" \| "unrolled")[]` | No | Which graph representations to generate. Defaults to `["dsl"]`. |
| `include_controllers` | `boolean` | No | Include graphs for declared controllers. Defaults to `false`. |
| `minimize_controllers` | `boolean \| null` | No | Override per-controller minimize setting. `null` uses each controller's declared setting. |

### Response Body

| Field | Type | Description |
|-------|------|-------------|
| `success` | `boolean` | `true` on success. |
| `context` | `ContextSummary` | Summary of the parsed context. |
| `graphs` | `GraphData[]` | One entry per automaton per graph type. |

**GraphData**

| Field | Type | Description |
|-------|------|-------------|
| `automaton` | `string` | Automaton name (or `<name>_controller` for controller graphs). |
| `graph_type` | `"dsl" \| "unrolled" \| "controller"` | Type of graph. |
| `elements` | `GraphElement[]` | Cytoscape-compatible elements. |
| `metadata` | `GraphMetadata` | Aggregate statistics. |

**GraphElement**

| Field | Type | Description |
|-------|------|-------------|
| `data` | `GraphElementData` | Node or edge data (discriminated by `data.type`). |
| `position` | `{ x: number, y: number } \| null` | Optional layout coordinates. |
| `classes` | `string \| null` | CSS class names for styling. |

**GraphElementData (Node)**

| Field | Type | Description |
|-------|------|-------------|
| `type` | `"node"` | Discriminator. |
| `id` | `string` | Unique node ID. |
| `label` | `string` | Display label. |
| `parent` | `string \| null` | Parent node ID for compound graphs. |
| `vars` | `string[]` | State variable annotations. |
| `actions` | `string[]` | Enabled actions. |

**GraphElementData (Edge)**

| Field | Type | Description |
|-------|------|-------------|
| `type` | `"edge"` | Discriminator. |
| `id` | `string` | Unique edge ID. |
| `source` | `string` | Source node ID. |
| `target` | `string` | Target node ID. |
| `label` | `string \| null` | Transition label. |
| `action` | `string \| null` | Action name. |
| `action_type` | `string \| null` | `"controllable"` or `"uncontrollable"`. |
| `guard` | `string \| null` | Guard expression. |
| `effect` | `string \| null` | Effect expression. |

**GraphMetadata**

| Field | Type | Description |
|-------|------|-------------|
| `states_count` | `number` | Number of states in the graph. |
| `transitions_count` | `number` | Number of transitions. |
| `initial_states` | `string[]` | Names of initial states. |

### Example

```bash
curl -X POST http://localhost:3000/api/v1/context/graphs \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "name": "traffic.ctxdsl",
      "content": "... CTXDSL source ..."
    },
    "graph_types": ["dsl", "unrolled"],
    "include_controllers": true,
    "minimize_controllers": true
  }'
```

### Example Response (abbreviated)

```json
{
  "success": true,
  "context": {
    "context_name": "traffic",
    "automata": [{ "name": "light", "states_count": 2, "transitions_count": 2 }],
    "formulas_count": 1,
    "controllers_count": 0,
    "controllers": []
  },
  "graphs": [
    {
      "automaton": "light",
      "graph_type": "dsl",
      "elements": [
        {
          "data": { "type": "node", "id": "light_red", "label": "red", "vars": [], "actions": ["go"] },
          "position": null,
          "classes": "state initial"
        },
        {
          "data": { "type": "node", "id": "light_green", "label": "green", "vars": [], "actions": ["stop"] },
          "position": null,
          "classes": "state"
        },
        {
          "data": { "type": "edge", "id": "light_e0", "source": "light_red", "target": "light_green", "label": "go", "action": "go", "action_type": "controllable" },
          "position": null,
          "classes": null
        }
      ],
      "metadata": {
        "states_count": 2,
        "transitions_count": 2,
        "initial_states": ["red"]
      }
    }
  ]
}
```

---

## POST /api/v1/context/verify

Evaluate mu-calculus formulas over automata and report which initial states satisfy or violate each formula. When a formula is not satisfied and `counterstrategy` is requested, the response includes the environment's winning region and a Cytoscape graph of the counterstrategy automaton.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `context` | `FileContent` | Yes | Main CTXDSL file. |
| `sidecars` | `SidecarFile[]` | No | Sidecar files. Defaults to `[]`. |
| `formula` | `string \| null` | No | Evaluate a specific formula by name. `null` evaluates all user-defined formulas. Mutually exclusive with `template_ref`. |
| `template_ref` | `TemplateRef \| null` | No | Instantiate a [property template](Property-Templates) instead of selecting an existing formula. `{"template": "no_deadlock"}` or `{"template": "reachable", "args": {"TARGET": "Idle"}}`. Mutually exclusive with `formula`. |
| `automaton` | `string \| null` | No | Target automaton. `null` uses each formula's declared targets. |
| `counterstrategy` | `boolean` | No | Compute counterstrategy for failed formulas. Defaults to `false`. |
| `minimize_counterstrategy` | `boolean` | No | Apply bisimulation minimization to counterstrategy. Defaults to `false`. |

### Response Body

| Field | Type | Description |
|-------|------|-------------|
| `success` | `boolean` | `true` on success. |
| `all_satisfied` | `boolean` | `true` if every formula is satisfied on every target. |
| `results` | `FormulaVerificationResult[]` | One entry per formula-automaton pair. |

**FormulaVerificationResult**

| Field | Type | Description |
|-------|------|-------------|
| `formula_name` | `string` | Formula identifier. |
| `automaton` | `string` | Automaton evaluated against. |
| `satisfied` | `boolean` | `true` if all initial states satisfy the formula. |
| `total_states` | `number` | Total states in the automaton. |
| `satisfying_states` | `number` | Number of states satisfying the formula. |
| `initial_states` | `string[]` | All initial state names. |
| `initial_satisfying` | `string[]` | Initial states that satisfy the formula. |
| `initial_violating` | `string[]` | Initial states that violate the formula. |
| `satisfying_state_names` | `string[]` | All satisfying state names (sorted). |
| `counterstrategy` | `CounterstrategyResult \| null` | Present when requested and formula is not satisfied. |

**CounterstrategyResult**

| Field | Type | Description |
|-------|------|-------------|
| `environment_winning_states` | `string[]` | States where the environment can force a violation. |
| `graph_elements` | `GraphElement[]` | Cytoscape graph of the counterstrategy. |
| `inverted_formula` | `string` | The negated formula used internally (for debugging). |
| `minimized` | `boolean` | Whether minimization was applied. |

### Example

```bash
curl -X POST http://localhost:3000/api/v1/context/verify \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "name": "arbiter.ctxdsl",
      "content": "... CTXDSL source ..."
    },
    "formula": "mutual_exclusion",
    "automaton": "arbiter",
    "counterstrategy": true,
    "minimize_counterstrategy": false
  }'
```

### Example Response (Satisfied)

```json
{
  "success": true,
  "all_satisfied": true,
  "results": [
    {
      "formula_name": "mutual_exclusion",
      "automaton": "arbiter",
      "satisfied": true,
      "total_states": 6,
      "satisfying_states": 6,
      "initial_states": ["idle"],
      "initial_satisfying": ["idle"],
      "initial_violating": [],
      "satisfying_state_names": ["idle", "grant1", "grant2", "wait1", "wait2", "reset"]
    }
  ]
}
```

### Example Response (Not Satisfied, with Counterstrategy)

```json
{
  "success": true,
  "all_satisfied": false,
  "results": [
    {
      "formula_name": "liveness",
      "automaton": "arbiter",
      "satisfied": false,
      "total_states": 6,
      "satisfying_states": 4,
      "initial_states": ["idle"],
      "initial_satisfying": [],
      "initial_violating": ["idle"],
      "satisfying_state_names": ["grant1", "grant2", "wait1", "wait2"],
      "counterstrategy": {
        "environment_winning_states": ["idle", "reset"],
        "graph_elements": [
          { "data": { "type": "node", "id": "cs_idle", "label": "idle", "vars": [], "actions": [] }, "classes": "winning" },
          { "data": { "type": "edge", "id": "cs_e0", "source": "cs_idle", "target": "cs_reset", "label": "timeout" } }
        ],
        "inverted_formula": "mu X . (<tau> true & [grant] false) | (<tau> X)",
        "minimized": false
      }
    }
  ]
}
```

---

## POST /api/v1/context/import

Import an external format (XState, SystemVerilog, TLSF, AIGER, Promela) and translate it to CTXDSL.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `content` | string | Yes | Raw file content in the source format |
| `format` | string | No | Format hint: `"auto"` (default), `"tlsf"`, `"aiger"`, `"promela"`, `"xstate"`, `"systemverilog"` |
| `filename` | string | No | Original filename (used for extension-based detection if format is `"auto"`) |

### Response Body

| Field | Type | Description |
|-------|------|-------------|
| `success` | boolean | Whether the import succeeded |
| `ctxdsl` | string | Translated CTXDSL content |
| `source_format` | string | Detected source format name |
| `warnings` | string[] | Translation warnings (unsupported constructs, neutral controllability, etc.) |
| `signal_count` | number | Number of signals/events in the source |
| `state_count` | number | Number of states (for signal-state encoding) |
| `property_count` | number | Number of properties translated |

### Example

```bash
curl -X POST http://localhost:3000/api/v1/context/import \
  -H "Content-Type: application/json" \
  -d '{
    "content": "{\"id\":\"light\",\"initial\":\"green\",\"states\":{\"green\":{\"on\":{\"TIMER\":\"yellow\"}},\"yellow\":{\"on\":{\"TIMER\":\"red\"}},\"red\":{\"on\":{\"TIMER\":\"green\"}}}}",
    "format": "xstate"
  }'
```

See [Adapter Formats](Adapter-Formats.md) for details on each supported format.

---

## GET /api/v1/templates

List available property templates. Templates provide parameterized mu-calculus formula patterns that can be used in `template_ref` fields of verify and synthesize requests.

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `domain` | `string` | No | Filter by domain: `game`, `rtl`, `agentic`, `software`, `synthesis`, `universal` |

### Response Body

Without `domain` filter, returns a `TemplateCatalog`:

```json
{
  "version": "1.0",
  "templates": [
    {
      "id": "no_deadlock",
      "display_name": "No Deadlock",
      "description": "Every reachable state has at least one enabled transition",
      "kind": "safety",
      "role": "guarantee",
      "domains": ["universal"],
      "params": [],
      "formula_pattern": "nu X. (<> true && [] X)",
      "domain_hints": {},
      "tags": ["deadlock", "softlock", "safety"]
    }
  ]
}
```

With `domain` filter, returns a filtered array of `PropertyTemplate` objects.

### Example

```bash
curl http://localhost:8080/api/v1/templates
curl http://localhost:8080/api/v1/templates?domain=game
```

See [Property Templates](Property-Templates) for the full catalog and usage guide.

---

## Common Types

### FileContent

Used for both the main context and synthesized controller output.

```json
{
  "name": "example.ctxdsl",
  "content": "context example { ... }"
}
```

### SidecarFile

Structurally identical to `FileContent`. Sidecars are merged into the main context during parsing, allowing modular specifications.

```json
{
  "name": "properties.ctxdsl",
  "content": "context example_props { mu_formulas { ... } }"
}
```

---

## Error Responses

All endpoints return errors in a consistent format:

```json
{
  "error": {
    "code": "BAD_REQUEST",
    "message": "Failed to parse context: unexpected token at line 5",
    "details": "unexpected token 'xyz' at line 5, column 12"
  }
}
```

| HTTP Status | Code | Meaning |
|-------------|------|---------|
| 400 | `BAD_REQUEST` | Invalid input: parse errors, unknown formula/automaton names. |
| 500 | `INTERNAL_ERROR` | Server-side failure during realization, synthesis, or evaluation. |

---

## Logging

Set the `RUST_LOG` environment variable to control server log verbosity:

```bash
RUST_LOG=mununu=info mununu serve
```

The server logs timing breakdowns for each request (parse, realize, evaluate durations in milliseconds).
