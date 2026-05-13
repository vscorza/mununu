#!/usr/bin/env bash
# validate.sh — reproduce the Document C §C.4 industrial codesign example.
#
# Composes the UART_LITE register-map sidecar with a hand-authored
# firmware automaton, runs `mununu codesign verify` against three
# properties, and writes a byte-deterministic transcript at
# `transcript.txt`. Per CLAUDE.md claims-integrity rules this transcript
# is the evidence the README cites — any drift between transcript and
# what `validate.sh` produces today is a bug.
#
# Run from the repo root:
#   ./examples/industrial/codesign_uart/validate.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

EXAMPLE_DIR="examples/industrial/codesign_uart"
TRANSCRIPT="$EXAMPLE_DIR/transcript.txt"

echo "build: mununu binary (cargo)" 1>&2
cargo build --quiet --bin mununu

# Strip tracing's per-run noise so the transcript reproduces byte-for-byte:
strip_logs() {
    perl -pe '
        s/\e\[[0-9;]*m//g;
        s/^\s*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s+//;
    ' | grep -v 'Logging initialized'
}

run() {
    local title="$1"; shift
    printf '\n=== %s ===\n' "$title"
    printf '$ %s\n' "$*"
    { "$@" 2>&1 || true; } | strip_logs
}

{
    printf '# UART codesign — validation transcript\n'
    printf '# regenerate via examples/industrial/codesign_uart/validate.sh\n'
    printf '# transcript is byte-deterministic; re-running validate.sh\n'
    printf '# against the same commit should produce identical output.\n'

    run "1. emit the coupling fragment from the register-map sidecar" \
        ./target/debug/mununu codesign couple \
            "$EXAMPLE_DIR/register_map.json" \
            --firmware-member UartDriver

    run "2. codesign verify — init_reachable (smoke test)" \
        ./target/debug/mununu codesign verify \
            "$EXAMPLE_DIR/register_map.json" \
            "$EXAMPLE_DIR/firmware.ctxdsl" \
            --formula init_reachable

    run "3. codesign verify — safety_protocol_respected (over composed system)" \
        ./target/debug/mununu codesign verify \
            "$EXAMPLE_DIR/register_map.json" \
            "$EXAMPLE_DIR/firmware.ctxdsl" \
            --formula safety_protocol_respected

    run "4. codesign verify — sending_reachable (expected: VIOLATED under chaotic peripheral)" \
        ./target/debug/mununu codesign verify \
            "$EXAMPLE_DIR/register_map.json" \
            "$EXAMPLE_DIR/firmware.ctxdsl" \
            --formula sending_reachable

    printf '\n=== end of transcript ===\n'
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
