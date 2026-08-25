#!/usr/bin/env bash
# Capture mununu's OWN lifted BTOR2 for the OT case and compare its array + read-index
# shape against the working agr_small_mem.btor2.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=mununu=info
export MUNUNU_KEEP_YOSYS_TMP=1
BIN=/cargo-target/release/mununu
cd /work/scratchpad/array_numu_p1

echo "=== run OT small with KEEP_YOSYS_TMP to capture the lift ==="
"$BIN" sv verify-recoverability ot_cfgtable_recovery.sv \
    --top ot_cfgtable_recovery_small --frontend slang --target "stalled == 0" 2>/tmp/ot.err >/dev/null
echo "--- yosys tmp dir(s) mentioned ---"
grep -ioE "/tmp/[^ ]*yosys[^ ]*|/tmp/\.tmp[^ ]*|keeping[^\n]*" /tmp/ot.err | head
echo "--- search for any lifted .btor2 under /tmp ---"
find /tmp -name '*.btor2' -newermt '-3 minutes' 2>/dev/null | head
LIFT=$(find /tmp -name '*.btor2' -newermt '-3 minutes' 2>/dev/null | grep -iE "ot|preprocess|design|out" | head -1)
[ -z "$LIFT" ] && LIFT=$(find /tmp -name '*.btor2' -newermt '-3 minutes' 2>/dev/null | head -1)
echo "LIFT=$LIFT"

echo ""
echo "================= OT lift: array + read/write + index cone ================="
if [ -n "$LIFT" ]; then
  echo "--- sort array / read / write lines ---"
  grep -nE "sort array|[0-9]+ (read|write) " "$LIFT" | head
  echo "--- states (name if any) ---"
  grep -nE "^[0-9]+ state " "$LIFT" | head -20
  echo "--- lines mentioning cfg / active / chan / stalled ---"
  grep -inE "chan_cfg|active_chan|stalled" "$LIFT" | head -20
  echo "--- total lines ---"; wc -l "$LIFT"
  cp "$LIFT" /work/scratchpad/array_numu_p1/ot_small.mununu_lift.btor2
  echo "saved -> ot_small.mununu_lift.btor2"
fi

echo ""
echo "================= WORKING agr_small_mem.btor2 for comparison ================="
grep -nE "sort array|[0-9]+ (read|write) " agr_small_mem.btor2 | head
echo "--- agr states ---"; grep -nE "^[0-9]+ state " agr_small_mem.btor2 | head -20
echo "DONE"
