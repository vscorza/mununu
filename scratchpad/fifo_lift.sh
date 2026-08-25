#!/usr/bin/env bash
set -e
SRC=/work/examples/verify/v8_opentitan_fifo_recoverability/source
OUT=/work/scratchpad/fifo_lift; mkdir -p "$OUT"
sv2v -I"$SRC" "$SRC/prim_count_pkg.sv" "$SRC/prim_util_pkg.sv" "$SRC/prim_fifo_sync_cnt.sv" > "$OUT/cnt.v" 2>"$OUT/sv2v.err"
yosys -q -p "read_verilog -sv $OUT/cnt.v; hierarchy -check -top prim_fifo_sync_cnt -chparam Depth 4; proc; flatten; opt -full; async2sync; dffunmap; connect -set rst_ni 1'b1; opt_clean -purge; write_btor $OUT/fifo4.btor2" 2>"$OUT/yosys.err"
python3 - "$OUT/fifo4.btor2" <<'PY'
import sys
p=sys.argv[1]; L=open(p).read().splitlines(); out=[]; n=[]
for ln in L:
    t=ln.split()
    if len(t)==3 and t[1]=="state":
        nm="cnta" if not n else "cntb"; n.append(nm); out.append(ln+" "+nm)
    else: out.append(ln)
open(p,"w").write("\n".join(out)+"\n")
PY
echo "=== lifted ==="; grep -E "^[0-9]+ (input|state|output)" "$OUT/fifo4.btor2" | sed -E 's/ ;.*//'
