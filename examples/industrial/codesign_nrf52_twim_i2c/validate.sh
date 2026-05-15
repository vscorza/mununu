#!/usr/bin/env bash
# validate.sh — reproduces the Nordic nRF52840 TWIM (I2C) codesign flagship
# end-to-end against the mununu release binary.
#
# This is the evidence-of-record for the README's verification claims.
# Per CLAUDE.md § Claims Integrity Rule 1 (models from source) and Rule 2
# (planted bugs are demos, not findings), the transcript this script
# produces is the byte-deterministic record of what the binary actually
# emits — not a fabricated or human-tweaked summary. Drift between this
# transcript and what `validate.sh` produces today is a bug.
#
# Run from this directory:
#   ./validate.sh
#
# Or from the repo root:
#   ./examples/industrial/codesign_nrf52_twim_i2c/validate.sh

set -euo pipefail
cd "$(dirname "$0")"

# Resolve mununu binary. We prefer the release binary because verify
# performance matters for the transcript's wall-clock claims; fall back
# to debug if release is not built.
MUNUNU="$(cd ../../.. && pwd)/target/release/mununu"
if [[ ! -x "$MUNUNU" ]]; then
    MUNUNU="$(cd ../../.. && pwd)/target/debug/mununu"
fi
if [[ ! -x "$MUNUNU" ]]; then
    echo "FATAL: no mununu binary found. Run 'cargo build --release -p mununu-cli' from the repo root." 1>&2
    exit 1
fi

# Strip per-run noise so the transcript reproduces byte-for-byte across runs.
strip_logs() {
    perl -pe '
        s/\e\[[0-9;]*m//g;
        s/^\s*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s+//;
    ' | grep -v 'Logging initialized'
}

run() {
    local title="$1"; shift
    local cmd_path="$1"; shift
    # Display only the basename of the mununu binary so the transcript
    # is byte-deterministic across machines (no absolute paths leaked).
    local cmd_display
    cmd_display="$(basename "$cmd_path")"
    printf '\n=== %s ===\n' "$title"
    printf '$ %s %s\n' "$cmd_display" "$*"
    { "$cmd_path" "$@" 2>&1 || true; } | strip_logs
}

# ---------------------------------------------------------------------------
# clean firmware properties: every one is expected to HOLD.
# ---------------------------------------------------------------------------
CLEAN_PROPERTIES=(
    init_reachable
    safety_protocol_respected
    twim_enable_before_tasks
    data_pointer_set_before_tasks
    no_data_after_error
    stop_after_last_byte_with_shorts
    error_event_cleared_before_retry
    shorts_only_between_transactions
    frequency_only_when_disabled
    suspend_resume_ordering
)

# ---------------------------------------------------------------------------
# buggy firmware properties: at least `data_pointer_set_before_tasks` MUST
# fail. The full safety property (`safety_protocol_respected`, which is the
# nu X. ([] X) over the composed system) still holds because the bug shows
# up as an additional admissible edge, not as a structurally malformed
# automaton — that asymmetry is the whole point of the codesign verifier.
# ---------------------------------------------------------------------------
BUGGY_PROPERTIES=(
    data_pointer_set_before_tasks
    safety_protocol_respected
)

{
    printf '# Nordic nRF52840 TWIM (I2C) codesign flagship — validation transcript\n'
    printf '# regenerate via examples/industrial/codesign_nrf52_twim_i2c/validate.sh\n'
    printf '# transcript is byte-deterministic; re-running validate.sh against the\n'
    printf '# same commit should produce identical output.\n'
    printf '#\n'
    printf '# Upstream: nrfx @ 0883a272c34004697dd56dfa44f6e2d0f8705689 (BSD-3-Clause)\n'

    run "1. emit the coupling fragment from the register-map sidecar" \
        "$MUNUNU" codesign couple \
            register_map.json \
            --firmware-member TwimDriver

    # firmware.ctxdsl and firmware_buggy.ctxdsl are auto-extracted
    # from firmware.c / firmware_buggy.c via the LLVM-IR backend
    # (regenerate via regenerate-ctxdsl.sh). They contain
    # synthesised automata only — no mu_formulas. The
    # protocol-conformance properties below verify against the
    # *hand-authored* variants at firmware.hand_authored.ctxdsl /
    # firmware_buggy.hand_authored.ctxdsl, which carry the formulas
    # written against the hand-authored state names (Reset, Idle,
    # Polling, …). See README.md "CTXDSL provenance" for the
    # split's rationale.

    for FORMULA in "${CLEAN_PROPERTIES[@]}"; do
        run "clean firmware: codesign verify — $FORMULA" \
            "$MUNUNU" codesign verify \
                register_map.json \
                firmware.hand_authored.ctxdsl \
                --formula "$FORMULA"
    done

    for FORMULA in "${BUGGY_PROPERTIES[@]}"; do
        run "buggy firmware: codesign verify — $FORMULA" \
            "$MUNUNU" codesign verify \
                register_map.json \
                firmware_buggy.hand_authored.ctxdsl \
                --formula "$FORMULA"
    done

    run "buggy firmware: codesign verify — data_pointer_set_before_tasks (JSON)" \
        "$MUNUNU" codesign verify \
            register_map.json \
            firmware_buggy.hand_authored.ctxdsl \
            --formula data_pointer_set_before_tasks \
            --json

    printf '\n=== end of transcript ===\n'
} > transcript.txt

printf 'wrote transcript to %s (%d lines)\n' \
    "transcript.txt" "$(wc -l < transcript.txt | tr -d ' ')"
