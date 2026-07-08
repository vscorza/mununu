#!/usr/bin/env bash
# HWMCC bit-vector track engine study.
#
# Documents per-engine VERDICT + WALL-CLOCK for the four portfolio members —
# exact (BDD, in-process), native (BMC + k-induction, in-process), btormc and
# Pono (subprocess) — over a directory of BTOR2 benchmarks the USER provides.
#
# The HWMCC benchmark set ships UNLICENSED, so it is never vendored into this repo
# (see tests/differential_oracle_e2e.rs::hwmcc_adjudication_over_user_dir). Download
# it yourself and point MUNUNU_HWMCC_DIR at a leaf directory of .btor2 files.
#
# Usage (inside the mununu-sva image; btormc + pono on PATH):
#   MUNUNU_HWMCC_DIR=/hwmcc/bv \
#   RESULTS=/results BUDGET=1200 JOBS=4 \
#     scripts/hwmcc_bv_study.sh
#
# Outputs under $RESULTS:
#   inhouse.tsv   file  native_v native_ms  exact_v exact_ms
#   sub/<b>.tsv   b  btormc_v btormc_ms  pono_v pono_ms    (one per benchmark, resumable)
#   results.tsv   the joined per-benchmark table
#   summary.txt   coverage per engine, soundness alarms, native-vs-Pono comparison
#
# Resumable: re-running skips any benchmark whose sub/<b>.tsv already exists.
# Verdicts: sat = bad reachable, unsat = proven safe, ? = timeout / unknown / abstain.
set -uo pipefail

DIR="${MUNUNU_HWMCC_DIR:?set MUNUNU_HWMCC_DIR to a directory of .btor2 files}"
RESULTS="${RESULTS:-/tmp/hwmcc_study}"
BUDGET="${BUDGET:-1200}"          # per-engine wall-clock cap (seconds)
JOBS="${JOBS:-4}"                 # concurrent benchmarks in the subprocess phase
KMAX="${BTORMC_KMAX:-1000}"       # btormc --kind depth bound
NATIVE_MAXK="${NATIVE_MAXK:-100}"
NATIVE_MS="${NATIVE_MS:-30000}"   # native per-check budget (ms)
mkdir -p "$RESULTS/sub"

mapfile -t FILES < <(find "$DIR" -maxdepth 1 -name '*.btor2' | sort)
echo "== HWMCC bv study: ${#FILES[@]} benchmarks · budget=${BUDGET}s · jobs=$JOBS =="

# ---- Phase 1: in-house engines (exact + native), one fast cargo run ----
echo "== phase 1: exact + native (in-process, kmax=$NATIVE_MAXK, ${NATIVE_MS}ms/check) =="
MUNUNU_HWMCC_DIR="$DIR" MUNUNU_NATIVE_MAXK="$NATIVE_MAXK" MUNUNU_NATIVE_MS="$NATIVE_MS" \
  cargo test -p mununu-core --test differential_oracle_e2e hwmcc_inhouse_timing \
    -- --ignored --nocapture 2>/dev/null | grep '^INHOUSE' > "$RESULTS/inhouse.tsv" || true
echo "  in-house rows: $(wc -l < "$RESULTS/inhouse.tsv" 2>/dev/null || echo 0)"

