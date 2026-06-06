# Canonical entry point for build/test/lint/verify on the mununu workspace.
#
# These verbs are the same whether you run them on the host or inside the
# `mununu-dev` Docker image. CI invokes the docker form; contributors are
# free to use either.
#
# Image: docker build -f docker/Dockerfile.dev -t mununu-dev .
# Run:   docker run --rm -v $(pwd):/work mununu-dev make <verb>

CARGO       ?= cargo
VERIFY_FILE ?= examples/hw/handshake.ctxdsl

# Reproducible-experiments scaffold (see notebook/0000-overview.md and the
# per-experiment archives under experiments/EXP-NNNN-<slug>/).
EXP        ?=
BASELINE   ?= main
PROPTEST_CASES ?= 64

.PHONY: build test lint verify ci clean help \
        test-fast test-properties stress fuzz proptest-deep \
        coverage mem-profile sweep \
        bench-baseline bench-compare bench-record \
        experiment replay publish-prep

help:
	@echo "Build / test / lint:"
	@echo "  build          - cargo build --release for mununu-cli and mununu-extract"
	@echo "  test           - cargo test --workspace"
	@echo "  test-fast      - cargo test --lib + integration tests, sub-minute (pre-commit gate)"
	@echo "  lint           - cargo fmt --check && cargo clippy -D warnings"
	@echo "  verify         - cargo run mununu against $(VERIFY_FILE)"
	@echo "  ci             - lint + test (the gate)"
	@echo "  clean          - cargo clean"
	@echo
	@echo "Properties / stress / fuzz:"
	@echo "  test-properties - run proptest suite (PROPTEST_CASES=$(PROPTEST_CASES))"
	@echo "  proptest-deep   - run proptest suite with PROPTEST_CASES=4096"
	@echo "  stress          - run #[ignore]+stress feature tests (tier 3)"
	@echo "  fuzz            - run cargo-fuzz harnesses for 5 minutes each"
	@echo
	@echo "Coverage / memory:"
	@echo "  coverage        - cargo-llvm-cov HTML + JSON summary"
	@echo "  mem-profile     - run dhat-instrumented stress tests"
	@echo
	@echo "Disk hygiene:"
	@echo "  sweep           - cargo sweep on mununu's siblings under ~/git_repo/ (dry-run; SWEEP_APPLY=1 to delete)"
	@echo
	@echo "Benchmarks:"
	@echo "  bench-baseline  - cargo bench --save-baseline $(BASELINE)"
	@echo "  bench-compare   - cargo bench --baseline $(BASELINE) and run scripts/bench_diff.sh"
	@echo "  bench-record    - EXP=<EXP-ID> -- <bench-args> ; archive criterion + manifest"
	@echo
	@echo "Experiments:"
	@echo "  experiment      - EXP=NNNN-<slug> ; scaffolds experiments/EXP-<EXP>/"
	@echo "  replay          - EXP=<EXP-ID> ; replays an archived experiment"
	@echo "  publish-prep    - validate every experiment archive (scripts/check_repro.sh)"

build:
	$(CARGO) build --release -p mununu-cli
	$(CARGO) build --release -p mununu-extract

# Prefer cargo-nextest when available (CI's dev image installs it). nextest
# is faster but does NOT run doctests — invoke them separately so they
# cannot silently drop. Falling back to `cargo test --workspace` covers
# both unit and doc tests in one shot.
test:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		$(CARGO) nextest run --workspace && \
		$(CARGO) test --workspace --doc; \
	else \
		$(CARGO) test --workspace; \
	fi

# tier-1 + tier-2: pre-commit gate. Skips #[ignore]'d stress tests by default.
# Eventually this will be the canonical pre-commit form; for now it is an
# alias to `test` until tier-3 stress tests are split out.
test-fast:
	$(CARGO) test --workspace --lib --bins --tests --examples

test-properties:
	PROPTEST_CASES=$(PROPTEST_CASES) $(CARGO) test --workspace --test 'properties*' -- --nocapture

