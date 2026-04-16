---
name: security-audit
description: >
  Security audit of Rust code for vulnerabilities, unsafe usage,
  dependency risks, and API exposure.
  Use when asked to audit, check security, or review for vulnerabilities.
---

Perform a security audit of $ARGUMENTS (or the full workspace if no args).

## Audit Checklist

1. **Unsafe Code**: review every `unsafe` block for memory safety justification. Expected locations:
   - Tree-sitter FFI in `mununu-extract` — verify all pointer dereferences are bounded
   - Flag any `unsafe` outside of FFI boundaries

2. **Command Injection**: check any use of `std::process::Command` — arguments must not come from unsanitized user input. The CLI parses CTXDSL files; verify file paths are validated.

3. **Path Traversal**: CTXDSL `@import` or file includes must not allow `../` escapes outside the workspace. Check `context_dsl/` parser for path handling.

4. **Denial of Service**:
   - BDD construction: check for exponential blowup guards (state count limits)
   - LTL formula depth: verify recursion depth limits exist
   - Input size limits on API endpoints (if `api` feature is enabled)

5. **API Security** (behind `api` feature flag):
   - CORS configuration in `axum` setup — must not be `*` in production
   - No authentication tokens logged or included in error responses
   - Request body size limits on `axum` routes
   - Timeout enforcement on long-running synthesis operations

6. **Secrets & Credentials**: search with Grep for hardcoded keys, tokens, passwords. Check `.gitignore` covers `.env` files.

7. **Dependencies**:
   - Check `Cargo.lock` exists and is committed
   - Flag any dependencies without pinned versions
   - Note any dependencies with known advisories (check `cargo audit` output if available)

8. **Serialization**: `serde_json` deserialization of adapter input (XState JSON, CrewAI JSON, etc.) — check for:
   - Size limits on deserialized structures
   - No `#[serde(deny_unknown_fields)]` missing on public-facing types
   - No unbounded `Vec` or `HashMap` from untrusted input

9. **Integer Overflow**: arithmetic on state counts, label counts — verify `checked_*` or saturating ops where counts come from user input.

## Output Format

Severity: **Critical** (fix now) / **High** / **Medium** / **Informational** — with file:line references.
