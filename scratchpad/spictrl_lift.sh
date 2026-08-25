#!/bin/sh
# Lift spiCtrl (AssertLLM2 spiMaster SD/SPI dispatcher FSM) to BTOR2.
# rst is kept as a primary input so the game verb's --assume-clock-reset can pin it inactive + inject init.
set -e
SRC=/work/scratchpad/spictrl_src
OUT=/work/scratchpad/spictrl_lift
mkdir -p "$OUT"
cd "$OUT"

# sv2v resolves `include "timescale.v" / "spiMaster_defines.v" via -I.
sv2v -I "$SRC" "$SRC/spiCtrl.v" > spiCtrl.sv2v.v 2> sv2v.err

yosys -q -p "
  read_verilog spiCtrl.sv2v.v;
  hierarchy -top spiCtrl -check;
  proc; flatten; opt -full; async2sync; dffunmap; opt_clean;
  write_btor spiCtrl.btor2
" 2> yosys.err

echo '=== state + input lines ==='
grep -E '^[0-9]+ (state|input|output)' spiCtrl.btor2 | head -30
