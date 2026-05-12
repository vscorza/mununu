#!/usr/bin/env bash
# validate.sh — reproduce the Document B §B.8 dual-frontend SoC example.
#
# Runs the mununu subcommands the example exercises, captures their
# output, and produces a byte-deterministic transcript at
# `transcript.txt`.
#
# Today's slice exercises:
#   - `mununu contract sidecars` against the hand-authored
#     `blackbox_interfaces.json` — the same JSON shape the yosys
#     adapter will auto-emit once Document B's yosys integration ships.
#   - `mununu context eval` against the hand-authored SoC composition.
#
# When the yosys-side auto-emission lands, this script will gain a
# step that runs the yosys adapter on `soc.sv` and cross-checks the
# auto-emitted sidecars against the hand-authored ones (byte-identical).
# That step is intentionally not present today; the README explains
# why.
#
# Run from the repo root:
#   ./examples/industrial/dual_frontend_soc/validate.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

EXAMPLE_DIR="examples/industrial/dual_frontend_soc"
TRANSCRIPT="$EXAMPLE_DIR/transcript.txt"
SIDECARS_DIR="$EXAMPLE_DIR/sidecars_generated"

echo "build: mununu binary (cargo)" 1>&2
cargo build --quiet --bin mununu

strip_logs() {
    perl -pe '
        s/\e\[[0-9;]*m//g;
        s/^\s*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s+//;
    ' | grep -v 'Logging initialized'
}

# Make paths in output deterministic by stripping the workspace prefix.
strip_paths() {
    sed -e "s|$(pwd)/||g"
}

run() {
    local title="$1"; shift
    printf '\n=== %s ===\n' "$title"
    printf '$ %s\n' "$*"
    { "$@" 2>&1 || true; } | strip_logs | strip_paths
}

# Clean any prior run's sidecar output so the transcript is byte-stable.
rm -rf "$SIDECARS_DIR"

{
    printf '# dual-frontend SoC — validation transcript\n'
    printf '# regenerate via examples/industrial/dual_frontend_soc/validate.sh\n'
    printf '# transcript is byte-deterministic; re-running validate.sh\n'
    printf '# against the same commit should produce identical output.\n'

    run "1. Emit sidecars for the DDR3 PHY black box" \
        ./target/debug/mununu contract sidecars \
            "$EXAMPLE_DIR/blackbox_interfaces.json" \
            --out-dir "$SIDECARS_DIR"

    run "2. Inspect the auto-emitted interface JSON" \
        cat "$SIDECARS_DIR/DDR3_PHY_V2.interface.json"

    run "3. Inspect the auto-emitted gap-report JSON" \
        cat "$SIDECARS_DIR/DDR3_PHY_V2.gap_report.json"

    run "4. Discover phase-1 contract using the auto-emitted interface" \
        ./target/debug/mununu contract discover \
            "$SIDECARS_DIR/DDR3_PHY_V2.interface.json"

    run "5. Run the gap diagnostic (strict mode expected to exit non-zero)" \
        ./target/debug/mununu contract gaps \
            "$SIDECARS_DIR/DDR3_PHY_V2.gap_report.json" \
            --strict-contracts

    run "6. SoC composition — well-formedness (safety, under chaotic DDR)" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/soc.ctxdsl" \
            --formula soc_well_formed \
            --automaton SoC

    run "7. Host can reach the DDR burst-wait state" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/soc.ctxdsl" \
            --formula burst_path_reachable \
            --automaton HostController

    run "8. Host can reach the UART send path" \
        ./target/debug/mununu context eval \
            "$EXAMPLE_DIR/soc.ctxdsl" \
            --formula uart_send_reachable \
            --automaton HostController

    printf '\n=== end of transcript ===\n'
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
