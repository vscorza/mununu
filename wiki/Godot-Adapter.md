# Godot Adapter (GDScript)

> **Alpha Software** — Mununu is under active development. APIs, syntax, and behavior may change.

Mununu supports verification of Godot game state machines through GDScript source extraction and `.espec.json` extraction specs. The integration uses the ExtractionAdapter pipeline with a `game_fsm` domain profile.

## Supported Patterns

### Enum-based State Machines

The most common GDScript FSM pattern uses an `enum` for states and a `match` statement in `_process` or `_physics_process`:

```gdscript
enum State { IDLE, RUNNING, JUMPING, FALLING, ATTACKING, DEAD }
var current_state: State = State.IDLE

func _physics_process(delta):
    match current_state:
        State.IDLE:
            if Input.is_action_pressed("move"):
                current_state = State.RUNNING
            if Input.is_action_just_pressed("jump"):
                current_state = State.JUMPING
        State.RUNNING:
            if not Input.is_action_pressed("move"):
                current_state = State.IDLE
        State.JUMPING:
            if velocity.y > 0:
                current_state = State.FALLING
        State.FALLING:
            if is_on_floor():
                current_state = State.IDLE
        State.ATTACKING:
            if attack_finished:
                current_state = State.IDLE
        State.DEAD:
            pass  # No transition out — potential softlock
```

### What Gets Extracted

| GDScript Construct | Extraction Result |
|-------------------|------------------|
| `enum State { A, B, C }` | Automaton states: `A`, `B`, `C` |
| `var current_state: State = State.IDLE` | State field with initial value |
| `current_state = State.RUNNING` inside `match` arm | Transition from current state to `RUNNING` |
| `Input.is_action_*` guard | Uncontrollable label |
| `velocity.*`, `is_on_floor()` conditions | Uncontrollable (physics/environment) |

## Domain Profile: `game_fsm`

The `game_fsm` domain profile configures extraction heuristics for game state machines.

### Controllability Classification

| Pattern | Classification | Rationale |
|---------|---------------|-----------|
| `_on_*`, `_input*`, `_unhandled_input*` | Uncontrollable | Player input callbacks |
| `_physics_process*`, `_process*` | Uncontrollable | Engine tick callbacks |
| `handle_*`, `on_*` | Uncontrollable | Event handlers |
| `spawn*`, `respawn*` | Controllable | System-initiated spawning |
| `activate*`, `deactivate*` | Controllable | System state changes |
| `grant*`, `revoke*` | Controllable | Permission/item grants |
| `set_state*` | Controllable | Explicit state transitions |
| Everything else | Uncontrollable (default) | Conservative assumption |

### Abstraction Defaults

| Field Type | Abstraction | Notes |
|-----------|-------------|-------|
| `bool` | Boolean | Two-valued |
| `Optional`/nullable | Presence | Present/absent |
| `Dictionary`/`Array` | BoundedCounter (max 5) | Size abstraction |
| `enum` | EnumValues | All variants |
| `String` | Ignored | Too large to enumerate |
| `int`/`float` | BoundedCounter | Finite range |

### Composition

Default: **asynchronous**. Game systems (player, NPC, physics, quest) run independently and interact through shared events. Use synchronous composition when systems must tick in lockstep.

### Label Naming

Prefix: `ev_`. Transition labels are generated as `ev_<method_name>` (e.g., `ev_move`, `ev_jump`, `ev_die`).

## Soundness Notes

Game FSM extraction involves abstractions that affect verification soundness:

- **Physics conditions** (velocity, position, collisions) are abstracted as nondeterministic events. This is an **over-approximation**: the model admits more behaviors than reality. Conservative for safety properties (if the model says safe, the real game is safe for the modeled scope).
- **Timer-based transitions** are modeled as nondeterministic. The model doesn't capture timing constraints — a transition that takes 2 seconds in-game can fire immediately in the model.
- **Continuous variables** (health as `float`, position as `Vector2`) must be discretized into finite domains. The abstraction boundaries determine what properties can be verified.
- **Missing transitions**: if a GDScript `match` arm has side effects beyond state assignment (e.g., spawning objects, emitting signals), those effects are not captured in the FSM model. Document these gaps in the `.espec.json` `comment` fields.

Per CLAUDE.md soundness rules: every abstraction must be documented. Over-approximation is conservative for safety; under-approximation is unsound.

## Godot Version Support

- **Godot 4.x**: GDScript 2.0 syntax (recommended). `enum`, `match`, typed `var` declarations.
- **Godot 3.x**: GDScript 1.0. Similar patterns but some syntax differences in type annotations.

The tree-sitter grammar (`tree-sitter-gdscript` v6) targets Godot 4.x syntax.

## Examples

See `examples/game/` in the mununu repository:

- `player_fsm.espec.json` — 6-state player controller with a Dead-state softlock
- `player_fsm_fixed.espec.json` — Same FSM with respawn transition (softlock fixed)
- `quest_deadlock.espec.json` — Quest system with circular prerequisite chain
- `npc_ai_loop.espec.json` — NPC AI with Chase-Retreat oscillation pattern

See also: [Game Engine Integration](Game-Engine-Integration), [Property Templates](Property-Templates)
