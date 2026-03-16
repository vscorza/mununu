# HTTP API Validation Tutorial

Hands-on walkthrough for validating the HENOS HTTP API server. This tutorial assumes you have cloned the official GitHub repository and can run the `henos` server. Each section demonstrates how to interact with the REST API using `curl` or similar HTTP clients. We'll test all endpoints with example files stored in a temporary working directory—feel free to adapt paths to your environment.

---

## 1. Prerequisites

- Rust toolchain installed (`cargo` on PATH)
- GitHub access: `git clone https://github.com/vscorza/henos-rust.git`
- `curl` command available (or similar HTTP client)
- The `henos` server available (run from repo root with `cargo run --features api --bin henos -- server`)
- A text editor for creating test files

Throughout the tutorial we use `WORKDIR=/tmp/henos_tutorial`. Create it up front:

```bash
mkdir -p /tmp/henos_tutorial
cd henos-rust
```

---

## 2. Starting the Server

### 2.1 Start the HTTP Server

From the repository root, start the server:

```bash
cargo run --features api --bin henos -- server --addr 127.0.0.1:8080
```

The server will start and display:
```
Server listening on 127.0.0.1:8080
```

Keep this terminal running. The server will handle requests on port 8080.

**Note**: For production use, you may want to bind to `0.0.0.0:8080` to accept connections from other machines.

### 2.2 Verify Server is Running

In a separate terminal, test the health check endpoint:

```bash
curl -X GET http://127.0.0.1:8080/api/v1/health
```

Expected response:
```json
{
  "status": "ok",
  "service": "henos-api"
}
```

If you see this response, the server is running correctly.

---

## 3. Basic Context Operations

### 3.1 Create a Test Context File

Create a simple context file for testing. Open your editor and create `/tmp/henos_tutorial/traffic_light.ctxdsl`:

```ctxdsl
context traffic_light {
    alphabet {
        label next;
    }
    automata {
        automaton Light {
            predicates {
                predicate is_green = state Green;
                predicate is_yellow = state Yellow;
            }
            states {
                state Green initial;
                state Yellow;
                state Red;
            }
            transitions {
                transition Green -> Yellow on label next;
                transition Yellow -> Red on label next;
                transition Red -> Green on label next;
            }
        }
    }
}
```

### 3.2 Context Summarize

Test the `/api/v1/context/summarize` endpoint to get information about the context:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/context/summarize \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "name": "traffic_light.ctxdsl",
      "content": "context traffic_light {\n    alphabet {\n        label next;\n    }\n    automata {\n        automaton Light {\n            predicates {\n                predicate is_green = state Green;\n                predicate is_yellow = state Yellow;\n            }\n            states {\n                state Green initial;\n                state Yellow;\n                state Red;\n            }\n            transitions {\n                transition Green -> Yellow on label next;\n                transition Yellow -> Red on label next;\n                transition Red -> Green on label next;\n            }\n        }\n    }\n}"
    },
    "sidecars": [],
    "format": "json"
  }' | jq .
```

Or read from file using `jq` to construct the request:

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/traffic_light.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/summarize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"traffic_light.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"format\": \"json\"
  }" | jq .
```

Expected response:
```json
{
  "success": true,
  "summary": {
    "context_name": "traffic_light",
    "automata": [
      {
        "name": "Light",
        "states_count": 3,
        "transitions_count": 3
      }
    ],
    "formulas_count": 0,
    "controllers_count": 0
  }
}
```

### 3.3 Context Graphs

Test the `/api/v1/context/graphs` endpoint to generate graph visualization data:

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/traffic_light.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/graphs \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"traffic_light.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": null,
    \"graph_types\": [\"dsl\"]
  }" | jq .
