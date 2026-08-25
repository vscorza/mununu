#!/bin/sh
set -e
SRC=/work/scratchpad/i2c_src
OUT=/work/scratchpad/i2c_lift
mkdir -p "$OUT"; cd "$OUT"
sv2v -I "$SRC" "$SRC/i2c_master_byte_ctrl.v" > byte.sv2v.v 2> sv2v.err
yosys -q -p "
  read_verilog byte.sv2v.v $SRC/bit_ctrl_bb.v;
  hierarchy -top i2c_master_byte_ctrl;
  proc; flatten; opt -full; async2sync; dffunmap; opt_clean;
  write_btor i2c_byte.btor2
" 2> yosys.err
echo 'c_state?'; grep -E '^[0-9]+ state [0-9]+ [a-zA-Z]' i2c_byte.btor2 | head
echo 'inputs:'; grep -E '^[0-9]+ input' i2c_byte.btor2 | head -20
