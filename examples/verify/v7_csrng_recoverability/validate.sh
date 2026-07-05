#!/usr/bin/env bash
# validate.sh — V.7-c recoverability of OpenTitan csrng_main_sm (AG EF idle),
# decided SOUNDLY by the exact-symbolic engine.
#
# The branching-time question SVA cannot state: "from every reachable state, can
# the FSM still get back to MainSmIdle?" — recoverability,
#     always_recoverable = nu Y. ((mu X. (idle || <> X)) && [] Y)   (AG EF idle)
# with idle = (state_q == MainSmIdle = 6'b110111 = 55).
#
# SOUNDNESS NOTE (2026-07-05). An earlier version of this example evaluated the
# property over the PREDICATE-CUBE path (`btor2 cegar`) with the DEFAULT
# `may=off` (sampling) may-edges. Sampling under-approximates the may-relation
# (one representative per cube + a capped input set), which violates the KMTS
# `concrete ⊆ may` precondition and is UNSOUND for this branching property: it
# produced a spurious reset-dependent "flip" that does not match the RTL (where
# MainSmError self-loops and is a permanent trap without reset). See
# `mununu-private/.../recoverability-soundness-findings.md`. This script now uses
# the EXACT-SYMBOLIC engine (full bit-blasted state, no abstraction, no ⊥), which
# decides the property definitely and soundly.
#
# The sound result — recovery depends on reset, exactly as the SEC_CM
# sparse-FSM-plus-alert design intends:
#   * Normal operation (init = MainSmIdle): AG EF idle HOLDS — verified out of
#     reset, the running FSM always returns to idle; it never wedges.
#   * Fault premise (FSM forced into MainSmError, reset withheld): AG EF idle
#     VIOLATED — from the hardened error state, idle is UNREACHABLE without reset;
#     the error state is a permanent trap. Recovery is possible only through
#     reset (the flop's `state_q_next = rst_ni ? state_d : MainSmIdle` mux).
#
# Pedigree / claims integrity: csrng_main_sm.sv is real OpenTitan RTL (Apache-2.0)
# vendored under ../m2_opentitan_csrng_main_sm/source/. The fault premise forces
# the flop's reset value to MainSmError to MODEL a fault that has driven the FSM
# into its error state; the FSM's error self-loop (the behaviour under test) is
# unchanged, so the VIOLATED verdict reflects the real design's error-trap
# behaviour. This is a demonstration of the recoverability property class on real
# silicon, not a vulnerability finding.
#
# Requires the mununu-sva toolchain (slang + sv2v + yosys) — run in the
# `mununu-sva` Docker image. The standard prim_assert macros come from the M.0
# prim_arbiter fixture (the csrng dir's own prim_assert.sv is the dummy variant).
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
CS="examples/verify/m2_opentitan_csrng_main_sm/source"
PR="examples/verify/m0_opentitan_prim_arbiter/source"
MUNUNU="${MUNUNU:-./target/debug/mununu}"

if [[ ! -x "${MUNUNU}" ]]; then
  echo "validate.sh: mununu not found at ${MUNUNU} — build with: cargo build -p mununu-cli" >&2
  exit 2
fi
for tool in slang sv2v yosys; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "validate.sh: required tool '${tool}' not on PATH (run in the mununu-sva image)" >&2; exit 2; }
done

OUT="$(mktemp -d -t mununu-v7c-XXXXXX)"
trap 'rm -rf "${OUT}"' EXIT
AGEF='// @mununu_guarantee nu Y. ((mu X. ((state_q == 55) or <> X)) and [] Y)'

cp "${CS}/csrng_pkg.sv" "${PR}/prim_assert.sv" "${PR}/prim_assert_standard_macros.svh" \
   "${PR}/prim_assert_sec_cm.svh" "${PR}/prim_flop_macros.sv" "${OUT}/"
SRCS=(--source "${OUT}/csrng_pkg.sv" --source "${OUT}/prim_assert.sv"
      --source "${OUT}/prim_assert_standard_macros.svh" --source "${OUT}/prim_assert_sec_cm.svh"
      --source "${OUT}/prim_flop_macros.sv")

# Normal: FSM reset value is MainSmIdle (unchanged).
{ echo "${AGEF}"; cat "${CS}/csrng_main_sm.sv"; } > "${OUT}/normal.sv"
# Fault premise: force the FSM reset value to MainSmError (models a fault-injected
# error state); the error self-loop logic is untouched.
{ echo "${AGEF}"; sed 's/main_sm_state_e, MainSmIdle)/main_sm_state_e, MainSmError)/' "${CS}/csrng_main_sm.sv"; } > "${OUT}/fault.sv"

run() { "${MUNUNU}" sv verify-auto "$1" "${SRCS[@]}" --top csrng_main_sm --preprocess-sv2v \
          --engine exact-symbolic 2>&1 | grep -E "ann_guarantee" | head -1; }

echo "=== V.7-c — recoverability of OpenTitan csrng_main_sm (AG EF MainSmIdle), exact-symbolic ==="
echo
echo "--- normal operation (init = MainSmIdle) — expect HOLDS ---"
NORMAL="$(run "${OUT}/normal.sv")"; echo "  ${NORMAL}"
echo
echo "--- fault premise (FSM in MainSmError, reset withheld) — expect VIOLATED ---"
FAULT="$(run "${OUT}/fault.sv")"; echo "  ${FAULT}"
echo

ok=1
grep -qi "HOLDS" <<<"${NORMAL}" || { echo "FAIL: normal operation expected HOLDS, got: ${NORMAL}" >&2; ok=0; }
grep -qi "VIOLATED" <<<"${FAULT}" || { echo "FAIL: fault premise expected VIOLATED, got: ${FAULT}" >&2; ok=0; }
[[ "${ok}" == "1" ]] || exit 1

echo "=== V.7-c VALIDATION PASSED ==="
echo "The running FSM always recovers to idle (HOLDS); a fault-induced error state"
echo "cannot reach idle without reset (VIOLATED) — a branching-time (AG EF)"
echo "recoverability property decided soundly and exactly on real OpenTitan RTL."
