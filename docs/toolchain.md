# Toolchain

> Concept: how the Rust version is pinned across host, container, and CI — and what clippy patterns to keep in mind so toolchain bumps don't break the gate.

## Where the version lives

The workspace pins an exact Rust version in two places:

- [`rust-toolchain.toml`](../rust-toolchain.toml) — `channel = "1.95.0"`. rustup honors this at runtime, so host devs, CI, and the dev container all use the same toolchain.
- [`docker/Dockerfile.dev`](../docker/Dockerfile.dev) — `ARG RUST_VERSION=1.95`. Mirrors the toolchain pin so the base image's bundled toolchain matches.

Edition is **2024** (set in each `Cargo.toml`).

## Bumping the toolchain

A toolchain bump is a two-file edit plus a `make ci` validation, all in one commit:

1. Edit `rust-toolchain.toml`'s `channel`.
2. Edit the matching `ARG RUST_VERSION` in `docker/Dockerfile.dev`.
3. Run `make ci` locally. New clippy lints often surface on bumps — fixing them is part of the bump commit, not a follow-up.

**Drift between the two pins is silent.** rustup will auto-download the toolchain that `rust-toolchain.toml` names regardless of the image's bundled version, so CI may pass against a "wrong" toolchain. Treat any PR that touches one pin but not the other as a review red flag.

Before committing the pin change, run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

against the new toolchain so any new lint failures land in the same commit.

## Clippy compatibility patterns

The pinned toolchain locks the lint set so contributors don't get bitten by surprise lints from upstream Rust releases. Keep these patterns in mind when writing new code — each has bitten a past bump:

- **`unnecessary_unwrap`.** After `x.is_some()`, use `if let Some(v) = x` rather than `x.unwrap()`.
- **`needless_return`.** Don't use explicit `return` at the end of a function body.
- **`redundant_closure`.** Use `foo` instead of `|x| foo(x)` when passing to `.map()` and friends.
- **`collapsible_match`** (Rust 1.95+). A nested `if` inside a `match` arm should become an arm guard — `Pat if cond => { ... }` rather than `Pat => { if cond { ... } }`.
- **`implicit_borrowing`** (Edition 2024). Closure patterns like `|(&(a, _), _)|` are not allowed in Rust 2024 — use `|((a, _), _)| *a` instead.

For the most recent bumps and what they revealed, check the git log on `rust-toolchain.toml`.
