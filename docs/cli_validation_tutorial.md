# CLI Validation Tutorial

Hands-on walkthrough for validating MUNUNU via the CLI and a text editor (e.g., `nano`, `vim`, VS Code). It assumes you have cloned the official GitHub repository and can run `mununu` from its root directory (with Cargo installed). Each section builds on files stored in a temporary working directory—feel free to adapt paths to your environment.

---

## 1. Prerequisites

- Rust toolchain installed (`cargo` on PATH)
- GitHub access: `git clone https://github.com/vscorza/mununu.git`
- The `mununu` CLI available (run from repo root with `cargo run --bin mununu -- …`)
- A text editor for editing `.ctxdsl` files

Throughout the tutorial we use `WORKDIR=/tmp/mununu_tutorial`. Create it up front:

```bash
mkdir -p /tmp/mununu_tutorial
cd mununu
```

---

## 2. Basic CLTS DSL Modeling

### 2.1 Create a minimal context

Open your editor and create `/tmp/mununu_tutorial/traffic_light.ctxdsl` with the following content:

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

The `predicates` block names state predicates (`is_green`, `is_yellow`) that become available to every μ-calculus formula in this automaton. Predicates defined this way behave like atomic propositions: the runtime automatically marks the corresponding states and keeps those bitsets consistent under composition.

### 2.2 Sanity-check via CLI

```bash
cargo run --bin mununu -- context summarize /tmp/mununu_tutorial/traffic_light.ctxdsl
```

You should see state/transition counts for automaton `Light`.

**Inspecting internal structure:**

To see a detailed view of the materialized CLTS structure (states, transitions, labels, controllability), use `--print-structure`:

```bash
cargo run --bin mununu -- context summarize /tmp/mununu_tutorial/traffic_light.ctxdsl --print-structure
```

This prints the JSON summary followed by a compact representation showing:
- Global variables and controllable alphabet
- All automata with state counts, initial states, variables, and labels
- Transition summaries (total, controllable, uncontrollable)
- Detailed state information with outgoing/incoming transition counts
- Per-transition details (source, target, labels, controllability kind)

To write the structure to a file instead of stdout:

```bash
cargo run --bin mununu -- context summarize /tmp/mununu_tutorial/traffic_light.ctxdsl --print-structure /tmp/mununu_tutorial/traffic_light_structure.txt
```

### 2.3 Graph Visualization with Cytoscape

MUNUNU can generate interactive graph visualizations of automata using Cytoscape.js. This is useful for understanding the structure of your automata, especially when working with complex models or unrolled representations.

#### 2.3.1 Basic DSL Graph Visualization

Generate a graph visualization of the DSL automata (the original Context DSL representation):

```bash
cargo run --bin mununu -- context graph \
  /tmp/mununu_tutorial/traffic_light.ctxdsl \
  --type dsl \
  --output /tmp/mununu_tutorial/traffic_light_dsl.html
```

Open the generated HTML file in your browser to see an interactive visualization showing:
- Automaton compound nodes with variables and actions metadata
- States (ellipses) with initial states marked with double borders
- Terminal states (dead states) shown as rounded rectangles
- Transitions with labels, guards, and effects
- Action types distinguished by line style (solid for controllable, dashed for uncontrollable, dotted for internal)

#### 2.3.2 Unrolled Graph Visualization

For automata with variables, you can visualize the unrolled representation where variable values are encoded directly into state names. **Note**: Unrolling requires that the DSL file contains variable declarations. If the automaton was already unrolled by the BPM translator, it won't have variables and unrolling will not be possible.

Create a simple DSL file with variables:

```bash
cat <<'CTXDSL' > /tmp/mununu_tutorial/counter_with_vars.ctxdsl
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
CTXDSL

# Generate unrolled graph
cargo run --bin mununu -- context graph \
  /tmp/mununu_tutorial/counter_with_vars.ctxdsl \
  --type unrolled \
  --output /tmp/mununu_tutorial/counter_unrolled.html
```

The unrolled visualization shows:
- Abstract states with variable values encoded in state names (e.g., `Counting_count_0`, `Counting_count_1`)
- Transitions between abstract states
- How guards and effects are resolved during unrolling

**Important**: If you get an error saying "has no variables to unroll", it means the automaton was already processed (e.g., by the BPM translator with unrolling enabled) and variables are already encoded in state names. In that case, use `--type dsl` to visualize the already-unrolled representation.

#### 2.3.3 Both DSL and Unrolled Representations

To see both the original DSL representation and the unrolled representation side-by-side:

```bash
cargo run --bin mununu -- context graph \
  /tmp/mununu_tutorial/counter_out/Counter.ctxdsl \
  --type both \
  --output /tmp/mununu_tutorial/counter_both.html
```

This generates a single HTML file with both automata visualizations, making it easy to compare the original model with its unrolled expansion.

#### 2.3.4 Visualizing Specific Automata

To visualize only a specific automaton from a context with multiple automata:

```bash
cargo run --bin mununu -- context graph \
  /tmp/mununu_tutorial/traffic_light.ctxdsl \
  --type dsl \
  --automaton Light \
  --output /tmp/mununu_tutorial/traffic_light_light_only.html
```

#### 2.3.5 Graph Visualization with Sidecars

When working with contexts that have sidecars (e.g., guard predicates, properties), include them for complete visualization:

```bash
cargo run --bin mununu -- context graph \
  /tmp/mununu_tutorial/counter_out/Counter.ctxdsl \
  --sidecar /tmp/mununu_tutorial/counter_out/Counter_guards.ctxdsl \
  --sidecar /tmp/mununu_tutorial/counter_out/Counter_properties.ctxdsl \
  --type dsl \
  --output /tmp/mununu_tutorial/counter_with_sidecars.html
```

**Key Features of Graph Visualization:**
- **Interactive**: Pan and zoom in the browser
- **Styled by Action Type**: Controllable actions (solid lines), uncontrollable (dashed), internal (dotted)
- **State Markers**: Initial states have double borders, terminal states are rounded rectangles
- **Metadata Display**: Automaton nodes show variables and actions
- **Transition Labels**: Show action names, guards, and effects
- **Unrolled States**: Variable values are visible in state names for unrolled representations

**Use Cases:**
- Understanding automaton structure during development
- Debugging guard conditions and effects
- Visualizing state space expansion during unrolling
- Documenting automata for presentations or reports
- Comparing DSL vs unrolled representations

---

## 3. Declaring controllable/internal labels per automaton

The DSL lets each automaton declare which labels it controls or treats as internal. Ownership is exclusive across automata; declaring the same label as controllable or internal in more than one automaton is rejected.

```ctxdsl
automaton Producer {
    controllable { label produce; }
    internal { label tick; }
    states { state idle initial; state busy; }
    transitions {
        transition idle -> busy on label produce;
        transition busy -> busy on label tick;
    }
}

automaton Consumer {
    controllable { } // no owned labels; its transitions are uncontrollable
    internal { }
    states { state wait initial; state done; }
    transitions { transition wait -> done on label produce; }
}
```

If `controllable`/`internal` blocks are omitted, controllability falls back to legacy inference (epsilon or input-signal labels/guards ⇒ uncontrollable; otherwise controllable).

Try it: summarize the provided example

```bash
cargo run --bin mununu -- context summarize examples/manual/controllability_demo.ctxdsl
```

You should see `Producer` and `Consumer` automata (2 states, 2 transitions each) with no controllability conflicts, reflecting the explicit ownership in the file.

---

## 4. Property Verification

Add a sidecar with simple safety/liveness formulas. Create `/tmp/mununu_tutorial/traffic_light_props.ctxdsl`:

```ctxdsl
context traffic_light_props {
    mu_formulas {
        // Safety: whenever we're in Green, the next step must be Yellow
        formula green_then_yellow {
            over Light;
            body = nu Always. ((is_green -> <next> is_yellow) && [] Always);
        }

        // Liveness (existential): there is always a path back to Green
        formula eventually_green {
            over Light;
            body = mu ReachGreen. (is_green || <next> ReachGreen);
        }
    }
}
```

Evaluate each formula:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/traffic_light.ctxdsl \
  --sidecar /tmp/mununu_tutorial/traffic_light_props.ctxdsl \
  --formula green_then_yellow \
  --automaton Light

cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/traffic_light.ctxdsl \
  --sidecar /tmp/mununu_tutorial/traffic_light_props.ctxdsl \
  --formula eventually_green \
  --automaton Light
```

**Inspecting structure during evaluation:**

You can also print the internal CLTS structure when evaluating formulas:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/traffic_light.ctxdsl \
  --sidecar /tmp/mununu_tutorial/traffic_light_props.ctxdsl \
  --formula eventually_green \
  --automaton Light \
  --print-structure
```

This is useful for debugging formula evaluation issues or understanding how the model is materialized internally.

`eventually_green` demonstrates how a least fixpoint (`mu ReachGreen. ...`) repeatedly applies the diamond modality to explore longer paths. In deterministic models this matches the usual "eventually" requirement. For universally quantified liveness (all branches must lead back to Green) wrap the reachability clause inside a `nu Always. (...)` skeleton so every successor must continue satisfying the obligation.

### 4.1 Fairness violation with counterexample and counterstrategy

To see how controller synthesis surfaces an unrealizable *fairness* property (with both a counterexample trace and a counterstrategy), you can reuse the bundled example `examples/manual/core/req_grant_unfair.ctxdsl`. It encodes a simple service automaton where:

- `req1` holds whenever a request is pending,
- `grant1` holds whenever the first grant has been issued,
- `grant2` is a second grant that is **never** realised.

The LTL property in the file is:

```ctxdsl
formula fairness_req1_grants {
    over Service;
    body = ltl (G F req1 -> (G F grant1 && G F grant2));
}
```

This is the textual form of the requirement

> []<> req1 -> ([]<> grant1 && []<> grant2)

i.e. *if `req1` holds infinitely often, then both `grant1` and `grant2` must also hold infinitely often*. Because the `Service` automaton never reaches the `grant2` predicate, the property is unrealizable and synthesis reports a counterexample and prototype counterstrategy.

Run controller synthesis with diagnostics enabled:

```bash
cargo run --bin mununu -- context synth \
  examples/manual/core/req_grant_unfair.ctxdsl \
  --formula fairness_req1_grants \
  --automaton Service \
  --counterexample \
  --max-counter-traces 3
```

Typical output:

```text
Controller synthesis for formula 'fairness_req1_grants' over automaton 'Service':
  Realizable: no
  Controller states: 0 (initial: 0)
  Alphabet: (none)
  Structural hash: 0x53a1c347f384f7ce
  Diagnostics:
    note: Controller unrealizable: initial state(s) Idle do not satisfy the specification.
    violating initials: Idle
    counterexample trace: Idle
    counterstrategy trace #0: Idle
    proof obligations: 1
    counterstrategy states: G1, Idle, Req1
```

The `counterexample trace` and `counterstrategy` entries give you a concrete witness for why the property cannot be enforced: there is a way for the system to visit `req1` infinitely often while never visiting the `grant2` predicate.

### 3.1 Predicates under composition

Predicates remain available even when automata participate in synchronous/asynchronous compositions. For example, create `/tmp/mununu_tutorial/composed.ctxdsl`:

```ctxdsl
context composed_demo {
    alphabet {
        label master_tick;
        label worker_tick;
    }

    automata {
        automaton Master {
            states { state Idle initial; state Busy; }
            transitions {
                transition Idle -> Busy on label master_tick;
                transition Busy -> Idle on label master_tick;
            }
            predicates { predicate master_idle = state Idle; }
        }

        automaton Worker {
            states { state Wait initial; state Run; }
            transitions {
                transition Wait -> Run on label worker_tick;
                transition Run -> Wait on label worker_tick;
            }
            predicates { predicate worker_wait = state Wait; }
        }
    }

    composition {
        synchronous Duo { members [Master, Worker]; }
    }

    mu_formulas {
        formula master_idle_check { over Master; body = master_idle; }
        formula duo_initial_check { over Duo; body = master_idle && worker_wait; }
    }
}
```

Then evaluate:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/composed.ctxdsl \
  --formula master_idle_check \
  --automaton Master
```

The command reports the `Idle` state as satisfying the formula, proving that user-declared predicates survive composition and remain available to μ-calculus properties.

**Evaluating formulas over composed automata:**

You can also evaluate formulas directly over the composed automaton. The `duo_initial_check` formula verifies that both Master and Worker are in their initial states simultaneously:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/composed.ctxdsl \
  --formula duo_initial_check \
  --automaton Duo
```

**Inspecting the structure of composed automata:**

To see the detailed internal structure of the composed Duo automaton (states, transitions, labels, controllability), use `--print-structure`:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/composed.ctxdsl \
  --formula duo_initial_check \
  --automaton Duo \
  --print-structure
```

This prints the evaluation results followed by a compact representation showing:
- Global variables and controllable alphabet
- The Duo automaton with state counts, initial states, variables, and labels
- Transition summaries (total, controllable, uncontrollable)
- Detailed state information with outgoing/incoming transition counts
- Per-transition details (source, target, labels, controllability kind)

To write the structure to a file instead of stdout:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/composed.ctxdsl \
  --formula duo_initial_check \
  --automaton Duo \
  --print-structure /tmp/mununu_tutorial/duo_structure.txt
```

This is particularly useful for understanding how the synchronous composition materializes the product states and transitions.

### 3.2 Blocking Behavior in Asynchronous Composition

Asynchronous composition allows automata to interleave their transitions independently. However, when automata share labels and need to coordinate, blocking can occur if one automaton moves past the point where it can participate in a shared action that another automaton still needs.

Create `/tmp/mununu_tutorial/blocking_example.ctxdsl`:

```ctxdsl
context blocking_demo {
    alphabet {
        label shared_action;
        label action_a;
        label action_b;
    }

    automata {
        automaton A {
            controllable { label action_a; }
            internal { }
            states {
                state StartA initial;
                state MiddleA;
                state WaitingA;
                state EndA;
            }
            transitions {
                transition StartA -> MiddleA on label shared_action;
                transition MiddleA -> WaitingA on label action_a;
                transition WaitingA -> EndA on label shared_action;
            }
        }

        automaton B {
            controllable { label action_b; }
            internal { }
            states {
                state StartB initial;
                state MiddleB;
                state EndB;
            }
            transitions {
                transition StartB -> MiddleB on label shared_action;
                transition MiddleB -> EndB on label action_b;
            }
        }
    }

    composition {
        synchronous Composed { members [A, B]; }
    }
}
```

**Note on controllability**: The `shared_action` label is not declared as controllable in either automaton, so it defaults to uncontrollable (environment-controlled). This is appropriate for synchronization labels that require coordination between automata. The automata explicitly declare their own unique labels (`action_a` and `action_b`) as controllable to avoid conflicts.

**Scenario**:
1. Both `A` and `B` start in their initial states (`StartA`, `StartB`)
2. They both use `shared_action` **synchronously** to move to their middle states (`MiddleA`, `MiddleB`)
   - In synchronous composition, both automata must fire `shared_action` simultaneously
3. `A` uses `action_a` to reach `WaitingA` (this is independent, no synchronization needed)
4. `B` uses `action_b` to reach `EndB` (this is independent, no synchronization needed)
5. `A` needs `shared_action` again to reach `EndA`, but `B` is now in `EndB` where `shared_action` is not available
6. Since `shared_action` requires **synchronous** firing in both automata, and `B` cannot provide it from `EndB`, `A` is **blocked** in `WaitingA`

