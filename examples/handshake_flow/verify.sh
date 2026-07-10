#!/usr/bin/env bash
# Runs inside the mununu-sva image: lift the assembled design and check the
# whole-system monitors. Expects /work/_build.sv.
#
#   Safety + assume-guarantee discharge  -> k-induction (sound, unbounded proof).
#   Liveness (eager transfer)            -> bounded model checking to depth 100.
#     The eager-transfer property holds but is not k-inductive in this system
#     (the enq<->FIFO-occupancy<->full feedback resists a simple invariant), so
#     we report it honestly as a bounded check, not an unbounded proof.
set -e
# sv2v first so the FIFO's concurrent SVA lifts cleanly (yosys' built-in Verilog
# frontend doesn't parse `assert property (@(posedge clk) ...)`).
sv2v /work/_build.sv > /work/_flat.v 2>/dev/null
yosys -q -p "read_verilog -formal /work/_flat.v; prep -top top -flatten; write_btor /work/_sys.btor" 2>/dev/null

check () {  # <signal> <mode: kind|bmc> <label>
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

echo "Safety + discharge (k-induction, sound):"
check bad_overflow       kind "no overflow"
check bad_underflow      kind "no underflow"
check bad_drop           kind "no drop"
check bad_phantom        kind "no phantom accept"
check bad_assume_cready  kind "consumer discharges G F c_ready"
echo "Liveness (bounded model checking, depth 100):"
check bad_stall_enq      bmc  "eager accept, no >1-cycle stall"
check bad_stall_deq      bmc  "eager deliver, no >1-cycle stall"
rm -f /work/_one.btor /work/_sys.btor /work/_flat.v
