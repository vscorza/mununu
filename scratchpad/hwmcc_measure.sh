#!/usr/bin/env bash
# Full-suite HWMCC measurement for #5, run INSIDE the mununu-sva container.
#
# Pass 1: `btor2 verify` (bit-level portfolio, now with native_interp as a concurrent
#   member + the uncorroborated-SPACER guard) over ALL 136 designs.
# Pass 2: `btor2 verify-safety` (KMTS 3-valued cube + emergent-K) over a NAMED sample
#   (the cube is not a bit-level-safety engine; a full second pass would ~double a
#   multi-hour run for an expected ~0 decides — the sample is listed so nothing is
#   silently dropped).
#
# Robust output: the mununu binary prints an INFO log line to stdout before the JSON,
# and the container has no `jq`. So per design we extract the pretty-printed JSON block
# (`sed '/^{/,/^}/p'`) and compact it to ONE line (tr) — engine attribution
# (reachable_by / unreachable_by, incl. `interp`) is preserved and parsed on the host.
#
# Mounts: /work=repo  /cargo-target=warm target volume  /hwmcc=~/hwmcc20-bench
# Output: /hwmcc/results-13jul/ (persisted on host).
set -u
BENCH=/hwmcc/flat
OUT=/hwmcc/results-13jul
mkdir -p "$OUT"
PER_DESIGN_TIMEOUT=90

echo "=== building release binary (warm cache) ==="
cargo build --release -p mununu-cli 2>&1 | tail -3
BIN=/cargo-target/release/mununu
[ -x "$BIN" ] || { echo "FATAL: $BIN not built"; exit 1; }
"$BIN" --version 2>/dev/null | head -1
echo "tools: btormc=$(command -v btormc||echo NO) pono=$(command -v pono||echo NO) cvc5=$(command -v cvc5||echo NO)"

emit() {  # design | rc | dur | one-line JSON (engine attribution preserved)
  local f="$1" verb="$2" extra="$3"
  local d; d=$(basename "$f" .btor2)
  local start=$SECONDS
  local raw; raw=$(timeout "$PER_DESIGN_TIMEOUT" "$BIN" btor2 "$verb" $extra "$f" 2>/dev/null)
  local rc=$?
  local dur=$((SECONDS-start))
  local json; json=$(printf '%s\n' "$raw" | sed -n '/^{/,/^}/p' | tr '\n' ' ' | tr -s ' ')
  [ -z "$json" ] && json='{ADAPTER-ERR-OR-TIMEOUT}'
  printf '%-46s | rc=%s dur=%ss | %s\n' "$d" "$rc" "$dur" "$json"
}

echo "=== PASS 1: btor2 verify (all 136) ==="
: > "$OUT/verify.log"
for f in "$BENCH"/*.btor2; do emit "$f" verify "" | tee -a "$OUT/verify.log"; done

echo "=== PASS 2: btor2 verify-safety cube (named sample) ==="
: > "$OUT/verify-safety.log"
SAMPLE="gen12 gen14 gen39 gen10 gen21 vcegar_arrays_itc99_b12_p2 vis_arrays_am2910_p1 paper_v3 cal159 mul7 arbitrated_top_n2_w8_d16_e0"
for d in $SAMPLE; do
  f="$BENCH/$d.btor2"; [ -f "$f" ] || { echo "MISSING $d"; continue; }
  emit "$f" verify-safety "" | tee -a "$OUT/verify-safety.log"
done
echo "=== ALL DONE ==="
