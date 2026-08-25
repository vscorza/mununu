# §D FULL actionable-⊥ sweep (post-#414-424) — measured 2026-08-01

Binary: release from main (48d4285 — all of #414-424). Bare `--refine --discover-assumptions` = the full
refined UX (config_partition A · holds_under InputHold/EnvStrategy B/2b · vacuous/bot_diagnosis Phase-0),
with the A→B composition + output-target fix (#422/#423) active.

## Reproducible recoverability set

| design | src | verdict | refinement | actionable |
|---|---|---|---|---|
| aes_cipher (OT) | host btor2 | HOLDS | config_partition{h:[rst_ni=0], v:[rst_ni=1]} | ✅ A |
| csrng (OT) | host btor2 | HOLDS | config_partition{h:[rst_ni=0], v:[rst_ni=1]} | ✅ A |
| aes_ctr (OT) | host btor2 | HOLDS | config_partition{h:[rst_ni=0], v:[rst_ni=1]} | ✅ A |
| **compute_engine_faulty** | image, slang | HOLDS | config_partition{h:[rst_n=0],v:[rst_n=1]} **+ holds_under[InputHold: rst_n==1 && err==0]** | ✅✅ A + B-composed |
| compute_engine (good) | image, slang | HOLDS | {} | — control (no FP) |
| i2c_scl_padoen == 1 | host btor2 | HOLDS | {} | — config-indep (no FP) |
| i2c_scl_padoen == 0 | host btor2 | TIMEOUT >200s | — (base doesn't decide) | out of exact reach |

## Headline delta vs §3.12b/c (pre-#422-424)

**compute_engine_faulty is now FULLY actionable end-to-end on real RTL:** it moved from *config_partition
only, holds_under empty* (§3.12c, pre-#423) → *config_partition + `holds_under[rst_n==1 && err==0]`*. The
A→B composition (#422) + the output-target monitor fix (#423) close the loop: capability A localizes the
operational trap (`rst_n=1` violated), capability B says how to live with it (hold the fault strobe low).

## §D actionable picture (honest, unchanged framing from §3.12b)

- **Config-dependent recovery: 4/4 actionable** (config_partition; faulty also gets the composed operational
  assumption). config-partition ENRICHES config-dependent HOLDS/VIOLATED rows — this is its reach.
- **Config-independent: 2/2 correctly no refinement** (good core + i2c stuck-at-1) — 0 false positives.
- **Base-timeout: i2c==0** — the exact-residual gap (>200s), NOT a refinement failure (the base verdict
  itself doesn't complete; refinement is moot where the base can't decide).
- **Env-strategy (2b):** the fallback only fires when base != Holds AND slice-1 finds no constant hold; none
  of the reproducible rows hit it (OT are HOLDS/reset-only; faulty is covered by the composed InputHold). Its
  marginal reach is validated on the synthetic POSITIONAL_TRAP + wall-class test.
- **Literal ⊥-% mover** stays Phase-0 (vacuous/bot_diagnosis) + assumption discovery; the AssertLLM2
  posture-⊥ set is uncommitted (re-fetchable). This sweep's measurable NEW result = the composition/output-fix
  making the real-RTL faulty case fully actionable.

## Sweep hygiene note
The first host invocation prints the `Logging initialized` INFO to stdout (subsequent ones don't), so a
naive `2>/dev/null | jq` blanks the FIRST row (aes_cipher showed empty; standalone = HOLDS+config_partition).
Fix for future sweeps: `RUST_LOG=error` or `sed -n '/^{/,$p'` before jq.
