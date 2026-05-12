#!/usr/bin/env bash
# validate.sh — reproduce the Document A §9 industrial example end-to-end.
#
# Runs every mununu subcommand exercised by the secure boot ROM example
# against a pinned commit, captures their output, and produces a
# byte-deterministic transcript at `transcript.txt`. Per the
# CLAUDE.md claims-integrity rules, this transcript is the evidence
# the README cites — any drift between the transcript and what
# `validate.sh` produces today is a bug.
#
# Run from the repo root:
#   ./examples/industrial/secure_boot_rom/validate.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

EXAMPLE_DIR="examples/industrial/secure_boot_rom"
TRANSCRIPT="$EXAMPLE_DIR/transcript.txt"

# Build the binary once; suppress the build noise in the transcript.
echo "build: mununu binary (cargo)" 1>&2
cargo build --quiet --bin mununu

# Strip tracing's per-run noise so the transcript reproduces byte-for-byte:
#   - ANSI colour escape codes (\e[...m)
#   - leading ISO-8601 timestamp prefix that tracing puts on every line
#   - the "Logging initialized" INFO line, emitted once at startup
# We keep the structured WARN body so the contract diagnostics are visible.
strip_logs() {
    perl -pe '
        s/\e\[[0-9;]*m//g;
        s/^\s*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s+//;
    ' | grep -v 'Logging initialized'
}

# Run a command, label it in the transcript with a banner, and strip
# the per-run noise.
run() {
    local title="$1"; shift
    printf '\n=== %s ===\n' "$title"
    printf '$ %s\n' "$*"
    { "$@" 2>&1 || true; } | strip_logs
}

{
    printf '# secure boot ROM — validation transcript\n'
    printf '# regenerate via examples/industrial/secure_boot_rom/validate.sh\n'
    printf '# transcript is byte-deterministic; re-running validate.sh\n'
    printf '# against the same commit should produce identical output.\n'

    run "1. phase-1 discovery on SHA-256 IP (chaotic stub default)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/sha256_interface.json"

    run "2. phase-1 discovery on RSA-verify IP (with fairness gap)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/rsa_verify_interface.json" \
            --emit-fairness-gap

    run "3. discharge check — acyclic contract set" \
        ./target/debug/mununu contract validate \
            "$EXAMPLE_DIR/contract_set_acyclic.json"

    run "4. discharge check — circular contract set (no rank witness)" \
        ./target/debug/mununu contract validate \
            "$EXAMPLE_DIR/contract_set_circular.json"

    run "5. discharge check — circular contract set with mu-rank witness" \
        ./target/debug/mununu contract validate \
            "$EXAMPLE_DIR/contract_set_rank_witness.json"

    run "6. strict-mode gate (expected to exit non-zero)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/sha256_interface.json" \
            --strict-contracts

    run "7. safety property — no execution without verified signature" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/secure_boot.ctxdsl" \
            --formula safety_no_execution_without_signature \
            --automaton SecureBoot

    run "8. reachability — BootValid is reachable from Reset" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/secure_boot.ctxdsl" \
            --formula bootvalid_reachable \
            --automaton BootController

    run "9. reachability — Reset is always reachable (reboot smoke test)" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/secure_boot.ctxdsl" \
            --formula reset_always_reachable \
            --automaton BootController

    printf '\n=== end of transcript ===\n'
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
