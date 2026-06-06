#!/usr/bin/env bash
#
# scripts/docs-audit.sh — Cadence-trigger for the /docs-traceability skill.
#
# Per CLAUDE.md §Documentation Cadence Guideline, this script reports
# whether N or more commits have landed on main since the last touch
# of docs/, wiki/, README.md, or examples/*/README.md. When the
# threshold fires, it lists the file categories whose recent commits
# warrant a wiki / docs review and recommends running the
# /docs-traceability skill before the next commit.
#
# Exit codes:
#   0 — cadence within threshold; no audit needed.
#   1 — cadence threshold exceeded; manual review recommended.
#   2 — usage / git error.
#
# Usage:
#   ./scripts/docs-audit.sh            # default threshold: 10 commits
#   ./scripts/docs-audit.sh 20         # custom threshold
#
# This is an ADVISORY check — does NOT block commits/pushes. It just
# reports drift so the contributor can choose to update docs/wiki or
# run /docs-traceability to confirm no drift.

set -euo pipefail

THRESHOLD="${1:-10}"

if ! [[ "$THRESHOLD" =~ ^[0-9]+$ ]]; then
    echo "usage: $0 [threshold-commits]" >&2
    exit 2
fi

# Doc-bearing paths: any commit touching one of these is a "docs touch".
DOC_PATHS=(
    "docs/"
    "wiki/"
    "README.md"
    "examples/"  # catches examples/*/README.md and example fixtures docs
)

# Find the most recent commit touching any doc-bearing path.
LAST_DOCS_COMMIT=$(git log -n 1 --pretty=format:'%H' -- "${DOC_PATHS[@]}" 2>/dev/null || true)

if [[ -z "$LAST_DOCS_COMMIT" ]]; then
    echo "docs-audit: no commits found touching ${DOC_PATHS[*]} — repo may be too new"
    exit 0
fi

# Count commits since the last docs touch (exclusive).
COMMITS_SINCE=$(git rev-list "${LAST_DOCS_COMMIT}..HEAD" --count 2>/dev/null || echo "0")

if [[ "$COMMITS_SINCE" -lt "$THRESHOLD" ]]; then
    echo "docs-audit: OK — ${COMMITS_SINCE} commits since last docs touch (threshold: ${THRESHOLD})"
    exit 0
fi

echo "docs-audit: ⚠ ${COMMITS_SINCE} commits since last docs touch (threshold: ${THRESHOLD})"
echo
echo "Last docs/wiki touch: $(git log -n 1 --pretty=format:'%h %s' "$LAST_DOCS_COMMIT")"
echo

# Categorise recent commits by whether they touched code areas that
# typically warrant wiki updates per CLAUDE.md §Code Reuse... bullet:
# "Update [wiki] when DSL syntax, endpoints, UI flow, composition
# modes, or formula operators change."

# Categories: name|pattern (pipe-separated). Avoids bash associative
# arrays (which trip `set -u` on hyphenated keys in some bash
# versions).
CATEGORIES=(
    "CTXDSL-syntax|crates/mununu-core/src/context_dsl/parser.rs|crates/mununu-core/src/context_dsl/ast.rs|crates/mununu-core/src/context_dsl/realize.rs"
    "API-surface|crates/mununu-core/src/api/models.rs|crates/mununu-core/src/api/handlers.rs|crates/mununu-core/src/api/server.rs"
    "CLI-surface|crates/mununu-cli/src/main.rs"
    "Composition|crates/mununu-core/src/composition/"
    "Formula-operators|crates/mununu-core/src/mu_calculus/parser.rs|crates/mununu-core/src/mu_calculus/mod.rs"
    "Adapters|crates/mununu-core/src/adapter/"
)

ANY_HIT=0
for entry in "${CATEGORIES[@]}"; do
    category="${entry%%|*}"
    pattern="${entry#*|}"
    # Replace literal | with regex | for awk match.
    awk_pattern="$(printf '%s' "$pattern" | tr '|' '\n' | awk 'BEGIN{ORS="|"} {print}' | sed 's/|$//')"
    HITS=$(git log "${LAST_DOCS_COMMIT}..HEAD" --pretty=format:'%h %s' --name-only \
        | awk -v pat="$awk_pattern" '
            /^[0-9a-f]{7,} / { commit=$0; next }
            $0 ~ pat { print commit; commit="" }
        ' | sort -u)
    if [[ -n "$HITS" ]]; then
        ANY_HIT=1
        COUNT=$(printf '%s\n' "$HITS" | wc -l | tr -d ' ')
        echo "  • ${category}: ${COUNT} commit(s) touched relevant files"
        printf '%s\n' "$HITS" | head -3 | sed 's/^/      /'
        if [[ "$COUNT" -gt 3 ]]; then
            echo "      … and $((COUNT - 3)) more"
        fi
    fi
done

if [[ "$ANY_HIT" -eq 0 ]]; then
    echo "  (no commits touched wiki-relevant code areas — drift unlikely)"
fi

echo
echo "Recommended action: run the /docs-traceability skill before the next commit"
echo "                    to verify Source-of-truth anchors still resolve, and update"
echo "                    docs/ or wiki/ where new surface was added."
echo
echo "Cross-repo reminder: API changes (api/models.rs) may also require type updates"
echo "                     in mununu-ui/src/api/types.ts."

exit 1
