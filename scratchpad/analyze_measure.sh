#!/usr/bin/env bash
# Analyze the #5 full-suite measurement (host, jq available). Answers the questions
# that decide the owned-engine story:
#   1. Overall decide-rate (holds/violated/unknown of 136).
#   2. Per-engine decide frequency — who carries the portfolio.
#   3. Is `interp` EVER a SOLE decider (owned-unique net-new decide)? — the #1 payoff.
#   4. Contradictions (must be 0 — the soundness net).
#   5. vcegar_arrays / array designs (the #4 guard region).
#   6. Regression check vs FINAL.md (any design decided there but unknown here).
set -u
L=~/hwmcc20-bench/results-13jul/verify.log
LS=~/hwmcc20-bench/results-13jul/verify-safety.log
jqline() { sed 's/^[^{]*//'; }  # strip the "design | rc | dur |" prefix, keep JSON

echo "===================== PASS 1: btor2 verify (of $(grep -c '|' "$L") designs) ====================="
for v in holds violated unknown; do printf "  %-9s %d\n" "$v" "$(grep -c "\"verdict\": \"$v\"" "$L")"; done
echo "  {adapter-err/timeout no-JSON}: $(grep -c 'ADAPTER-ERR-OR-TIMEOUT' "$L")"

echo "--- per-engine decide frequency (reachable_by ∪ unreachable_by) ---"
grep -oE '"(exact|native|spacer|interp|btormc|pono)"' "$L" | sort | uniq -c | sort -rn

echo "--- #1 payoff: designs where interp appears at all ---"
grep '"interp"' "$L" | sed 's/ | {.*//' || echo "  (none)"
echo "--- #1 KEY: designs where interp is the SOLE decider (owned-unique net-new) ---"
sole=0
while IFS= read -r ln; do
  d=$(printf '%s' "$ln" | sed 's/ *|.*//')
  json=$(printf '%s' "$ln" | jqline)
  decs=$(printf '%s' "$json" | jq -r '(.reachable_by + .unreachable_by) | join(",")' 2>/dev/null)
  if [ "$decs" = "interp" ]; then echo "  SOLE-INTERP: $d"; sole=$((sole+1)); fi
done < <(grep '"interp"' "$L")
echo "  => interp-sole count: $sole"

echo "--- #4 soundness: contradictions (must be 0) ---"
grep '"contradiction": true' "$L" | sed 's/ | {.*//' || true
echo "  contradiction count: $(grep -c '"contradiction": true' "$L")"

echo "--- #4 guard region: array designs ---"
grep -E '^vcegar_arrays|^vis_arrays' "$L"

echo "===================== PASS 2: verify-safety cube (sample) ====================="
cat "$LS" 2>/dev/null

echo "===================== REGRESSION vs FINAL.md ====================="
# Designs FINAL.md decided (reach/unreach non-empty) but we now call unknown.
if [ -f ~/hwmcc20-bench/verify-full.log ]; then
  echo "(compare manually: prior decides vs current unknowns)"
  comm -12 \
    <(grep -iE ':.*(reach|unreach)=\[[a-z]' ~/hwmcc20-bench/verify-full.log 2>/dev/null | sed 's/:.*//' | sort -u) \
    <(grep '"verdict": "unknown"' "$L" | sed 's/ *|.*//' | sort -u) \
    | sed 's/^/  REGRESSED?: /' || echo "  (prior log format differs; manual check)"
else
  echo "  (no ~/hwmcc20-bench/verify-full.log to diff)"
fi
echo "===================== DONE ====================="