**Key Insight**: This demonstrates blocking in synchronous composition. In gateway scenarios with asynchronous composition, branch automata need self-loops for shared labels in their idle states to prevent similar blocking—allowing other processes to use shared labels even when the branch is not active.

Summarize and inspect the structure:

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/blocking_example.ctxdsl \
  --print-structure
```

You can verify the blocking by checking reachability. First, create `/tmp/mununu_tutorial/blocking_properties.ctxdsl`:

```ctxdsl
context blocking_properties {
    mu_formulas {
        formula can_reach_end_a {
            over Composed;
            body = mu ReachEndA. (EndA || <> ReachEndA);
        }
        
        formula a_blocked_in_waiting {
            over Composed;
            body = nu Always. ((!WaitingA || [] WaitingA) && [] Always);
        }
    }
}
```

Then evaluate:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/blocking_example.ctxdsl \
  --sidecar /tmp/mununu_tutorial/blocking_properties.ctxdsl \
  --formula can_reach_end_a \
  --automaton Composed \
  --print-structure
```

The formula evaluation will show which states can reach `EndA`. Inspect the structure output to see the reachable states and understand the composition behavior.

**Understanding the blocking scenario**:
- In the composed automaton, valid traces include:
  - `StartA|StartB → MiddleA|MiddleB` (both fire `shared_action` synchronously)
  - `MiddleA|MiddleB → WaitingA|MiddleB` (`A` fires `action_a` independently)
  - `WaitingA|MiddleB → WaitingA|EndB` (`B` fires `action_b` independently)
- From `WaitingA|EndB`, `A` needs `shared_action` but `B` is in `EndB` where `shared_action` is not available
- Since `shared_action` requires **synchronous** firing in both automata, `A` cannot progress
- The state `EndA|EndB` is **unreachable** because `A` is blocked in `WaitingA`

**Connection to Gateway Semantics**: This blocking scenario illustrates why branch automata in gateways need self-loops for shared labels in their idle states. In asynchronous gateway composition:
- If a branch automaton is in `Idle` (before fork or after join) and doesn't have a self-loop for a shared label
- Other processes that need that shared label can become blocked
- Adding `Idle → Idle` self-loops for shared labels allows other processes to use them independently, preventing blocking

**Verification**: The structure output will show that `EndA|EndB` is not reachable, confirming that `A` is blocked in `WaitingA` due to the missing `shared_action` transition in `B`'s `EndB` state.

**Key Insight**: In asynchronous composition, if automaton `A` needs a shared label `L` in state `S`, but automaton `B` (which also uses `L`) has moved to a state where `L` is no longer available, then `A` becomes blocked. This is why branch automata in gateways need self-loops for shared labels in their idle states.

---

## 4. LTL → μ-Calculus Patterns (Synchronous & Asynchronous)

### 4.1 Synchronous example (`examples/manual/gr1/elevator`)

Safety (`G (door_open ⇒ at_floor)`):

```
nu X. ((!open_door) || (Floor1_Open || Floor2_Open || Floor3_Open)) && [] X
```

Evaluate using the shipped example:

```bash
cargo run --bin mununu -- context eval \
  examples/manual/gr1/elevator/elevator.ctxdsl \
  --sidecar examples/manual/gr1/elevator/elevator_properties.ctxdsl \
  --formula door_safety \
  --automaton lift
```

Liveness (`G (call ⇒ F arrive)`):

```bash
cargo run --bin mununu -- context eval \
  examples/manual/gr1/elevator/elevator.ctxdsl \
  --sidecar examples/manual/gr1/elevator/elevator_properties.ctxdsl \
  --formula serve_floor1 \
  --automaton lift
```

### 4.2 Asynchronous example (`tests/data/bpm/realizable/order_approval.json`)

Translate and review:

```bash
cargo run --bin mununu -- translate bpm \
  --input tests/data/bpm/realizable/order_approval.json \
  --output /tmp/mununu_tutorial/order_approval \
  --name OrderApproval \
  --force

cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/order_approval/OrderApproval.ctxdsl \
  --sidecar /tmp/mununu_tutorial/order_approval/OrderApproval_properties.ctxdsl \
  --sidecar /tmp/mununu_tutorial/order_approval/OrderApproval_bpmn_structural.ctxdsl
```

Look for formulas like `no_deadlocks`, `response_within_deadline`, etc., that correspond to classical LTL patterns.

---

## 5. Diagnosis for Unrealizable Specifications

Use the elevator GR(1) spec to demonstrate counterexamples:

```bash
cargo run --bin mununu -- context synth \
  examples/manual/gr1/elevator/elevator.ctxdsl \
  --sidecar examples/manual/gr1/elevator/elevator_properties.ctxdsl \
  --formula gr1_spec \
  --automaton lift \
  --counterexample \
  --deadlock-traces \
  --dump-json /tmp/mununu_tutorial/elevator_cex.json \
  --dump-diagnostics /tmp/mununu_tutorial/elevator_diag.ctxdsl
```

**Inspecting structure during synthesis:**

To see the internal CLTS structure when synthesizing controllers:

```bash
cargo run --bin mununu -- context synth \
  examples/manual/gr1/elevator/elevator.ctxdsl \
  --sidecar examples/manual/gr1/elevator/elevator_properties.ctxdsl \
  --formula gr1_spec \
  --automaton lift \
  --print-structure /tmp/mununu_tutorial/elevator_structure.txt
```

This helps understand the controller structure and diagnose synthesis issues.

If the spec is unrealizable, MUNUNU prints “Realizable: no” and writes diagnostics describing the failing obligation. Inspect `/tmp/mununu_tutorial/elevator_diag.ctxdsl` in your editor to drill into traces and offending states.

To experiment with an unrealizable variant, edit `elevator_properties.ctxdsl` to tighten a liveness clause (e.g., require serving a floor without allowing movement) and rerun the command to observe the new counterexample.

---

## 6. BPM Translation Walkthrough

### 6.1 Custom BPM Prototype

1. **Author BPMN JSON** with a text editor. The following self-contained example is enough to exercise translation, structural predicates, and response templates:

    ```bash
    mkdir -p /tmp/mununu_tutorial
    cat <<'JSON' > /tmp/mununu_tutorial/custom_process.json
    {
      "name": "custom_process",
      "states": [
        { "name": "Intake", "initial": true },
        { "name": "Review" },
        { "name": "Escalated" },
        { "name": "Completed" }
      ],
      "variables": [
        { "name": "requires_escalation", "type": "bool", "initial": false }
      ],
      "transitions": [
        { "from": "Intake", "to": "Review", "label": "submit" },
        {
          "from": "Review",
          "to": "Escalated",
          "label": "escalate",
          "guard": "requires_escalation"
        },
        { "from": "Review", "to": "Completed", "label": "approve" },
        { "from": "Escalated", "to": "Completed", "label": "resolve" }
      ],
      "properties": [
        {
          "name": "review_eventually_completes",
          "pattern": "response",
          "over": "Process",
          "trigger": "Review",
          "response": "Completed",
          "comment": "Every review should eventually finish."
        }
      ]
    }
    JSON
    ```

2. **Translate**:

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/custom_process.json \
  --output /tmp/mununu_tutorial/custom_out \
  --name CustomProcess \
  --force
```

**Inspecting translation structure:**

To see the internal CLTS structure immediately after translation:

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/custom_process.json \
  --output /tmp/mununu_tutorial/custom_out \
  --name CustomProcess \
  --force \
  --print-structure
```

Or write it to a file:

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/custom_process.json \
  --output /tmp/mununu_tutorial/custom_out \
  --name CustomProcess \
  --force \
  --print-structure /tmp/mununu_tutorial/custom_structure.txt
