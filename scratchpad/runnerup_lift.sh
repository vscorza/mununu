#!/bin/sh
# Lift the two fairness runner-ups to BTOR2. rst kept as input (posture pins it).
set -e

# --- sd_data_master (wishbone_sd_card_controller) ---
SRC=/work/scratchpad/sddm_src
OUT=/work/scratchpad/sddm_lift
mkdir -p "$OUT"; cd "$OUT"
sv2v -I "$SRC" "$SRC/sd_data_master.v" > sd_data_master.sv2v.v 2> sv2v.err
yosys -q -p "
  read_verilog sd_data_master.sv2v.v;
  hierarchy -top sd_data_master -check;
  proc; flatten; opt -full; async2sync; dffunmap; opt_clean;
  write_btor sd_data_master.btor2
" 2> yosys.err
echo '=== sd_data_master state/input ==='
grep -E '^[0-9]+ (state|input)' sd_data_master.btor2 | head -25

# --- sd_cmd_serial_host (sdc_controller) + sd_crc_7 submodule ---
SRC2=/work/scratchpad/sdcmd_src
OUT2=/work/scratchpad/sdcmd_lift
mkdir -p "$OUT2"; cd "$OUT2"
sv2v -I "$SRC2" "$SRC2/sd_cmd_serial_host.v" "$SRC2/sd_crc_7.v" > sd_cmd_serial_host.sv2v.v 2> sv2v.err
yosys -q -p "
  read_verilog sd_cmd_serial_host.sv2v.v;
  hierarchy -top sd_cmd_serial_host -check;
  proc; flatten; opt -full; async2sync; dffunmap; opt_clean;
  write_btor sd_cmd_serial_host.btor2
" 2> yosys.err
echo '=== sd_cmd_serial_host state/input ==='
grep -E '^[0-9]+ (state|input)' sd_cmd_serial_host.btor2 | head -30
