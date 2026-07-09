#!/usr/bin/env bash
# Runs inside the mununu-sva image: lift the assembled design and check every
# whole-system monitor by k-induction. Expects /work/_build.sv.
set -e
yosys -q -p "read_verilog -sv -formal /work/_build.sv; prep -top top -flatten; write_btor /work/_sys.btor" 2>/dev/null
for sig in bad_mutex bad_starve_0 bad_starve_1 bad_proto_0 bad_proto_1; do
  node=$(awk -v s="$sig" '$2=="output" && $4==s{print $3}' /work/_sys.btor)
  grep -vE ' bad ' /work/_sys.btor > /work/_one.btor
  mid=$(awk 'NF && $1 ~ /^[0-9]+$/ {if($1>m)m=$1} END{print m}' /work/_one.btor)
  echo "$((mid+1)) bad ${node}" >> /work/_one.btor
  printf "  %-14s : " "$sig"
  btormc -kmax 60 --kind /work/_one.btor 2>&1 | grep -vE "unsupported tag" | head -1
done
rm -f /work/_one.btor /work/_sys.btor
