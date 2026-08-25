#!/usr/bin/env bash
# P1.1 batch 2: the 5 truncated designs + verify-recoverability on the completion-output ones.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs

lift() { # $1=dir
  timeout 200 "$BIN" sv check-fsm --frontend auto --design-dir "$1" 2>&1 | \
    grep -iE "fsm_registers_checked|\"register\"|top candidates|error:|ERROR|unsupported|Compilation failed|no top" | head -8
}
recov() { # $1=dir $2=target
  timeout 240 "$BIN" sv verify-recoverability --frontend auto --design-dir "$1" --target "$2" 2>/dev/null | \
    grep -iE "verdict|property"
}

echo "############## DSP_CORE/ecg (elliptic-curve point_add; output done) ##############"
lift "$AL/DSP_CORE/ecg"
echo "-- AG EF(done==1) [can always complete] --"; recov "$AL/DSP_CORE/ecg" "done == 1"
echo "-- AG EF(done==0) [always returns to not-done; non-vacuity dual] --"; recov "$AL/DSP_CORE/ecg" "done == 0"

echo ""
echo "############## OTHER/bubblesortmodule (bublesort.v; output done_o) ##############"
lift "$AL/OTHER/bubblesortmodule"
echo "-- AG EF(done_o==1) --"; recov "$AL/OTHER/bubblesortmodule" "done_o == 1"
echo "-- AG EF(done_o==0) --"; recov "$AL/OTHER/bubblesortmodule" "done_o == 0"

echo ""
echo "############## OTHER/programmable_interval_timer ##############"
lift "$AL/OTHER/programmable_interval_timer"
echo ""
echo "############## OTHER/pwm ##############"
lift "$AL/OTHER/pwm"
echo ""
echo "############## OTHER/datetime ##############"
lift "$AL/OTHER/datetime"
echo ""
echo "BATCH2 DONE"
