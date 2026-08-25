# §D actionable-⊥ sweep — measured results (2026-07-31, bare `--refine`, slice-2b binary a8cecfe)

Bare `--refine` = the default UX (auto config-atom identification from slice 2b). Verified via direct runs
(the sweep-script's empty-verdict rows were a scratch jq-parsing glitch; stdout JSON is clean — the log-init
line goes to stderr).

## The config-partition TARGET CLASS — config-dependent (reset-gated) recovery: 4/4 auto-partitioned

| design | source | verdict | refinement (bare --refine) |
|---|---|---|---|
| aes_cipher (OT) | host, pure-btor2 | HOLDS | **config_partition{holds:[rst_ni=0], violated:[rst_ni=1]}** |
| csrng (OT) | host, pure-btor2 | HOLDS | **config_partition{holds:[rst_ni=0], violated:[rst_ni=1]}** |
| aes_ctr (OT) | host, pure-btor2 | HOLDS | **config_partition{holds:[rst_ni=0], violated:[rst_ni=1]}** |
| compute_engine_faulty | mununu-sva-pono, lifted | HOLDS | **config_partition{holds:[rst_n=0], violated:[rst_n=1], exhaustive}** |

## Config-INDEPENDENT designs — correctly NO partition (0 false positives)

| design | verdict | refinement |
|---|---|---|
| compute_engine (good) | HOLDS | {} (recovers via own logic, not reset-gated) |
| i2c_scl_padoen == 1 | HOLDS | {} (stuck-at-1, config-independent) |

## Genuine hard-⊥ — --refine is MOOT (the base verdict itself doesn't complete)

| design | canonical verdict |
|---|---|
| i2c_scl_padoen == 0 | TIMEOUT >280s (155-bit exact-residual gap, §3.11 — pre-existing, not slice-2b) |

## §D HEADLINE FINDING (measure-first / wall-class-matrix marginal reach)

**config-partition (capability A) is 4/4 on its target class (config-dependent reset-gated recovery),
0 false positives, and ~0 marginal reach on the genuine-⊥ set.** This is the correct measured result, not a
gap: the genuine ⊥ rows are unreachable-target / value-dependent / base-timeout — NOT config-dependent recovery,
so config-partition is the wrong tool for them (and correctly stays silent). The actionable-⊥ % over genuine
⊥ rows is moved by **Phase 0** (vacuous / bot_diagnosis) and the future **Phase 2** (holds_under), NOT capability A.

**Capability A's real contribution = ENRICHMENT of config-dependent HOLDS/VIOLATED verdicts**, turning a flat
verdict into the SVA-inexpressible "operational trap vs reset escape" partition. The clean-room contrast proves
it: `compute_engine` (good) → HOLDS + {}; `compute_engine_faulty` (reset-only escape) → HOLDS + partition
revealing the operational lockup. **The partition itself distinguishes a recovering core from a lockup** —
exactly the documented bug, surfaced automatically with no user-named config.

**Reframe for §D:** the literal "fraction of ⊥/VIOLATED rows with a refinement" understates capability A
(its wins are config-dependent HOLDS rows, outside that denominator). The honest per-capability metric:
- config-partition: 4/4 on config-dependent recovery, 0 FP → **the enrichment lever**
- vacuous / bot_diagnosis (Phase 0): the ⊥-row diagnostic lever (i2c ==0 would carry it if the base decided)
- holds_under (Phase 2, unbuilt): the ⊥-row DECIDE lever — this is what moves the literal §D ⊥ %.
