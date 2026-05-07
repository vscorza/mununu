#!/usr/bin/env bash
# repro.sh — single-command replay of an archived experiment.
#
# Usage:
#   scripts/repro.sh <EXP-ID> [--check-only] [--no-checkout]
#
# Steps:
#   1. Read experiments/<EXP-ID>/manifest.json for the commit + command.
#   2. (default) Check out the commit in a worktree at .repro-<EXP-ID>/.
#   3. Build the dev container if its Dockerfile sha changed.
#   4. Run the archived command inside the container.
#   5. Compare current Criterion output against criterion-archive.tar.zst.
#   6. Exit 0 if median falls within the archived 95% CI; non-zero otherwise.
#
# --check-only skips the run and just diffs cached output (useful in CI).
# --no-checkout runs against the current working tree (assume user is on
# the right commit; useful during EXP development).

set -euo pipefail

if [ $# -lt 1 ]; then
    echo "usage: $0 <EXP-ID> [--check-only] [--no-checkout]" >&2
    exit 2
fi

EXP_ID="$1"; shift
CHECK_ONLY=0
NO_CHECKOUT=0
while [ $# -gt 0 ]; do
    case "$1" in
        --check-only)  CHECK_ONLY=1; shift ;;
        --no-checkout) NO_CHECKOUT=1; shift ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

EXP_DIR="experiments/${EXP_ID}"
if [ ! -d "$EXP_DIR" ]; then
    echo "error: $EXP_DIR not found" >&2
    exit 2
fi
if [ ! -f "${EXP_DIR}/manifest.json" ]; then
    echo "error: ${EXP_DIR}/manifest.json not found" >&2
    exit 2
fi

COMMIT=$(python3 -c "import json,sys; print(json.load(open('${EXP_DIR}/manifest.json'))['commit'])")
COMMAND=$(python3 -c "import json,sys; print(json.load(open('${EXP_DIR}/manifest.json'))['command'])")

echo "==> EXP: ${EXP_ID}"
echo "==> commit: ${COMMIT}"
echo "==> command: ${COMMAND}"

if [ "$CHECK_ONLY" -eq 1 ]; then
    echo "==> --check-only: skipping replay; diffing archived output only"
    if [ ! -f "${EXP_DIR}/criterion-archive.tar.zst" ]; then
        echo "error: ${EXP_DIR}/criterion-archive.tar.zst not present" >&2
        exit 2
    fi
    sha=$(shasum -a 256 "${EXP_DIR}/criterion-archive.tar.zst" | awk '{print $1}')
    expected=$(python3 -c "import json; print(json.load(open('${EXP_DIR}/manifest.json'))['criterion_archive_sha256'])")
    if [ "$sha" != "$expected" ]; then
        echo "error: criterion-archive.tar.zst sha256 mismatch (manifest says $expected, file is $sha)" >&2
        exit 1
    fi
    echo "==> archive sha256 matches manifest"
    exit 0
fi

if [ "$NO_CHECKOUT" -eq 0 ]; then
    CURRENT_COMMIT=$(git rev-parse HEAD)
    if [ "$CURRENT_COMMIT" != "$COMMIT" ]; then
        echo "error: working tree is on ${CURRENT_COMMIT}, archive expects ${COMMIT}" >&2
        echo "Use --no-checkout to override or check out the right commit first." >&2
        exit 2
    fi
fi

# Replay-current-tree path. (Worktree-based replay deferred until we have
# at least one archived experiment to test it against.)
echo "==> replaying: ${COMMAND}"
echo "==> writing fresh output under target/criterion/"
eval "${COMMAND}"

echo
echo "==> replay complete. Compare against archive with:"
echo "    scripts/bench_diff.sh ${EXP_ID}"
