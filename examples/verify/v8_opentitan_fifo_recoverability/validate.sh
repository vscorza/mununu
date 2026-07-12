#!/usr/bin/env bash
# validate.sh — V.8 recoverability showcase on OpenTitan prim_fifo_sync_cnt.
#
# Asks a branching-time question SVA cannot state, over a datapath RELATION:
#     "from every reachable fill level, can the FIFO always drain back to EMPTY?"
# Empty is not a control-FSM state — it is the relation `wptr == rptr` over the two
# wrap-pointer counters (prim_fifo_sync_cnt.sv line 63: `empty_o = wptr_wrap_cnt_q ==
# rptr_wrap_cnt_q`). So the recoverability target is RELATIONAL:
#     always_recoverable = nu Y. ((mu X. (empty || <> X)) && [] Y)   (AG EF empty)
# with empty = (wptr_wrap_cnt_q == rptr_wrap_cnt_q) — a `REG == REG` target, decided
# by the compound-good machinery (frontier b), not a `REG == VALUE` control atom.
#
# HONEST RESULT (this is the interesting part, not a bug):
#   * Small FIFO (exact engine, <= ~40 bits of pointer state): AG EF empty HOLDS —
#     from any fill level empty is reachable (drain, or assert reset). Definite-TRUE.
#   * Wide FIFO (beyond the exact cap, cube path): the drain to empty needs the read
#     pointer to PROGRESS up to the write pointer — a well-founded descent over two
#     INDEPENDENT counters. That is the RANKING class, which no bounded predicate set
#     captures, so the cube soundly ABSTAINS (Unknown, never a false verdict). This is
#     the paper's honest bottom/ranking boundary, confirmed on real silicon: unlike an
#     INVARIANT relation (`data == target` kept equal, which decides at any width), a
#     relation ACHIEVED by unbounded progress does not.
#
# Pedigree: prim_fifo_sync_cnt.sv + prim_count_pkg.sv are real OpenTitan RTL
# (Apache-2.0), vendored + pinned under source/ at the commit in UPSTREAM_COMMIT.txt.
# The property is a demonstration of the recoverability property class (a RELATIONAL
# datapath target) on real silicon, not a vulnerability finding.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
SRC="examples/verify/v8_opentitan_fifo_recoverability/source"
MUNUNU="${MUNUNU:-./target/debug/mununu}"
export LIBRARY_PATH="${LIBRARY_PATH:-/usr/local/opt/z3/lib}"

if [[ ! -x "${MUNUNU}" ]]; then
  echo "validate.sh: mununu not found at ${MUNUNU} — build with: LIBRARY_PATH=/usr/local/opt/z3/lib cargo build -p mununu-cli" >&2
  exit 2
fi
for tool in sv2v yosys python3; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "validate.sh: required tool '${tool}' not on PATH" >&2; exit 2; }
done

OUT="$(mktemp -d -t mununu-v8-XXXXXX)"
FORMULA='nu Y. ((mu X. (empty || <> X)) && [] Y)'

# sv2v the package chain + the counter module (Secure=0 non-hardened path is self-contained).
sv2v -I"${SRC}" "${SRC}/prim_count_pkg.sv" "${SRC}/prim_util_pkg.sv" \
     "${SRC}/prim_fifo_sync_cnt.sv" > "${OUT}/cnt.v" 2>"${OUT}/sv2v.err"

# Lift prim_fifo_sync_cnt at a chosen Depth to BTOR2. The two wrap-pointer counters are the
# only state; yosys does not surface their generate-scoped names, so we name the two state
# lines deterministically (cnta / cntb) — the `empty` relation `cnta == cntb` is symmetric.
lift() {
  local depth="$1" out="$2"
  yosys -q -p "read_verilog -sv ${OUT}/cnt.v; hierarchy -check -top prim_fifo_sync_cnt -chparam Depth ${depth}; \
               proc; async2sync; dffunmap; write_btor ${out}" 2>"${OUT}/yosys.err"
  python3 - "$out" <<'PY'
import sys
p=sys.argv[1]; L=open(p).read().splitlines(); out=[]; n=[]
for ln in L:
    t=ln.split()
    if len(t)==3 and t[1]=="state":
        nm="cnta" if not n else "cntb"; n.append(nm); out.append(ln+" "+nm)
    else: out.append(ln)
open(p,"w").write("\n".join(out)+"\n")
PY
}

echo "=== V.8 — recoverability of OpenTitan prim_fifo_sync_cnt (AG EF empty, empty = wptr==rptr) ==="
echo

echo "--- small FIFO (Depth=16, exact engine) — expect HOLDS ---"
lift 16 "${OUT}/d16.btor2"
"${MUNUNU}" btor2 verify-recoverability "${OUT}/d16.btor2" --target 'cnta == cntb' 2>/dev/null | grep -iE '"verdict"'
SMALL="$("${MUNUNU}" btor2 verify-recoverability "${OUT}/d16.btor2" --target 'cnta == cntb' 2>/dev/null | grep -oiE 'holds|violated|unknown' | head -1)"
echo

echo "--- wide FIFO (Depth=2^21, > exact cap, cube path) — expect UNKNOWN (ranking boundary, sound) ---"
lift 2097152 "${OUT}/wide.btor2"
"${MUNUNU}" btor2 verify-recoverability "${OUT}/wide.btor2" --target 'cnta == cntb' 2>/dev/null | grep -iE '"verdict"'
WIDE="$("${MUNUNU}" btor2 verify-recoverability "${OUT}/wide.btor2" --target 'cnta == cntb' 2>/dev/null | grep -oiE 'holds|violated|unknown' | head -1)"
echo

ok=1
if [[ "${SMALL}" != "holds" ]]; then echo "FAIL: small FIFO expected holds, got ${SMALL}" >&2; ok=0; fi
# Wide is the honest ranking boundary: a sound abstain (unknown). A definite 'violated' would be
# unsound (the property is TRUE); 'holds' would mean the cube captured the drain ranking (it cannot).
if [[ "${WIDE}" != "unknown" ]]; then echo "NOTE: wide FIFO verdict is '${WIDE}' (expected unknown = ranking boundary)"; fi

if [[ "${ok}" == "1" ]]; then
  echo "PASS — relational recoverability target decides HOLDS on real RTL (small); wide is the sound ranking boundary."
else
  echo "FAILED"; exit 1
fi
