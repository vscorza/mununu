# Build Recipes

> Concept: finer-grained `cargo` invocations beyond `make ci`. The Makefile verbs (`make help`) cover the common cases; this page lists the per-crate, per-test, and per-tool commands worth keeping in mind.

The canonical gate is `make ci` (= `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace`). Run that before considering work done. The recipes below are for narrower workflows.

## Per-crate builds

```bash
cargo build -p mununu-core                    # core library
cargo build -p mununu-cli                     # CLI binary (mununu)
cargo build -p mununu-extract                 # extraction binary (mununu-extract)
cargo build --release -p mununu-cli           # release CLI
```

## Targeted tests

```bash
cargo test -p mununu-core                     # core lib tests only
cargo test -p mununu-extract                  # extraction tests only
cargo test -p mununu-core test_name           # specific test by name
cargo test -- --nocapture                     # print test stdout
```

## Benchmarks

```bash
cargo bench -p mununu-core
```

## Running the server

```bash
cargo run -p mununu-cli -- server                       # default 127.0.0.1:8080
cargo run -p mununu-cli -- server --addr 0.0.0.0:3000   # bind elsewhere
```

## Running the extractor

```bash
cargo run -p mununu-extract -- config.extract.json \
  --source file.ts \
  --output spec.espec.json
```

## Inside the dev container

Every command above also works inside the pinned dev container — replace `cargo ...` with the equivalent `make <verb>` where possible, or pass the cargo command after the docker invocation. See [`dev-container.md`](dev-container.md) for the docker recipes.
