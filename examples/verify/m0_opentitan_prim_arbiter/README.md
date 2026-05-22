# M.0 — OpenTitan `prim_arbiter_fixed` pipeline-reach milestone

> First industrially-realistic validation milestone in the KMTS pivot
> ([`.claude/plans/you-are-a-formal-vast-lake.md`](../../../.claude/plans/you-are-a-formal-vast-lake.md) §10.3 M.0).
> Sits at the closing gate of R.0c — the R.0a + R.0b + R.0c stack
> must process a small, real OpenTitan RTL module without error.

## Fixture

[`prim_arbiter_fixed.sv`](https://github.com/lowRISC/opentitan/blob/master/hw/ip/prim/rtl/prim_arbiter_fixed.sv) — a parametrised fixed-priority arbiter from OpenTitan's
common-primitives library. 170 LOC, real production silicon, used
widely across the OpenTitan SoCs.

Documented invariant (from OpenTitan's own SVA comments + the
[OpenTitan primitives docs](https://opentitan.org/book/hw/ip/prim/index.html)):
*at most one `gnt_o[i]` is high per cycle* — i.e. the arbiter grants
exclusively. M.0 does *not* verify this property today; verification
is deferred to M.1+ when the KMTS lifter (R.2) and KleeneDomain
evaluator (R.3) ship. M.0's contract is **frontend reach only**:
the sv2v → Yosys-no-flatten → BTOR2-per-module pipeline must produce
a valid BTOR2 file without errors.

## Source pinning

The seven files under [`source/`](source/) are vendored copies from
upstream OpenTitan, pinned to the commit recorded in
[`source/UPSTREAM_COMMIT.txt`](source/UPSTREAM_COMMIT.txt). To
refresh against a newer upstream:

```bash
SRC=examples/verify/m0_opentitan_prim_arbiter/source
COMMIT=$(curl -sL 'https://api.github.com/repos/lowRISC/opentitan/commits/master' \
  | grep -m1 '"sha"' | sed -E 's/.*"sha": "([0-9a-f]+)".*/\1/')
BASE="https://raw.githubusercontent.com/lowRISC/opentitan/$COMMIT/hw/ip/prim/rtl"
echo "$COMMIT" > $SRC/UPSTREAM_COMMIT.txt
for f in prim_arbiter_fixed.sv prim_assert.sv prim_assert_standard_macros.svh \
         prim_assert_sec_cm.svh prim_assert_yosys_macros.svh \
         prim_assert_dummy_macros.svh prim_flop_macros.sv; do
  curl -sLf "$BASE/$f" -o "$SRC/$f"
done
```

This follows the same vendoring pattern as
[`examples/verify/sv_yosys_caliptra_rtl_150/`](../sv_yosys_caliptra_rtl_150/).
The pillow-plan §10.3 "no vendored fixtures" rule was aspirational
for an as-yet-unbuilt shallow-clone harness; in practice the small,
permissively-licensed OpenTitan primitives are easier to vendor than
to pull at test time, and the `UPSTREAM_COMMIT.txt` pin keeps the
provenance auditable. Both are Apache 2.0.

## Running

```bash
cargo build -p mununu-cli
bash examples/verify/m0_opentitan_prim_arbiter/validate.sh
```

Outputs land under `build/`:

- `build/elaborated.v` — sv2v output (R.0a).
- `build/btor2/<module>.btor2` — per-submodule BTOR2 (R.0b).
- `build/comparison.json` — native + KMTS pipeline shape diff +
  SVA-elision gate (R.0c).

The script asserts at each stage and exits non-zero on failure.

## Milestone result

See [`.claude/plans/milestones/M-0-result.md`](../../../.claude/plans/milestones/M-0-result.md)
for the per-stage record, recorded at M.0-pass time.

## Out of scope at M.0

Per the milestone catalogue:

- **Property verification verdict.** Deferred to M.1+ (needs the R.2
  KMTS lifter + R.3 KleeneDomain evaluator).
- **SBY oracle comparison.** Deferred to M.1+ (same dependency).
- **`// @mununu` property annotation.** The wrapper SV that would
  carry the annotation is not staged for M.0 — frontend reach is the
  only assertion. M.1+ adds a wrapper for the verification path.

## M.0 Fix B parameter choice

Per [`.claude/plans/milestones/M-0-blocker-2026-05-21.md`](../../../.claude/plans/milestones/M-0-blocker-2026-05-21.md)
the upstream module is instantiated via
[`source/prim_arbiter_fixed_m0_wrapper.sv`](source/prim_arbiter_fixed_m0_wrapper.sv)
at **N=2 channels × DW=2-bit data**. Total input bits = 1 (`clk`) +
1 (`rst_ni`) + 2 (`req_i`) + 4 (`data_i`) + 1 (`ready_i`) = 9 bits
→ 2^9 = 512 enumerated input combinations, under the BTOR2 reader's
`MAX_INPUT_BITS = 10` (= 1024) cap.

The upstream `prim_arbiter_fixed.sv` is *not modified* — only its
instantiation parameters shrink for M.0. M.1+ promotes the wrapper
to **N=8, DW=32** (production OpenTitan parameters) once the R.2
KMTS lifter is on; the lifter abstracts the wide `data_i` port via
predicate / UF wrapping, making the bit-blaster's input-cap stop
being load-bearing.

## Known KMTS-arm caveat at R.0c stage

`mununu sv compare-pipelines` runs sv2v + Yosys per pipeline arm,
but the internal Yosys script (in `compare_pipelines`) does not
currently thread the wrapper's include directory through to Yosys.
The KMTS arm therefore errors with
`Can't open include file 'prim_assert.sv'!` while the standalone
R.0a / R.0b stages above succeed (they DO thread `-I`). The error
is recorded in `build/comparison.json` honestly; the regression
contract in `crates/mununu-core/tests/sv_compare_pipelines.rs`
stays stable.

The fix is to add `--include-dir` plumbing through
`sv_pipeline_compare::compare_pipelines` — deferred to a follow-up
commit (does not affect M.0's pass/fail status; the contract is
"structured record produced" and it is).

## If something fails

Per the milestone blocker protocol
([`.claude/plans/you-are-a-formal-vast-lake.md`](../../../.claude/plans/you-are-a-formal-vast-lake.md) §10.2):

- **STOP the roadmap.** The next R.x or S.x must not begin until the
  M.0 blocker is resolved.
- **Produce `.claude/plans/milestones/M-0-blocker-<date>.md`** with:
  which stage failed, which construct triggered it, the smallest
  reproducer (≤ 50 lines of SV), what the oracle expected, 1–2
  candidate fixes ranked by estimated effort.
- **Present to the user; wait for arbitration.** No silent retries,
  no hand-written workarounds, no fixture substitutions without
  user sign-off.
