# `docker/` — image catalogue

| File | Purpose | Status |
|---|---|---|
| `Dockerfile.dev` | **Reproducible dev/test image.** Rust workspace toolchain only — source is mounted, not COPYed. Same `make <verb>` command works locally and in CI. | Active |
| `Dockerfile` | Production image. Multi-stage build, copies source, ships the `mununu` CLI/server binary as the entrypoint. | Active |
| `Dockerfile.extract` | Production image for the `mununu-extract` tree-sitter frontend. Same multi-stage pattern. | Active |
| `Dockerfile.extract-circt` | Placeholder for a future CIRCT-based SystemVerilog extraction frontend. | Not yet implemented |
| `Dockerfile.extract-llvm` | Placeholder for a future LLVM/SVF-based C/C++/Rust extraction frontend. | Not yet implemented |

## Quick reference

```sh
# dev/test (the canonical local + CI workflow)
docker build -f docker/Dockerfile.dev -t mununu-dev .
docker volume create mununu-target   # one-time, warm cargo cache across runs
docker run --rm \
  -v $(pwd):/work \
  -v mununu-target:/cargo-target \
  mununu-dev make ci

# production CLI/server
docker build -f docker/Dockerfile -t mununu .
docker run -p 8080:8080 mununu server --addr 0.0.0.0:8080

# production extract
docker build -f docker/Dockerfile.extract -t mununu-extract .
docker run --rm -v $(pwd):/work mununu-extract \
  /work/config.extract.json --source /work/server.ts --output /work/spec.espec.json
```

For RTL counterexample-trace validation (Verilator), use the sibling
`hw-verif:latest` image from `../hw-verification-uba`. It is intentionally
kept out of `Dockerfile.dev` to avoid a ~900 MB OSS-CAD-Suite layer that
the Rust workspace itself does not need.
