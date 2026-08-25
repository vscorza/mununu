#!/usr/bin/env bash
# No-regression: designs --design-dir is KNOWN to lift (ledger §3.6/§3.8) must STILL lift
# after the source-manifest hardening.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs

probe() { timeout 200 "$BIN" sv check-fsm --frontend auto --design-dir "$1" 2>&1 | \
  grep -iE "fsm_registers_checked|top candidates|already staged|error:|ERROR|Compilation failed|no un-instantiated" | head -5; }

for d in COMMUNICATION_CONTROLLER/sdspi COMMUNICATION_CONTROLLER/uart \
         COMMUNICATION_CONTROLLER/i2c_slave COMMUNICATION_CONTROLLER/rtfsimpleuart; do
  echo ""; echo "### $d (known-lift §3.6) ###"; probe "$AL/$d"
done
echo ""
echo "NOREGRESS DONE"
