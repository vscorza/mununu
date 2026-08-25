#!/bin/sh
set -e
SRC=/work/scratchpad/t6507_src
OUT=/work/scratchpad/t6507_lift
mkdir -p "$OUT"; cd "$OUT"
sv2v -I "$SRC" "$SRC/t6507lp_fsm.v" > t6507lp_fsm.sv2v.v 2> sv2v.err
# No `opt -full` — preserve the `state` reg name.
yosys -q -p "
  read_verilog t6507lp_fsm.sv2v.v;
  hierarchy -top t6507lp_fsm -check;
  proc; flatten; async2sync; dffunmap; opt_clean;
  write_btor t6507lp_fsm.named.btor2
" 2> yosys.err
grep -E '^[0-9]+ state [0-9]+ [a-zA-Z]' t6507lp_fsm.named.btor2 | head
