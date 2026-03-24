# Testing Guide: Diagnosis and Strategy Features in the Web UI

This guide walks you through testing the latest diagnosis, counterstrategy, and strategy extraction features in mununu-ui.

## Prerequisites

1. **Start the backend** (release mode recommended for speed):
   ```bash
   cd mununu
   cargo run --release --features api -- server --addr 127.0.0.1:8080
   ```

2. **Start the frontend**:
   ```bash
   cd mununu-ui
   npm run dev
   ```

3. Open http://localhost:5173 in your browser.

---

## Test 1: Basic Verification (Satisfied Properties)

**File:** `tutorial/examples/01_basic_modeling.ctxdsl`

1. Paste the contents into the editor
2. Go to the **Verification** tab
3. Click **Verify**
4. **Expected:**
   - All formulas show "Satisfied" (green)
   - `safety_invariant`: 5/5 states satisfying
   - `lockout_reachable`: 5/5 states satisfying
   - No "Counterstrategy" button (all pass)

---

## Test 2: Unrealizable Property with Counterstrategy

**File:** `tutorial/examples/10_unrealizable_sync_liveness.ctxdsl`

1. Paste contents into the editor
2. Go to **Verification** tab, click **Verify**
3. **Expected results:**

   | Formula | Status | Satisfying |
   |---------|--------|-----------|
   | `ctrl_force_green` | Not Satisfied | 1/3 |
   | `env_prevents_green` | Satisfied | 2/3 |
   | `green_existentially_reachable` | Satisfied | 3/3 |
   | `fair_green_liveness` | Satisfied | 3/3 |
   | `safety` | Satisfied | 3/3 |

4. Click **Counterstrategy** on `ctrl_force_green`
5. **Expected:** A graph appears showing:
   - Two nodes: **Red** and **PedWaiting** (amber, environment winning region)
   - Red is marked as initial (thicker border)
   - Transitions between them, with `ped_request` marked as dashed red (uncontrollable)
   - The graph shows HOW the environment prevents Green: it loops `ped_request` forever

---

## Test 3: Unrealizable Async Response

**File:** `tutorial/examples/11_unrealizable_async_response.ctxdsl`

1. Paste contents, verify
2. **Expected:**
   - `ctrl_force_sent`: Not Satisfied (1/4)
   - `env_prevents_sent`: Satisfied (3/4)
   - `sent_existentially_reachable`: Satisfied (4/4)
   - `receiver_can_deliver`: Satisfied (3/3)

3. Click **Counterstrategy** on `ctrl_force_sent`
4. **Expected graph:** States Idle, Sending, Dropped with environment's `drop` transition (dashed red)

---

## Test 4: Controllable Reachability (Partial Satisfaction)

**File:** `tutorial/examples/05_ltl_properties.ctxdsl`

1. Paste contents, verify
2. **Expected:**
   - `ctrl_reachable_full`: Not Satisfied (2/4 — only Full and Filling satisfy)
   - `ltl_safety`, `ltl_liveness`, `ltl_response`: Satisfied
   - `mu_safety`, `mu_full_reachable`, `mu_gf_empty`: Satisfied

3. Click **Counterstrategy** on `ctrl_reachable_full`
4. **Expected graph:** States Empty and Draining (environment wins by looping `idle` at Empty)

---

## Test 5: Deadlock Detection (CLI Only)

**File:** `tutorial/examples/12_deadlock_trace.ctxdsl`

Deadlock traces are currently CLI-only:
```bash
mununu context synth tutorial/examples/12_deadlock_trace.ctxdsl \
  --formula safety --automaton Conveyor --deadlock-traces
```

**Expected output:**
```
Controller synthesis for formula 'safety' over automaton 'Conveyor':
  Realizable: yes
  Controller states: 3 (initial: 1)
  Diagnostics:
    note: Deadlock traces recorded: 1
    deadlock trace #0: Idle -> Running -> Jammed
```

---

## Test 6: Lasso Traces for Liveness (CLI Only)

**File:** `tutorial/examples/10_unrealizable_sync_liveness.ctxdsl`

```bash
mununu context synth tutorial/examples/10_unrealizable_sync_liveness.ctxdsl \
  --formula ctrl_force_green --automaton TrafficLight --counterexample
```

**Expected output (includes lasso):**
```
  Realizable: no
  Diagnostics:
    violating initials: Red
    counterexample trace: Red
    lasso trace #0: Red -> (PedWaiting)^ω
    counterstrategy states: PedWaiting, Red
```

The lasso `Red -> (PedWaiting)^ω` means: from Red, the environment forces PedWaiting, then loops there forever (infinite cycle).

---

## Test 7: Strategy Extraction (CLI Only)

```bash
# Full projection (all transitions):
mununu context synth tutorial/examples/06_controllability.ctxdsl \
  --formula safety_invariant --automaton RobotArm --emit-dsl /tmp/full.ctxdsl
grep -c transition /tmp/full.ctxdsl

# Witness-guided strategy (fewer transitions):
mununu context synth tutorial/examples/06_controllability.ctxdsl \
  --formula safety_invariant --automaton RobotArm \
  --extract-strategy --emit-dsl /tmp/strategy.ctxdsl
grep -c transition /tmp/strategy.ctxdsl
```

**Expected:** The strategy has fewer transitions than the full projection. The strategy keeps only the transitions chosen by the fixpoint evaluation witnesses.

---

## Test 8: GR(1) with Fairness Assumption

**File:** `tutorial/examples/10_unrealizable_sync_liveness.ctxdsl`

The `fair_green_liveness` formula uses a GR(1) pattern:
```
(¬GF(Red)) ∨ GF(Green)
```
"If Red is visited infinitely often (fairness), then Green is visited infinitely often."

1. Verify in the UI → should show 3/3 Satisfied
2. This is the fix for the unrealizable `ctrl_force_green`: by assuming environment fairness (ped_timeout eventually fires), the controller CAN guarantee Green

---

## Summary of Features by Interface

| Feature | Web UI | CLI |
|---------|--------|-----|
| Formula verification | ✓ Verify button | `mununu context eval` |
| Counterstrategy graph | ✓ Counterstrategy button | — |
| Deadlock traces | — | `--deadlock-traces` |
| Lasso traces | — | `--counterexample` (shows lasso) |
| Strategy extraction | — | `--extract-strategy` |
| Controller synthesis | — | `mununu context synth` |
| Graph visualization | ✓ Graphs tab | `mununu context graph` |

---

## Troubleshooting

- **"Request Timeout"**: The counterstrategy uses an extended timeout (120s). If it still times out, check that the API server is running in release mode.
- **No counterstrategy button**: Only appears for formulas that are "Not Satisfied."
- **Empty counterstrategy graph**: The formula may be trivially unsatisfied (e.g., `Open && Closed`). The environment wins from ALL states — the graph shows the entire automaton.
