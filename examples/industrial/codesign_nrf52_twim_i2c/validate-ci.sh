#!/usr/bin/env bash
# validate-ci.sh - CI-friendly variant of validate.sh.
#
# Differences vs. validate.sh:
#   - Reads the mununu binary path from $MUNUNU_BIN (no host-specific path
#     resolution; the CI workflow points it at the build artefact).
#   - Writes per-run logs to ./ci-output/ instead of overwriting transcript.txt
#     (transcript.txt is the canonical record of the upstream example and must
#     not be touched by CI).
#   - Exits non-zero if:
#       * any CLEAN_PROPERTY is not "HOLDS" on firmware.ctxdsl, OR
#       * every BUGGY_PROPERTY is "HOLDS" on firmware_buggy.ctxdsl (the bug
#         must show up somewhere; if it does not, the demo is broken).
#   - Emits ./ci-output/comment.md - a Markdown summary the workflow posts as
#     a PR comment. The first line is the Rule 2 disclosure verbatim.
#   - Emits ./ci-output/buggy_data_pointer.json - the structured-JSON verdict
#     for the canonical counterexample property.
#
# Per CLAUDE.md Claims Integrity Rule 2, this script is wired so the PR
# comment always starts with the planted-bug disclosure. The demo is a
# pattern study, not a finding about Nordic silicon.

set -euo pipefail
cd "$(dirname "$0")"

: "${MUNUNU_BIN:?MUNUNU_BIN must point at the mununu CLI binary}"
if [[ ! -x "$MUNUNU_BIN" ]]; then
    echo "FATAL: \$MUNUNU_BIN ($MUNUNU_BIN) is not executable" 1>&2
    exit 2
fi

mkdir -p ci-output
: > ci-output/run.log

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

# The canonical counterexample property is data_pointer_set_before_tasks;
# safety_protocol_respected is checked too (it is expected to still HOLD on
# the buggy variant - the bug shows up as an additional admissible edge, not
# as a structurally malformed automaton).
BUGGY_PROPERTIES=(
    data_pointer_set_before_tasks
    safety_protocol_respected
)

strip_ansi() {
    perl -pe 's/\e\[[0-9;]*m//g'
}

verdict_for() {
    # $1 = stdout from a "mununu codesign verify ..." run.
    # Echoes one of: HOLDS | VIOLATED | UNKNOWN
    local out="$1"
    if grep -qE '^\s*verdict:\s*HOLDS' <<<"$out"; then
        echo HOLDS
    elif grep -qE '^\s*verdict:\s*VIOLATED' <<<"$out"; then
        echo VIOLATED
    else
        echo UNKNOWN
    fi
}

run_verify() {
    # $1 firmware file, $2 property name. Returns captured stdout.
    local fw="$1" formula="$2" out
    set +e
    out="$("$MUNUNU_BIN" codesign verify register_map.json "$fw" --formula "$formula" 2>&1 | strip_ansi)"
    set -e
    printf '\n=== verify %s :: %s ===\n%s\n' "$fw" "$formula" "$out" >> ci-output/run.log
    printf '%s' "$out"
}

fail=0
clean_results=()
for f in "${CLEAN_PROPERTIES[@]}"; do
    out="$(run_verify firmware.ctxdsl "$f")"
    v="$(verdict_for "$out")"
    clean_results+=("$f $v")
    if [[ "$v" != "HOLDS" ]]; then
        echo "FAIL: clean firmware property $f -> $v (expected HOLDS)" 1>&2
        fail=1
    fi
done

buggy_results=()
buggy_all_hold=1
for f in "${BUGGY_PROPERTIES[@]}"; do
    out="$(run_verify firmware_buggy.ctxdsl "$f")"
    v="$(verdict_for "$out")"
    buggy_results+=("$f $v")
    if [[ "$v" != "HOLDS" ]]; then
        buggy_all_hold=0
    fi
done

if [[ "$buggy_all_hold" == "1" ]]; then
    echo "FAIL: every buggy-firmware property HELD - the planted bug is no longer surfacing" 1>&2
    fail=1
fi

# Capture the structured-JSON verdict for the canonical counterexample.
set +e
"$MUNUNU_BIN" codesign verify register_map.json firmware_buggy.ctxdsl \
    --formula data_pointer_set_before_tasks --json \
    > ci-output/buggy_data_pointer.json 2> ci-output/buggy_data_pointer.stderr
set -e

# ---------------------------------------------------------------------------
# Build the PR-comment Markdown. First line is the Rule 2 disclosure verbatim.
# ---------------------------------------------------------------------------
{
    printf '**Note**: this demo example contains a deliberately-introduced bug for demonstration purposes. It is a pattern study; it is NOT a finding about Nordic silicon. See the example'\''s README for details.\n\n'
    printf '## mununu codesign verify\n\n'
    printf '### Clean firmware (`firmware.ctxdsl`) - every property expected to HOLD\n\n'
    printf '| Property | Verdict |\n|---|---|\n'
    for r in "${clean_results[@]}"; do
        name="${r% *}"
        v="${r##* }"
        printf '| `%s` | %s |\n' "$name" "$v"
    done
    printf '\n### Buggy firmware (`firmware_buggy.ctxdsl`) - the planted bug must surface\n\n'
    printf '| Property | Verdict |\n|---|---|\n'
    for r in "${buggy_results[@]}"; do
        name="${r% *}"
        v="${r##* }"
        printf '| `%s` | %s |\n' "$name" "$v"
    done
    printf '\n### Structured counterexample (`data_pointer_set_before_tasks` on the buggy firmware)\n\n'
    printf '```json\n'
    cat ci-output/buggy_data_pointer.json
    printf '\n```\n'
    printf '\n_Generated by `.github/workflows/codesign-verify.yml` against the `example/` directory._\n'
} > ci-output/comment.md

if [[ "$fail" != "0" ]]; then
    echo "validate-ci: FAILED (see ci-output/run.log)" 1>&2
    exit 1
fi
echo "validate-ci: OK (clean=all HOLDS, buggy=$(IFS=,; echo "${buggy_results[*]}"))"
