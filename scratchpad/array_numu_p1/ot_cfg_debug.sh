#!/usr/bin/env bash
# Diagnose why the OT-shaped case gave `unknown` with no SPCR log.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=mununu=debug
BIN=/cargo-target/release/mununu
cd /work/scratchpad/array_numu_p1

echo "############ BASELINE: agr small (known-good SPCR case) with CURRENT binary ############"
RUST_LOG=mununu=info "$BIN" sv verify-recoverability array_gates_recovery.sv \
    --top array_gates_recovery_small --frontend slang --target "busy == 0" 2>/tmp/agr.err
echo "--- agr trace ---"; grep -iE "SPCR|eliminated|prophecy|holds|violat|unknown|exact|skip|abstain" /tmp/agr.err | head -15

echo ""
echo "############ Manual yosys lift of OT small: does it KEEP an array? ############"
cat > /tmp/lift_ot.ys <<'YS'
plugin -i slang
read_slang /work/scratchpad/array_numu_p1/ot_cfgtable_recovery.sv --top ot_cfgtable_recovery_small
prep -top ot_cfgtable_recovery_small
flatten
memory -nomap
opt
write_btor /work/scratchpad/array_numu_p1/ot_small.keepmem.btor2
YS
yosys -q /tmp/lift_ot.ys 2>/tmp/yosys_ot.err || { echo "YOSYS FAILED"; tail -20 /tmp/yosys_ot.err; }
echo "--- array/mem nodes in the lifted btor2 ---"
grep -nE "sort array|[0-9]+ (read|write) " /work/scratchpad/array_numu_p1/ot_small.keepmem.btor2 | head
echo "--- total lines / states ---"
wc -l /work/scratchpad/array_numu_p1/ot_small.keepmem.btor2 2>/dev/null
grep -cE "^[0-9]+ state " /work/scratchpad/array_numu_p1/ot_small.keepmem.btor2 2>/dev/null

echo ""
echo "############ OT small: FULL debug trace via sv verify-recoverability ############"
"$BIN" sv verify-recoverability ot_cfgtable_recovery.sv \
    --top ot_cfgtable_recovery_small --frontend slang --target "stalled == 0" 2>/tmp/ot.err
echo "--- OT engine trace (exact / escalate / spcr / cube) ---"
grep -iE "SPCR|eliminat|prophecy|exact|escalat|scalable|cube|array|\\\$mem|holds|violat|unknown|skip|abstain|Err" /tmp/ot.err | head -40

echo ""
echo "############ OT small: direct btor2 verify-recoverability on the manual keep-mem lift ############"
RUST_LOG=mununu=info "$BIN" btor2 verify-recoverability ot_small.keepmem.btor2 --target "stalled == 0" 2>/tmp/ot_b.err
echo "--- direct btor2 trace ---"; grep -iE "SPCR|eliminat|prophecy|exact|cube|holds|violat|unknown|skip|abstain" /tmp/ot_b.err | head -20
echo "DONE"
