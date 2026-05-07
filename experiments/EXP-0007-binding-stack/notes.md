# Free-form observations for EXP-0007-binding-stack

Append-only. Date entries with `## YYYY-MM-DD` headers.

## 2026-05-07 — Refactor scope

The change touched 8 method signatures in `mu_calculus/evaluator.rs`: `eval_node`, `bitwise_and`, `bitwise_or`, `eval_modal`, `eval_fixpoint`, `variable_bits`, `eval_node_tri`, `eval_fixpoint_tri`. Plus 4 top-level call sites (`evaluate`, `evaluate_with_witnesses`, `evaluate_tri_with_options` and the helper test path). The signature change `&HashMap` → `&mut HashMap` was mechanical. The interesting work was in `eval_fixpoint`:

```rust
// outer: manage scope
let prev = bindings.remove(&var);
let result = self.eval_fixpoint_in_scope(var, body, kind, bindings);
match prev {
    Some(p) => { bindings.insert(var, p); }
    None => { bindings.remove(&var); }
}
result

// inner: actual fixpoint loop
fn eval_fixpoint_in_scope(...) -> Result<...> {
    let mut current_set = ...;
    bindings.insert(var, self.clone_bitvec(&current_set)?);
    loop {
        let next_set = self.eval_node(body, bindings)?;
        // record ranks ...
        if next_set == current_set { return self.clone_bitvec(&next_set); }
        current_set = self.clone_bitvec(&next_set)?;
        bindings.insert(var, self.clone_bitvec(&current_set)?);
    }
}
```

The two-function split avoids the explicit cleanup at every `?` early-return — the outer function runs the restore unconditionally regardless of whether the inner returned Ok or Err.

## 2026-05-07 — Why the prediction missed

I assumed `HashMap::clone()` was meaningfully expensive at small K. Empirically it isn't. The std HashMap's RawTable::clone uses `ptr::copy_nonoverlapping` on the bucket array — at K=0 that's a 24-byte memcpy. The "savings" I thought I was getting (eliminate the clone) was 24 bytes per iteration, well below the cost of `HashMap::insert` (which recomputes the hash, walks the probe sequence, drops the old value, writes the new one).

If I'd written a microbench *just* for `HashMap.clone()` vs `HashMap.insert(same_var, ...)` on a 1-entry map before doing the full refactor, I'd have caught this in 5 minutes instead of 90.

This pairs with EXP-0012 (§B4 changed-flag): both falsifications come from the same root cause — assuming the std/bitvec primitive being replaced was the bottleneck, when in fact the primitive's hot loop is already heavily optimized. ADR-0017 captures this as the "already-optimized primitive" trap.

## 2026-05-07 — Pre-existing minimization::idempotence proptest failure

While running tests on the BindingStack candidate, `cargo test --features test_support --test properties` failed on `minimization::idempotence` with seed 9382785361923416088. The seed is checked in to `crates/mununu-core/tests/properties/minimization.proptest-regressions` as a known regression — meaning proptest found this case at some prior run and committed it for re-test. The bug is in `composition/minimize.rs` (K-S minimize_bisimulation), not in the evaluator.

The lib tests (`cargo test -p mununu-core --lib --features test_support`) all pass — 825 / 825. So the BindingStack change is observably equivalent on every input the lib tests exercise. The orthogonal property failure is opened as EXP-0040-minimize-bug (followup, not scheduled).

## 2026-05-07 — What would actually win on alternation-2

If we want to reduce the per-iteration BitVec clone cost on alternation-2 fixpoints (the synthesis workload), the right targets are different:

1. **Avoid the `let next_set = self.eval_node(body, bindings)?;` allocation.** eval_node returns an owned BitVec; if we could write into a caller-provided buffer, we'd save an allocation per iteration.
2. **Avoid `current_set = self.clone_bitvec(&next_set)?;` at end of loop.** This is purely to detach `next_set` from the loop body. If we used an owned `Box<BitVec>` we could move instead of clone.
3. **Avoid the per-iteration `bindings.insert(var, self.clone_bitvec(&current_set)?)`.** If bindings holds `&BitVec` rather than `BitVec`, no clone needed — but then lifetimes get messy.

These are what §A6b *should have been* — none of them are bindings-clone-related. Schedule as EXP-0007b-fixpoint-alloc if pursued.

## 2026-05-07 — Statistical artifact note

`invariance_nu/grid_32x32` reports +7.4% with p=0.451 (above 0.01 significance threshold). That's a wide CI ([+0.8%, +14.3%]) and the bench may be near the noise floor for this fixture. NOT counted as a regression in bench_diff.sh's robust mode. Worth flagging because the *direction* across all benches is consistently positive (slowdown), which is suggestive even when individual benches are noisy.
