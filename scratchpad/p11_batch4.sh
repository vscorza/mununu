#!/usr/bin/env bash
# P1.1 batch 4: reset-pin the datetime HOLDS (soundness — recovery via own logic, not reset-escape),
# and probe pwm's vacuity (does state leave 0 to the PWM-active value 2?).
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs

recov() { # $1=dir $2=top $3=target $4..=extra flags
  local dir="$1" top="$2" tgt="$3"; shift 3
  timeout 240 "$BIN" sv verify-recoverability --frontend auto --design-dir "$dir" --top "$top" --target "$tgt" "$@" 2>&1 | \
    grep -iE "verdict|property|error:|ERROR" | head -4
}

echo "############## datetime RESET-PINNED (rst_i=0 inactive) — sound operational recovery ##############"
echo "-- AG EF(hourL==0) [rst_i=0] --"; recov "$AL/OTHER/datetime" Datetime "hourL == 0" --config-value rst_i=0
echo "-- AG EF(hourL==1) [rst_i=0] --"; recov "$AL/OTHER/datetime" Datetime "hourL == 1" --config-value rst_i=0
echo "-- 2nd digit: AG EF(minL==0) + ==1 [rst_i=0] (if minL exists) --"
recov "$AL/OTHER/datetime" Datetime "minL == 0" --config-value rst_i=0
recov "$AL/OTHER/datetime" Datetime "minL == 1" --config-value rst_i=0

echo ""
echo "############## pwm vacuity probe (does state reach the PWM-active value 2?) ##############"
echo "-- AG EF(state==2) [free] --"; recov "$AL/OTHER/pwm" PWM "state == 2"
echo "-- AG EF(state==0) [reset-pinned i_wb_rst=0] --"; recov "$AL/OTHER/pwm" PWM "state == 0" --config-value i_wb_rst=0

echo ""
echo "BATCH4 DONE"
