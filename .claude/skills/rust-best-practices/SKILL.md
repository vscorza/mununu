---
name: rust-review
description: >
  Reviews Rust code for idiomatic patterns, ownership/borrowing correctness,
  error handling, performance, and adapter architecture compliance.
  Use when asked to review, audit, or check Rust code quality.
---

Perform a Rust best-practices review of $ARGUMENTS (or changed files via `git diff --name-only HEAD~1 HEAD | grep '\.rs$'` if no args).

This is a Cargo workspace with three crates: `mununu-core` (verification engine), `mununu-cli` (CLI), and `mununu-extract` (AST extraction via tree-sitter).

## Review Checklist

1. **Ownership & Borrowing**: flag unnecessary `.clone()`, lifetime issues, `Rc`/`RefCell` overuse. The adapter system passes `&SharedIR` — check adapters don't take ownership unnecessarily.

2. **Error Handling**: library code must use `thiserror` and `?` propagation. No bare `unwrap()` / `expect()` in `mununu-core` or `mununu-extract`. CLI code (`mununu-cli`) may use `expect()` with descriptive messages at top-level entry points only.

3. **Idiomatic Patterns**: iterators over manual loops, `Option`/`Result` combinators, `impl Trait` for return types. Check adapter implementations follow the delegation pattern (parse native format -> build IR -> emit CTXDSL).

4. **Performance**: avoid allocations in BDD/LTL/mu-calculus hot paths (`clts/`, `ltl/`, `mu_calculus/`). Check for O(n^2) patterns in state enumeration. `bitvec` and `smallvec` should be preferred over `Vec<bool>` and small `Vec<T>`.

5. **API Guidelines**: snake_case functions, CamelCase types. Builder pattern for complex config structs. `into_*` for ownership-consuming conversions, `as_*` for cheap borrows.

6. **Unsafe Code**: every `unsafe` block must have a `// SAFETY:` comment. Tree-sitter FFI in `mununu-extract` is the primary expected location — flag any unsafe elsewhere.

7. **Adapter Architecture**: adapters in `src/adapter/` must:
   - Implement the shared IR pipeline (parse -> IR -> emit)
   - Include unit tests per construct, integration tests, and regression tests
   - Not duplicate logic already in the shared IR emitter

8. **Feature Flags**: check that optional dependencies (`tokio`, `axum`, `tower`) are gated behind the `api` feature flag. No unconditional heavy dependencies.

## Output Format

Group findings as: **Critical** / **Warning** / **Suggestion** — with `file_path:line_number` references.