proptest-deep:
	$(MAKE) test-properties PROPTEST_CASES=4096

stress:
	$(CARGO) test --workspace --features stress --tests 'stress_*' -- --ignored --nocapture

fuzz:
	@if [ ! -d fuzz ]; then echo "fuzz/ not initialized; run: cargo fuzz init"; exit 1; fi
	@for target in $$(cargo fuzz list 2>/dev/null); do \
		echo "==> fuzzing $$target for 5 minutes"; \
		cargo fuzz run $$target -- -max_total_time=300 || exit 1; \
	done

coverage:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "install: cargo install cargo-llvm-cov"; exit 1; }
	$(CARGO) llvm-cov --workspace --html --output-dir target/coverage
	$(CARGO) llvm-cov --workspace --json --summary-only --output-path target/coverage/summary.json

mem-profile:
	$(CARGO) test --workspace --features dhat,stress --tests 'stress_*' -- --ignored --nocapture

# Reclaim Cargo build artifacts on mununu's siblings under ~/git_repo/.
# Default: dry-run (shows what would be removed). Set SWEEP_APPLY=1 to delete.
# Override age threshold with SWEEP_DAYS=N (default 14).
sweep:
	@./scripts/sweep_targets.sh

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Use `cargo run` so the binary path resolves correctly whether or not
# CARGO_TARGET_DIR is set (the dev Docker image redirects target/ to
# /cargo-target so host and container caches stay independent).
verify:
	$(CARGO) run --release --quiet -p mununu-cli -- context summarize $(VERIFY_FILE)

ci: lint test

# Advisory documentation cadence check. Reports whether more than
# THRESHOLD commits (default 10) have landed since the last touch of
# docs/, wiki/, README.md, or examples/. When triggered, lists the
# code areas whose recent commits warrant a wiki / docs review and
# recommends invoking the /docs-traceability skill. See CLAUDE.md
# §Documentation Cadence Guideline for the contributor protocol.
#
# Usage:
#   make docs-audit                       # default threshold = 10
#   make docs-audit DOCS_THRESHOLD=20     # custom threshold
#
# This target does NOT block commits/pushes; exit code 1 on threshold
# breach is advisory only.
docs-audit:
	@scripts/docs-audit.sh $(DOCS_THRESHOLD)

clean:
	$(CARGO) clean

# ─────────────────────────────────────────────────────────────────────────
# Experiment / replay verbs (see notebook/0000-overview.md).
# ─────────────────────────────────────────────────────────────────────────

experiment:
	@if [ -z "$(EXP)" ]; then echo "usage: make experiment EXP=NNNN-<slug>  (e.g. EXP=0002-iter-rank-soa)"; exit 2; fi
	@nnnn=$$(echo "$(EXP)" | cut -d- -f1); \
	 slug=$$(echo "$(EXP)" | cut -d- -f2-); \
	 scripts/new_experiment.sh "$$nnnn" "$$slug"

bench-baseline:
	$(CARGO) bench --workspace -- --save-baseline $(BASELINE)

bench-compare:
	$(CARGO) bench --workspace -- --baseline $(BASELINE)
	scripts/bench_diff.sh $(BASELINE)

# Usage: make bench-record EXP=EXP-0001-baseline -- --bench mu_calculus_only
bench-record:
	@if [ -z "$(EXP)" ]; then echo "usage: make bench-record EXP=EXP-NNNN-<slug> -- <bench-args>"; exit 2; fi
	scripts/bench_record.sh $(EXP) $(filter-out $@,$(MAKECMDGOALS))

replay:
	@if [ -z "$(EXP)" ]; then echo "usage: make replay EXP=EXP-NNNN-<slug>"; exit 2; fi
	scripts/repro.sh $(EXP)

publish-prep:
	scripts/check_repro.sh

# Allow extra args after `make bench-record EXP=...` to fall through silently
# (otherwise make tries to interpret each `--bench foo` as its own target).
%:
	@:
