# Free-form observations for EXP-0002b-mem

## 2026-05-06 — heap-axis hypothesis partially falsified, wall-clock unchanged

### What this archive proves

- The SoA migration's wall-clock 2.4× speedup on grid_32x32 (EXP-0002b) is NOT a heap-pressure win. Total allocation count and total bytes are within 0.02% across A and B.
- Peak heap drops 6.7% (76 KB), reflecting the iteration_ranks structure itself being smaller. The pre-registered ≥100 KB threshold is falsified at this fixture scale.
- The wall-clock win lives in cache locality + per-lookup cost, not in allocator pressure.

### What this changes

The paper §3.x and blog post 2 narrative shifts: SoA is a **cache-locality** win, not a **heap-pressure** win, at the tested scale. Honest framing, defensible claim, predicts where the win scales further (larger plant states, deeper alternation).

### What this archive does NOT mean

- The SoA migration is wrong. EXP-0002b's 2.4× wall-clock win is real and citable. EXP-0002b-mem just clarifies the mechanism.
- All heap-axis claims are wrong. Plan §A3 (CSR adjacency) targets `Vec<Vec<Transition>>` where heap reduction IS the primary mechanism (predicted -240 MB → -80 MB at million-edge scale). EXP-0002b-mem doesn't transfer to A3.
- dhat is the wrong tool. dhat is correct here; it's the hypothesis that was wrong.

### dhat methodology lessons

- `#[global_allocator] = dhat::Alloc` adds significant wall-clock overhead (~5-10× slowdown on the synthesis path). Don't conflate dhat-mode timing with bench-mode timing. Use dhat for allocation-axis answers; use Criterion-with-bench-record for wall-clock-axis answers.
- Total bytes / alloc count are nearly noise-free across runs (the workload is deterministic). Peak heap can vary by a few percent across runs due to allocator state at the moment t-gmax is sampled. Run twice to verify peak doesn't drift; in this EXP both runs were within 0.5%.
- 3 synthesis iterations is sufficient to amortize fixture-build cost. Adding more would inflate the totals proportionally without revealing new shape.

### What would have shipped without dhat

If I'd taken EXP-0002 README's "≥100 KB heap reduction" claim at face value and shipped post 2 without dhat instrumentation, I'd have published a wrong claim. The audit gate caught it.

### Iteration policy actions

- Used `--fresh` on the implicit Criterion archive (n/a for this EXP — no Criterion data).
- Documented the partial falsification (heap hypothesis) alongside the standing confirmation (wall-clock from EXP-0002b). Both archives stay; ADR-0010 will record the methodology lesson.
- Followup EXP-0002b-mem-deep at L4 with larger fixtures is the natural next step for paper-grade evidence.

### Anti-patterns avoided

- Did not silently downgrade the original EXP-0002 hypothesis. The README pre-registered ≥100 KB; this archive explicitly says "falsified by 24 KB."
- Did not over-claim. The 2.4× wall-clock win stands — but the mechanism is now correctly attributed to cache locality, not heap pressure.
- Did not skip the dhat measurement just because it might falsify a hypothesis. The whole point of the protocol is to expose claims to evidence.
