#!/bin/sh
set -e
SRC=/work/scratchpad/sdrc_src
OUT=/work/scratchpad/sdrc_lift
mkdir -p "$OUT"; cd "$OUT"
sv2v -I "$SRC" "$SRC/sdrc_req_gen.v" > sdrc_req_gen.sv2v.v 2> sv2v.err
# page_ovflw is now a genuine input; the old address arithmetic dangles → opt_clean prunes it.
yosys -q -p "
  read_verilog sdrc_req_gen.sv2v.v;
  hierarchy -top sdrc_req_gen -check;
  proc; flatten; opt -full; async2sync; dffunmap; opt_clean;
  write_btor sdrc_req_gen.env.btor2
" 2> yosys.err
echo 'inputs:'; grep -E '^[0-9]+ input' sdrc_req_gen.env.btor2
echo 'req_ack:'; grep -E 'req_ack' sdrc_req_gen.env.btor2 | head -1