```

Expected response structure:
```json
{
  "success": true,
  "context": {
    "context_name": "traffic_light",
    "automata": [
      {
        "name": "Light",
        "states_count": 3,
        "transitions_count": 3
      }
    ],
    "formulas_count": 0,
    "controllers_count": 0
  },
  "graphs": [
    {
      "automaton": "Light",
      "graph_type": "dsl",
      "elements": [
        {
          "data": {
            "type": "node",
            "id": "Light",
            "label": "Automaton Light",
            "vars": [],
            "actions": ["next (uncontrollable)"]
          }
        },
        {
          "data": {
            "type": "node",
            "id": "Light_Green",
            "label": "Light_Green",
            "parent": "Light",
            "vars": [],
            "actions": []
          },
          "classes": "state start"
        }
        // ... more elements for states and transitions ...
      ],
      "metadata": {
        "states_count": 3,
        "transitions_count": 3,
        "initial_states": ["Light_Green"]
      }
    }
  ]
}
```

**Request both graph types** (DSL and unrolled):

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/traffic_light.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/graphs \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"traffic_light.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Light\",
    \"graph_types\": [\"dsl\", \"unrolled\"]
  }" | jq '.graphs | length'
```

**Note**: For automata without variables, unrolled graphs may not be available. The API will return an error if unrolling is requested but not possible.

---

## 4. BPMN Translation

### 4.1 Create a Simple BPMN JSON File

Create `/tmp/henos_tutorial/simple_process.json`:

```json
{
  "name": "SimpleProcess",
  "states": [
    {"name": "Start", "initial": true},
    {"name": "Task1"},
    {"name": "End"}
  ],
  "transitions": [
    {"from": "Start", "to": "Task1", "label": "begin"},
    {"from": "Task1", "to": "End", "label": "complete"}
  ]
}
```

### 4.2 Translate BPMN to ctxdsl

Test the `/api/v1/translate/bpm` endpoint:

```bash
BPMN_CONTENT=$(cat /tmp/henos_tutorial/simple_process.json | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/translate/bpm \
  -H "Content-Type: application/json" \
  -d "{
    \"bpmn\": {
      \"content\": ${BPMN_CONTENT},
      \"format\": \"json\"
    },
    \"sidecars\": [],
    \"options\": {
      \"name\": \"SimpleProcess\",
      \"force\": false
    }
  }" | jq .
```

Expected response:
```json
{
  "success": true,
  "output": {
    "context": {
      "name": "SimpleProcess.ctxdsl",
      "content": "context SimpleProcess {\n    alphabet {\n        label begin;\n        label complete;\n    }\n    automata {\n        automaton Process {\n            states {\n                state Start initial;\n                state Task1;\n                state End;\n            }\n            transitions {\n                transition Start -> Task1 on label begin;\n                transition Task1 -> End on label complete;\n            }\n        }\n    }\n}"
    },
    "sidecars": []
  },
  "metadata": {
    "context_name": "SimpleProcess",
    "automata_count": 1,
    "sidecars_count": 0
  },
  "warnings": [],
  "errors": []
}
```

Save the translated context for later use:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/translate/bpm \
  -H "Content-Type: application/json" \
  -d "{
    \"bpmn\": {
      \"content\": ${BPMN_CONTENT},
      \"format\": \"json\"
    },
    \"sidecars\": [],
    \"options\": {
      \"name\": \"SimpleProcess\",
      \"force\": false
    }
  }" | jq -r '.output.context.content' > /tmp/henos_tutorial/simple_process_translated.ctxdsl
```

---

## 5. Controller Synthesis

### 5.1 Create Context with Formula

Create `/tmp/henos_tutorial/controller_example.ctxdsl`:

```ctxdsl
context ControllerExample {
    alphabet {
        label request;
        label grant;
    }
    automata {
        automaton Process {
            controllable {
                label grant;
            }
            states {
                state Idle initial;
                state Waiting;
                state Granted;
            }
            transitions {
                transition Idle -> Waiting on label request;
                transition Waiting -> Granted on label grant;
                transition Granted -> Idle on label grant;
            }
        }
    }
    mu_formulas {
        formula safety {
            over Process;
            body = nu X. ([grant]X && [request]X);
        }
    }
}
```

**Note**: The `controllers` block is not required for API synthesis. The API endpoint takes the automaton and formula names as parameters, so you only need to define the automaton and formula in the context.

### 5.2 Synthesize Controller

Test the `/api/v1/context/synthesize` endpoint:

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/controller_example.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"controller_example.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Process\",
    \"formula\": \"safety\",
    \"options\": {
      \"minimize\": true,
      \"diagnostics\": {
        \"counterexample\": true,
        \"counterstrategy\": false,
        \"deadlock_traces\": true,
        \"max_counter_traces\": 10
      }
    }
  }" | jq .
```

