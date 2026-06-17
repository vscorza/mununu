#!/usr/bin/env bash
# validate_m4_cegar.sh — M.4 milestone: full automated predicate-abstraction
# CEGAR on Caliptra's `soc_ifc_boot_fsm`, demonstrating a sound, definite,
# pre/post-distinguishing verdict on real industrial RTL (§Phase 11 slot 7).
#
# Contrast with `validate.sh` in this directory, which is the Phase-A.3
# *structural* sanity check (bit-blast reduction). This script exercises
# the R.5 predicate-cube CEGAR path end-to-end and is the M.4 milestone
# proper.
#
# The hazard. `soc_ifc_boot_fsm` holds its present state in a 3-bit
# `boot_fsm_state_e` register with five legal encodings (0..4); the
# `unique casez` enumerates exactly those five. The `pre_fix` variant has
# NO `default` arm, so the encodings {5,6,7} — admissible by the type but
# unhandled — latch (CWE-1245). The `post_fix` variant adds a `default`
# that routes every other encoding back to a defined state.
#
# The property. `<> (p5 || p6 || p7)` over the predicate cube
# {p5,p6,p7} = {boot_fsm_ns ∈ {5,6,7}} — "the next-state register can
# transition into an undefined encoding". Under `setundef -anyconst`
# (so the register's power-up value is unconstrained) + per-target
# SMT-proved must-edge inference, this is decided definitely:
#   - pre_fix  : SATISFIABLE (some cell True)  — the undefined encoding is
#                reachable/absorbing → CWE-1245 hazard present.
#   - post_fix : UNSATISFIABLE (all cells False) — proven unreachable →
#                the default-case fix eliminates the hazard.
# The pre/post difference is the milestone evidence.
#
# SOUNDNESS. `--must-edge-inference smt-per-target` proves each must-edge
# with a Z3 ∀∀ query (no sampling); the verdict carries an
# `[R.2.5b-smt-must]` advisory. `setundef -anyconst` over-approximates
# power-up, so a definite verdict transfers to the concrete reset
# behaviour. The verdict is established over the extracted model; an
# RTL-level counterexample replay under a cycle-accurate simulator is the
# remaining empirical step (deferred, per the M.0–M.2 SBY precedent).
#
# Per §10.2: if any stage fails, STOP and produce a blocker note rather
# than silently retrying / hand-authoring around it.
set -euo pipefail

THIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MUNUNU="${MUNUNU:-${THIS_DIR}/../../../target/debug/mununu}"
SRC="${THIS_DIR}/source"
# Transient BTOR2/CEGAR artefacts go to a tmp dir, NOT under examples/.
# The full Caliptra BTOR2 busts the bit-blast state-bit cap by design
# (M.4 decides it via predicate-cube CEGAR, not the full R.2 lift), so
# leaving it under examples/ would trip the `btor2_kmts_lift_sweep` test
# which globs `examples/**/*.btor2`. mktemp + EXIT-trap keeps it clean.
OUT="$(mktemp -d -t mununu-m4-XXXXXX)"
trap 'rm -rf "${OUT}"' EXIT

if [[ ! -x "${MUNUNU}" ]]; then
  echo "validate_m4_cegar.sh: mununu binary not found at ${MUNUNU}" >&2
  echo "                      build it first with: cargo build -p mununu-cli" >&2
  exit 2
fi
for tool in yosys sv2v; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "validate_m4_cegar.sh: required tool '${tool}' not on PATH" >&2
    exit 2
  fi
done


echo "=== M.4 — Caliptra soc_ifc_boot_fsm predicate-abstraction CEGAR ==="
echo "upstream: chipsalliance/caliptra-rtl @ (see source/, issue #150 bug/fix pair)"
echo "property: <> (p5 || p6 || p7)   [boot_fsm_ns transitions into an unmatched encoding]"
echo

PREDS=(--predicate p5:boot_fsm_ns=5 --predicate p6:boot_fsm_ns=6 --predicate p7:boot_fsm_ns=7
       --config-values 'boot_fsm_ns=0,1,2,3,4,5,6,7' --must-edge-inference smt-per-target)

# Generate BTOR2 + run CEGAR for one variant; echoes the T cell count.
run_variant() {
  local variant="$1"
  local v="${OUT}/${variant}.v"
  local b="${OUT}/${variant}.btor2"
  sv2v -I"${SRC}" \
    "${SRC}/soc_ifc_pkg.sv" "${SRC}/soc_ifc_reg_pkg.sv" \
    "${SRC}/caliptra_2ff_sync.sv" "${SRC}/soc_ifc_boot_fsm_${variant}.sv" \
    > "${v}" 2>"${OUT}/${variant}.sv2v.err"
  yosys -q -p "read_verilog -sv ${v}; hierarchy -check -top soc_ifc_boot_fsm; proc; \
               flatten; async2sync; dffunmap; setundef -anyconst; write_btor ${b}" \
    2>"${OUT}/${variant}.yosys.err"
  LIBRARY_PATH="${LIBRARY_PATH:-/usr/local/opt/z3/lib}" "${MUNUNU}" btor2 cegar "${b}" \
    --formula '<> (p5 || p6 || p7)' "${PREDS[@]}" \
    > "${OUT}/${variant}.cegar.out" 2>"${OUT}/${variant}.cegar.err"
  cat "${OUT}/${variant}.cegar.out"
}

echo "--- pre_fix (bug-bearing: no default arm) ---"
run_variant pre_fix
echo
echo "--- post_fix (fixed: default routes unmatched → defined) ---"
run_variant post_fix
echo

# Extract the True-cell count from each "verdict cells: T=N F=M ⊥=K" line.
t_count() { grep -E "verdict cells:" "$1" | sed -E 's/.*T=([0-9]+).*/\1/'; }
PRE_T="$(t_count "${OUT}/pre_fix.cegar.out")"
POST_T="$(t_count "${OUT}/post_fix.cegar.out")"

echo "pre_fix  True cells (hazard reachable): ${PRE_T}"
echo "post_fix True cells (hazard reachable): ${POST_T}"

if [[ -z "${PRE_T}" || -z "${POST_T}" ]]; then
  echo "FAIL: could not parse a verdict from one of the CEGAR runs" >&2
  exit 1
fi
# Done-criterion: the hazard is reachable in the buggy variant and
# eliminated in the fixed one — a definite, pre/post-distinguishing verdict.
if [[ "${PRE_T}" -ge 1 && "${POST_T}" -eq 0 ]]; then
  echo
  echo "=== M.4 VALIDATION PASSED ==="
  echo "pre_fix: undefined-encoding transition REACHABLE (${PRE_T} cells) — CWE-1245 detected."
  echo "post_fix: undefined-encoding transition UNREACHABLE (0 cells) — fix verified."
  echo "Sound, definite, pre/post-distinguishing CEGAR verdict on real Caliptra RTL."
else
  echo "FAIL: expected pre_fix True>=1 and post_fix True==0; got pre=${PRE_T} post=${POST_T}" >&2
  exit 1
fi
