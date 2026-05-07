#!/usr/bin/env bash
# check_repro.sh — pre-publish gate: validates every archived experiment
# is well-formed and (optionally) replays each one.
#
# Usage:
#   scripts/check_repro.sh                # validate manifests only
#   scripts/check_repro.sh --replay-all   # also run scripts/repro.sh per EXP
#
# Validation is per-schema-version (see experiments/SCHEMA.md). Older
# archives at lower schema versions are NOT held to newer field
# requirements — the scaffold can evolve without invalidating history.
#
# Exits non-zero on the first malformed experiment or replay failure.

set -euo pipefail

REPLAY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --replay-all) REPLAY=1; shift ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# Files required at the EXP directory level (independent of schema version).
REQUIRED_FILES=(
    README.md
    log.md
    manifest.json
    command.txt
    hw-fingerprint.txt
)

cd "$(git rev-parse --show-toplevel)"

fail=0
for exp_dir in experiments/EXP-*; do
    [ -d "$exp_dir" ] || continue
    name=$(basename "$exp_dir")

    # An EXP can be marked "draft" via a `.draft` file. Drafts are
    # exempt from manifest/command/hw-fingerprint requirements but
    # MUST still carry README.md + log.md so the lab notebook is
    # populated before any code change lands. Drop the .draft marker
    # at recording time (bench_record.sh removes it automatically).
    if [ -f "${exp_dir}/.draft" ]; then
        echo "==> ${name} [DRAFT]"
        for required in README.md log.md; do
            if [ ! -f "${exp_dir}/${required}" ]; then
                echo "    MISSING (draft): ${required}"
                fail=1
            fi
        done
        continue
    fi

    echo "==> ${name}"
    for required in "${REQUIRED_FILES[@]}"; do
        if [ ! -f "${exp_dir}/${required}" ]; then
            echo "    MISSING: ${required}"
            fail=1
        fi
    done
    if [ -f "${exp_dir}/manifest.json" ]; then
        if ! python3 -c "
import json, sys

# Per-schema-version required field set. Adding fields to a NEW version
# does not break OLDER archives. See experiments/SCHEMA.md.
REQUIRED = {
    1: {
        'schema_version', 'exp_id', 'commit', 'branch', 'git_dirty',
        'host', 'container', 'command', 'started_at', 'ended_at',
        'exit_code', 'hw_fingerprint_sha256', 'criterion_archive_sha256',
        'rustc', 'rust_toolchain_toml_sha256', 'dev_container_dockerfile_sha256',
    },
}

m = json.load(open('${exp_dir}/manifest.json'))
v = m.get('schema_version', 1)
if v not in REQUIRED:
    print(f'    UNKNOWN schema_version: {v}', file=sys.stderr)
    sys.exit(1)
missing = REQUIRED[v] - set(m.keys())
if missing:
    print(f'    MISSING manifest fields (schema v{v}): {sorted(missing)}', file=sys.stderr)
    sys.exit(1)
" 2>&1; then
            fail=1
        fi
    fi
done

if [ $REPLAY -eq 1 ] && [ $fail -eq 0 ]; then
    for exp_dir in experiments/EXP-*; do
        [ -d "$exp_dir" ] || continue
        name=$(basename "$exp_dir")
        echo "==> replaying ${name}"
        if ! scripts/repro.sh "$name" --check-only; then
            echo "    REPLAY FAILED: ${name}"
            fail=1
        fi
    done
fi

if [ $fail -eq 0 ]; then
    echo "==> check_repro.sh: all experiments well-formed"
else
    echo "==> check_repro.sh: failures detected"
    exit 1
fi
