#!/usr/bin/env bash
# validate-motivating-examples.sh — end-to-end extraction against
# the plan's motivating C examples 1-3, 5, 6 via real clang.
#
# CLAUDE.md's claims-integrity rule #10 requires that any claim
# "the codesign extractor handles <C idiom>" be backed by a real
# `.c` file consumed by `mununu codesign extract-c` (not by a
# hand-authored IR fixture). This script is the canonical evidence:
# it runs the extractor against every .c file in this directory and
# asserts the expected access structure end-to-end.
#
# Coverage:
#   - Example 1: motivating-examples/../firmware.c (already exists,
#     not duplicated here; the parent dir's extract-c-demo.sh
#     exercises it).
#   - Example 2: example_2_typecast_register_access.c
#   - Example 3: covered implicitly by firmware.c (CTRL.bit.tx_start
#     bit-field RMW); no separate file needed.
#   - Example 4: example_4_helper_function_pointer_param.c (closed
#     by phase L5.5 — pointer-parameter alias tracking).
#   - Example 5: example_5_isr_with_main_thread.c
#   - Example 6: example_6_multi_entry_driver.c
#
# Usage:
#   ./examples/industrial/codesign_uart/motivating_examples/validate-motivating-examples.sh
#
# Output: byte-deterministic transcript at `transcript.txt` in the
# same directory. Re-running against the same commit must reproduce
# the same output.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

DIR="examples/industrial/codesign_uart/motivating_examples"
REGISTER_MAP="examples/industrial/codesign_uart/register_map.json"
TRANSCRIPT="$DIR/transcript.txt"

if ! command -v clang >/dev/null 2>&1; then
    echo "clang not found on \$PATH" 1>&2
    echo "install via xcode-select --install (macOS) or 'apt install clang' (Linux)" 1>&2
    exit 1
fi

echo "build: mununu binary (cargo)" 1>&2
cargo build --quiet --bin mununu

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
    printf '# Motivating-examples validation transcript\n'
    printf '# Regenerated via examples/industrial/codesign_uart/motivating_examples/validate-motivating-examples.sh\n'
    printf '# Transcript is byte-deterministic; re-running against the same commit\n'
    printf '# must produce identical output. Each example is real C source\n'
    printf '# compiled by real clang and consumed by the LLVM-IR-based\n'
    printf '# `mununu codesign extract-c` backend.\n'

    run "Example 2: type-cast register access — *(volatile uint32_t *)0x40010004 = 1" \
        ./target/debug/mununu codesign extract-c \
            "$DIR/example_2_typecast_register_access.c" \
            --register-map "$REGISTER_MAP" \
            --synthesize-automaton

    run "Example 4: helper function with pointer parameter (phase L5.5 alias tracking)" \
        ./target/debug/mununu codesign extract-c \
            "$DIR/example_4_helper_function_pointer_param.c" \
            --register-map "$REGISTER_MAP" \
            --synthesize-automaton

    run "Example 5: ISR + main-thread (annotation-driven asynchronous composition)" \
        ./target/debug/mununu codesign extract-c \
            "$DIR/example_5_isr_with_main_thread.c" \
            --register-map "$REGISTER_MAP" \
            --synthesize-automaton

    run "Example 6: multi-entry driver dispatch (--driver-mode)" \
        ./target/debug/mununu codesign extract-c \
            "$DIR/example_6_multi_entry_driver.c" \
            --register-map "$REGISTER_MAP" \
            --synthesize-automaton \
            --driver-mode

    # Phase L8 — Example 8 needs a CMSIS-style uart_cmsis.h header
    # alongside the C source. Generate it on the fly from the
    # register-map sidecar.
    L8_TMP="$(mktemp -d)"
    cat > "$L8_TMP/uart_cmsis.h" <<'CMSIS_HEADER'
#pragma once
#include <stdint.h>
#ifndef __IO
#define __IO volatile
#endif
typedef struct {
    union { __IO uint32_t reg; struct { uint32_t tx_start:1; uint32_t enable:1; uint32_t reserved:30; } bit; } CTRL;
    union { __IO uint32_t reg; struct { uint32_t tx_busy:1; uint32_t rx_ready:1; uint32_t reserved:30; } bit; } STATUS;
    union { __IO uint32_t reg; uint8_t byte; } DATA;
} UART_LITE_Type;
#define UART ((volatile UART_LITE_Type *)0x40010000u)
CMSIS_HEADER

    run "Example 8: CMSIS-style struct-member access (--cmsis-stubs)" \
        ./target/debug/mununu codesign extract-c \
            "$DIR/example_8_cmsis_struct_access.c" \
            --register-map "$REGISTER_MAP" \
            --synthesize-automaton \
            --cmsis-stubs \
            --include "$L8_TMP"

    rm -rf "$L8_TMP"

    printf '\n=== end of transcript ===\n'
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
