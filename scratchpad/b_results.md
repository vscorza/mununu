# B1–B4 industrial validation — assumption discovery (capability B, slice 1) — measured 2026-07-31

Binary: release with Phase-2a discover_assumptions (commit 4221d31). mununu-sva-pono image.
(jq is NOT in the image — parse raw JSON on the host.)

## Measured

| case | design (verb) | verdict | refinement | reading |
|---|---|---|---|---|
| B3 control | compute_engine (good), sv verify-recoverability --frontend slang, free | HOLDS | `{}` | no spurious φ, no partition — clean control ✓ |
| A-on-faulty | compute_engine_faulty, free | HOLDS | `config_partition{holds:[rst_n=0], violated:[rst_n=1]}` | free reset ⇒ HOLDS; capability A ALREADY reveals the reset-escape / operational-trap. `holds_under` empty (B doesn't fire on HOLDS). |
| B mechanism | EN_TRAP (RTL-shaped btor2), verify_recoverability_refined | VIOLATED | `holds_under:[en == 1]` | slice-1 constant-input-hold WORKS on the input-gated-trap class (unit + wall-class tests, CI-gated) ✓ |
| B1 | OT aes_cipher operational | — | — | slice-1 (constant hold) does NOT express the temporal `ResetEventually` the plan expects here; correctly abstains (deferred temporal slice). Capability A covers the reset-dependence. |

## The honest finding (plan-aligned)

Slice-1 (single **constant** input-hold) behaves exactly as designed:
- **REACH:** the input-gated-trap class — holding a non-reset input avoids an absorbing trap (EN_TRAP → `en==1`). Validated on RTL-shaped btor2, CI-gated (unit + wall-class `assumption_discovery_marginal_reach_on_input_gated_trap`).
- **NO false positives:** good core → ∅ (B3); unconditional trap (STALLER) → ∅; structurally-dead target → ∅ (soundness control).
- **CORRECT abstention:** reset-only / temporal recovery (OT, B1) is NOT a constant hold → slice-1 finds nothing; the meaningful φ there is the temporal `ResetEventually` — a DEFERRED slice. Capability A (config_partition over the reset) already delivers the reset-escape narrative on those designs.

## The one wiring gap (for the real-RTL constant-hold case)

`compute_engine_faulty` operationally recovers by holding `err==0` (a non-reset constant hold — squarely in slice-1's class), BUT:
- `sv verify-recoverability` lifts the **FREE** model (reset not gated) ⇒ verdict HOLDS via the reset escape ⇒ B doesn't fire.
- It has **no `--config-value` reset-pin** (that flag is on `sv verify-auto`, which gates the reset by default but drives @mununu_guarantee, not `--target`+`--discover-assumptions`).

⇒ To exercise B on reset-having RTL's OPERATIONAL model, EITHER:
1. **Reset-gate `verify-recoverability`** (or add `--config-value`) so it verifies the running design — arguably the right default for `AG EF` (a free reset trivially satisfies recoverability via the escape); OR
2. **Compose A→B**: when `config_partition` finds a VIOLATED (operational) cell, run `discover_assumptions` on that pinned cell → the operational enabling assumption (`err==0`). Reuses the pin machinery, no new flag.

Both are clean follow-ups; (2) is the more elegant (A localizes the bad config, B explains how to live with it).

## Deliverable committed
- Wall-class `assumption` column: `assumption_discovery_marginal_reach_on_input_gated_trap` (tests/wall_class_matrix.rs, non-#[ignore], CI-gated) — the lever's fixed-set entry (marginal reach + no-FP), per the RULE.
