#!/usr/bin/env bash
# Slice 3 industrial: A1 (uart_to_spi cfg_dataout — the wide-config case) + a §D actionable-⊥ sample.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
cargo build --release -p mununu-cli 2>&1 | tail -2
BIN=/cargo-target/release/mununu
U="/work/AssertLLM2/designs/COMMUNICATION_CONTROLLER/uart_to_spi"

echo "=== inspect uart config inputs (candidate config atoms to partition over) ==="
"$BIN" sv emit-btor2-per-module "$U" 2>/dev/null | head -1 || true
# Lift + look for narrow config-like inputs
timeout 120 "$BIN" sv verify-recoverability --design-dir "$U" --top top --frontend auto \
  --target "cfg_dataout == 1" --refine 2>/dev/null | grep -iE "verdict|vacuous|wide_influences|uncertified|bot_diagnosis|config_partition|note" | head -20
echo "--- A1: cfg_dataout==1 with --refine (expect bot_diagnosis wide_influences; config-independent unless a config input named) ---"

echo ""
echo "=== §D sample: --refine on 3 still-⊥ uart targets (do they carry a refinement?) ==="
for t in "shift_enb == 1" "cs_int_n == 0" "cfg_dataout == 5"; do
  echo "-- $t --"
  timeout 150 "$BIN" sv verify-recoverability --design-dir "$U" --top top --frontend auto \
    --target "$t" --refine 2>/dev/null | grep -iE "verdict|vacuous|wide_influences|uncertified|config_partition" | head -8
done
echo "SLICE3 A1 DONE"
