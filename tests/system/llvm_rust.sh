#!/usr/bin/env bash
# System test: LLVM IR extraction pipeline for Rust
# Tests: sample_protocol.rs → rustc --emit=llvm-ir → llvm_extract.py → .espec.json → mununu verify
#
# Prerequisites: rustc (standard Rust toolchain)
# Skips if rustc is not available.
#
# Usage: bash tests/system/llvm_rust.sh

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

# Check for rustc
if ! which rustc >/dev/null 2>&1; then
    echo -e "${YELLOW}SKIP${NC}: rustc not found"
    exit 0
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
echo "=== LLVM IR Rust Extraction Pipeline ==="
echo ""

# Test 1: Compile Rust to LLVM IR
echo -n "  Compile sample_protocol.rs → LLVM IR ... "
if rustc --edition 2021 --crate-type=lib --emit=llvm-ir \
    examples/ast_extract/rust/sample_protocol.rs -o "$TMPDIR/sample.ll" 2>/dev/null; then
    LINES=$(wc -l < "$TMPDIR/sample.ll")
    echo -e "${GREEN}PASS${NC} ($LINES lines)"
    PASS=$((PASS + 1))
else
    echo -e "${RED}FAIL${NC} (compilation failed)"
    FAIL=$((FAIL + 1))
fi

# Test 2: Extract from LLVM IR
echo -n "  LLVM IR → .espec.json via llvm_extract.py ... "
if python3 tools/llvm_extract.py "$TMPDIR/sample.ll" \
    --struct Connection --output "$TMPDIR/spec.espec.json" 2>/dev/null; then
    METHODS=$(python3 -c "import json; d=json.load(open('$TMPDIR/spec.espec.json')); print(len(d['model_config']['automata'][0]['transitions']))")
    echo -e "${GREEN}PASS${NC} ($METHODS transitions)"
    PASS=$((PASS + 1))
else
    echo -e "${RED}FAIL${NC} (extraction failed)"
    FAIL=$((FAIL + 1))
fi

# Test 3: Verify with mununu
echo -n "  .espec.json → mununu verify ... "
OUTPUT=$("$MUNUNU" context eval "$TMPDIR/spec.espec.json" \
    --formula safety --automaton ConnectionFSM 2>/dev/null || true)
SAT=$(echo "$OUTPUT" | grep "Initial states satisfying:" | sed 's/.*: //' | cut -d/ -f1)

if [ "$SAT" = "1" ]; then
    echo -e "${GREEN}PASS${NC} (safety holds)"
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
