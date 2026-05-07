#!/usr/bin/env bash
# new_experiment.sh — scaffold a new experiment directory from the template.
#
# Usage:
#   scripts/new_experiment.sh <NNNN> <slug>
#   scripts/new_experiment.sh 0002 iter-rank-soa
#
# Creates experiments/EXP-NNNN-<slug>/ as a copy of experiments/_template/
# with today's date pre-filled where possible.

set -euo pipefail

if [ $# -ne 2 ]; then
    echo "usage: $0 <NNNN> <slug>" >&2
    echo "example: $0 0002 iter-rank-soa" >&2
    exit 2
fi

NNNN="$1"
SLUG="$2"

if [[ ! "$NNNN" =~ ^[0-9]{4}[a-z]?$ ]]; then
    echo "error: NNNN must be 4 digits with optional lowercase suffix (e.g. 0002 or 0002a), got: $NNNN" >&2
    exit 2
fi

if [[ ! "$SLUG" =~ ^[a-z0-9-]+$ ]]; then
    echo "error: slug must be lowercase, alphanumeric, hyphens only, got: $SLUG" >&2
    exit 2
fi

cd "$(git rev-parse --show-toplevel)"

DEST="experiments/EXP-${NNNN}-${SLUG}"
if [ -e "$DEST" ]; then
    echo "error: $DEST already exists" >&2
    exit 1
fi

cp -r experiments/_template "$DEST"
# Mark scaffolded-but-not-yet-run. bench_record.sh removes this on
# successful recording; until then check_repro.sh treats the EXP as
# a draft (README + log only, no manifest required).
touch "$DEST/.draft"
TODAY=$(date -u +%Y-%m-%d)

# Pre-fill the date and EXP-ID in the templated files.
for f in "${DEST}/README.md" "${DEST}/log.md"; do
    sed -i.bak "s/EXP-NNNN/EXP-${NNNN}-${SLUG}/g; s/YYYY-MM-DD/${TODAY}/g" "$f"
    rm "${f}.bak"
done

echo "==> created ${DEST}"
echo "==> next steps:"
echo "    1. edit ${DEST}/README.md (motivation, hypothesis, headline)"
echo "    2. edit ${DEST}/log.md (motivation, hypothesis, method)"
echo "    3. develop the change on a branch"
echo "    4. make bench-record EXP=EXP-${NNNN}-${SLUG} -- --bench <bench-name>"
echo "    5. update ${DEST}/log.md with results + interpretation"
echo "    6. commit ${DEST} (along with the code change)"
