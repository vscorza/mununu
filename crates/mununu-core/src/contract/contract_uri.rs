//! `contract://` URI parser — Document D §D.5 + §D.2.
//!
//! `@mununu_interface` annotations carry a small URI grammar:
//!
//! ```text
//! contract://<domain>/<name>[@<version>][?alt=<alternative>]
//! ```
//!
//! Examples:
//! - `contract://rtl_protocol/axi4_slave`
//! - `contract://rtl_protocol/axi4_slave@2.0.1`
//! - `contract://rtl_protocol/axi4_slave@2.0.1?alt=strict`
//! - `contract://software_library/lodash.debounce`
//!
//! Anything that does not begin with `contract://` is treated as an
//! opaque sidecar path (filesystem reference) — corpus lookup is
//! skipped for those, but the URI is preserved on the contract object
//! so HITL UX can surface it.

use serde::{Deserialize, Serialize};

/// Parsed shape of a `contract://` URI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractUri {
    /// Corpus domain bucket (`rtl_protocol`, `software_library`, …).
    pub domain: String,
    /// Module / library / class name.
    pub name: String,
    /// Optional pinned version. `None` means "any version, pick best
    /// per the ranker."
    #[serde(default)]
    pub version: Option<String>,
    /// Optional named alternative (`?alt=strict`).
    #[serde(default)]
    pub alternative: Option<String>,
    /// The original URI text, kept for diagnostics and round-trip.
    pub raw: String,
}

/// Parse a `contract://` URI. Returns `None` for any string that does
/// not begin with the scheme — callers can use that to distinguish
/// "corpus reference" from "sidecar path."
///
/// The parser is deliberately permissive: it accepts trailing
/// whitespace, mixed-case scheme, and missing trailing `?alt=` values.
/// Empty domain or empty name yield `None` so callers can flag a
/// malformed URI distinctly from "not a URI at all."
pub fn parse_contract_uri(text: &str) -> Option<ContractUri> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    let prefix = "contract://";
    if !lower.starts_with(prefix) {
        return None;
    }
    let raw = trimmed.to_string();
    let body = &trimmed[prefix.len()..];
    // Split off `?...` query string first so it doesn't pollute the
    // domain/name split.
    let (path, query) = match body.find('?') {
        Some(idx) => (&body[..idx], Some(&body[idx + 1..])),
        None => (body, None),
    };
    // Split `<domain>/<name>[@<version>]` on the first `/`.
    let slash = path.find('/')?;
    let domain = &path[..slash];
    let rest = &path[slash + 1..];
    if domain.is_empty() || rest.is_empty() {
        return None;
    }
    let (name, version) = match rest.find('@') {
        Some(at) => (&rest[..at], Some(rest[at + 1..].to_string())),
        None => (rest, None),
    };
    if name.is_empty() {
        return None;
    }
    let alternative = query.and_then(parse_alt);
    Some(ContractUri {
        domain: domain.to_string(),
        name: name.to_string(),
        version,
        alternative,
        raw,
    })
}

fn parse_alt(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        if k.eq_ignore_ascii_case("alt") {
            let v = it.next()?.trim();
            if v.is_empty() {
                return None;
            }
            return Some(v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_uri() {
        let u = parse_contract_uri("contract://rtl_protocol/axi4_slave").unwrap();
        assert_eq!(u.domain, "rtl_protocol");
        assert_eq!(u.name, "axi4_slave");
        assert_eq!(u.version, None);
        assert_eq!(u.alternative, None);
    }

    #[test]
    fn parses_versioned_uri() {
        let u = parse_contract_uri("contract://rtl_protocol/axi4_slave@2.0.1").unwrap();
        assert_eq!(u.version.as_deref(), Some("2.0.1"));
        assert_eq!(u.alternative, None);
    }

    #[test]
    fn parses_alternative_query() {
        let u = parse_contract_uri("contract://rtl_protocol/axi4_slave@2.0.1?alt=strict").unwrap();
        assert_eq!(u.version.as_deref(), Some("2.0.1"));
        assert_eq!(u.alternative.as_deref(), Some("strict"));
    }

    #[test]
    fn non_scheme_uri_returns_none() {
        assert!(parse_contract_uri("./sidecars/foo.json").is_none());
        assert!(parse_contract_uri("https://example.com").is_none());
        assert!(parse_contract_uri("").is_none());
    }

    #[test]
    fn rejects_missing_name() {
        assert!(parse_contract_uri("contract://rtl_protocol/").is_none());
        assert!(parse_contract_uri("contract:///axi4_slave").is_none());
        assert!(parse_contract_uri("contract://rtl_protocol").is_none());
    }

    #[test]
    fn dotted_software_name_round_trips() {
        let u = parse_contract_uri("contract://software_library/lodash.debounce").unwrap();
        assert_eq!(u.domain, "software_library");
        assert_eq!(u.name, "lodash.debounce");
    }

    #[test]
    fn alt_without_value_is_ignored() {
        let u = parse_contract_uri("contract://rtl_protocol/axi4_slave?alt=").unwrap();
        assert_eq!(u.alternative, None);
    }

    #[test]
    fn trims_whitespace() {
        let u = parse_contract_uri("  contract://rtl_protocol/axi4_slave  ").unwrap();
        assert_eq!(u.domain, "rtl_protocol");
        assert!(u.raw.starts_with("contract://"));
    }
}
