# Witness Refinement Plan: Progress-Oriented Strategy Extraction

## Problem

The current witness recording happens during the **final** fixpoint iteration, when all winning states are already in the target set. This means witnesses may select self-loops or non-progress transitions (e.g., from S0, the witness might pick `env→S0` instead of `c→Target` even though `c→Target` is the actual progress move).

## Root Cause

In `eval_modal` (evaluator.rs), after `modal_exists` returns `true`, the witness scan finds "the first outgoing transition whose target is in `target_set`." But by the final iteration of `mu X. (target || <> X)`, `target_set` includes ALL winning states (including the source state itself via self-loops). So the witness may select a transition to a state that was ALREADY winning, rather than the transition that MADE the state winning.

## Solution: Use Iteration Ranks for Witness Selection

The `iteration_ranks` field already records when each state entered the fixpoint set:
```
iteration_ranks: HashMap<(usize, FormulaVarId), usize>
```

For a `mu` (least) fixpoint, states with **lower** iteration ranks are "closer" to the target — they entered the set earlier. The progress-oriented witness should prefer transitions to states with lower ranks.

### Implementation

In `eval_modal` (evaluator.rs), the witness recording block (currently lines ~391-407):

**Current:**
```rust
// Find the first outgoing transition whose target is in target_set
for (idx, transition) in self.clts.outgoing(state).iter().enumerate() {
    if target_set.get(transition.target().index()) ... {
        wm.witnesses.insert((state.index(), modal_node_id), idx);
        break;
    }
}
```

**Refined:**
```rust
// Find the transition whose target has the LOWEST iteration rank
// (entered the fixpoint earliest = closest to target)
let mut best_idx = None;
let mut best_rank = usize::MAX;
for (idx, transition) in self.clts.outgoing(state).iter().enumerate() {
    if !self.guard_matches(state, transition, guard) { continue; }
    if !target_set.get(transition.target().index()).map(|b| *b).unwrap_or(false) { continue; }

    // Look up iteration rank for the target state
    // For the active fixpoint variable, lower rank = entered earlier = more progress
    let rank = self.witness_map.as_ref()
        .and_then(|wm| {
            // Find the most relevant fixpoint variable from current bindings
            // Use the iteration rank if available, otherwise use MAX
            wm.iteration_ranks.iter()
                .filter(|(&(si, _), _)| si == transition.target().index())
                .map(|(_, &r)| r)
                .min()
        })
        .unwrap_or(0); // States in base predicate (rank 0 = target itself)

    if rank < best_rank || (rank == best_rank && best_idx.is_none()) {
        best_rank = rank;
        best_idx = Some(idx);
    }
}
if let Some(idx) = best_idx {
    wm.witnesses.insert((state.index(), modal_node_id), idx);
}
```

### Why This Works

For `mu X. (Target || <> X)` with states S0, S1, S2, Target:
- Iteration 1: Target enters X (rank 1)
- Iteration 2: S1, S2 enter X via transitions to Target (rank 2)
- Iteration 3: S0 enters X (rank 3)

When recording the witness for S0:
- `a→S1` has rank 2, `b→S2` has rank 2, `c→Target` has rank 1, `env→S0` has rank 3
- Refined selection: `c→Target` (rank 1 = shortest path to target)
- Current selection: `env→S0` (first found, but rank 3 = self-loop)

### Complexity

No additional asymptotic cost. The rank lookup is O(k) where k = number of fixpoint variables (typically 1-3). The transition scan is already O(outgoing).

### Files to Modify

- `src/mu_calculus/evaluator.rs` — witness recording block in `eval_modal`

### Testing

```bash
# Before: S0 witness = env→S0 (self-loop, rank 3)
# After:  S0 witness = c→Target (direct, rank 1)
cargo run -- context synth /tmp/test_witness_strategy.ctxdsl \
  --formula reach_target --automaton M --extract-strategy --emit-dsl /tmp/refined.ctxdsl
cat /tmp/refined.ctxdsl | grep transition
# Should show S0 → Target via c, not S0 → S0 via env
```

### References

- Jurdziński (2000) "Small Progress Measures for Solving Parity Games" — progress measures rank states by distance to winning condition
- Bruse, Friedmann & Lange (2016) — iteration numbers as strategy certificates