```

This helps verify that the translation produced the expected states and transitions, especially when unrolling is applied for models with variables.

3. **Inspect outputs**:
   - `CustomProcess.ctxdsl`: main automaton
   - `CustomProcess_bpmn_structural.ctxdsl`: structural predicates (`Process_is_completion_state`, etc.)
   - `CustomProcess_properties.ctxdsl`: translator-emitted response properties

4. **Summarize**:

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/custom_out/CustomProcess.ctxdsl \
  --sidecar /tmp/mununu_tutorial/custom_out/CustomProcess_properties.ctxdsl \
  --sidecar /tmp/mununu_tutorial/custom_out/CustomProcess_bpmn_structural.ctxdsl
```

**With structure inspection:**

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/custom_out/CustomProcess.ctxdsl \
  --sidecar /tmp/mununu_tutorial/custom_out/CustomProcess_properties.ctxdsl \
  --sidecar /tmp/mununu_tutorial/custom_out/CustomProcess_bpmn_structural.ctxdsl \
  --print-structure
```

This shows both the JSON summary and the detailed internal structure, which is particularly useful for understanding how variables were unrolled into states.

5. **Verify with property library** (optional):

```bash
cargo run --bin mununu -- mining verify \
  --cache-dir /tmp/mununu_tutorial/cache \
  --output-dir /tmp/mununu_tutorial/results \
  --workers 1
```

*(Populate the cache via `mununu mining fetch` or by copying BPMN files beforehand.)*

### 6.2 Industrial Composition + Verification

1. **Copy the industrial fixture** (ships with the repo) into the tutorial workspace:

    ```bash
    mkdir -p /tmp/mununu_tutorial
    cp tests/data/bpm/industrial/sterile_batch_release.json \
       /tmp/mununu_tutorial/
    ```

2. **Translate the full process** (do this from the repo root so relative paths resolve):

    ```bash
    cargo run --bin mununu -- translate bpm \
      --input /tmp/mununu_tutorial/sterile_batch_release.json \
      --output /tmp/mununu_tutorial/sterile_out \
      --name SterileBatchRelease \
      --force
    ```

    The translator logs all generated DSL artifacts (`SterileBatchRelease.ctxdsl`, `*_guards`, `*_properties`, `*_bpmn_structural`).

3. **Start a composition session** directly from the source BPMN (the compose CLI replays the translation pipeline and tracks artefacts/scopes for you):

    ```bash
    cargo run --bin mununu -- compose start \
      --artifact /tmp/mununu_tutorial/sterile_batch_release.json \
      --name sterile_session
    ```

4. **Preview and materialise composed outputs**:

    ```bash
    # Inspect contexts/sidecars (writes them under /tmp/mununu_tutorial/sterile_preview)
    cargo run --bin mununu -- compose preview \
      --session sterile_session.compose-session.json \
      --output /tmp/mununu_tutorial/sterile_preview

    # Emit final artefacts + manifest for downstream tools
    cargo run --bin mununu -- compose write \
      --output /tmp/mununu_tutorial/sterile_composed \
      --force \
      --manifest
    ```

    The composed folder contains:

    - `contexts/01_sterile_batch_release.ctxdsl`
    - `sidecars/01_sterile_batch_release_{guards,properties,bpmn_structural}.ctxdsl`
    - `session_manifest.json` (if `--manifest` was set)

5. **Summarize and verify**:

    ```bash
    cargo run --bin mununu -- context summarize \
      /tmp/mununu_tutorial/sterile_composed/contexts/01_sterile_batch_release.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_guards.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_properties.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_bpmn_structural.ctxdsl

    cargo run --bin mununu -- context eval \
      /tmp/mununu_tutorial/sterile_composed/contexts/01_sterile_batch_release.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_guards.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_properties.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_bpmn_structural.ctxdsl \
      --formula guard_valid \
      --automaton Process

    cargo run --bin mununu -- context eval \
      /tmp/mununu_tutorial/sterile_composed/contexts/01_sterile_batch_release.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_guards.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_properties.ctxdsl \
      --sidecar /tmp/mununu_tutorial/sterile_composed/sidecars/01_sterile_batch_release_bpmn_structural.ctxdsl \
      --formula quality_review_leads_to_decision \
      --automaton Process
    ```

    - `guard_valid` should report **8/8** states satisfying (all guard invariants hold).
    - `quality_review_leads_to_decision` currently reports **0/8** satisfying states.

6. **Diagnostic notes for the failing property**:

    - The auto-generated response is encoded as `nu X. (ReleaseDecision || (QualityReview && X))`. Because it never introduces a modal `<>`, the only states that can satisfy it are `ReleaseDecision` itself—no other state (including the initial `Preparation`) can join the greatest fixpoint. This is why the CLI prints `States satisfying: 0/8`.
    - Inspecting the `sterile_batch_release_properties.ctxdsl` snippet (emitted during preview) confirms the absence of a reachability clause. To model “eventually reach a release decision” you need a least-fixpoint diamond, e.g.
      `nu Always. (!QualityReview || mu ReachDecision. (ReleaseDecision || <next> ReachDecision)) && [] Always;`
    - Until that formula is amended (either manually or by tweaking the translator template), treat the failing result as expected. Capture the CLI output (`context summarize`, `context eval`) in triage notes so other reviewers understand why the industrial property is pessimistic.

---

## 7. Multi-Process Collaboration (Message Flows)

To validate basic multi-process collaboration support (Issue 8 in the BPMN spec), you can use a JSON model with multiple `processes` and `message_flows`:

```json
{
  "name": "Collab",
  "processes": [
    {
      "name": "P1",
      "states": [
        {"name": "A", "initial": true},
        {"name": "A_done"}
      ],
      "transitions": [
        {"from": "A", "to": "A_done", "label": "p1_do"}
      ]
    },
    {
      "name": "P2",
      "states": [
        {"name": "B", "initial": true},
        {"name": "B_done"}
      ],
      "transitions": [
        {"from": "B", "to": "B_done", "label": "p2_do"}
      ]
    }
  ],
  "message_flows": [
    {"id": "msg1", "name": "notify", "source_ref": "A_done", "target_ref": "B"},
    {"id": "msg2", "name": "ping",   "source_ref": "A_done", "target_ref": "B"}
  ]
}
```

1. **Translate to CLTS context**:

```bash
cargo run --bin mununu -- translate bpm \
  /tmp/mununu_tutorial/collab.json \
  --out /tmp/mununu_tutorial/collab.ctxdsl
```

2. **Inspect composition and message labels**:

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/collab.ctxdsl
```

- You should see automata `P1` and `P2`.
- The alphabet should contain `msg_notify` and `msg_ping`.
- The composition metadata (printed in the summary) should list a `combined` composition with `sync_labels` including the `msg_*` labels.

This mirrors the automated check in `tests/multi_process_collaboration.rs` and provides a CLI path to validate multi-process message-flow wiring end-to-end.

---

## 8. Gateway Examples

BPMN gateways control the flow of execution through conditional branching and parallel execution. MUNUNU supports XOR (exclusive), AND (parallel), and OR (inclusive) gateways.

### 8.1 XOR Gateway (Exclusive Gateway)

XOR gateways select exactly one path based on guard conditions. Create `/tmp/mununu_tutorial/xor_gateway.json`:

