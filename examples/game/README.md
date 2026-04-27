# Game Engine Verification Examples

Example `.espec.json` files demonstrating mununu's game state machine verification capabilities. Each example models a common game system and uses [property templates](../../wiki/Property-Templates.md) to check for bugs.

## Examples

### Player FSM (Softlock Detection)

A 6-state player controller with a Dead state that has no outgoing transitions.

```bash
# Detect the softlock (FAILS — Dead is a dead end)
mununu context eval examples/game/player_fsm.espec.json \
    --adapter extraction --formula no_softlock --automaton PlayerState

# Or use --template directly
mununu context eval examples/game/player_fsm.espec.json \
    --adapter extraction --template no_deadlock --automaton PlayerState
```

The fixed version adds a `ev_respawn` transition from Dead to Idle:

```bash
# Verify the fix (PASSES — all states have exits)
mununu context eval examples/game/player_fsm_fixed.espec.json \
    --adapter extraction --formula no_softlock --automaton PlayerState
```

### Quest System (Unreachable Goal)

A quest system with circular prerequisites. Quest C requires Quest B, but the chain creates a dependency loop, making `AllComplete` unreachable.

```bash
# Check if all quests are completable (FAILS — circular dependency)
mununu context eval examples/game/quest_deadlock.espec.json \
    --adapter extraction --formula all_quests_completable --automaton QuestProgress
```

### NPC AI (Behavior Loop)

An NPC with PATROL/CHASE/ATTACK/RETREAT states. Under certain conditions, the AI can oscillate between CHASE and RETREAT.

```bash
# Check for AI deadlocks (PASSES — all states have exits)
mununu context eval examples/game/npc_ai_loop.espec.json \
    --adapter extraction --formula no_ai_deadlock --automaton AIState

# Check if Attack is reachable (PASSES)
mununu context eval examples/game/npc_ai_loop.espec.json \
    --adapter extraction --formula attack_reachable --automaton AIState
```

## Property Templates Used

| Template | What It Checks |
|----------|---------------|
| `no_deadlock` | Every state has at least one outgoing transition |
| `reachable(TARGET)` | Target state is reachable from initial |
| `always_eventually(TARGET)` | Target is always eventually reached (GF pattern) |

See `mununu templates` for the full list, or the [Property Templates wiki page](../../wiki/Property-Templates.md).

## Writing Your Own

1. Model your game FSM as states and transitions in an `.espec.json` file
2. Add properties using `template_ref` (no mu-calculus knowledge needed)
3. Run `mununu context eval` with `--adapter extraction`

See the [Game Engine Integration wiki page](../../wiki/Game-Engine-Integration.md) for a full guide.
