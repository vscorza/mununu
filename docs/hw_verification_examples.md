# Hardware Verification Examples

CLTS models extracted from SystemVerilog designs in [hw-verification-uba](https://github.com/vscorza/hw-verification-uba) labs. Each example models a hardware FSM as a Compositional Labeled Transition System with mu-calculus properties.

## Examples

### 1. Handshake Protocol (`examples/hw/handshake.ctxdsl`)

**Source:** lab01 — Req/Ack handshake FSM

A 4-state protocol modeling request/acknowledge handshaking with latency:
- **IDLE** → WAIT_ACK (on `req_assert`)
- **WAIT_ACK** → ACTIVE (on `ack_assert`, after latency ticks)
- **ACTIVE** → DONE (on `req_deassert`)
- **DONE** → IDLE (on `ack_deassert`)

**Properties:**
- `safety_invariant` — all states are valid (nu X. [] X)
- `ack_reachable` — Active state is reachable from any state
- `cycle_completes` — Idle is reachable from any state (full cycle)
- `active_can_complete` — from Active, Done is always reachable

### 2. Round-Robin Arbiter (`examples/hw/arbiter.ctxdsl`)

**Source:** lab02 — 2-client round-robin arbiter

A 3-state mutual exclusion arbiter:
- **IDLE** → GRANT_A or GRANT_B (on request)
- **GRANT_A** → IDLE (on `release_a`)
- **GRANT_B** → IDLE (on `release_b`)

**Properties:**
- `safety_invariant` — all states valid
- `mutual_exclusion` — no state is simultaneously GrantA and GrantB
- `grant_a_reachable` / `grant_b_reachable` — both grants are reachable
- `grant_a_releases` / `grant_b_releases` — grants always eventually release

### 3. Traffic Light Controller (`examples/hw/traffic_light.ctxdsl`)

**Source:** lab05 — Traffic light with timer and sensor

A 4-state FSM with timer-driven transitions:
- **GREEN** → YELLOW (on `timer_expire`)
- **YELLOW** → RED (on `timer_expire`)
- **RED** → RED_WAIT (on `timer_expire`)
- **RED_WAIT** → GREEN (on `sensor_trigger`)

**Properties:**
- `safety_invariant` — all states valid
- `yellow/red/red_wait_reachable` — all states reachable
- `full_cycle` — Green is reachable from any state
- `green_to_yellow` — from Green, timer_expire leads to Yellow
- `yellow_to_red` — from Yellow, timer_expire leads to Red

### 4. Ready/Valid Adapter (`examples/hw/rv_adapter.ctxdsl`)

**Source:** lab09 — Skid buffer for ready/valid protocol

A 2-state FSM for backpressure handling:
- **EMPTY** → FULL (on `push`, when consumer not ready)
- **FULL** → EMPTY (on `drain`, when consumer accepts)
- **EMPTY** → EMPTY (on `passthrough`, direct flow)

**Properties:**
- `safety_invariant` — all states valid
- `buffer_reachable` — Full state is reachable
- `cycle_completes` — Empty is reachable from any state
- `drain_possible` — from Full, Empty is always reachable
- `push_possible` — from Empty, Full is always reachable

## Running Verification

### CLI

```bash
cd /path/to/mununu

# Summarize a context
cargo run -- context summarize examples/hw/handshake.ctxdsl

# Evaluate a formula
cargo run -- context eval examples/hw/handshake.ctxdsl \
  --formula ack_reachable \
  --automaton Handshake

# Synthesize a controller
cargo run -- context synth examples/hw/handshake.ctxdsl \
  --formula safety_invariant \
  --automaton Handshake

# Generate graph visualization
cargo run -- context graph examples/hw/handshake.ctxdsl \
  --output handshake_graph.html
```

### API Server + UI

```bash
# Terminal 1: Start the API server
cargo run --features api -- server --addr 127.0.0.1:8080

# Terminal 2: Start the UI
cd /path/to/mununu-ui
npm run dev

# Open http://localhost:5173 in a browser
# Use the Verification or Synthesis workflow to load .ctxdsl files
```

## Mu-Calculus Formula Patterns

| Pattern | Formula | Meaning |
|---------|---------|---------|
| Safety invariant | `nu X. ([] X)` | All reachable states satisfy the property |
| Reachability | `mu X. (P \|\| <> X)` | Predicate P is reachable from current state |
| Conditional reach | `(! P) \|\| (mu Y. (Q \|\| <> Y))` | If in P, then Q is reachable |
| Transition check | `(! P) \|\| < labels = {l} > Q` | If in P, label l leads to Q |

- `nu` = greatest fixpoint (safety/invariant — "always")
- `mu` = least fixpoint (liveness/reachability — "eventually")
- `[] X` = box modality (all successors satisfy X)
- `<> X` = diamond modality (some successor satisfies X)
- `< labels = {l} > X` = labeled diamond (some l-successor satisfies X)

## Higher-alternation example: bus arbiter with retry

For hardware-style patterns where a stability event triggers a forever-after recurrence obligation — e.g., a bus that must answer every retry once granted — the resulting property has alternation depth 4 (above GR(1)). [`examples/bus_arbiter_retry.ctxdsl`](../examples/bus_arbiter_retry.ctxdsl) is a 4-state benchmark for this case. The mu-calculus formula is `νZ. μY. νX. μW. (...)` — three strict mu/nu alternations. Use `mununu context synth ... --controller-mode parity-game` for theoretical correctness on this depth; see [`docs/ltl_templates/temporal_logic_patterns.md`](ltl_templates/temporal_logic_patterns.md#beyond-gr1-recurrence-after-stability-σ3) for the pattern reference.

A dual-channel extension is at [`examples/dual_arbiter_alt4.ctxdsl`](../examples/dual_arbiter_alt4.ctxdsl): two independent service loops with their own recurrence-after-stability obligations, plus controllable `swap` transitions that the synthesized strategy must omit. The example demonstrates both required behaviours of memory-aware synthesis — *memory* of which channel's obligation is currently outstanding, and *strategy selection* via implicit disabling of controllable transitions. Functional mode synthesizes 31 transitions while projection keeps all 48; the 17-transition gap is the disabled set.