```json
{
  "name": "priority_routing",
  "states": [
    { "name": "Request", "initial": true },
    { "name": "Decision" },
    { "name": "HighPriority" },
    { "name": "LowPriority" },
    { "name": "Completed" }
  ],
  "variables": [
    { "name": "priority", "type": "i64", "initial": 7 }
  ],
  "transitions": [
    { "from": "Request", "to": "Decision", "label": "evaluate" },
    {
      "from": "Decision",
      "to": "HighPriority",
      "label": "route_high",
      "guard": "priority > 5"
    },
    {
      "from": "Decision",
      "to": "LowPriority",
      "label": "route_low",
      "guard": "priority <= 5"
    },
    { "from": "HighPriority", "to": "Completed", "label": "finish" },
    { "from": "LowPriority", "to": "Completed", "label": "finish" }
  ]
}
```

**Translate and inspect:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/xor_gateway.json \
  --output /tmp/mununu_tutorial/xor_out \
  --name PriorityRouting \
  --force \
  --print-structure
```

**Verify gateway behavior:**

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/xor_out/PriorityRouting.ctxdsl \
  --print-structure
```

You should see:
- The `Decision` state (gateway)
- Two guarded transitions from `Decision` (one to `HighPriority`, one to `LowPriority`)
- Both paths converge to `Completed`

**Key points:**
- XOR gateways create multiple guarded transitions from a single state
- Guards should be mutually exclusive to ensure deterministic routing
- The structure output shows all possible paths through the gateway

### 8.2 AND Gateway (Parallel Gateway)

AND gateways execute all branches concurrently. Create `/tmp/mununu_tutorial/parallel_gateway.json`:

```json
{
  "name": "parallel_processing",
  "states": [
    { "name": "Start", "initial": true },
    { "name": "Fork" },
    { "name": "BranchA" },
    { "name": "BranchB" },
    { "name": "Join" },
    { "name": "Complete" }
  ],
  "transitions": [
    { "from": "Start", "to": "Fork", "label": "begin" },
    { "from": "Fork", "to": "BranchA", "label": "fork_a" },
    { "from": "Fork", "to": "BranchB", "label": "fork_b" },
    { "from": "BranchA", "to": "Join", "label": "join_a" },
    { "from": "BranchB", "to": "Join", "label": "join_b" },
    { "from": "Join", "to": "Complete", "label": "finish" }
  ],
  "gateways": [
    {
      "id": "fork_gateway",
      "name": "Fork",
      "gateway_type": "and",
      "branches": [
        { "target": "BranchA", "label": "fork_a" },
        { "target": "BranchB", "label": "fork_b" }
      ]
    },
    {
      "id": "join_gateway",
      "name": "Join",
      "gateway_type": "and",
      "branches": [
        { "target": "BranchA", "label": "join_a" },
        { "target": "BranchB", "label": "join_b" }
      ]
    }
  ]
}
```

**Translate and verify:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/parallel_gateway.json \
  --output /tmp/mununu_tutorial/parallel_out \
  --name ParallelProcessing \
  --force \
  --print-structure
```

**Key points:**
- AND gateways create separate automata for each branch
- The main automaton coordinates fork/join synchronization
- All branches must complete before the join state is enabled
- The structure output shows the composed automata with synchronization labels

### 8.3 OR Gateway (Inclusive Gateway)

OR gateways execute one or more branches based on conditions. Create `/tmp/mununu_tutorial/or_gateway.json`:

```json
{
  "name": "inclusive_routing",
  "states": [
    { "name": "Start", "initial": true },
    { "name": "Fork" },
    { "name": "PathA" },
    { "name": "PathB" },
    { "name": "Join" },
    { "name": "Complete" }
  ],
  "variables": [
    { "name": "condition_a", "type": "bool", "initial": true },
    { "name": "condition_b", "type": "bool", "initial": false }
  ],
  "transitions": [
    { "from": "Start", "to": "Fork", "label": "begin" },
    {
      "from": "Fork",
      "to": "PathA",
      "label": "route_a",
      "guard": "condition_a"
    },
    {
      "from": "Fork",
      "to": "PathB",
      "label": "route_b",
      "guard": "condition_b"
    },
    { "from": "PathA", "to": "Join", "label": "join_a" },
    { "from": "PathB", "to": "Join", "label": "join_b" },
    { "from": "Join", "to": "Complete", "label": "finish" }
  ],
  "gateways": [
    {
      "id": "fork_gateway",
      "name": "Fork",
      "gateway_type": "or",
      "branches": [
        { "target": "PathA", "label": "route_a", "guard": "condition_a" },
        { "target": "PathB", "label": "route_b", "guard": "condition_b" }
      ]
    }
  ]
}
```

**Key points:**
- OR gateways activate branches based on guard conditions
- Multiple branches can execute concurrently if their guards are satisfied
- The join state waits for all activated branches to complete
- Use `--print-structure` to see how branch activation is tracked

---

## 9. State Unrolling (Abstraction) Examples

When a BPMN model contains variables, MUNUNU automatically applies state unrolling to expand the state space by incorporating variable values into state names. This eliminates explicit variables in the generated CLTS and enables precise guard evaluation.

### 9.1 Simple Unrolling Example

Create `/tmp/mununu_tutorial/simple_unroll.json`:

```json
{
  "name": "counter_process",
  "states": [
    { "name": "Start", "initial": true },
    { "name": "Counting" },
    { "name": "Done" }
  ],
  "variables": [
    { "name": "count", "type": "i64", "initial": 0 }
  ],
  "transitions": [
    { "from": "Start", "to": "Counting", "label": "begin" },
    {
      "from": "Counting",
      "to": "Counting",
      "label": "increment",
      "effects": [{ "target": "count", "value": "count + 1" }]
    },
    {
      "from": "Counting",
      "to": "Done",
      "label": "finish",
      "guard": "count >= 5"
    }
  ]
}
```

**Translate with structure inspection:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/simple_unroll.json \
  --output /tmp/mununu_tutorial/unroll_out \
  --name CounterProcess \
  --force \
  --print-structure
```

**Observe unrolling effects:**

The structure output shows:
- States like `Start_count_0`, `Counting_count_0`, `Counting_count_1`, etc.
- Variable values are encoded in state names (e.g., `_count_0`, `_count_1`)
- Transitions connect states with appropriate variable values
- The `Done` state is only reachable from states where `count >= 5`

**Compare with non-unrolled version:**

To see the difference, temporarily remove the `variables` field and translate again. Without unrolling:
- States are just `Start`, `Counting`, `Done`
- Variables are declared separately in the automaton
- Guards are evaluated at runtime using guard predicates

### 9.2 Complex Unrolling with Multiple Variables

Use the provided example `tests/data/bpm/realizable/loan_processing.json`:

```bash
cargo run --bin mununu -- translate bpm \
  --input tests/data/bpm/realizable/loan_processing.json \
  --output /tmp/mununu_tutorial/loan_out \
  --name LoanProcessing \
  --force \
  --print-structure
```

**Inspect the unrolled structure:**

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/loan_out/LoanProcessing.ctxdsl \
  --sidecar /tmp/mununu_tutorial/loan_out/LoanProcessing_properties.ctxdsl \
  --print-structure
```

**Key observations:**
- States encode both `documents` and `risk_score` values (e.g., `Submitted_documents_0_risk_score_600`)
- Multiple paths through the state space based on variable values
- Guard conditions like `documents < 3` and `risk_score >= 650` determine reachability
- The unrolling algorithm explores all reachable combinations of variable values

### 9.3 Unrolling with Unreachable States

Some models produce empty transition sets when unrolling, indicating that no transitions are reachable from the initial state. Create `/tmp/mununu_tutorial/unreachable_unroll.json`:

```json
{
  "name": "unreachable_escalation",
  "states": [
    { "name": "Open", "initial": true },
    { "name": "Escalated" }
  ],
  "variables": [
    { "name": "engineers", "type": "i64", "initial": 0 }
  ],
  "transitions": [
    {
      "from": "Open",
      "to": "Escalated",
      "label": "escalate",
      "guard": "engineers > 5"
    }
  ]
}
```

**Translate and observe fallback:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/unreachable_unroll.json \
  --output /tmp/mununu_tutorial/unreachable_out \
  --name UnreachableEscalation \
  --force \
  --print-structure
```