# ---- Phase 2: subprocess engines (btormc || pono in parallel), resumable ----
echo "== phase 2: btormc || pono (subprocess, ${BUDGET}s each, ${JOBS}-way) =="
run_one() {
  local f="$1" base out s bms pms b p
  base=$(basename "$f" .btor2)
  out="$RESULTS/sub/$base.tsv"
  [ -s "$out" ] && { echo "  skip $base (cached)"; return; }
  # btormc and pono concurrently so the benchmark's wall-clock is bounded by BUDGET.
  ( s=$(date +%s); b=$(timeout "$BUDGET" btormc --kind -kmax "$KMAX" "$f" 2>/dev/null | grep -oE '^(sat|unsat)' | head -1); \
    printf '%s\t%s' "${b:-?}" "$(( ($(date +%s) - s) ))" > "$RESULTS/sub/.$base.b" ) &
  local bp=$!
  ( s=$(date +%s); p=$(timeout "$BUDGET" pono -e ic3bits "$f" 2>/dev/null | grep -oE '^(sat|unsat)' | head -1); \
    printf '%s\t%s' "${p:-?}" "$(( ($(date +%s) - s) ))" > "$RESULTS/sub/.$base.p" ) &
  local pp=$!
  wait "$bp" "$pp"
  IFS=$'\t' read -r b bms < "$RESULTS/sub/.$base.b"
  IFS=$'\t' read -r p pms < "$RESULTS/sub/.$base.p"
  rm -f "$RESULTS/sub/.$base.b" "$RESULTS/sub/.$base.p"
  printf '%s\t%s\t%s\t%s\t%s\n' "$base" "$b" "$((bms*1000))" "$p" "$((pms*1000))" > "$out"
  echo "  done $base: btormc=$b(${bms}s) pono=$p(${pms}s)"
}
export -f run_one; export RESULTS BUDGET KMAX
n=0
for f in "${FILES[@]}"; do
  run_one "$f" &
  n=$((n+1)); [ $((n % JOBS)) -eq 0 ] && wait
done
wait

# ---- Phase 3: merge + summarize ----
echo "== phase 3: merge + summary =="
{
  printf 'benchmark\tnative_v\tnative_ms\texact_v\texact_ms\tbtormc_v\tbtormc_ms\tpono_v\tpono_ms\n'
  for f in "${FILES[@]}"; do
    base=$(basename "$f" .btor2)
    # native/exact from inhouse.tsv (col2=file with .btor2); sub/<base>.tsv for subprocess.
    ih=$(awk -F'\t' -v b="$base.btor2" '$2==b{print $3"\t"$4"\t"$5"\t"$6; found=1} END{if(!found)print "?\t0\t?\t0"}' "$RESULTS/inhouse.tsv")
    sub=$(cat "$RESULTS/sub/$base.tsv" 2>/dev/null | cut -f2-5)
    sub=${sub:-$'?\t0\t?\t0'}
    printf '%s\t%s\t%s\n' "$base" "$ih" "$sub"
  done
} > "$RESULTS/results.tsv"

awk -F'\t' 'NR>1{
  total++
  for(e=0;e<4;e++){}
  # columns: 1 bench, 2 native_v 3 native_ms, 4 exact_v 5 exact_ms, 6 btormc_v 7 btormc_ms, 8 pono_v 9 pono_ms
  nv=$2; ev=$4; bv=$6; pv=$8
  if(nv!="?")nd++; if(ev!="?")ed++; if(bv!="?")bd++; if(pv!="?")pd++
  # soundness: any two DEFINITE verdicts that disagree
  split("",vs); c=0
  if(nv!="?"){vs[c++]="native:"nv}; if(ev!="?"){vs[c++]="exact:"ev}
  if(bv!="?"){vs[c++]="btormc:"bv}; if(pv!="?"){vs[c++]="pono:"pv}
  hasS=0; hasU=0
  for(i=0;i<c;i++){if(vs[i]~/:sat/)hasS=1; if(vs[i]~/:unsat/)hasU=1}
  if(hasS&&hasU){ contra[$1]=1 }
  # native vs pono
  if(nv!="?"&&pv=="?") nonly[$1]=nv
  if(nv=="?"&&pv!="?") ponly++
  if(nv!="?"&&pv!="?"){ both++; if($3+0 < $9+0) nfaster++; else pfaster++ }
}
END{
  printf "benchmarks: %d\n", total
  printf "decided:  native=%d  exact=%d  btormc=%d  pono=%d\n", nd, ed, bd, pd
  nc=0; for(k in contra)nc++
  printf "SOUNDNESS: definite disagreements = %d\n", nc
  for(k in contra) printf "  ALARM %s\n", k
  no=0; for(k in nonly)no++
  printf "native decides where Pono does NOT: %d\n", no
  for(k in nonly) printf "  native-only %s = %s\n", k, nonly[k]
  printf "Pono decides where native does NOT: %d\n", ponly
  printf "both decide: %d  (native faster %d, pono faster %d)\n", both, nfaster, pfaster
}' "$RESULTS/results.tsv" | tee "$RESULTS/summary.txt"

echo "== results: $RESULTS/results.tsv  summary: $RESULTS/summary.txt =="
