#!/usr/bin/env bash
# P1.1 batch 5: confirm datetime's reset AUTO-GATING (RUST_LOG=info shows detect_resets pinning
# rst_i) so the hourL both-directions HOLDS is the sound OPERATIONAL verdict, not reset-escape.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=mununu=info
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs

echo "############## datetime hourL==0 — WITH info log (reset auto-gate evidence) ##############"
timeout 240 "$BIN" sv verify-recoverability --frontend auto --design-dir "$AL/OTHER/datetime" \
    --top Datetime --target "hourL == 0" 2>&1 | grep -iE "reset|rst|gate|pin|detect|config|verdict|property|holds|violat" | head -20

echo ""
echo "############## datetime internal digit regs (what else can we target?) ##############"
grep -rhoE "reg +(\[[0-9:]+\] +)?[a-z_][a-z_0-9]*(N[0-9]|H|L|sec|min)" "$AL/OTHER/datetime/Datetime.v" 2>/dev/null | sort -u | head -20

echo ""
echo "BATCH5 DONE"