**What happens:**
- Unrolling produces state `Open_engineers_0` (initial state with `engineers=0`)
- Guard `engineers > 5` evaluates to `false` for `engineers=0`
- No transitions are reachable from the initial state
- System falls back to original (non-unrolled) states and transitions
- The structure output shows the fallback behavior

**Diagnosis:**
- Use `--print-structure` to see why unrolling produced zero transitions
- Check guard conditions against initial variable values
- Verify that at least one transition is enabled from the initial state

---

## 10. Guard Handling Examples

MUNUNU uses a two-phase approach for guard handling:
- **Phase 1 (Build-time)**: Static guards (`guard: true`, `guard: false`) are filtered
- **Phase 2 (Runtime)**: Dynamic guards are evaluated using guard predicates

### 10.1 Static Guard Filtering

**Static `guard: false` (always disabled):**

Create `/tmp/mununu_tutorial/static_false_guard.json`:

```json
{
  "name": "disabled_path",
  "states": [
    { "name": "Start", "initial": true },
    { "name": "Disabled" },
    { "name": "Enabled" }
  ],
  "transitions": [
    {
      "from": "Start",
      "to": "Disabled",
      "label": "disabled_path",
      "guard": "false"
    },
    {
      "from": "Start",
      "to": "Enabled",
      "label": "enabled_path",
      "guard": "true"
    }
  ]
}
```

**Translate and verify filtering:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/static_false_guard.json \
  --output /tmp/mununu_tutorial/static_false_out \
  --name DisabledPath \
  --force \
  --print-structure
```

**Verify:**
- The transition to `Disabled` should be **absent** from the DSL
- The transition to `Enabled` should be **present** without a guard predicate
- The structure output shows only the enabled transition

**Static `guard: true` (always enabled):**

The transition with `guard: true` is included in the CLTS without a guard predicate, as it's always satisfiable.

### 10.2 Dynamic Guard Evaluation

**Dynamic guards (state-dependent):**

Create `/tmp/mununu_tutorial/dynamic_guard.json`:

```json
{
  "name": "conditional_routing",
  "states": [
    { "name": "Start", "initial": true },
    { "name": "Check" },
    { "name": "PathA" },
    { "name": "PathB" }
  ],
  "variables": [
    { "name": "value", "type": "i64", "initial": 7 }
  ],
  "transitions": [
    { "from": "Start", "to": "Check", "label": "begin" },
    {
      "from": "Check",
      "to": "PathA",
      "label": "route_a",
      "guard": "value > 5"
    },
    {
      "from": "Check",
      "to": "PathB",
      "label": "route_b",
      "guard": "value <= 5"
    }
  ]
}
```

**Translate and inspect:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/dynamic_guard.json \
  --output /tmp/mununu_tutorial/dynamic_out \
  --name ConditionalRouting \
  --force \
  --print-structure
```

**Key observations:**
- With unrolling: States encode variable values, guards are evaluated during unrolling
- Guard predicates are generated in the guards sidecar file
- The structure output shows which paths are reachable based on guard conditions

**Evaluate guard predicates:**

```bash
cargo run --bin mununu -- context summarize \
  /tmp/mununu_tutorial/dynamic_out/ConditionalRouting.ctxdsl \
  --sidecar /tmp/mununu_tutorial/dynamic_out/ConditionalRouting_guards.ctxdsl \
  --print-structure
```

The summary shows guard predicates like `Process_Check_route_a_guard_0` that track guard satisfaction.

### 10.3 Mixed Static and Dynamic Guards

Create `/tmp/mununu_tutorial/mixed_guards.json`:

```json
{
  "name": "mixed_guards",
  "states": [
    { "name": "Start", "initial": true },
    { "name": "Decision" },
    { "name": "AlwaysPath" },
    { "name": "NeverPath" },
    { "name": "ConditionalPath" }
  ],
  "variables": [
    { "name": "flag", "type": "bool", "initial": true }
  ],
  "transitions": [
    { "from": "Start", "to": "Decision", "label": "begin" },
    {
      "from": "Decision",
      "to": "AlwaysPath",
      "label": "always",
      "guard": "true"
    },
    {
      "from": "Decision",
      "to": "NeverPath",
      "label": "never",
      "guard": "false"
    },
    {
      "from": "Decision",
      "to": "ConditionalPath",
      "label": "conditional",
      "guard": "flag"
    }
  ]
}
```

**Translate and analyze:**

```bash
cargo run --bin mununu -- translate bpm \
  --input /tmp/mununu_tutorial/mixed_guards.json \
  --output /tmp/mununu_tutorial/mixed_out \
  --name MixedGuards \
  --force \
  --print-structure
```

**Verify behavior:**
- `AlwaysPath` transition: Present, no guard predicate (static `true`)
- `NeverPath` transition: **Absent** (static `false` filtered at build time)
- `ConditionalPath` transition: Present with guard predicate (dynamic guard)

**Test guard evaluation:**

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/mixed_out/MixedGuards.ctxdsl \
  --sidecar /tmp/mununu_tutorial/mixed_out/MixedGuards_guards.ctxdsl \
  --formula reachability \
  --automaton Process \
  --print-structure
