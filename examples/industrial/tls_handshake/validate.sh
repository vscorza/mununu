#!/usr/bin/env bash
# validate.sh — reproduce the Document D §D.9 industrial example end-to-end.
#
# Runs every mununu subcommand exercised by the TLS handshake example
# against a pinned commit, captures their output, and produces a
# byte-deterministic transcript at `transcript.txt`. Per the
# CLAUDE.md claims-integrity rules, this transcript is the evidence
# the README cites — any drift between the transcript and what
# `validate.sh` produces today is a bug.
#
# Run from the repo root:
#   ./examples/industrial/tls_handshake/validate.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

EXAMPLE_DIR="examples/industrial/tls_handshake"
CORPUS_DIR="corpus"
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
    printf '# TLS handshake — validation transcript\n'
    printf '# regenerate via examples/industrial/tls_handshake/validate.sh\n'
    printf '# transcript is byte-deterministic; re-running validate.sh\n'
    printf '# against the same commit should produce identical output.\n'

    run "1. corpus query — AES-CTR contract exists and resolves" \
        ./target/debug/mununu contract query \
            rtl_crypto/aes_ctr \
            --corpus "$CORPUS_DIR"

    run "2. phase-2 discovery — AES_CTR_v1 (annotation + corpus hit)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/aes_ctr_interface.json" \
            --corpus "$CORPUS_DIR"

    run "3. phase-2 discovery — TRNG_V2 (annotation + corpus miss)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/rng_interface.json" \
            --corpus "$CORPUS_DIR"

    run "4. phase-2 discovery — AES_CTR_v1 without --corpus (NoCorpus)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/aes_ctr_interface.json"

    run "5. strict-mode gate — AES_CTR_v1 (exits non-zero on residual gap)" \
        ./target/debug/mununu contract discover \
            "$EXAMPLE_DIR/aes_ctr_interface.json" \
            --corpus "$CORPUS_DIR" \
            --strict-contracts

    run "6. safety property — every reachable composed state respects the protocol" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/tls_handshake.ctxdsl" \
            --formula safety_protocol_respected \
            --automaton TLSSession

    run "7. reachability — Application reachable from Idle" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/tls_handshake.ctxdsl" \
            --formula application_reachable \
            --automaton TLSHandshake

    run "8. reachability — Idle reachable from any state (teardown smoke test)" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/tls_handshake.ctxdsl" \
            --formula idle_always_reachable \
            --automaton TLSHandshake

    printf '\n=== end of transcript ===\n'
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
