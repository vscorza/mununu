#!/usr/bin/env bash
# Runs inside the mununu-sva image: lift the assembled design and check the
# whole-system monitors. Expects /work/_build.sv.
#
#   Interlock + assume-guarantee discharge -> k-induction (sound, unbounded).
#   Liveness (eager sleep honor)           -> bounded model checking, depth 100.
set -e
sv2v /work/_build.sv > /work/_flat.v 2>/dev/null
yosys -q -p "read_verilog -formal /work/_flat.v; prep -top top -flatten; write_btor /work/_sys.btor" 2>/dev/null

check () {  # <signal> <kind|bmc> <label>
  node=$(awk -v s="$1" '$2=="output" && $4==s{print $3}' /work/_sys.btor)
  grep -vE ' bad ' /work/_sys.btor > /work/_one.btor
  mid=$(awk 'NF && $1 ~ /^[0-9]+$/ {if($1>m)m=$1} END{print m}' /work/_one.btor)
  echo "$((mid+1)) bad ${node}" >> /work/_one.btor
  printf "  %-18s : " "$1"
  if [ "$2" = kind ]; then
    v=$(btormc -kmax 80 --kind /work/_one.btor 2>&1 | grep -viE "unsupported tag" | head -1)
    echo "${v:-inconclusive} ($3)"
  else
    v=$(btormc -kmax 100 /work/_one.btor 2>&1 | grep -viE "unsupported tag" | head -1)
    [ -z "$v" ] && echo "no counterexample to depth 100 ($3)" || echo "$v ($3)"
  fi
}

echo "Interlock + discharge (k-induction, sound):"
check bad_interlock    kind "never gate an active domain"
check bad_assume_idle  kind "domain discharges G F !activity"
echo "Liveness (bounded model checking, depth 100):"
check bad_sleep_stall  bmc  "eager sleep honor, no >1-cycle stall"
rm -f /work/_one.btor /work/_sys.btor /work/_flat.v
