#!/usr/bin/env bash
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
cd /work/scratchpad/array_numu_p1

cat > /tmp/lift_ot2.ys <<'YS'
plugin -i slang
read_slang /work/scratchpad/array_numu_p1/ot_cfgtable_recovery.sv --top ot_cfgtable_recovery_small
prep -top ot_cfgtable_recovery_small
flatten
memory -nomap
async2sync

opt
dffunmap
write_btor /work/scratchpad/array_numu_p1/ot_small.keepmem.btor2
YS
yosys -q /tmp/lift_ot2.ys 2>/tmp/yo.err || { echo "YOSYS FAILED"; tail -25 /tmp/yo.err; exit 1; }

echo "=== OT small keep-mem btor2 ==="
echo "--- array / read / write ---"
grep -nE "sort array|[0-9]+ (read|write) " ot_small.keepmem.btor2 | head
echo "--- states ---"
grep -nE "^[0-9]+ state " ot_small.keepmem.btor2 | head -20
echo "--- next lines (to see index register frames) ---"
grep -nE "^[0-9]+ next " ot_small.keepmem.btor2 | head -20
echo "--- lines referencing chan_cfg/active/stalled ---"
grep -inE "chan_cfg|active|stalled" ot_small.keepmem.btor2 | head
echo "--- total lines ---"; wc -l ot_small.keepmem.btor2
echo "DONE"
