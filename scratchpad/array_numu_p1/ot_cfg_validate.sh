#!/usr/bin/env bash
# OT-shaped SPCR case: build release mununu (with the committed soundness fix) and
# run sv verify-recoverability on the small (ROBDD-oracle) + large (array-defeats-
# registerization) instances. Expect: both HOLDS, decided VIA SPCR (array dropped).
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=mununu=info

cd /work
echo "=== build release mununu-cli ==="
cargo build --release -p mununu-cli 2>&1 | tail -3
BIN=/cargo-target/release/mununu
echo "bin:"; ls -la "$BIN"

cd /work/scratchpad/array_numu_p1
for TOP in ot_cfgtable_recovery_small ot_cfgtable_recovery_large; do
  echo ""
  echo "############ $TOP — sv verify-recoverability AG EF(stalled==0) ############"
  "$BIN" sv verify-recoverability ot_cfgtable_recovery.sv --top "$TOP" \
      --frontend slang --target "stalled == 0" 2>/tmp/$TOP.err
  echo "--- SPCR / engine / verdict trace ---"
  grep -iE "SPCR|eliminated|prophecy|holds|violat|unknown|exact|cube|abstain|skip|keep|\\\$mem" /tmp/$TOP.err | head -30
done
echo ""
echo "ALL DONE"
