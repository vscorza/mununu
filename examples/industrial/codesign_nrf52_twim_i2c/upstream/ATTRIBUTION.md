# Upstream attribution — nrfx (Nordic Semiconductor)

This directory contains files derived (verbatim, except where noted) from the
upstream `nrfx` HAL repository maintained by Nordic Semiconductor ASA. The
files anchor the codesign flagship at
`examples/industrial/codesign_nrf52_twim_i2c/` to a real vendor's silicon spec
and a real vendor's production firmware code, satisfying CLAUDE.md's Claims
Integrity § Rule 1 (models from source, not documentation).

## Source

- **Upstream URL**: https://github.com/NordicSemiconductor/nrfx
- **Commit SHA**: `0883a272c34004697dd56dfa44f6e2d0f8705689`
- **Retrieval date**: 2026-05-14

## Licence

**BSD-3-Clause** (not Apache-2.0 — Plan 1 §1.2's assumption was incorrect and is
corrected here). The full licence text is checked in alongside the derived
files at [`nrfx-LICENSE.txt`](nrfx-LICENSE.txt).

> Copyright (c) 2017 - 2026, Nordic Semiconductor ASA
> All rights reserved.
> SPDX-License-Identifier: BSD-3-Clause

BSD-3-Clause permits unmodified and modified redistribution provided the
copyright notice, conditions, and disclaimer are preserved, and the Nordic
Semiconductor name is not used to endorse derived work. All three conditions
are honoured by this directory and the surrounding README.

## Files derived

| File in this directory | Upstream path | Verbatim? |
|---|---|---|
| `nrf52840.svd` | `bsp/stable/mdk/nrf52840.svd` | Yes |
| `nrfx_twim.c` | `drivers/src/nrfx_twim.c` | Yes |
| `nrfx-LICENSE.txt` | `LICENSE` | Yes |
| `nrfx_twim_buggy.c` | derived from `drivers/src/nrfx_twim.c` | **Modified — see README "Planted-bug disclosure" section** |

`nrfx_twim_buggy.c` carries an in-source disclosure block (per CLAUDE.md
Claims Integrity § Rule 2) explaining that the modification is a deliberately
introduced demonstration bug, not a real defect in nrfx. The pattern is
anchored against public Nordic errata (Errata 211 and the broader family of
register-write-ordering anomalies on early nRF5x silicon).

## What about Plan 1's Apache-2.0 assumption?

The plan at `.claude/plans/pre-deal-shipping-nrf52-twim-flagship.md` calls the
upstream licence "Apache-2.0". That was incorrect at the time the plan was
written; nrfx ships under BSD-3-Clause. The flagship's redistribution
obligations are essentially the same in practice (preserve notice, no
endorsement), so the example's structure does not change — but the README,
this attribution file, and any downstream publication drafts must say
BSD-3-Clause, not Apache-2.0.
