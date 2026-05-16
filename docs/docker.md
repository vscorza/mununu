# Docker Best Practices

> Concept: when writing or reviewing Dockerfiles for this project (a Rust binary service), use these patterns. The dev container at `docker/Dockerfile.dev` follows them; new images should too.

## Build images

- **Multi-stage builds.** Use a full `rust` builder image to compile, then copy only the binary into a slim runtime image (`debian:bookworm-slim` or `gcr.io/distroless/cc`). CI tools stay in the build stage; the production image stays clean.
- **Pin exact tags.** Never use `FROM ubuntu:latest` or `FROM rust:latest`. Use `FROM rust:1.82-slim-bookworm`. Builds must stay reproducible. If you update the tutorial Dockerfile's `ubuntu:24.04`, pin it to a digest or full minor tag.
- **Order layers by change frequency.** Copy `Cargo.toml` / `Cargo.lock` first and run a dummy build to cache dependencies, then copy `src/`. The dependency cache only busts when dependencies change, not on every source edit.

## Layer hygiene

- **Combine RUN commands.** Chain with `&&` and clean up in the same layer:
  ```dockerfile
  RUN apt-get update \
   && apt-get install -y --no-install-recommends curl \
   && rm -rf /var/lib/apt/lists/*
  ```
- **Always use `.dockerignore`.** Exclude `target/`, `.git/`, `.env`, `*.key`, `wiki/`, `tutorial/` from the build context. Avoids leaking secrets and slow builds.

## Runtime hardening

- **Never run as root.** Add `RUN useradd -m appuser && chown -R appuser /app` and `USER appuser` before the entrypoint.
- **Add a HEALTHCHECK.**
  ```dockerfile
  HEALTHCHECK --interval=30s --timeout=5s \
    CMD curl -f http://localhost:PORT/health || exit 1
  ```
  Kubernetes / ECS uses this to gate traffic.

## ARG vs ENV

- `ARG` for build-time values (versions, feature flags).
- `ENV` is baked into the image and visible in `docker inspect` — **never** put secrets in `ENV`. Use runtime secret injection instead.

## Tutorial / example Dockerfiles

The dev container ([`docker/Dockerfile.dev`](../docker/Dockerfile.dev)) is the canonical reference. Tutorial Dockerfiles may relax the multi-stage rule for readability, but must still pin exact tags and avoid root.
