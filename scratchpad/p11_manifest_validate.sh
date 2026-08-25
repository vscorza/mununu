#!/usr/bin/env bash
# Validate the source-manifest hardening on the REAL designs that failed the §3.10 sweep:
# top mis-tokenization (apb_to_i2c/PIT/ecg/bubblesort) + .sv/.v dedup (cop/versatile_fifo/sd_fifo).
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
AL=/work/AssertLLM2/designs

echo "=== rebuild release mununu-cli (with the source-manifest fix) ==="
cargo build --release -p mununu-cli 2>&1 | tail -3
BIN=/cargo-target/release/mununu

probe() { # $1=dir
  timeout 200 "$BIN" sv check-fsm --frontend auto --design-dir "$1" 2>&1 | \
    grep -iE "fsm_registers_checked|top candidates|\.sv/\.v twin|already staged|error:|ERROR|Compilation failed|no top|no un-instantiated" | head -8
}

for d in OTHER/computer_operating_properly MEMORY_CORE/versatile_fifo MEMORY_CORE/sd_fifo \
         COMMUNICATION_CONTROLLER/apb_to_i2c OTHER/programmable_interval_timer \
         DSP_CORE/ecg OTHER/bubblesortmodule; do
  echo ""
  echo "############## $d ##############"
  probe "$AL/$d"
done
echo ""
echo "MANIFEST VALIDATE DONE"