```

This demonstrates how static guards are filtered at build time, while dynamic guards are evaluated at runtime.

---

## 11. Modality Validation with Controllability

This section demonstrates how mu-calculus modalities `[]` (necessity/box) and `<>` (possibility/diamond) interact with controllable and uncontrollable actions, including the Skolem paradigm for grouping transitions by uncontrollable labels.

### 11.1 Box Modality (`[]`) - Necessity Operator

The `[]` operator requires that **all** transitions (grouped by uncontrollable labels) lead to states satisfying the formula.

#### 11.1.1 Box with All Uncontrollable Actions (Validating)

Create `/tmp/mununu_tutorial/box_all_unctrl_valid.ctxdsl`:

```ctxdsl
context box_all_unctrl_valid {
    alphabet {
        label env_a;
        label env_b;
        label env_c;
        label env_d;
    }

    automata {
        automaton System {
            controllable { }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
                predicate good3 = state Good3;
                predicate good4 = state Good4;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Good3;
                state Good4;
            }
            transitions {
                transition Start -> Good1 on label env_a;
                transition Start -> Good2 on label env_b;
                transition Start -> Good3 on label env_c;
                transition Start -> Good4 on label env_d;
            }
        }
    }

    mu_formulas {
        formula all_paths_good {
            over System;
            body = [] (good1 || good2 || good3 || good4);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/box_all_unctrl_valid.ctxdsl \
  --formula all_paths_good \
  --automaton System \
  --print-structure
```

**Expected**: Formula is **satisfied** because all four transitions (all uncontrollable) lead to states where at least one of the good predicates (`good1`, `good2`, `good3`, or `good4`) is true.

#### 11.1.2 Box with All Uncontrollable Actions (Invalidating)

Create `/tmp/mununu_tutorial/box_all_unctrl_invalid.ctxdsl`:

```ctxdsl
context box_all_unctrl_invalid {
    alphabet {
        label env_a;
        label env_b;
        label env_c;
        label env_d;
    }

    automata {
        automaton System {
            controllable { }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
                predicate good3 = state Good3;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Good3;
                state Bad;
            }
            transitions {
                transition Start -> Good1 on label env_a;
                transition Start -> Good2 on label env_b;
                transition Start -> Good3 on label env_c;
                transition Start -> Bad on label env_d;
            }
        }
    }

    mu_formulas {
        formula all_paths_good {
            over System;
            body = [] (good1 || good2 || good3);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/box_all_unctrl_invalid.ctxdsl \
  --formula all_paths_good \
  --automaton System
```

**Expected**: Formula is **not satisfied** because one transition leads to `Bad` where none of the good predicates are true.

#### 11.1.3 Box with All Controllable Actions (Validating)

Create `/tmp/mununu_tutorial/box_all_ctrl_valid.ctxdsl`:

```ctxdsl
context box_all_ctrl_valid {
    alphabet {
        label action_a;
        label action_b;
        label action_c;
        label action_d;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; label action_c; label action_d; }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
                predicate good3 = state Good3;
                predicate good4 = state Good4;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Good3;
                state Good4;
            }
            transitions {
                transition Start -> Good1 on label action_a;
                transition Start -> Good2 on label action_b;
                transition Start -> Good3 on label action_c;
                transition Start -> Good4 on label action_d;
            }
        }
    }

    mu_formulas {
        formula all_paths_good {
            over System;
            body = [] (good1 || good2 || good3 || good4);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/box_all_ctrl_valid.ctxdsl \
  --formula all_paths_good \
  --automaton System
```

**Expected**: Formula is **satisfied** because all controllable transitions lead to good states.

#### 11.1.4 Box with All Controllable Actions (Invalidating)

Create `/tmp/mununu_tutorial/box_all_ctrl_invalid.ctxdsl`:

```ctxdsl
context box_all_ctrl_invalid {
    alphabet {
        label action_a;
        label action_b;
        label action_c;
        label action_d;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; label action_c; label action_d; }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
                predicate good3 = state Good3;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Good3;
                state Bad;
            }
            transitions {
                transition Start -> Good1 on label action_a;
                transition Start -> Good2 on label action_b;
                transition Start -> Good3 on label action_c;
                transition Start -> Bad on label action_d;
            }
        }
    }

    mu_formulas {
        formula all_paths_good {
            over System;
            body = [] (good1 || good2 || good3 || good4);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/box_all_ctrl_invalid.ctxdsl \
  --formula all_paths_good \
  --automaton System
```

**Expected**: Formula is **not satisfied** because one controllable transition leads to `Bad`.

#### 11.1.5 Box with Mixed Controllable/Uncontrollable (Validating - Skolem Test)

Create `/tmp/mununu_tutorial/box_mixed_valid.ctxdsl`:

```ctxdsl
context box_mixed_valid {
    alphabet {
        label env_x;
        label action_a;
        label action_b;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
                predicate good3 = state Good3;
                predicate good4 = state Good4;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Good3;
                state Good4;
            }
            transitions {
                // Group 1: uncontrollable env_x only
                transition Start -> Good1 on label env_x;
                // Group 2: env_x + action_a (sharing uncontrollable env_x)
                transition Start -> Good2 on label env_x, label action_a;
                transition Start -> Good3 on label env_x, label action_a;
                // Group 3: action_b only (controllable, no uncontrollable)
                transition Start -> Good4 on label action_b;
            }
        }
    }

    mu_formulas {
        formula all_paths_good {
            over System;
            body = [] (good1 || good2 || good3 || good4);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/box_mixed_valid.ctxdsl \
  --formula all_paths_good \
  --automaton System \
  --print-structure
```

**Expected**: Formula is **satisfied** because:
- Group 1 (env_x only): All transitions lead to states where good predicates are true
- Group 2 (env_x + action_a): Both transitions sharing `env_x` lead to states where good predicates are true
- Group 3 (action_b only): All controllable transitions lead to states where good predicates are true

**Key insight**: Transitions sharing the same uncontrollable label (`env_x`) are grouped together. For `[]`, ALL transitions in each group must satisfy the formula.

#### 11.1.6 Box with Mixed Controllable/Uncontrollable (Invalidating - Skolem Test)

Create `/tmp/mununu_tutorial/box_mixed_invalid.ctxdsl`:

```ctxdsl
context box_mixed_invalid {
    alphabet {
        label env_x;
        label action_a;
        label action_b;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
                predicate good3 = state Good3;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Good3;
                state Bad;
            }
            transitions {
                // Group 1: uncontrollable env_x only
                transition Start -> Good1 on label env_x;
                // Group 2: env_x + action_a (sharing uncontrollable env_x)
                transition Start -> Good2 on label env_x, label action_a;
                transition Start -> Bad on label env_x, label action_a;  // This breaks the group
                // Group 3: action_b only (controllable)
                transition Start -> Good3 on label action_b;
            }
        }
    }

    mu_formulas {
        formula all_paths_good {
            over System;
            body = [] (good1 || good2 || good3 || good4);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/box_mixed_invalid.ctxdsl \
  --formula all_paths_good \
  --automaton System
```

**Expected**: Formula is **not satisfied** because in Group 2 (transitions sharing `env_x`), one transition leads to `Bad`, violating the requirement that ALL transitions in the group must satisfy the formula.

### 11.2 Diamond Modality (`<>`) - Possibility Operator

The `<>` operator requires that **at least one** transition (in each group of transitions sharing uncontrollable labels) leads to a state satisfying the formula.

#### 11.2.1 Diamond with All Uncontrollable Actions (Validating)

Create `/tmp/mununu_tutorial/diamond_all_unctrl_valid.ctxdsl`:

```ctxdsl
context diamond_all_unctrl_valid {
    alphabet {
        label env_a;
        label env_b;
        label env_c;
        label env_d;
    }

    automata {
        automaton System {
            controllable { }
            internal { }
            predicates {
                predicate good1 = state Good1;
            }
            states {
                state Start initial;
                state Good1;
                state Bad1;
                state Bad2;
                state Bad3;
            }
            transitions {
                transition Start -> Good1 on label env_a;
                transition Start -> Bad1 on label env_b;
                transition Start -> Bad2 on label env_c;
                transition Start -> Bad3 on label env_d;
            }
        }
    }

    mu_formulas {
        formula some_path_good {
            over System;
            body = <> good1;
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/diamond_all_unctrl_valid.ctxdsl \
  --formula some_path_good \
  --automaton System
```

**Expected**: Formula is **satisfied** because at least one transition (env_a) leads to `Good1` where `good1` is true.

#### 11.2.2 Diamond with All Uncontrollable Actions (Invalidating)

Create `/tmp/mununu_tutorial/diamond_all_unctrl_invalid.ctxdsl`:

```ctxdsl
context diamond_all_unctrl_invalid {
    alphabet {
        label env_a;
        label env_b;
        label env_c;
        label env_d;
    }

    automata {
        automaton System {
            controllable { }
            internal { }
            predicates {
                predicate good1 = state Good1;
            }
            states {
                state Start initial;
                state Good1;
                state Bad1;
                state Bad2;
                state Bad3;
                state Bad4;
            }
            transitions {
                transition Start -> Bad1 on label env_a;
                transition Start -> Bad2 on label env_b;
                transition Start -> Bad3 on label env_c;
                transition Start -> Bad4 on label env_d;
            }
        }
    }

    mu_formulas {
        formula some_path_good {
            over System;
            body = <> good1;
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/diamond_all_unctrl_invalid.ctxdsl \
  --formula some_path_good \
  --automaton System
```

**Expected**: Formula is **not satisfied** because no transitions lead to a state where `good1` is true.

#### 11.2.3 Diamond with All Controllable Actions (Validating)

Create `/tmp/mununu_tutorial/diamond_all_ctrl_valid.ctxdsl`:

```ctxdsl
context diamond_all_ctrl_valid {
    alphabet {
        label action_a;
        label action_b;
        label action_c;
        label action_d;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; label action_c; label action_d; }
            internal { }
            predicates {
                predicate good1 = state Good1;
            }
            states {
                state Start initial;
                state Good1;
                state Bad1;
                state Bad2;
                state Bad3;
            }
            transitions {
                transition Start -> Good1 on label action_a;
                transition Start -> Bad1 on label action_b;
                transition Start -> Bad2 on label action_c;
                transition Start -> Bad3 on label action_d;
            }
        }
    }

    mu_formulas {
        formula some_path_good {
            over System;
            body = <> good1;
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/diamond_all_ctrl_valid.ctxdsl \
  --formula some_path_good \
  --automaton System
```

**Expected**: Formula is **satisfied** because at least one controllable transition leads to a state where `good1` is true.

#### 11.2.4 Diamond with All Controllable Actions (Invalidating)

Create `/tmp/mununu_tutorial/diamond_all_ctrl_invalid.ctxdsl`:

```ctxdsl
context diamond_all_ctrl_invalid {
    alphabet {
        label action_a;
        label action_b;
        label action_c;
        label action_d;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; label action_c; label action_d; }
            internal { }
            predicates {
                predicate good1 = state Good1;
            }
            states {
                state Start initial;
                state Good1;
                state Bad1;
                state Bad2;
                state Bad3;
                state Bad4;
            }
            transitions {
                transition Start -> Bad1 on label action_a;
                transition Start -> Bad2 on label action_b;
                transition Start -> Bad3 on label action_c;
                transition Start -> Bad4 on label action_d;
            }
        }
    }

    mu_formulas {
        formula some_path_good {
            over System;
            body = <> good1;
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/diamond_all_ctrl_invalid.ctxdsl \
  --formula some_path_good \
  --automaton System
```

**Expected**: Formula is **not satisfied** because no controllable transitions lead to a state where `good1` is true.

#### 11.2.5 Diamond with Mixed Controllable/Uncontrollable (Validating - Skolem Test)

Create `/tmp/mununu_tutorial/diamond_mixed_valid.ctxdsl`:

```ctxdsl
context diamond_mixed_valid {
    alphabet {
        label env_x;
        label action_a;
        label action_b;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; }
            internal { }
            predicates {
                predicate good1 = state Good1;
                predicate good2 = state Good2;
            }
            states {
                state Start initial;
                state Good1;
                state Good2;
                state Bad1;
                state Bad2;
            }
            transitions {
                // Group 1: uncontrollable env_x only (must have at least one purely uncontrollable transition)
                transition Start -> Good1 on label env_x;
                // Group 2: env_x + action_a (sharing uncontrollable env_x)
                // Both transitions have the same label set {env_x, action_a}
                // For <>: there must exist a controllable choice (action_a) such that ALL states
                // reached through {env_x, action_a} satisfy the formula
                transition Start -> Good1 on label env_x, label action_a;
                transition Start -> Bad2 on label env_x, label action_a;
                // Group 3: action_b only (controllable, no uncontrollable labels)
                transition Start -> Good2 on label action_b;
            }
        }
    }

    mu_formulas {
        formula some_path_good {
            over System;
            body = <> (good1 || good2);
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/diamond_mixed_valid.ctxdsl \
  --formula some_path_good \
  --automaton System \
  --print-structure
```

**Expected**: Formula is **NOT satisfied** because:
- Group 1 (env_x only): The transition leads to Good1, which satisfies (good1 || good2) ✓
- Group 2 (env_x + action_a): Two transitions share the same label set `{env_x, action_a}`:
  - `Start -> Good1` satisfies (good1 || good2) ✓
  - `Start -> Bad2` does NOT satisfy (good1 || good2) ✗
  - For `<>` with Skolem paradigm: there must exist a controllable choice (action_a) such that ALL states reached through `{env_x, action_a}` satisfy. Since Bad2 doesn't satisfy, this group fails.
- Group 3 (action_b only): The transition leads to Good2, which satisfies (good1 || good2) ✓

**Key insight**: For `<>`, when multiple transitions share the same full label set (including controllable labels), ALL transitions with that label set must satisfy the formula. This ensures that when the system chooses a controllable action, all possible nondeterministic outcomes satisfy.

#### 11.2.6 Diamond with Mixed Controllable/Uncontrollable (Invalidating - Skolem Test)

Create `/tmp/mununu_tutorial/diamond_mixed_invalid.ctxdsl`:

```ctxdsl
context diamond_mixed_invalid {
    alphabet {
        label env_x;
        label action_a;
        label action_b;
    }

    automata {
        automaton System {
            controllable { label action_a; label action_b; }
            internal { }
            predicates {
                predicate good1 = state Good1;
            }
            states {
                state Start initial;
                state Good1;
                state Bad1;
                state Bad2;
                state Bad3;
            }
            transitions {
                // Group 1: uncontrollable env_x only
                transition Start -> Bad1 on label env_x;
                // Group 2: env_x + action_a (sharing uncontrollable env_x)
                transition Start -> Bad2 on label env_x, label action_a;
                transition Start -> Bad3 on label env_x, label action_a;
                // Group 3: action_b only (controllable)
                transition Start -> Bad1 on label action_b;
            }
        }
    }

    mu_formulas {
        formula some_path_good {
            over System;
            body = <> good1;
        }
    }
}
```

**Evaluate**:

```bash
cargo run --bin mununu -- context eval \
  /tmp/mununu_tutorial/diamond_mixed_invalid.ctxdsl \
  --formula some_path_good \
  --automaton System
```

**Expected**: Formula is **not satisfied** because:
- Group 1 (env_x only): No transition leads to a state where `good1` is true
- Group 2 (env_x + action_a): No transition leads to a state where `good1` is true (both lead to bad states)
- Group 3 (action_b only): No transition leads to a state where `good1` is true

**Key insight**: For `<>`, at least one transition in **each group** must satisfy the formula. Since Group 2 has no transitions leading to a good state, the formula fails.

### 11.3 Summary of Modality Semantics

**Box (`[]`) - Necessity**:
- Requires **ALL** transitions in each uncontrollable group to satisfy the formula
- For controllable transitions (not in any group), all must satisfy
- Skolem paradigm: Transitions sharing the same uncontrollable labels form a group; all transitions in the group must satisfy

**Diamond (`<>`) - Possibility**:
- Requires **AT LEAST ONE** transition in each uncontrollable group to satisfy the formula
- For controllable transitions (not in any group), at least one must satisfy
- Skolem paradigm: Transitions sharing the same uncontrollable labels form a group; at least one transition in the group must satisfy

**Controllability Grouping**:
- Transitions are grouped by their **uncontrollable labels** (not all labels)
- Controllable transitions that share the same uncontrollable labels are included in the group
- This ensures proper handling of reactive systems where the environment chooses uncontrollable actions, and the controller must respond appropriately

---

## 12. Summary

- **Modeling**: author `.ctxdsl` contexts + sidecars with your editor.
- **Verification**: `mununu context eval` for formula checks; `context synth` for GR(1) realizability + diagnostics.
- **LTL → μ-calculus**: reuse the cheatsheet patterns; synchronous examples (elevator) and asynchronous BPMN translations demonstrate both safety/liveness cases.
- **Diagnosis**: enable `--counterexample` / `--deadlock-traces` to capture artifacts for unrealizable specs.
- **BPM translation**: `mununu translate bpm` + `context summarize` gives end-to-end visibility from BPMN to CLTS.
- **Gateways**: XOR (exclusive), AND (parallel), and OR (inclusive) gateways control flow through conditional branching and parallel execution.
- **State unrolling**: Automatic abstraction when variables are present, expanding state space by encoding variable values in state names.
- **Guard handling**: Two-phase approach with static guard filtering at build time and dynamic guard evaluation at runtime.
- **Modality validation**: Box (`[]`) requires all transitions in each uncontrollable group to satisfy the formula; Diamond (`<>`) requires at least one transition in each group to satisfy. The Skolem paradigm groups transitions by their uncontrollable labels.

With these steps you can validate the full toolchain using nothing more than the CLI and your preferred text editor.