Expected response structure:
```json
{
  "success": true,
  "realizable": true,
  "controller": {
    "name": "Process_controller.ctxdsl",
    "content": "// Synthesised controller derived from automaton 'Process' and formula 'safety'\ncontext ControllerExample_Process_safety_controller {\n    alphabet {\n        label grant;\n        label request;\n    }\n    automata {\n        automaton ControllerExample_Process_safety_controller_automaton {\n            states {\n                state Idle initial;\n                state Waiting;\n                state Granted;\n            }\n            transitions {\n                transition Waiting -> Granted on label grant; // controllable\n                transition Granted -> Idle on label grant; // controllable\n            }\n        }\n    }\n    mu_formulas {\n        formula safety {\n            over ControllerExample_Process_safety_controller_automaton;\n            body = nu X. ([grant]X && [request]X);\n        }\n    }\n}"
  },
  "diagnostics": {
    "messages": [],
    "violating_initials": [],
    "counterexample_trace": null,
    "counterstrategy_traces": [],
    "deadlock_traces": [],
    "minimization": {
      "removed_states": 0,
      "removed_transitions": 1,
      "merged_states": []
    },
    "proof_obligations": []
  }
}
```

**Save the synthesized controller**:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"controller_example.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Process\",
    \"formula\": \"safety\",
    \"options\": {
      \"minimize\": true,
      \"diagnostics\": {
        \"counterexample\": true,
        \"deadlock_traces\": true
      }
    }
  }" | jq -r '.controller.content' > /tmp/henos_tutorial/synthesized_controller.ctxdsl
```

### 5.3 Unrealizable Formula

Test with an unrealizable formula to see error handling. Create `/tmp/henos_tutorial/unrealizable_example.ctxdsl`:

```ctxdsl
context UnrealizableExample {
    alphabet {
        label request;
        label grant;
    }
    automata {
        automaton Process {
            controllable {
                label grant;
            }
            states {
                state Idle initial;
                state Waiting;
            }
            transitions {
                transition Idle -> Waiting on label request;
                transition Waiting -> Waiting on label request;
            }
        }
    }
    mu_formulas {
        formula must_grant {
            over Process;
            body = mu X. (<grant>true || <request>X);
        }
    }
}
```

Attempt synthesis:

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/unrealizable_example.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"unrealizable_example.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Process\",
    \"formula\": \"must_grant\",
    \"options\": {
      \"minimize\": false,
      \"diagnostics\": {
        \"counterexample\": true,
        \"deadlock_traces\": true
      }
    }
  }" | jq .
```

Expected response (formula is unrealizable):
```json
{
  "success": true,
  "realizable": false,
  "controller": null,
  "diagnostics": {
    "messages": ["Formula is unrealizable"],
    "violating_initials": ["Waiting"],
    "counterexample_trace": ["Waiting"],
    "counterstrategy_traces": [],
    "deadlock_traces": [],
    "minimization": null,
    "proof_obligations": []
  }
}
```

---

## 6. Context with Sidecars

### 6.1 Create Main Context and Sidecar

Create `/tmp/henos_tutorial/main_context.ctxdsl`:

```ctxdsl
context MainContext {
    alphabet {
        label action;
    }
    automata {
        automaton Main {
            states {
                state S0 initial;
                state S1;
            }
            transitions {
                transition S0 -> S1 on label action;
            }
        }
    }
}
```

Create `/tmp/henos_tutorial/properties.ctxdsl`:

