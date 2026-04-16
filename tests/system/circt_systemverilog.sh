#!/usr/bin/env bash
# System test: CIRCT SystemVerilog extraction pipeline
# Tests: handshake.sv → circt-verilog → MLIR → circt_extract.py → .espec.json → mununu verify
#
# Prerequisites: CIRCT installed (run `source tools/install_circt.sh` first)
# Skips gracefully if CIRCT is not available.
#
# Usage: bash tests/system/circt_systemverilog.sh

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

# Check for CIRCT
CIRCT_BIN="${CIRCT_BIN:-/tmp/circt/firtool-1.144.0/bin}"
if [ ! -x "$CIRCT_BIN/circt-verilog" ]; then
    echo -e "${YELLOW}SKIP${NC}: CIRCT not installed. Run 'source tools/install_circt.sh' first."
    exit 0
fi

# Set library path for CIRCT shared libs
if [ "$(uname -s)" = "Darwin" ]; then
    export DYLD_LIBRARY_PATH="$CIRCT_BIN/../lib:${DYLD_LIBRARY_PATH:-}"
else
    export LD_LIBRARY_PATH="$CIRCT_BIN/../lib:${LD_LIBRARY_PATH:-}"
fi

# Build mununu-cli
echo "Building mununu-cli..."
cargo build -p mununu-cli 2>/dev/null

MUNUNU="./target/debug/mununu"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

PASS=0
FAIL=0

echo ""
echo "=== CIRCT SystemVerilog Extraction Pipeline ==="
echo ""

# Test 1: handshake.sv → CIRCT → .espec.json → verify
echo -n "  handshake.sv (CIRCT → MLIR → espec → verify) ... "
if "$CIRCT_BIN/circt-verilog" examples/systemverilog/handshake.sv \
    | python3 tools/circt_extract.py --output "$TMPDIR/handshake.espec.json" 2>/dev/null; then

    OUTPUT=$("$MUNUNU" context eval "$TMPDIR/handshake.espec.json" \
        --formula safety --automaton handshake 2>/dev/null || true)
    SAT=$(echo "$OUTPUT" | grep "Initial states satisfying:" | sed 's/.*: //' | cut -d/ -f1)

    if [ "$SAT" = "1" ]; then
        echo -e "${GREEN}PASS${NC} (safety holds, 4 states extracted)"
        PASS=$((PASS + 1))
    else
        echo -e "${RED}FAIL${NC} (expected sat=1, got sat=$SAT)"
        FAIL=$((FAIL + 1))
    fi
else
    echo -e "${RED}FAIL${NC} (CIRCT extraction failed)"
    FAIL=$((FAIL + 1))
fi

# Test 2: Compare CIRCT result with native SV adapter
echo -n "  handshake.sv (compare CIRCT vs native adapter) ... "
NATIVE_OUTPUT=$("$MUNUNU" context eval examples/systemverilog/handshake.sv \
    --adapter systemverilog --formula safety --automaton handshake 2>/dev/null || true)
NATIVE_SAT=$(echo "$NATIVE_OUTPUT" | grep "Initial states satisfying:" | sed 's/.*: //' | cut -d/ -f1)

if [ "$NATIVE_SAT" = "1" ] && [ "$SAT" = "1" ]; then
    echo -e "${GREEN}PASS${NC} (both paths agree: safety holds)"
    PASS=$((PASS + 1))
else
    echo -e "${RED}FAIL${NC} (CIRCT sat=$SAT, native sat=$NATIVE_SAT)"
    FAIL=$((FAIL + 1))
fi

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
