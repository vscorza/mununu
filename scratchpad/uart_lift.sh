#!/bin/sh
set -e
SRC=/work/scratchpad/uart_src; OUT=/work/scratchpad/uart_lift
mkdir -p "$OUT"; cd "$OUT"
sv2v -I "$SRC" "$SRC/uart_msg_handler.v" > uart.sv2v.v 2> sv2v.err
# No opt -full — preserve the State reg name.
yosys -q -p "read_verilog uart.sv2v.v; hierarchy -top uart_msg_handler -check; proc; flatten; async2sync; dffunmap; opt_clean; write_btor uart.named.btor2" 2> yosys.err
echo '=== ALL inputs (names + widths) ==='
awk '/^[0-9]+ input/ {print}' uart.named.btor2 | while read nid kw sort rest; do
  w=$(grep -E "^$sort sort bitvec" uart.named.btor2 | awk '{print $4}'); echo "nid=$nid w=$w name=$rest"; done
echo '=== named states ==='; grep -E '^[0-9]+ state [0-9]+ [a-zA-Z]' uart.named.btor2 | head
