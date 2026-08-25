#!/usr/bin/env bash
# Validate all four OT-shaped modules:
#   Variant A (index independent of write addr) -> SPCR SOUNDLY ABSTAINS -> unknown
#   Variant B (latch the write addr, P-B.1)     -> SPCR DECIDES         -> holds
# Plus a differential ROBDD oracle for B-small (whole-array registerized).
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=mununu=info
BIN=/cargo-target/release/mununu
cd /work/scratchpad/array_numu_p1

for TOP in ot_cfgtable_recovery_small ot_cfgtable_recovery_large \
           ot_lastcfg_recovery_small ot_lastcfg_recovery_large; do
  echo ""
  echo "############ $TOP ############"
  "$BIN" sv verify-recoverability ot_cfgtable_recovery.sv --top "$TOP" \
      --frontend slang --target "stalled == 0" 2>/tmp/$TOP.err
  echo "--- SPCR / verdict trace ---"
  grep -iE "SPCR: eliminat|prophecy" /tmp/$TOP.err | head -3
done

echo ""
echo "############ DIFFERENTIAL ORACLE: B-small, whole-array registerized -> exact ROBDD ############"
cat > /tmp/oracle.ys <<'YS'
plugin -i slang
read_slang /work/scratchpad/array_numu_p1/ot_cfgtable_recovery.sv --top ot_lastcfg_recovery_small
prep -top ot_lastcfg_recovery_small
flatten
memory            ; # FULL registerization: array -> individual FFs (no $mem)
async2sync
opt
dffunmap
write_btor /work/scratchpad/array_numu_p1/ot_lastcfg_small.reg.btor2
YS
yosys -q /tmp/oracle.ys 2>/tmp/oracle.err || { echo "ORACLE YOSYS FAILED"; tail -15 /tmp/oracle.err; }
echo "-- registerized oracle: array present? (expect NONE) / states --"
grep -cE "sort array" ot_lastcfg_small.reg.btor2 2>/dev/null
grep -cE "^[0-9]+ state " ot_lastcfg_small.reg.btor2 2>/dev/null
echo "-- oracle verdict (exact ROBDD on the array-free registerized design) --"
"$BIN" btor2 verify-recoverability ot_lastcfg_small.reg.btor2 --target "stalled == 0" 2>/dev/null
echo "DONE"