```ctxdsl
context Properties {
    mu_formulas {
        formula property1 {
            over Main;
            body = [action]false;
        }
    }
}
```

### 6.2 Summarize with Sidecars

```bash
MAIN_CONTENT=$(cat /tmp/henos_tutorial/main_context.ctxdsl | jq -Rs .)
PROPS_CONTENT=$(cat /tmp/henos_tutorial/properties.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/summarize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"main_context.ctxdsl\",
      \"content\": ${MAIN_CONTENT}
    },
    \"sidecars\": [
      {
        \"name\": \"properties.ctxdsl\",
        \"content\": ${PROPS_CONTENT}
      }
    ],
    \"format\": \"json\"
  }" | jq .
```

Expected response should show `formulas_count: 1` from the sidecar.

---

## 7. Graph Visualization with Variables

### 7.1 Create Context with Variables

Create `/tmp/henos_tutorial/counter_with_vars.ctxdsl`:

```ctxdsl
context Counter {
    alphabet {
        label begin;
        label increment;
        label finish;
    }
    automata {
        automaton Process {
            variables {
                var count : i64 = 0;
            }
            states {
                state Start initial;
                state Counting;
                state Done;
            }
            transitions {
                transition Start -> Counting on label begin;
                transition Counting -> Counting on label increment
                    effects { count = count + 1; };
                transition Counting -> Done on label finish
                    guard count >= 3;
            }
        }
    }
}
```

### 7.2 Generate Unrolled Graph

Request unrolled graph visualization:

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/counter_with_vars.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/graphs \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"counter_with_vars.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Process\",
    \"graph_types\": [\"unrolled\"]
  }" | jq '.graphs[0].metadata'
```

The unrolled graph should show abstract states with variable values encoded in state names (e.g., `Start_count_0`, `Counting_count_0`, `Counting_count_1`, etc.).

### 7.3 Request Both Graph Types

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/counter_with_vars.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/graphs \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"counter_with_vars.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Process\",
    \"graph_types\": [\"dsl\", \"unrolled\"]
  }" | jq '.graphs | map({automaton, graph_type, metadata: .metadata.states_count})'
```

This returns two graph objects, one for each type.

---

## 8. Error Handling

### 8.1 Invalid Context Syntax

Test error handling with invalid context:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/context/summarize \
  -H "Content-Type: application/json" \
  -d '{
    "context": {
      "name": "invalid.ctxdsl",
      "content": "context invalid { invalid syntax here"
    },
    "sidecars": [],
    "format": "json"
  }' | jq .
```

Expected error response:
```json
{
  "success": false,
  "error": {
    "code": "BAD_REQUEST",
    "message": "Failed to parse context: ...",
    "details": "..."
  }
}
```

### 8.2 Unknown Automaton

Test with unknown automaton name:

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/traffic_light.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"traffic_light.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"NonExistent\",
    \"formula\": \"some_formula\",
    \"options\": {
      \"minimize\": false,
      \"diagnostics\": {}
    }
  }" | jq .
```

Expected error:
```json
{
  "success": false,
  "error": {
    "code": "BAD_REQUEST",
    "message": "Unknown automaton 'NonExistent'",
    "details": null
  }
}
```

### 8.3 Unknown Formula

```bash
CONTEXT_CONTENT=$(cat /tmp/henos_tutorial/traffic_light.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"traffic_light.ctxdsl\",
      \"content\": ${CONTEXT_CONTENT}
    },
    \"sidecars\": [],
    \"automaton\": \"Light\",
    \"formula\": \"non_existent_formula\",
    \"options\": {
      \"minimize\": false,
      \"diagnostics\": {}
    }
  }" | jq .
```

Expected error:
```json
{
  "success": false,
  "error": {
    "code": "BAD_REQUEST",
    "message": "Unknown formula 'non_existent_formula'",
    "details": null
  }
}
```

---

## 9. Complete Workflow Example

This section demonstrates a complete workflow: translate BPMN → summarize → synthesize → visualize.

