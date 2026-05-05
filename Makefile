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

.PHONY: build test lint verify ci clean help

help:
	@echo "Targets:"
	@echo "  build   - cargo build --release for mununu-cli and mununu-extract"
	@echo "  test    - cargo test --workspace"
	@echo "  lint    - cargo fmt --check && cargo clippy -D warnings"
	@echo "  verify  - cargo run mununu against $(VERIFY_FILE)"
	@echo "  ci      - lint + test (the gate)"
	@echo "  clean   - cargo clean"

build:
	$(CARGO) build --release -p mununu-cli
	$(CARGO) build --release -p mununu-extract

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Use `cargo run` so the binary path resolves correctly whether or not
# CARGO_TARGET_DIR is set (the dev Docker image redirects target/ to
# /cargo-target so host and container caches stay independent).
verify:
	$(CARGO) run --release --quiet -p mununu-cli -- context summarize $(VERIFY_FILE)

ci: lint test

clean:
	$(CARGO) clean
