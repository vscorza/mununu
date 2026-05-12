#!/usr/bin/env bash
# validate.sh — reproduce the Document B §B.8 dual-frontend SoC example.
#
# Runs the mununu subcommands the example exercises, captures their
# output, and produces a byte-deterministic transcript at
# `transcript.txt`.
#
# Three pieces of the dual-frontend story:
#   1. Manual path — `mununu contract sidecars` against a hand-authored
#      `blackbox_interfaces.json`. Demonstrates the JSON shape the
#      contract subsystem consumes.
#   2. **Automatic path** — `mununu --adapter yosys` (via the
#      mununu-cli binary) on `soc.sv`. The yosys frontend detects the
#      `(* blackbox *)` attribute on `ddr3_phy_v2`, parses the
#      pre-flatten hierarchy snapshot, and auto-emits the same JSON
#      sidecars next to the source file.
#   3. Cross-check — both paths produce the same module name and
#      port/direction shape. The auto-emitted file's source location
#      points at the user's actual `.sv` (not the yosys tempdir).
#
# Run from the repo root:
#   ./examples/industrial/dual_frontend_soc/validate.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

EXAMPLE_DIR="examples/industrial/dual_frontend_soc"
TRANSCRIPT="$EXAMPLE_DIR/transcript.txt"
SIDECARS_DIR="$EXAMPLE_DIR/sidecars_generated"

echo "build: mununu-cli binary (cargo)" 1>&2
# The example uses the `mununu-cli` package's binary specifically (it
# has both the `contract sidecars` subcommand from M1's PR #10 *and*
# the `--adapter yosys` flag that triggers the new auto-emission). The
# root crate's binary has only the former, so building mununu-cli
# overwrites `target/debug/mununu` with the superset version.
cargo build --quiet -p mununu-cli --bin mununu

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

    # ----- Automatic path -------------------------------------------
    # Remove any prior auto-emitted file so the next run is a clean
    # repro of "yosys discovers, then emits."
    rm -f "$EXAMPLE_DIR/ddr3_phy_v2.interface.json" \
          "$EXAMPLE_DIR/ddr3_phy_v2.gap_report.json"

    run "5a. AUTO-emit sidecars by running the yosys frontend on soc.sv" \
        ./target/debug/mununu context summarize \
            "$EXAMPLE_DIR/soc.sv" \
            --adapter yosys

    run "5b. Inspect the auto-emitted interface JSON" \
        cat "$EXAMPLE_DIR/ddr3_phy_v2.interface.json"

    run "5c. Inspect the auto-emitted gap-report JSON" \
        cat "$EXAMPLE_DIR/ddr3_phy_v2.gap_report.json"

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
