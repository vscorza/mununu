#!/usr/bin/env bash
# validate.sh — SV multi-module composition under the verify framework.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
DIR="examples/verify/sv_multi_module"
TRANSCRIPT="$DIR/transcript.txt"

echo "build: mununu binary (cargo)" 1>&2
cargo build --quiet --bin mununu

strip_logs() {
    perl -pe '
        s/\e\[[0-9;]*m//g;
        s/^\s*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z\s+//;
    ' | grep -v 'Logging initialized'
}

{
    printf '# sv_multi_module — verify framework end-to-end transcript\n'
    printf '# Regenerated via examples/verify/sv_multi_module/validate.sh\n'
    printf '#\n'
    printf "# Producer + consumer SV modules composed via the SV adapter's\n"
    printf "# multi-module entry point. Demonstrates the verify framework's\n"
    printf '# multi-file source support (Stream A1 of the adjacent-work plan).\n\n'

    printf '=== mununu verify (human-readable) ===\n'
    ./target/debug/mununu verify "$DIR/verify.toml" 2>&1 | strip_logs
} > "$TRANSCRIPT"

printf 'wrote transcript to %s (%d lines)\n' \
    "$TRANSCRIPT" "$(wc -l < "$TRANSCRIPT" | tr -d ' ')"
