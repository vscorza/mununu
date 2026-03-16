# Security and Dependency Management

This document describes the security practices and dependency management for the Henos project.

## Dependency Auditing

Henos uses automated tools to check for security vulnerabilities and keep dependencies up to date.

### Security Vulnerabilities

We use [`cargo-audit`](https://github.com/rustsec/rustsec/tree/main/cargo-audit) to scan dependencies against the [RustSec Advisory Database](https://rustsec.org/).

**Local check:**
```bash
# Install (one-time)
cargo install cargo-audit --locked

# Run audit
cargo audit
```

**CI Integration:**
- Security audits run automatically on every push and pull request
- Vulnerabilities will **fail the CI build** to ensure they are addressed
- The audit uses the official `rustsec/audit-check` GitHub Action

### Outdated Dependencies

We use [`cargo-outdated`](https://github.com/kbknapp/cargo-outdated) to identify dependencies with newer versions available.

**Local check:**
```bash
# Install (one-time)
cargo install cargo-outdated --locked

# Check for outdated dependencies
cargo outdated
```

**Note:** This project requires Rust 1.91+ (see `rust-version` in `Cargo.toml`). The latest `cargo-outdated` requires Rust 1.91+.

**CI Integration:**
- Outdated dependency checks run automatically in CI
- Reports are **non-blocking** (won't fail the build)
- Helps maintain awareness of available updates

### Automated Script

For convenience, a script is provided that runs both checks:

```bash
./scripts/audit-dependencies.sh
```

This script:
- Checks for security vulnerabilities (fails if found)
- Reports outdated dependencies (informational)
- Provides installation instructions if tools are missing

## Reporting Security Issues

If you discover a security vulnerability in Henos, please report it responsibly:

1. **Do not** open a public GitHub issue
2. Email the maintainer: vscorza@gmail.com
3. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if available)

We will acknowledge receipt within 48 hours and work to address the issue promptly.

## Dependency Update Process

When updating dependencies:

1. **Check for vulnerabilities first:**
   ```bash
   cargo audit
   ```

2. **Update dependencies:**
   ```bash
   # Update all dependencies
   cargo update
   
   # Update a specific dependency
   cargo update -p <package-name>
   
   # Update to a specific version
   cargo update -p <package-name> --precise <version>
   ```

3. **Verify after update:**
   ```bash
   cargo audit  # Check for new vulnerabilities
   cargo test   # Ensure tests still pass
   cargo build  # Ensure code still compiles
   ```

4. **Commit changes:**
   - Update `Cargo.lock` (automatically updated by `cargo update`)
   - Test thoroughly
   - Commit with a clear message

## Best Practices

1. **Minimize dependency tree**: Regularly review and remove unused dependencies
2. **Evaluate new dependencies**: Check maintenance status, activity, and community trust before adding
3. **Keep dependencies updated**: Regularly run `cargo outdated` and update when appropriate
4. **Monitor security advisories**: Subscribe to RustSec announcements for critical vulnerabilities
5. **Use locked versions**: Always commit `Cargo.lock` to ensure reproducible builds

## Resources

- [RustSec Advisory Database](https://rustsec.org/)
- [cargo-audit Documentation](https://github.com/rustsec/rustsec/tree/main/cargo-audit)
- [cargo-outdated Documentation](https://github.com/kbknapp/cargo-outdated)
- [Rust Security Working Group](https://www.rust-lang.org/governance/wgs/wg-security)
