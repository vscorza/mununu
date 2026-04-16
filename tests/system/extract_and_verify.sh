#!/usr/bin/env bash
# System test: extraction → verification pipeline
# Tests the full multi-binary workflow for TypeScript, Python, and Rust.
#
# Usage: bash tests/system/extract_and_verify.sh
# Requires: cargo (builds both binaries if needed)

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

# Build both binaries
echo "Building mununu-extract and mununu-cli..."
cargo build -p mununu-extract -p mununu-cli 2>/dev/null

EXTRACT="./target/debug/mununu-extract"
MUNUNU="./target/debug/mununu"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

PASS=0
FAIL=0

run_test() {
    local name="$1"
    local config="$2"
    local source="$3"
    local formula="$4"
    local automaton="$5"
    local expect_sat="$6"  # "0" for FAILS, "1" for HOLDS

    echo -n "  $name ... "

    # Extract
    if ! "$EXTRACT" "$config" --source "$source" --output "$TMPDIR/spec.espec.json" 2>/dev/null; then
        echo -e "${RED}FAIL${NC} (extraction failed)"
        FAIL=$((FAIL + 1))
        return
    fi

    # Verify
    OUTPUT=$("$MUNUNU" context eval "$TMPDIR/spec.espec.json" \
        --formula "$formula" --automaton "$automaton" 2>/dev/null || true)

    SAT=$(echo "$OUTPUT" | grep "Initial states satisfying:" | sed 's/.*: //' | cut -d/ -f1)

    if [ "$SAT" = "$expect_sat" ]; then
        echo -e "${GREEN}PASS${NC} (sat=$SAT)"
        PASS=$((PASS + 1))
    else
        echo -e "${RED}FAIL${NC} (expected sat=$expect_sat, got sat=$SAT)"
        FAIL=$((FAIL + 1))
    fi
}

echo ""
echo "=== Extraction → Verification Pipeline Tests ==="
echo ""

# TypeScript: sample_server.ts — property should FAIL (0)
run_test "TypeScript (sample_server)" \
    "examples/ast_extract/typescript/sample_server.extract.json" \
    "examples/ast_extract/typescript/sample_server.ts" \
    "no_requests_after_close" \
    "ServerLifecycle" \
    "0"

# Rust: sample_protocol.rs — extract and verify (property should FAIL)
run_test "Rust (sample_protocol)" \
    "examples/ast_extract/rust/sample_protocol.extract.json" \
    "examples/ast_extract/rust/sample_protocol.rs" \
    "no_send_after_close" \
    "ConnectionFSM" \
    "0"

# Python: sample_handler.py — extract and verify (property should FAIL)
run_test "Python (sample_handler)" \
    "examples/ast_extract/python/sample_handler.extract.json" \
    "examples/ast_extract/python/sample_handler.py" \
    "no_requests_when_rate_limited" \
    "HandlerFSM" \
    "0"

# TypeScript: compound_guard.ts — compound || in early-return (De Morgan → &&)
# Property should HOLD — doWork only fires from ready=T AND active=T
run_test "TypeScript (compound guard)" \
    "examples/ast_extract/typescript/compound_guard.extract.json" \
    "examples/ast_extract/typescript/compound_guard.ts" \
    "doWork_only_when_ready_and_active" \
    "WorkerFSM" \
    "1"

# SystemVerilog: handshake.sv via adapter (not extraction) — property should HOLD
echo -n "  SystemVerilog (handshake via adapter) ... "
OUTPUT=$("$MUNUNU" context eval examples/systemverilog/handshake.sv \
    --adapter systemverilog --formula safety --automaton handshake 2>/dev/null || true)
SAT=$(echo "$OUTPUT" | grep "Initial states satisfying:" | sed 's/.*: //' | cut -d/ -f1)
if [ "$SAT" = "1" ]; then
    echo -e "${GREEN}PASS${NC} (sat=$SAT)"
    PASS=$((PASS + 1))
else
    echo -e "${RED}FAIL${NC} (expected sat=1, got sat=$SAT)"
    FAIL=$((FAIL + 1))
fi

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
