//! Library of parameterised CTXDSL component templates (plan Part 6
//! item 7).
//!
//! Ships a small, growing set of standard hardware-modelling
//! templates — PLIC, watchdog, tracked-memory — that every multicore
//! verification project re-derives by hand. Each template is a
//! `.ctxdsl.tpl` file with `{instance_id}` placeholders matching the
//! verify-framework's parameterised-instance substitution (plan Part
//! 6 item 6).
//!
//! ## Usage
//!
//! From the CLI:
//!
//! ```bash
//! mununu library list
//! mununu library emit plic --instance-id ext_7 > examples/my_plic.ctxdsl
//! mununu library emit watchdog --instance-id dma_wd > examples/my_wd.ctxdsl
//! mununu library emit tracked_memory --instance-id buffer_0 > examples/my_mem.ctxdsl
//! ```
//!
//! Or programmatically by referencing the source via the verify
//! framework's `count = N` field and pointing `files = ["plic.ctxdsl.tpl"]`
//! after `cp $(mununu library path)/plic.ctxdsl.tpl .` — every
//! instance gets a fresh `{instance_id}` substitution at dispatch
//! time.

use std::path::PathBuf;

/// One library template. The `body` is the raw CTXDSL text with
/// `{instance_id}` placeholders (substituted by the verify orchestrator
/// at dispatch time when the source is consumed under `count = N`).
#[derive(Debug, Clone)]
pub struct LibraryTemplate {
    /// Canonical identifier (`plic`, `watchdog`, `tracked_memory`, …).
    pub name: &'static str,
    /// One-line description for `mununu library list`.
    pub summary: &'static str,
    /// Raw CTXDSL template body.
    pub body: &'static str,
}

/// Enumerate every shipped library template.
pub fn templates() -> &'static [LibraryTemplate] {
    &TEMPLATES
}

/// Look up a template by name. Returns `None` if no template with
/// that name is shipped.
pub fn lookup(name: &str) -> Option<&'static LibraryTemplate> {
    TEMPLATES.iter().find(|t| t.name == name)
}

/// Emit a template with `{instance_id}` substituted by the given
/// `instance_id` argument. When `instance_id` is `None`, the placeholder
/// is preserved verbatim — useful if the caller intends to feed the
/// output to a `[[sources]]` block with `count = N` (the verify
/// framework will do the substitution per instance).
pub fn emit(template: &LibraryTemplate, instance_id: Option<&str>) -> String {
    match instance_id {
        Some(id) => template.body.replace("{instance_id}", id),
        None => template.body.to_string(),
    }
}

/// Workspace-relative path to the library directory. Tries the
/// `CARGO_MANIFEST_DIR`-relative path first (works in dev / tests),
/// then a `share/mununu/library/` fallback for installed binaries.
pub fn library_path() -> PathBuf {
    let candidates: &[PathBuf] = &[
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("library"),
        PathBuf::from("crates/mununu-core/library"),
        PathBuf::from("../share/mununu/library"),
    ];
    for c in candidates {
        if c.exists() {
            return c.clone();
        }
    }
    candidates[0].clone()
}

// ---------------------------------------------------------------------------
// Templates (compiled in via include_str! — single binary, no runtime
// file dependency).
// ---------------------------------------------------------------------------

const PLIC_BODY: &str = include_str!("../library/plic.ctxdsl.tpl");
const WATCHDOG_BODY: &str = include_str!("../library/watchdog.ctxdsl.tpl");
const TRACKED_MEMORY_BODY: &str = include_str!("../library/tracked_memory.ctxdsl.tpl");

const TEMPLATES: [LibraryTemplate; 3] = [
    LibraryTemplate {
        name: "plic",
        summary: "RISC-V PLIC interrupt-controller stub (one tracked source × one observer).",
        body: PLIC_BODY,
    },
    LibraryTemplate {
        name: "watchdog",
        summary: "Watchdog timer (Disabled / Armed / Expired) with kick + clear + expire labels.",
        body: WATCHDOG_BODY,
    },
    LibraryTemplate {
        name: "tracked_memory",
        summary: "Single-address memory tracker (Initial / Written) with wr / rd / fence labels.",
        body: TRACKED_MEMORY_BODY,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_templates_shipped() {
        let all: Vec<&str> = templates().iter().map(|t| t.name).collect();
        assert_eq!(all, vec!["plic", "watchdog", "tracked_memory"]);
    }

    #[test]
    fn lookup_returns_a_template_for_each_shipped_name() {
        for t in templates() {
            assert!(lookup(t.name).is_some(), "lookup failed for {}", t.name);
        }
    }

    #[test]
    fn lookup_returns_none_for_unknown_names() {
        assert!(lookup("nonsense").is_none());
    }

    #[test]
    fn emit_substitutes_placeholder_when_instance_id_is_set() {
        let t = lookup("plic").unwrap();
        let out = emit(t, Some("ext_7"));
        assert!(out.contains("PLIC_ext_7"));
        assert!(!out.contains("{instance_id}"));
    }

    #[test]
    fn emit_preserves_placeholder_when_instance_id_is_none() {
        let t = lookup("watchdog").unwrap();
        let out = emit(t, None);
        assert!(out.contains("{instance_id}"));
    }

    #[test]
    fn every_template_uses_the_canonical_placeholder() {
        for t in templates() {
            assert!(
                t.body.contains("{instance_id}"),
                "template `{}` does not contain `{{instance_id}}` — does it deliberately not parameterise?",
                t.name
            );
        }
    }

    #[test]
    fn substituted_plic_parses_as_ctxdsl() {
        let t = lookup("plic").unwrap();
        let out = emit(t, Some("test"));
        let parsed = crate::context_dsl::parser::parse(&out);
        assert!(parsed.is_ok(), "PLIC emit failed to parse: {parsed:?}");
    }

    #[test]
    fn substituted_watchdog_parses_as_ctxdsl() {
        let t = lookup("watchdog").unwrap();
        let out = emit(t, Some("test"));
        let parsed = crate::context_dsl::parser::parse(&out);
        assert!(parsed.is_ok(), "Watchdog emit failed to parse: {parsed:?}");
    }

    #[test]
    fn substituted_tracked_memory_parses_as_ctxdsl() {
        let t = lookup("tracked_memory").unwrap();
        let out = emit(t, Some("buffer_0"));
        let parsed = crate::context_dsl::parser::parse(&out);
        assert!(
            parsed.is_ok(),
            "TrackedMemory emit failed to parse: {parsed:?}"
        );
    }
}
