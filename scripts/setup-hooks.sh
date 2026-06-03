#!/usr/bin/env bash
# Install mununu's local git hooks.
#
# - pre-commit (scripts/pre-commit): per-crate scoped tests + workspace
#   fmt/clippy/doctests/machete. ~5-10 min wall-clock for a typical
#   single-crate commit; ~5 sec for doc-only commits.
# - pre-push (scripts/pre-push): full-workspace `make ci`. ~30-60 min
#   wall-clock. The safety net that catches cross-crate drift before
#   the push lands on GitHub.
#
# Both hooks are symlinked from .git/hooks/ to the scripts/ originals
# so future updates to the source files take effect immediately.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_DIR="$REPO_ROOT/.git/hooks"

echo "Installing pre-commit hook..."
ln -sf "$SCRIPT_DIR/pre-commit" "$HOOKS_DIR/pre-commit"
chmod +x "$HOOKS_DIR/pre-commit"
echo "Pre-commit hook installed (per-crate scoped tests; see scripts/pre-commit)."

echo "Installing pre-push hook..."
ln -sf "$SCRIPT_DIR/pre-push" "$HOOKS_DIR/pre-push"
chmod +x "$HOOKS_DIR/pre-push"
echo "Pre-push hook installed (full-workspace make ci; see scripts/pre-push)."
