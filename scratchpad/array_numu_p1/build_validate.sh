#!/usr/bin/env bash
# P1-a shot-①b validation: rebuild the release binary (with the select_guard_atoms
# reset-mux/uext see-through) and run btor2 verify-recoverability directly on the
# lifted keep-$mem BTOR2. Small = oracle HOLDS (expect ⊥ -> HOLDS now); large = scale.
set -uo pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=mununu=info

echo "=== build release mununu ==="
cargo build --release -p mununu-cli 2>&1 | tail -4
BIN=/cargo-target/release/mununu
echo "=== build exit / bin present ==="; ls -la "$BIN"

cd /work/scratchpad/array_numu_p1

echo ""
echo "### SMALL keep-\$mem — cross-check vs ROBDD oracle (HOLDS); was cube ⊥ ###"
echo "-- AG EF(busy==0) [EXPECT: violated->holds if Select decides]:"
"$BIN" btor2 verify-recoverability agr_small_mem.btor2 --target "busy == 0" 2>/tmp/small.err
echo "--- stderr (seed/discovery trace) ---"; grep -iE "sel_|select|compound|seed|predicate|holds|violat|unknown|bot|coi" /tmp/small.err | head -30
echo "DONE"
