#!/usr/bin/env bash
# P1.1 batch 3: explicit --top overrides the mis-tokenizing design-dir auto-detect. Lift + both
# recoverability directions (non-vacuity = BOTH AG EF(t==1) and AG EF(t==0) HOLD → t genuinely toggles).
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs

recov() { # $1=dir $2=top $3=target
  timeout 240 "$BIN" sv verify-recoverability --frontend auto --design-dir "$1" --top "$2" --target "$3" 2>&1 | \
    grep -iE "verdict|property|error:|ERROR|unsupported|Compilation failed|no top" | head -6
}

echo "############## ecg / point_add (elliptic-curve; done) ##############"
echo "-- AG EF(done==1) --"; recov "$AL/DSP_CORE/ecg" point_add "done == 1"
echo "-- AG EF(done==0) --"; recov "$AL/DSP_CORE/ecg" point_add "done == 0"

echo ""
echo "############## bubblesort / bublesort (done_o) ##############"
echo "-- AG EF(done_o==1) --"; recov "$AL/OTHER/bubblesortmodule" bublesort "done_o == 1"
echo "-- AG EF(done_o==0) --"; recov "$AL/OTHER/bubblesortmodule" bublesort "done_o == 0"

echo ""
echo "############## pwm / PWM (state FSM) ##############"
echo "-- AG EF(state==0) --"; recov "$AL/OTHER/pwm" PWM "state == 0"
echo "-- AG EF(state==1) --"; recov "$AL/OTHER/pwm" PWM "state == 1"

echo ""
echo "############## datetime / Datetime (BCD digit hourL — cyclic) ##############"
echo "-- AG EF(hourL==0) --"; recov "$AL/OTHER/datetime" Datetime "hourL == 0"
echo "-- AG EF(hourL==1) --"; recov "$AL/OTHER/datetime" Datetime "hourL == 1"

echo ""
echo "BATCH3 DONE"
