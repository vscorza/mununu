---
name: quality-setup
description: >
  Installs code quality measurement tools to upgrade the metrics tier.
  Use when asked to set up quality tools or upgrade measurement capabilities.
---

Install measurement tools to upgrade the quality inventory tier. Detect the current tier first, then install the next tier's tools.

## Tier Detection

```bash
echo "=== Current Tool Availability ==="
command -v tokei >/dev/null 2>&1 && echo "✓ tokei" || echo "✗ tokei"
command -v rust-code-analysis-cli >/dev/null 2>&1 && echo "✓ rust-code-analysis" || echo "✗ rust-code-analysis"
command -v cargo-modules >/dev/null 2>&1 && echo "✓ cargo-modules" || echo "✗ cargo-modules"
command -v cargo-geiger >/dev/null 2>&1 && echo "✓ cargo-geiger" || echo "✗ cargo-geiger"
command -v cargo-udeps >/dev/null 2>&1 && echo "✓ cargo-udeps" || echo "✗ cargo-udeps"
command -v cargo-mutants >/dev/null 2>&1 && echo "✓ cargo-mutants" || echo "✗ cargo-mutants"
command -v cargo-nextest >/dev/null 2>&1 && echo "✓ cargo-nextest" || echo "✗ cargo-nextest"
```

## Tier 1 — SLOC & Complexity

```bash
cargo install tokei
cargo install rust-code-analysis-cli
```

**Unlocks**: accurate SLOC (comments/blanks excluded), cyclomatic complexity, cognitive complexity, Halstead metrics per function.

## Tier 2 — Coupling & Unsafe Surface

```bash
cargo install cargo-modules
cargo install cargo-geiger
cargo install cargo-udeps
```

**Unlocks**: module dependency graph, afferent/efferent coupling, instability index, precise unsafe surface area, unused dependency detection.

**Note**: `cargo-udeps` requires nightly: `rustup install nightly` if not already present.

## Tier 3 — Mutation Testing

```bash
cargo install cargo-mutants
cargo install cargo-nextest
```

**Unlocks**: mutation testing scores (how many mutants are killed by tests), parallel test execution, per-test timing.

## After Installation

Run `/quality-inventory` to verify the new tier is detected and additional metrics are collected.

Report which tier was achieved and what new metrics are now available.
