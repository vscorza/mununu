#!/usr/bin/env bash
# bench_record.sh — wrap `cargo bench` with provenance capture.
#
# Usage:
#   scripts/bench_record.sh <EXP-ID> <bench-args...>
#   scripts/bench_record.sh EXP-0001 --bench mu_calculus_only
#   scripts/bench_record.sh EXP-0009 --bench minimization_only -- --save-baseline EXP-0009
#
# Side effects:
#   1. Captures hw fingerprint to experiments/<EXP-ID>/hw-fingerprint.txt.
#   2. Writes manifest.json with commit, container digest, command, timestamps.
#   3. Runs `cargo bench` with the provided args.
#   4. Archives target/criterion/ to experiments/<EXP-ID>/criterion-archive.tar.zst.
#   5. Writes command.txt with the exact replay command.

set -euo pipefail

FRESH=0
WARMUP=0
while true; do
    case "${1:-}" in
        --fresh) FRESH=1; shift ;;
        --warmup) WARMUP=1; shift ;;
        *) break ;;
    esac
done

if [ $# -lt 2 ]; then
    cat >&2 <<USAGE
usage: $0 [--fresh] [--warmup] <EXP-ID> <bench-args...>

  --fresh     clear target/criterion before running so the archive contains
              only the just-measured runs. Without this flag, the archive
              captures whatever target/criterion contains at run time —
              useful when iterating but problematic for paper-grade
              archives. See notebook/REFINEMENT.md.

  --warmup    run the bench once at --quick BEFORE the real recording and
              discard the result. Mitigates cache-state differences between
              the first post-recompile run (cold caches, page faults) and
              subsequent runs. Adds ~30-60s but produces more comparable
              numbers when the same binary is benched repeatedly across
              EXPs. See notebook/BENCH_POLICY.md "Regression mitigation".
USAGE
    exit 2
fi

EXP_ID="$1"; shift
EXP_DIR="experiments/${EXP_ID}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [[ ! "$EXP_ID" =~ ^EXP-[0-9]{4} ]]; then
    echo "error: EXP-ID must match /^EXP-[0-9]{4}/, got: $EXP_ID" >&2
    exit 2
fi

if [ ! -d "$EXP_DIR" ]; then
    echo "error: $EXP_DIR does not exist. Create it via: make experiment EXP=${EXP_ID#EXP-}" >&2
    exit 2
fi

START=$(date -u +%Y-%m-%dT%H:%M:%SZ)
COMMIT=$(git rev-parse HEAD 2>/dev/null || echo unknown)
BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)
DIRTY=$(if [ -n "$(git status --porcelain 2>/dev/null)" ]; then echo yes; else echo no; fi)
HOSTNAME=$(hostname 2>/dev/null || echo unknown)
CONTAINER=$(if [ -f /.dockerenv ]; then echo yes; else echo no; fi)

if [ "$FRESH" -eq 1 ]; then
    rm -rf "${CARGO_TARGET_DIR:-target}/criterion"
fi

if [ "$WARMUP" -eq 1 ]; then
    echo "==> warmup: running once at --quick (output discarded)"
    # Pull --quick into the cargo arg list. Idempotent: if the user
    # already passed --quick, the doubled flag is harmless.
    if [[ "$*" == *"--"* ]] && [[ "$*" == *"--quick"* ]]; then
        cargo bench "$@" >/dev/null 2>&1 || true
    else
        cargo bench "$@" -- --quick >/dev/null 2>&1 || true
    fi
    if [ "$FRESH" -eq 1 ]; then
        # Re-clear so the warmup output doesn't end up in the archive.
        rm -rf "${CARGO_TARGET_DIR:-target}/criterion"
    fi
    echo "==> warmup complete; recording for real"
fi

# Capture hardware fingerprint.
"${SCRIPT_DIR}/capture_hw.sh" > "${EXP_DIR}/hw-fingerprint.txt"
HW_SHA=$(shasum -a 256 "${EXP_DIR}/hw-fingerprint.txt" | awk '{print $1}')

# Write replay command.
echo "cargo bench $*" > "${EXP_DIR}/command.txt"

# Run the bench.
echo "==> running: cargo bench $*"
echo "==> archiving criterion output to ${EXP_DIR}/criterion-archive.tar.zst on completion"

set +e
cargo bench "$@" 2>&1 | tee "${EXP_DIR}/bench-stdout.log"
RC=${PIPESTATUS[0]}
set -e

END=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Archive criterion output.
TARGET_CRITERION="${CARGO_TARGET_DIR:-target}/criterion"
if [ -d "$TARGET_CRITERION" ]; then
    tar --use-compress-program=zstd -cf "${EXP_DIR}/criterion-archive.tar.zst" -C "$(dirname "$TARGET_CRITERION")" "$(basename "$TARGET_CRITERION")"
    CRIT_SHA=$(shasum -a 256 "${EXP_DIR}/criterion-archive.tar.zst" | awk '{print $1}')
else
    CRIT_SHA=""
    echo "warning: $TARGET_CRITERION not found; no criterion archive written" >&2
fi

# Write manifest.json. The schema_version field is the contract between
# bench_record.sh / check_repro.sh / repro.sh — bump it when adding a
# REQUIRED field. Optional fields can be added without a bump.
# See experiments/SCHEMA.md for the version history.
cat > "${EXP_DIR}/manifest.json" <<EOF
{
  "schema_version": 1,
  "exp_id": "${EXP_ID}",
  "commit": "${COMMIT}",
  "branch": "${BRANCH}",
  "git_dirty": "${DIRTY}",
  "host": "${HOSTNAME}",
  "container": "${CONTAINER}",
  "command": $(printf '%s\n' "cargo bench $*" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))'),
  "started_at": "${START}",
  "ended_at": "${END}",
  "exit_code": ${RC},
  "hw_fingerprint_sha256": "${HW_SHA}",
  "criterion_archive_sha256": "${CRIT_SHA}",
  "rustc": "$(rustc --version 2>/dev/null || echo unavailable)",
  "rust_toolchain_toml_sha256": "$(if [ -f rust-toolchain.toml ]; then shasum -a 256 rust-toolchain.toml | awk '{print $1}'; else echo unavailable; fi)",
  "dev_container_dockerfile_sha256": "$(if [ -f docker/Dockerfile.dev ]; then shasum -a 256 docker/Dockerfile.dev | awk '{print $1}'; else echo unavailable; fi)"
}
EOF

echo "==> manifest written to ${EXP_DIR}/manifest.json"

# Recording promotes a draft EXP to fully-validated status.
if [ -f "${EXP_DIR}/.draft" ] && [ "$RC" -eq 0 ]; then
    rm -f "${EXP_DIR}/.draft"
    echo "==> draft marker cleared"
fi

echo "==> bench exited with code ${RC}"
exit "${RC}"
