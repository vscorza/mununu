# Game Engine Integration

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change.

Mununu can formally verify game state machines for softlocks, unreachable content, AI behavior loops, and other properties that are hard to catch through playtesting alone. Game FSMs are typically 5-50 states — well within mununu's exhaustive verification sweet spot.

## Supported Engines

| Engine | Status | Integration Path |
|--------|--------|-----------------|
| **Godot** (GDScript) | Supported | `.gd` source extraction + `.espec.json` specs |
| **Unity** (C#) | Planned | `.espec.json` specs (manual extraction) |
| **Unreal** | Deferred | `.espec.json` specs (manual extraction) |

All game engine integration uses the **ExtractionAdapter** pipeline:

```
Game source (.gd) or hand-written spec
    --> .espec.json (extraction spec)
    --> ExtractionAdapter
    --> CTXDSL
    --> mununu eval/synth
```

## Use Cases

### Softlock Detection

A softlock occurs when a game state has no outgoing transitions — the player is permanently stuck.

```bash
mununu context eval player_fsm.espec.json --adapter extraction \
    --template no_deadlock --automaton PlayerState
```

The `no_deadlock` template checks `nu X. (<> true && [] X)`: every reachable state must have at least one enabled transition.

### Unreachable Content

Detect states (levels, quests, items) that no player can ever reach from the initial state.

```bash
mununu context eval game.espec.json --adapter extraction \
    --template reachable --template-arg TARGET=SecretLevel --automaton GameProgress
```

### Quest Completability

Verify that goal states are reachable despite prerequisite chains:

```bash
mununu context eval quest_system.espec.json --adapter extraction \
    --template reachable --template-arg TARGET=AllComplete --automaton QuestProgress
```

### AI Behavior Loops

Check that NPCs always eventually make progress (return to patrol, reach a goal):

```bash
mununu context eval npc_ai.espec.json --adapter extraction \
    --template always_eventually --template-arg TARGET=Patrol --automaton AIState
```

### Animation Return-to-Idle

Verify every animation state can eventually return to idle (no stuck characters):

```bash
mununu context eval animation.espec.json --adapter extraction \
    --template always_eventually --template-arg TARGET=Idle --automaton AnimationState
```

### Inventory Bounds

Check that counters never overflow or underflow:

```bash
mununu context eval inventory.espec.json --adapter extraction \
    --template bounded --template-arg OVERFLOW=inventory_full --automaton InventoryState
```

## Writing a Game `.espec.json`

An extraction spec for a game FSM declares states, transitions, and properties:

```json
{
  "$schema": "extraction_spec_v1",
  "source": {
    "file": "player_controller.gd",
    "class": "PlayerController"
  },
  "model_config": {
    "context_name": "PlayerFSM",
    "uncontrollable_labels": ["ev_move", "ev_jump", "ev_die"],
    "controllable_labels": ["ev_respawn"],
    "automata": [
      {
        "id": "PlayerState",
        "states": [
          { "name": "Idle", "initial": true },
          { "name": "Running" },
          { "name": "Dead" }
        ],
        "controllable_labels": ["ev_respawn"],
        "transitions": [
          { "from": "Idle", "to": "Running", "label": "ev_move" },
          { "from": "Running", "to": "Idle", "label": "ev_stop" },
          { "from": "Idle", "to": "Dead", "label": "ev_die" },
          { "from": "Dead", "to": "Idle", "label": "ev_respawn" }
        ]
      }
    ],
    "properties": [
      {
        "id": "no_softlock",
        "template_ref": { "template": "no_deadlock" },
        "over": "PlayerState"
      }
    ]
  }
}
```

### Controllability

In game FSMs, controllability determines who triggers each transition:

| Category | Examples | Classification |
|----------|---------|---------------|
| Player input | `ev_move`, `ev_jump`, `ev_attack` | **Uncontrollable** (environment) |
| Physics/timer events | `ev_fall`, `ev_land`, `ev_timeout` | **Uncontrollable** |
| System decisions | `ev_respawn`, `ev_spawn_enemy`, `ev_grant_item` | **Controllable** |

This matches mununu's synthesis paradigm: the controller (game system) must guarantee properties regardless of what the player (environment) does.

### Using Property Templates

Instead of writing raw mu-calculus, use `template_ref` in the properties section:

```json
{
  "id": "can_return_idle",
  "template_ref": {
    "template": "always_eventually",
    "args": { "TARGET": "Idle" }
  },
  "over": "PlayerState"
}
```

See [Property Templates](Property-Templates) for the full catalog.

## Examples

Pre-built game examples are in `examples/game/`:

| File | Description | Key Property | Expected Result |
|------|-------------|-------------|-----------------|
| `player_fsm.espec.json` | 6-state player controller with Dead softlock | `no_deadlock` | FAILS (Dead has no exits) |
| `player_fsm_fixed.espec.json` | Same FSM with respawn transition added | `no_deadlock` | PASSES |
| `quest_deadlock.espec.json` | Quest system with circular dependency | `reachable(AllComplete)` | FAILS (AllComplete unreachable) |
| `npc_ai_loop.espec.json` | NPC AI with Chase-Retreat oscillation | `no_deadlock` | PASSES (all states have exits) |

Run an example:

```bash
mununu context eval examples/game/player_fsm.espec.json \
    --adapter extraction --formula no_softlock --automaton PlayerState
```

## GDScript Extraction

Mununu supports tree-sitter-based extraction from GDScript (`.gd`) files. The `game_fsm` domain profile configures:

- **Controllability**: `_on_*`, `_input*`, `_physics_process*` callbacks are uncontrollable; `spawn*`, `respawn*`, `activate*` are controllable
- **Composition**: Asynchronous (independent game systems)
- **Label prefix**: `ev_`
- **Abstractions**: Enum values for state fields, bounded counters for numeric fields

See [Godot Adapter](Godot-Adapter) for details on GDScript extraction patterns.

## Controller Export (GDScript)

Synthesized controllers can be exported as GDScript code, producing a drop-in `Node` script for Godot:

```bash
# Synthesize and export as GDScript
mununu context synth combat_system.espec.json --adapter extraction \
    --formula dead_blocks_attack --automaton CombatWorld \
    --output-format gdscript --emit-native combat_controller.gd

# Dialogue tree: export blocks the dead-end path
mununu context synth dialogue_tree.espec.json --adapter extraction \
    --formula farewell_reachable --automaton Dialogue \
    --output-format gdscript --emit-native dialogue_controller.gd
```

The output includes:
- `enum State` with only states from the winning region (unsafe states removed)
- Controllable actions as `func name() -> bool` (returns `false` if blocked by controller)
- Uncontrollable events as `func on_name()` (signal handlers)
- `get_state_name()` query method

The controller acts as a **runtime guard**: attach it as a node, call its methods before state transitions, and the synthesized strategy prevents property violations.

Supported via CLI (`--output-format gdscript`), API (`output_format: "gdscript"` in synthesis request), and UI (GDScript option in the export dropdown).

## Web UI

The mununu-ui includes a "Game Engine (Godot)" workflow in the domain selector. The workflow steps are:

1. **Load Source** — Upload a `.gd` file or `.espec.json` spec
2. **Extract** — Extract state machine from GDScript (skipped for `.espec.json`)
3. **Edit Spec** — Review and refine the extracted spec
4. **Translate** — Generate CTXDSL from the spec
5. **Verify** — Evaluate properties using the template picker

See also: [Property Templates](Property-Templates), [Godot Adapter](Godot-Adapter), [CLI Reference](CLI-Reference)
