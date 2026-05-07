#!/usr/bin/env bash
# bench_compare.sh — same-session A/B comparison with cache-state mitigation.
#
# Usage:
#   scripts/bench_compare.sh <baseline-name> [--no-warmup] -- <bench-args>
#
# Protocol (see notebook/BENCH_POLICY.md "Regression mitigation"):
#   1. (default) Warmup: run the bench once at --quick and discard output.
#      This pays the page-fault + binary-cache costs that contaminate
#      first-after-recompile measurements.
#   2. Run the bench with `--save-baseline <baseline-name>`. This is the A side.
#   3. Apply your code change (the script does NOT do this — you must).
#   4. Re-run as `bench_compare.sh <baseline-name> --baseline-only -- <args>`
#      to measure the B side against the saved baseline.
#
# For a one-shot two-commit comparison, `bench_record.sh --warmup` per EXP
# combined with archived criterion JSON is the production protocol; this
# script is for ad-hoc local A/B work where you want to keep both sides
# in the same shell session and same compile-cache state.

set -euo pipefail

if [ $# -lt 2 ]; then
    cat >&2 <<USAGE
usage: $0 <baseline-name> [--no-warmup] [--baseline-only] -- <bench-args>

  --no-warmup       skip the --quick warmup (faster, less reliable).
  --baseline-only   compare against an existing saved baseline (B side).
                    Without this flag, the script SAVES a baseline (A side).

example workflow:
  # A side (before code change):
  scripts/bench_compare.sh exp-0009-pre -- -p mununu-core --features test_support \\
      --bench minimization_only

  # ... apply Paige-Tarjan migration ...

  # B side (after code change):
  scripts/bench_compare.sh exp-0009-pre --baseline-only -- -p mununu-core \\
      --features test_support --bench minimization_only
USAGE
    exit 2
fi

BASELINE_NAME="$1"; shift
WARMUP=1
BASELINE_ONLY=0
while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    case "$1" in
        --no-warmup) WARMUP=0 ;;
        --baseline-only) BASELINE_ONLY=1 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
    shift
done
[ "${1:-}" = "--" ] && shift
if [ $# -lt 1 ]; then
    echo "error: <bench-args> required after --" >&2
    exit 2
fi

cd "$(git rev-parse --show-toplevel)"

# Split args into cargo-side and criterion-side around the first `--`
# the user provided. If the user did not include a `--`, everything
# is cargo-side and we add `-- ...criterion args` ourselves.
CARGO_ARGS=()
CRITERION_ARGS=()
saw_separator=0
for arg in "$@"; do
    if [ "$saw_separator" -eq 0 ] && [ "$arg" = "--" ]; then
        saw_separator=1
        continue
    fi
    if [ "$saw_separator" -eq 1 ]; then
        CRITERION_ARGS+=("$arg")
    else
        CARGO_ARGS+=("$arg")
    fi
done

run_bench() {
    # $@ is the criterion-side args list to use for this run.
    cargo bench "${CARGO_ARGS[@]}" -- "$@"
}

# `${arr[@]+"${arr[@]}"}` is the bash-safe expansion of a possibly-empty
# array under `set -u`. Without it, an empty CRITERION_ARGS triggers
# "unbound variable".
crit_args() {
    if [ "${#CRITERION_ARGS[@]}" -eq 0 ]; then
        return 0
    fi
    printf '%s\n' "${CRITERION_ARGS[@]}"
}

if [ "$WARMUP" -eq 1 ]; then
    echo "==> warmup: running once at --quick (output discarded)"
    # Use --quick for the warmup regardless of what the real run uses.
    if [ "${#CRITERION_ARGS[@]}" -eq 0 ]; then
        run_bench --quick >/dev/null 2>&1 || true
    else
        run_bench --quick "${CRITERION_ARGS[@]}" >/dev/null 2>&1 || true
    fi
fi

if [ "$BASELINE_ONLY" -eq 0 ]; then
    echo "==> A side: saving baseline as '${BASELINE_NAME}'"
    if [ "${#CRITERION_ARGS[@]}" -eq 0 ]; then
        run_bench --save-baseline "${BASELINE_NAME}"
    else
        run_bench "${CRITERION_ARGS[@]}" --save-baseline "${BASELINE_NAME}"
    fi
    echo
    echo "==> baseline saved. To run the B side after applying your change:"
    echo "    scripts/bench_compare.sh ${BASELINE_NAME} --baseline-only -- $*"
else
    echo "==> B side: comparing against baseline '${BASELINE_NAME}'"
    if [ "${#CRITERION_ARGS[@]}" -eq 0 ]; then
        run_bench --baseline "${BASELINE_NAME}"
    else
        run_bench "${CRITERION_ARGS[@]}" --baseline "${BASELINE_NAME}"
    fi
    echo
    echo "==> done. Use scripts/bench_diff.sh ${BASELINE_NAME} for the regression gate."
fi
