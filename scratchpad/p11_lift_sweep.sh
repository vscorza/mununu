#!/usr/bin/env bash
# P1.1 lift-track re-run: probe un-measured AssertLLM2 designs for LIFT (Auto frontend) + FSM regs +
# candidate completion outputs. Phase 1 = which lift; targets for verify-recoverability come next.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs

# Un-measured self-contained / completion-FSM candidates + the named EFAIL retry (apb_to_i2c).
BATCH="
DSP_CORE/ecg
OTHER/bubblesortmodule
OTHER/programmable_interval_timer
OTHER/pwm
OTHER/datetime
OTHER/computer_operating_properly
LIBRARY/real-time-clock
MEMORY_CORE/versatile_fifo
MEMORY_CORE/sd_fifo
CRYPTO_CORE/sha3
COMMUNICATION_CONTROLLER/apb_to_i2c
"

for d in $BATCH; do
  dir="$AL/$d"
  [ -d "$dir" ] || { echo "### $d — MISSING DIR"; continue; }
  echo ""
  echo "############################## $d ##############################"
  # Candidate completion / status outputs (top-level module ports).
  echo "-- candidate outputs --"
  grep -rhoiE "output +(reg +)?(\[[0-9:]+\] +)?[a-z_][a-z_0-9]*(valid|done|ready|busy|ack|empty|full|irq|rdy|state|count|cnt|q|out)" "$dir" --include='*.v' --include='*.sv' 2>/dev/null | grep -viE "mutation|assign" | sort -u | head -10
  # Lift + FSM probe (Auto frontend, design-dir auto-assembly).
  echo "-- check-fsm --frontend auto --design-dir --"
  timeout 180 "$BIN" sv check-fsm --frontend auto --design-dir "$dir" 2>&1 | grep -iE "lift|error|fsm|reg|state|violat|holds|top|unsupported|fail|scanned|illegal|no .* found" | head -14
done
echo ""
echo "SWEEP DONE"