### 9.1 Translate BPMN

Create `/tmp/henos_tutorial/workflow_example.json`:

```json
{
  "name": "ApprovalWorkflow",
  "states": [
    {"name": "Submitted", "initial": true},
    {"name": "Reviewing"},
    {"name": "Approved"},
    {"name": "Rejected"}
  ],
  "transitions": [
    {"from": "Submitted", "to": "Reviewing", "label": "start_review"},
    {"from": "Reviewing", "to": "Approved", "label": "approve"},
    {"from": "Reviewing", "to": "Rejected", "label": "reject"}
  ]
}
```

Translate:

```bash
BPMN_CONTENT=$(cat /tmp/henos_tutorial/workflow_example.json | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/translate/bpm \
  -H "Content-Type: application/json" \
  -d "{
    \"bpmn\": {
      \"content\": ${BPMN_CONTENT},
      \"format\": \"json\"
    },
    \"sidecars\": [],
    \"options\": {}
  }" | jq -r '.output.context.content' > /tmp/henos_tutorial/workflow_translated.ctxdsl
```

### 9.2 Add Properties Sidecar

Create `/tmp/henos_tutorial/workflow_properties.ctxdsl`:

```ctxdsl
context WorkflowProperties {
    mu_formulas {
        formula no_deadlocks {
            over Process;
            body = nu X. (true && [true]X);
        }
        formula eventually_decided {
            over Process;
            body = mu X. (state Approved || state Rejected || <true>X);
        }
    }
    controllers {
        controller SafetyController {
            automaton Process;
            formula no_deadlocks;
            minimize;
        }
    }
}
```

### 9.3 Summarize Complete Context

```bash
MAIN_CONTENT=$(cat /tmp/henos_tutorial/workflow_translated.ctxdsl | jq -Rs .)
PROPS_CONTENT=$(cat /tmp/henos_tutorial/workflow_properties.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/summarize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"workflow_translated.ctxdsl\",
      \"content\": ${MAIN_CONTENT}
    },
    \"sidecars\": [
      {
        \"name\": \"workflow_properties.ctxdsl\",
        \"content\": ${PROPS_CONTENT}
      }
    ],
    \"format\": \"json\"
  }" | jq '.summary'
```

### 9.4 Synthesize Controller

```bash
MAIN_CONTENT=$(cat /tmp/henos_tutorial/workflow_translated.ctxdsl | jq -Rs .)
PROPS_CONTENT=$(cat /tmp/henos_tutorial/workflow_properties.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/synthesize \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"workflow_translated.ctxdsl\",
      \"content\": ${MAIN_CONTENT}
    },
    \"sidecars\": [
      {
        \"name\": \"workflow_properties.ctxdsl\",
        \"content\": ${PROPS_CONTENT}
      }
    ],
    \"automaton\": \"Process\",
    \"formula\": \"no_deadlocks\",
    \"options\": {
      \"minimize\": true,
      \"diagnostics\": {
        \"counterexample\": true,
        \"deadlock_traces\": true,
        \"max_counter_traces\": 5
      }
    }
  }" | jq '{realizable, controller: .controller.name, diagnostics: .diagnostics.minimization}'
```

### 9.5 Generate Graphs

```bash
MAIN_CONTENT=$(cat /tmp/henos_tutorial/workflow_translated.ctxdsl | jq -Rs .)
PROPS_CONTENT=$(cat /tmp/henos_tutorial/workflow_properties.ctxdsl | jq -Rs .)
curl -X POST http://127.0.0.1:8080/api/v1/context/graphs \
  -H "Content-Type: application/json" \
  -d "{
    \"context\": {
      \"name\": \"workflow_translated.ctxdsl\",
      \"content\": ${MAIN_CONTENT}
    },
    \"sidecars\": [
      {
        \"name\": \"workflow_properties.ctxdsl\",
        \"content\": ${PROPS_CONTENT}
      }
    ],
    \"automaton\": null,
    \"graph_types\": [\"dsl\"]
  }" | jq '.graphs[] | {automaton, graph_type, states: .metadata.states_count, transitions: .metadata.transitions_count}'
```

