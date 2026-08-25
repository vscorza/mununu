#!/usr/bin/env bash
# Re-measure the 16 config/sequence/data-dependent ⊥ recoverability targets with the CURRENT engine
# (rows are stale @2026-07-25, pre cap-raise #402 / source-manifest #412 / OOM-fix #413).
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=warn
cargo build --release -p mununu-cli 2>&1 | tail -2
BIN=/cargo-target/release/mununu
AL=/work/AssertLLM2/designs
U="$AL/COMMUNICATION_CONTROLLER/uart_to_spi"
I="$AL/COMMUNICATION_CONTROLLER/i2c_controller_core"

rec() { # $1=dir $2=top $3=atom
  local v
  v=$(timeout 150 "$BIN" sv verify-recoverability --design-dir "$1" --top "$2" --frontend auto --target "$3" 2>/tmp/e | grep -iE '"verdict"' | head -1)
  local rc=$?
  if [ -z "$v" ]; then
    v=$(grep -iE "error:|ERROR|does not bind|not a register|no signal|unknown" /tmp/e | head -1)
    [ $rc -eq 124 ] && v="(timeout 150s)"
    [ -z "$v" ] && v="(no verdict; rc=$rc)"
  fi
  printf "  %-28s -> %s\n" "$3" "$v"
}

echo "############ uart_to_spi (top=top) ############"
for a in "cs_n == 15" "sck == 0" "txd == 1" "so == 0" "cfg_dataout == 1" "cfg_dataout == 5" \
         "shift_enb == 1" "load_byte == 1" "cs_int_n == 0" "shift_in == 1"; do rec "$U" top "$a"; done

echo "############ i2c_controller_core (top=i2c_master_top) ############"
for a in "sda_padoen_o == 1" "sr == 170" "sr == 85" "core_txd == 1" "shift == 1" "core_cmd == 1"; do
  rec "$I" i2c_master_top "$a"; done

echo "REROLL DONE"
