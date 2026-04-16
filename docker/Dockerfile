# Dockerfile for mununu (core verification binary)
# Produces a lightweight image with the CLI + API server.
#
# Build:
#   docker build -f docker/Dockerfile -t mununu .
#
# Usage (CLI):
#   docker run --rm -v $(pwd):/work mununu context eval \
#     /work/spec.espec.json --formula safety --automaton Main
#
# Usage (server):
#   docker run -p 8080:8080 mununu server --addr 0.0.0.0:8080

FROM rust:1.91-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY src/ src/
RUN cargo build --release -p mununu-cli --features api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -m appuser
COPY --from=builder /build/target/release/mununu /usr/local/bin/
USER appuser
HEALTHCHECK --interval=30s --timeout=5s CMD curl -sf http://localhost:8080/api/v1/health || exit 1
EXPOSE 8080
ENTRYPOINT ["mununu"]
CMD ["server", "--addr", "0.0.0.0:8080"]