---

## 10. Tips and Best Practices

### 10.1 Using jq for JSON Processing

The `jq` tool is highly recommended for working with JSON responses:

```bash
# Pretty-print response
curl ... | jq .

# Extract specific field
curl ... | jq '.summary.automata[0].name'

# Extract and save content
curl ... | jq -r '.output.context.content' > output.ctxdsl

# Count items in array
curl ... | jq '.graphs | length'

# Filter and transform
curl ... | jq '.graphs[] | {automaton, states: .metadata.states_count}'
```

### 10.2 Error Handling in Scripts

When scripting API calls, check the `success` field:

```bash
RESPONSE=$(curl -s -X POST ...)
SUCCESS=$(echo "$RESPONSE" | jq -r '.success')

if [ "$SUCCESS" = "true" ]; then
    echo "Request succeeded"
    echo "$RESPONSE" | jq '.summary'
else
    echo "Request failed:"
    echo "$RESPONSE" | jq '.error'
    exit 1
fi
```

### 10.3 Request Size Limits

For very large context files, consider:
- Breaking contexts into smaller pieces with sidecars
- Using file-based processing when possible
- Monitoring server logs for performance

### 10.4 Testing Different Graph Types

Remember:
- **DSL graphs**: Show the original Context DSL structure (always available)
- **Unrolled graphs**: Show abstract states with variable values (only available for automata with variables)

Request both types to compare:

```bash
curl ... | jq '.graphs[] | {graph_type, states: .metadata.states_count}'
```

---

## 11. Validation Checklist

Use this checklist to verify all endpoints are working correctly:

- [ ] Health check returns `{"status": "ok"}`
- [ ] Context summarize returns correct automaton counts
- [ ] Context graphs returns graph elements for DSL view
- [ ] BPMN translation generates valid ctxdsl
- [ ] Controller synthesis returns controller DSL for realizable formulas
- [ ] Controller synthesis returns `realizable: false` for unrealizable formulas
- [ ] Error handling returns proper error structure for invalid requests
- [ ] Sidecars are correctly merged with main context
- [ ] Graph generation works with both `dsl` and `unrolled` types
- [ ] All endpoints return consistent JSON structure

---

## 12. Troubleshooting

### Server Won't Start

**Problem**: Server fails to start with address already in use error.

**Solution**: 
```bash
# Check if port is in use
lsof -i :8080

# Use a different port
cargo run --features api --bin henos -- server --addr 127.0.0.1:8081
```

### Connection Refused

**Problem**: `curl` returns "Connection refused".

**Solution**:
- Verify server is running: `curl http://127.0.0.1:8080/api/v1/health`
- Check firewall settings
- Ensure you're using the correct address and port

### Invalid JSON Response

**Problem**: Response is not valid JSON.

**Solution**:
- Check server logs for errors
- Verify request content-type header: `Content-Type: application/json`
- Ensure request body is valid JSON

### Graph Generation Fails

**Problem**: Unrolled graph request fails.

**Solution**:
- Verify automaton has variable declarations
- Check that variables are properly initialized
- Ensure guards and effects use valid syntax

---

## 13. Next Steps

After validating the API:

1. **Integrate with Web Client**: Use the API endpoints from a web application
2. **Automate Workflows**: Create scripts that chain multiple API calls
3. **Performance Testing**: Test with larger context files
4. **Production Deployment**: Configure CORS, authentication, and HTTPS for production use

For more information:
- See [UI Integration Plan](ui/ui_integration_plan.md) for web client setup
- See [Server Implementation Plan](ui/server_implementation_plan.md) for API details
- See [CLI Validation Tutorial](cli_validation_tutorial.md) for CLI-based workflows

---

This tutorial demonstrates all major API endpoints and validates the HTTP server implementation. You can use these examples as a foundation for building web-based tools that interact with HENOS.
