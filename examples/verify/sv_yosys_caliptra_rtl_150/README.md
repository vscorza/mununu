# sv_yosys_caliptra_rtl_150 — Caliptra-RTL boot FSM auto-extraction

> **Phase A.3 structural sanity check.** This fixture is *not yet*
> a property-verification claim — see [Status](#status) below.

## What this exercises

Runs the `sv-yosys → BTOR2 → bit-blast` pipeline on
chipsalliance/caliptra-rtl's
[`soc_ifc_boot_fsm.sv`](https://github.com/chipsalliance/caliptra-rtl/issues/150)
(`pre_fix` variant, the bug-bearing source). The headline measurement:

| Metric | Before Phase A.3 | After Phase A.3 |
|---|---|---|
| Bit-blaster verdict | killed at 22 min / 4.6 GB RSS | completes in seconds |
| Realised state count | n/a (didn't finish) | **4 096** |
| Raw state-cell width | 2^19 = 524 288 | unchanged (abstraction reduces enumeration, not width) |
| Reduction factor | — | **128×** |

The reduction comes from **two composing mechanisms**:

1. **Sidecar-declared abstractions** (`wait_count`, `fw_update_rst_wait_cycles`,
   `BootFSM_BrkPoint`, `BootFSM_Continue`) — user wins on collision per
   the Phase A.3 §3.5 policy.
2. **Auto cone-of-influence** — runs but does not classify any new
   drops on this fixture, because the user-curated sidecar already
   lists every relevant signal. The auto-COI pass is exercised through
   the same code path that drove the headline reduction on smaller
   designs (see
   [`crates/mununu-core/src/adapter/btor2/dep_graph.rs`](../../../crates/mununu-core/src/adapter/btor2/dep_graph.rs))
   and is observable in the bit-blaster's pre-partition state-count
   warning (`2^19 = 524288 explicit states` → post-abstraction 4 096).

## Status

> **Source of truth:** [`validate.sh`](validate.sh) (asserts state-count
> cap) — surface: CLI-only — this is an example fixture, not a CLI
> capability.
> **Status: structural milestone end-to-end; verdict is not yet a
> finding.** Two layers report cleanly:
>
> 1. **Translation + bit-blast:** completes in seconds. State count
>    4 096 (raw bit width 2^19 = 524 288 → 128× reduction via the
>    sidecar's `wait_count` bounded counter + per-input `Ignored` /
>    `BoundedCounter` declarations).
> 2. **Mu-calculus evaluation:** completes in roughly 2–3 minutes on a
>    debug build (sets the Phase A roadmap's runtime gate at ≈ 5 min
>    wall-clock). All three sidecar properties — `no_undef_reachable`,
>    `boot_idle_reachable`, `safety_all_states_have_successors` —
>    return verdicts.
>
> However, the verdicts are **not a property claim** today. The
> formula atoms (`boot_fsm_ps == 5 || boot_fsm_ps == 6 ||
> boot_fsm_ps == 7`) need to bind to state-cell valuations on the
> bit-blasted CLTS; today they parse as free atoms that map to "false"
> uniformly, which makes the negation vacuously true on every state
> and `no_undef_reachable` returns 4 096 / 4 096 satisfying — a
> **vacuous** verdict, not a discriminating one. The same pre-fix
> source `vs.` the post-fix source would produce identical verdicts,
> so the verdict cannot be cited as evidence the bug was found.
>
> The earlier "CTXDSL re-parse" blocker (`unexpected token
> Symbol(LBrace)` when passing `--sidecar foo.mununu.json` explicitly
> to `mununu context`) was a CLI UX issue, not a pipeline blocker. It
> was fixed in the same Phase A.3 commit that ships this example —
> `--sidecar foo.mununu.json` is now treated as informational (the
> adapter auto-loads it via path adjacency).
>
> Per
> [`docs/policies/claims-integrity.md`](../../../docs/policies/claims-integrity.md):
> this is a **"structural milestone, not yet reproduced"** claim. The
> abstraction landed the design under the bit-blaster's
> `MAX_STATE_BITS = 20` cap *and* the mu-calculus evaluator returns
> finite-time verdicts; the **CWE-1245 UNDEF-state reachability**
> finding referenced in
> [`docs/design/caliptra-abstraction-analysis.md`](../../../docs/design/caliptra-abstraction-analysis.md)
> §2.3 cannot be cited as a verdict until **state-cell-aware
> predicate binding** ships (separate scope from Phase A.3).

## Files

| Path | Purpose |
|---|---|
| `source/soc_ifc_boot_fsm_pre_fix.sv` | Upstream RTL (bug variant) |
| `source/soc_ifc_boot_fsm_post_fix.sv` | Upstream RTL (fix variant) |
| `source/soc_ifc_pkg.sv` | Package referenced by the boot FSM |
| `source/soc_ifc_reg_pkg.sv` | Register package referenced by the boot FSM |
| `source/caliptra_top_reg_defines.svh` | Header referenced by the package |
| `source/caliptra_sva.svh` | SVA macro file referenced by the source |
| `source/soc_ifc_boot_fsm_pre_fix.mununu.json` | Hand-curated sidecar (`mununu_sv_annotation_v1`) |
| `validate.sh` | Structural sanity check runnable from this directory |

## Reproduction

```bash
# From the repo root:
cargo build -p mununu-cli
./examples/verify/sv_yosys_caliptra_rtl_150/validate.sh
```

Requirements:

- `yosys` 0.59 or later on `PATH`
- `sv2v` 0.0.13 or later on `PATH` (the dev container provides both)

## Provenance

- Upstream issue: <https://github.com/chipsalliance/caliptra-rtl/issues/150>
- Upstream commit: `b436906f0b16ae` (chipsalliance/caliptra-rtl)
- CWE class referenced by the upstream maintainer: CWE-1245 (missing
  default in state machine).
- mununu phase context:
  [`.claude/plans/phase-a3-adapter-partition.md`](../../../.claude/plans/phase-a3-adapter-partition.md)
  step 3.7.

## See also

- [`docs/design/caliptra-abstraction-analysis.md`](../../../docs/design/caliptra-abstraction-analysis.md)
  — original Phase 1.7 analysis predicting the 4 000× reduction.
- [`docs/design/auto-extraction-architecture.md`](../../../docs/design/auto-extraction-architecture.md)
  §6 — decision-gate thresholds the Caliptra runtime feeds into.
- [`docs/design/proof-by-fire-findings.md`](../../../docs/design/proof-by-fire-findings.md)
  — the predecessor doc that motivated this example.
