#!/usr/bin/env bash
# run.sh — end-to-end extract+verify recipe.
#
# 1. Build mununu.
# 2. Run `mununu codesign extract-c` against firmware.c to produce the
#    synthesised CTXDSL automaton (JSON output).
# 3. Use `jq` to pull the `automaton_ctxdsl` fragment out of the JSON.
# 4. Splice it into `verify.template.ctxdsl` at the `{{AUTOMATON_CTXDSL}}`
#    placeholder, producing `verify.ctxdsl`.
# 5. Run `mununu context eval` on the spliced file for each formula.
#
# Output: byte-deterministic transcript at `transcript.txt`. The split
# between auto-extracted firmware automaton and hand-authored peripheral
# spec is documented in the template; this script is the connecting
# tissue.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

DIR="examples/industrial/codesign_uart/extract_verify_recipe"
REGISTER_MAP="examples/industrial/codesign_uart/register_map.json"
FIRMWARE="$DIR/firmware.c"
TEMPLATE="$DIR/verify.template.ctxdsl"
OUTPUT_CTXDSL="$DIR/verify.ctxdsl"
TRANSCRIPT="$DIR/transcript.txt"

if ! command -v clang >/dev/null 2>&1; then
    echo "clang not found on \$PATH" 1>&2
    exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "jq not found on \$PATH (install via 'brew install jq' on macOS)" 1>&2
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
    printf '# extract_verify_recipe — end-to-end transcript\n'
    printf '# Regenerated via examples/industrial/codesign_uart/extract_verify_recipe/run.sh\n'
    printf '# Demonstrates the full mununu codesign workflow:\n'
    printf '#   firmware.c -> mununu codesign extract-c -> splice -> mununu context eval\n'

    printf '\n=== Step 1: extract C firmware to synthesised CTXDSL automaton ===\n'
    EXTRACT_JSON_PATH="$DIR/extraction.json"
    ./target/debug/mununu codesign extract-c \
        "$FIRMWARE" \
        --register-map "$REGISTER_MAP" \
        --synthesize-automaton \
        --cmsis-stubs > "$EXTRACT_JSON_PATH" 2>/dev/null
    EXTRACT_JSON="$(cat "$EXTRACT_JSON_PATH")"
    printf 'extracted %d functions; the synthesised automaton fragment for the entry point is:\n\n' \
        "$(printf '%s' "$EXTRACT_JSON" | jq '.functions | length')"
    AUTOMATON="$(printf '%s' "$EXTRACT_JSON" | jq -r '.functions[0].automaton_ctxdsl')"
    printf '%s\n' "$AUTOMATON"

    printf '\n=== Step 1.5: derive firmware-side label list + reconcile against register map ===\n'
    # The reconcile-labels CLI consumes a plain `["label_1", ...]` array,
    # so we project the extraction's per-access (kind, register, field)
    # tuples into rendezvous labels using the same lowercase-and-ASCII-
    # only sanitisation `coupling::rendezvous_label_name` applies.
    FW_LABELS_PATH="$DIR/firmware_labels.json"
    printf '%s' "$EXTRACT_JSON" | jq '[
        .functions[].accesses[]
        | (if .kind == "read" then "rd" else "wr" end) as $kind
        | (.register | ascii_downcase | gsub("[^a-z0-9_]"; "_")) as $reg
        | (if .field then "_" + (.field | ascii_downcase | gsub("[^a-z0-9_]"; "_")) else "" end) as $field
        | "\($kind)_\($reg)\($field)"
    ] | unique' > "$FW_LABELS_PATH"
    printf 'firmware labels (%d):\n' \
        "$(jq 'length' "$FW_LABELS_PATH")"
    jq -r '.[] | "  - " + .' "$FW_LABELS_PATH"

    run "Step 1.6: reconcile against the register-map alphabet" \
        ./target/debug/mununu codesign reconcile-labels \
            "$FW_LABELS_PATH" \
            --peripheral-register-map "$REGISTER_MAP"

    printf '\n=== Step 2: splice the automaton into verify.template.ctxdsl ===\n'
    # Python one-shot string replace — handles multiline substitution
    # cleanly and only touches the literal placeholder marker, not
    # comment-line mentions of it.
    AUTO_TMP="$(mktemp)"
    printf '%s\n' "$AUTOMATON" > "$AUTO_TMP"
    python3 - "$TEMPLATE" "$AUTO_TMP" "$OUTPUT_CTXDSL" <<'PY'
import sys, pathlib
template_path, auto_path, output_path = sys.argv[1:4]
template = pathlib.Path(template_path).read_text()
auto = pathlib.Path(auto_path).read_text()
# Only substitute when the placeholder is the entire content of a line
# (modulo whitespace). Comment-line mentions of `{{AUTOMATON_CTXDSL}}`
# inside the template (used to explain the splice) must be preserved.
out_lines = []
for line in template.splitlines(keepends=True):
    if line.strip() == "{{AUTOMATON_CTXDSL}}":
        out_lines.append(auto if auto.endswith("\n") else auto + "\n")
    else:
        out_lines.append(line)
pathlib.Path(output_path).write_text("".join(out_lines))
PY
    rm -f "$AUTO_TMP"
    printf 'wrote spliced CTXDSL to %s (%d lines)\n' \
        "$OUTPUT_CTXDSL" "$(wc -l < "$OUTPUT_CTXDSL" | tr -d ' ')"

    run "Step 3a: verify firmware-side reachability (firmware_reaches_sending)" \
        ./target/debug/mununu context eval "$OUTPUT_CTXDSL" \
            --formula firmware_reaches_sending \
            --automaton Uart_send_byte

    run "Step 3b: verify peripheral-side reachability (peripheral_transmits)" \
        ./target/debug/mununu context eval "$OUTPUT_CTXDSL" \
            --formula peripheral_transmits \
            --automaton UartPeripheral

    run "Step 3c: verify composed-system safety (safety_protocol_respected)" \
        ./target/debug/mununu context eval "$OUTPUT_CTXDSL" \
            --formula safety_protocol_respected \
            --automaton UartProtocolSystem

    printf '\n=== end of transcript ===\n'
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
