#!/bin/sh
set -e
SRC=/work/scratchpad/sdcmd_src
OUT=/work/scratchpad/sdcmd_lift
cd "$OUT"
sv2v -I "$SRC" "$SRC/sd_cmd_serial_host.v" "$SRC/sd_crc_7.v" > sd_cmd_serial_host.sv2v.v 2> sv2v.err
# NO `opt -full` — preserve the one-hot `state` reg name + encoding.
yosys -q -p "
  read_verilog sd_cmd_serial_host.sv2v.v;
  hierarchy -top sd_cmd_serial_host -check;
  proc; flatten; async2sync; dffunmap; opt_clean;
  write_btor sd_cmd_serial_host.named.btor2
" 2> yosys_named.err
echo '=== named state lines ==='
grep -E "^[0-9]+ state [0-9]+ [a-zA-Z\\\\]" sd_cmd_serial_host.named.btor2 | head -20
