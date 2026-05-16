//! Alphabet-binding layer (A2.2 of the verify framework).
//!
//! Sits between [`crate::verify::config::VerifyConfig`] and the
//! orchestrator (A2.4). Given a parsed config and the working directory
//! the user invoked `mununu verify` from, this module produces an
//! [`AlphabetBinding`] value the orchestrator drives to rewrite each
//! source's emitted CTXDSL into a unified label alphabet before
//! composition.
//!
//! ## Strategies
//!
//! - **[`AlphabetBinding::Direct`]** — no rewriting. Labels must
//!   already match across sources. Default. Right when the user
//!   authored matching names by hand or when adapters all emit on a
//!   shared canonical alphabet (e.g., two XState machines using the
//!   same event names).
//!
//! - **[`AlphabetBinding::Renamings`]** — explicit `<source_id>.<local>
//!   → <canonical>` map. The binding splits the global renaming list
//!   into per-source maps via [`AlphabetBinding::per_source_renamings`]
//!   that the orchestrator applies to each source's emitted CTXDSL.
//!
//! - **[`AlphabetBinding::RegisterMap`]** — carries a parsed
//!   [`crate::codesign::register_map::RegisterMap`]. The orchestrator
//!   (A2.4) is responsible for deciding which source ID plays the
//!   firmware role and which plays the peripheral role, then deriving
//!   per-source renamings from the register map's
//!   `sv_signal` / `c_accessor` fields via
//!   [`crate::codesign::coupling::rendezvous_label_name`]. The binding
//!   here only loads the JSON and exposes it.
//!
//! ## Rewriting
//!
//! [`apply_renamings_to_ctxdsl`] is a word-boundary textual rewriter:
//! every occurrence of a renaming's `from` identifier that appears at a
//! token boundary is replaced with the corresponding `to` value. This
//! is **deliberately simpler than a structural CTXDSL rewrite**:
//!
//! - it's adapter-agnostic — the orchestrator hands the rewriter
//!   whatever CTXDSL the source's adapter emitted, without needing
//!   per-adapter knowledge of which identifiers carry labels.
//! - it's predictable — `\b<from>\b → <to>` is a single Perl-style
//!   substitution per renaming.
//! - it has a known soundness footnote: if a `from` identifier
//!   collides with a state or variable name *inside the same source*,
//!   the rewriter will also rewrite that occurrence. The
//!   [`lint_renamings_for_collisions`] helper surfaces the collision
//!   for the orchestrator to escalate as a warning or hard error.
//!
//! The orchestrator (A2.4) is expected to: (a) build the per-source
//! renaming map, (b) call [`lint_renamings_for_collisions`] on each
//! source's CTXDSL to flag collisions, (c) call
//! [`apply_renamings_to_ctxdsl`] to do the actual rewriting.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::codesign::register_map::RegisterMap;
use crate::verify::config::{AlphabetSection, Renaming, VerifyConfig};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The resolved alphabet-binding strategy.
///
/// Constructed from [`VerifyConfig::alphabet`] via
/// [`AlphabetBinding::from_config`]. For [`AlphabetBinding::RegisterMap`]
/// the constructor reads + parses the JSON sidecar at the configured
/// path.
#[derive(Debug, Clone)]
pub enum AlphabetBinding {
    /// No rewriting. Sources must agree on label names already.
    Direct,
    /// Explicit `<source_id>.<local_label> → <canonical_label>`
    /// renamings. Use [`AlphabetBinding::per_source_renamings`] to
    /// project into per-source maps.
    Renamings { renamings: Vec<Renaming> },
    /// Register-map-derived renamings (firmware ↔ peripheral
    /// rendezvous). The orchestrator drives the actual rewriting,
    /// since deciding which source plays which role requires
    /// adapter-aware logic beyond the scope of this module.
    RegisterMap {
        map: RegisterMap,
        path: PathBuf,
        /// Mirrors `verify::config::AlphabetSection.allow_peripheral_superset`.
        allow_peripheral_superset: bool,
    },
}

/// Per-source renaming maps. Keyed by source id; each value maps
/// `local_label_name → canonical_label_name`.
pub type PerSourceRenamings = BTreeMap<String, BTreeMap<String, String>>;

/// Diagnostics surfaced by [`lint_renamings_for_collisions`] when a
/// renaming's `from` identifier would collide with a non-label token
/// (state name, variable, identifier in a guard expression, …) in
/// the source's CTXDSL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenamingCollision {
    /// The `from` identifier being rewritten.
    pub from: String,
    /// The `to` identifier we'd rewrite it to.
    pub to: String,
    /// A short hint about which token kind we suspect is colliding.
    /// E.g. `"state"`, `"variable"`, `"identifier"`.
    pub colliding_kind: String,
    /// 1-indexed source line where the collision is suspected.
    pub source_line: usize,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors raised by [`AlphabetBinding::from_config`].
#[derive(Debug)]
pub enum BindingError {
    /// Failed to read the register-map JSON from disk.
    RegisterMapReadFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to parse the register-map JSON.
    RegisterMapParseFailed { path: PathBuf, message: String },
    /// The `register_map` strategy was selected but the config didn't
    /// supply a path. The validator should have caught this; if it
    /// didn't, the binding raises it explicitly.
    RegisterMapStrategyWithoutPath,
    /// The strategy name in the config isn't one of the known
    /// variants. The validator should have caught this too.
    UnknownStrategy(String),
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindingError::RegisterMapReadFailed { path, source } => write!(
                f,
                "failed to read register-map sidecar at {}: {source}",
                path.display()
            ),
            BindingError::RegisterMapParseFailed { path, message } => write!(
                f,
                "failed to parse register-map sidecar at {}: {message}",
                path.display()
            ),
            BindingError::RegisterMapStrategyWithoutPath => write!(
                f,
                "alphabet strategy `register_map` requires a `register_map` path in the config"
            ),
            BindingError::UnknownStrategy(s) => {
                write!(
                    f,
                    "unknown alphabet strategy `{s}` (valid: direct, renamings, register_map)"
                )
            }
        }
    }
}

impl std::error::Error for BindingError {}

// ---------------------------------------------------------------------------
// Constructor
// ---------------------------------------------------------------------------

impl AlphabetBinding {
    /// Build an `AlphabetBinding` from a parsed [`VerifyConfig`].
    ///
    /// `base_dir` is the directory the user invoked `mununu verify`
    /// from; relative register-map paths in the config are resolved
    /// against it. For the `register_map` strategy, the JSON sidecar
    /// is loaded eagerly so the orchestrator can fail-fast on a
    /// missing or malformed file.
    pub fn from_config(config: &VerifyConfig, base_dir: &Path) -> Result<Self, BindingError> {
        Self::from_alphabet_section(&config.alphabet, base_dir)
    }

    /// Lower-level entry point — most callers want
    /// [`AlphabetBinding::from_config`].
    pub fn from_alphabet_section(
        alphabet: &AlphabetSection,
        base_dir: &Path,
    ) -> Result<Self, BindingError> {
        match alphabet.strategy.as_str() {
            "direct" => Ok(AlphabetBinding::Direct),
            "renamings" => Ok(AlphabetBinding::Renamings {
                renamings: alphabet.renamings.clone(),
            }),
            "register_map" => {
                let path_rel = alphabet
                    .register_map
                    .as_ref()
                    .ok_or(BindingError::RegisterMapStrategyWithoutPath)?;
                let path = if path_rel.is_absolute() {
                    path_rel.clone()
                } else {
                    base_dir.join(path_rel)
                };
                let bytes =
                    std::fs::read(&path).map_err(|source| BindingError::RegisterMapReadFailed {
                        path: path.clone(),
                        source,
                    })?;
                let map: RegisterMap = serde_json::from_slice(&bytes).map_err(|e| {
                    BindingError::RegisterMapParseFailed {
                        path: path.clone(),
                        message: e.to_string(),
                    }
                })?;
                Ok(AlphabetBinding::RegisterMap {
                    map,
                    path,
                    allow_peripheral_superset: alphabet.allow_peripheral_superset,
                })
            }
            other => Err(BindingError::UnknownStrategy(other.to_string())),
        }
    }

    /// Project the binding into per-source renaming maps.
    ///
    /// - `Direct`: every source's map is empty (passthrough).
    /// - `Renamings`: each `<source_id>.<local> → <canonical>` entry
    ///   is filed under `source_id`. Malformed `from` entries are
    ///   silently skipped (the config validator already rejects them
    ///   with `MalformedRenamingFrom`).
    /// - `RegisterMap`: returns an empty map. The orchestrator is
    ///   responsible for deriving register-map-driven renamings since
    ///   it knows which source ID plays the firmware vs peripheral
    ///   role.
    pub fn per_source_renamings(&self) -> PerSourceRenamings {
        let mut out: PerSourceRenamings = BTreeMap::new();
        match self {
            AlphabetBinding::Direct => {}
            AlphabetBinding::Renamings { renamings } => {
                for r in renamings {
                    if let Some((sid, local)) = parse_qualified(&r.from) {
                        out.entry(sid.to_string())
                            .or_default()
                            .insert(local.to_string(), r.to.clone());
                    }
                }
            }
            AlphabetBinding::RegisterMap { .. } => {}
        }
        out
    }
}

/// Parse `<source_id>.<local_label>`. Mirrors the helper in
/// `verify::config`.
fn parse_qualified(s: &str) -> Option<(&str, &str)> {
    let (sid, local) = s.split_once('.')?;
    if sid.is_empty() || local.is_empty() {
        return None;
    }
    Some((sid, local))
}

// ---------------------------------------------------------------------------
// Rewriter
// ---------------------------------------------------------------------------

/// Rewrite every word-boundary occurrence of each renaming's `from`
/// key in `ctxdsl` with the corresponding `to` value.
///
/// Word-boundary semantics match Rust regex's `\b` — token edges in
/// CTXDSL identifiers (`[A-Za-z_][A-Za-z0-9_]*`). Renamings are
/// applied in sorted-key order to keep output deterministic; multiple
/// renamings that target overlapping substrings are independent (each
/// rewrites against the *original* text, not the running result, via
/// regex's longest-match-first behaviour over a combined alternation).
///
/// Returns the rewritten CTXDSL.
pub fn apply_renamings_to_ctxdsl(ctxdsl: &str, renamings: &BTreeMap<String, String>) -> String {
    if renamings.is_empty() {
        return ctxdsl.to_string();
    }

    // Build one regex alternation over all `from` keys. Longer keys
    // come first so a key like `tx_start_busy` doesn't get rewritten
    // by a shorter colliding key `tx_start` if both exist.
    let mut keys: Vec<&str> = renamings.keys().map(String::as_str).collect();
    keys.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    let alternation = keys
        .iter()
        .map(|k| regex::escape(k))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = format!(r"\b(?:{alternation})\b");
    let re = Regex::new(&pattern).expect("rewriter regex is well-formed by construction");

    re.replace_all(ctxdsl, |caps: &regex::Captures<'_>| {
        let matched = &caps[0];
        renamings
            .get(matched)
            .cloned()
            .unwrap_or_else(|| matched.to_string())
    })
    .into_owned()
}

/// Scan a CTXDSL fragment for non-label uses of any renaming key.
///
/// Reports a [`RenamingCollision`] for each occurrence of a renaming
/// `from` key that appears in a context where it is *probably not* a
/// label declaration or reference. Heuristic — true negatives are
/// possible (a label collides with a state name and we miss it),
/// but false positives are also possible (we flag a true label
/// reference that happens to look like a state). The orchestrator
/// surfaces these as warnings, not errors, by default.
///
/// Token contexts checked (most-common-first; not exhaustive):
///   - `state <NAME>` — a state declaration whose name matches a
///     renaming's `from`.
///   - `variable <NAME>` / `var <NAME>` — local variable declarations.
///
/// Label declarations (`label <NAME>`) and label references after `on`
/// (`on label <NAME>` or `on label <NAME>, label <NAME>`) and modal
/// guards (`<NAME>` / `[NAME]` inside μ-formulas) are *not* flagged
/// — they're the intended rewrite sites.
pub fn lint_renamings_for_collisions(
    ctxdsl: &str,
    renamings: &BTreeMap<String, String>,
) -> Vec<RenamingCollision> {
    if renamings.is_empty() {
        return Vec::new();
    }
    let from_set: BTreeSet<&str> = renamings.keys().map(String::as_str).collect();
    let mut collisions = Vec::new();

    // Patterns we treat as colliding tokens — the captured identifier
    // is in group 1.
    let state_decl = Regex::new(r"\bstate\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let var_decl = Regex::new(r"\b(?:variable|var)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();

    for (line_idx, line) in ctxdsl.lines().enumerate() {
        for caps in state_decl.captures_iter(line) {
            let ident = &caps[1];
            if from_set.contains(ident) {
                collisions.push(RenamingCollision {
                    from: ident.to_string(),
                    to: renamings.get(ident).cloned().unwrap_or_default(),
                    colliding_kind: "state".to_string(),
                    source_line: line_idx + 1,
                });
            }
        }
        for caps in var_decl.captures_iter(line) {
            let ident = &caps[1];
            if from_set.contains(ident) {
                collisions.push(RenamingCollision {
                    from: ident.to_string(),
                    to: renamings.get(ident).cloned().unwrap_or_default(),
                    colliding_kind: "variable".to_string(),
                    source_line: line_idx + 1,
                });
            }
        }
    }
    collisions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::config::VerifyConfig;
    use std::path::PathBuf;

    fn renamings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Constructor
    // -----------------------------------------------------------------------

    #[test]
    fn direct_strategy_yields_direct_binding() {
        let cfg = VerifyConfig::from_toml(
            r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "ctxdsl"
files = ["a.ctxdsl"]

[alphabet]
strategy = "direct"

[composition]
semantics = "synchronous"
members = ["a"]
"#,
        )
        .unwrap();
        let binding = AlphabetBinding::from_config(&cfg, &PathBuf::from("/tmp")).unwrap();
        assert!(matches!(binding, AlphabetBinding::Direct));
        assert!(binding.per_source_renamings().is_empty());
    }

    #[test]
    fn renamings_strategy_projects_to_per_source_map() {
        let cfg = VerifyConfig::from_toml(
            r#"
[project]
name = "x"

[[sources]]
id = "fw"
adapter = "ctxdsl"
files = ["fw.ctxdsl"]

[[sources]]
id = "p"
adapter = "ctxdsl"
files = ["p.ctxdsl"]

[[alphabet.renamings]]
from = "fw.tx_start"
to = "wr_ctrl_tx_start"

[[alphabet.renamings]]
from = "fw.tx_busy"
to = "rd_status_tx_busy"

[[alphabet.renamings]]
from = "p.ctrl_tx_start"
to = "wr_ctrl_tx_start"

[alphabet]
strategy = "renamings"

[composition]
semantics = "asynchronous"
members = ["fw", "p"]
"#,
        )
        .unwrap();
        let binding = AlphabetBinding::from_config(&cfg, &PathBuf::from("/tmp")).unwrap();
        let per_source = binding.per_source_renamings();
        assert_eq!(per_source.len(), 2);
        let fw_map = per_source.get("fw").unwrap();
        assert_eq!(
            fw_map.get("tx_start").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
        assert_eq!(
            fw_map.get("tx_busy").map(String::as_str),
            Some("rd_status_tx_busy")
        );
        let p_map = per_source.get("p").unwrap();
        assert_eq!(
            p_map.get("ctrl_tx_start").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
    }

    #[test]
    fn register_map_strategy_loads_json_eagerly() {
        let temp = tempfile::tempdir().unwrap();
        let rm_path = temp.path().join("rm.json");
        // Minimal register-map JSON sufficient for round-trip parse.
        std::fs::write(
            &rm_path,
            r#"{
                "peripheral": "UART",
                "base_address": "0x40010000",
                "registers": [
                    {
                        "name": "CTRL",
                        "offset": 0,
                        "width_bits": 32,
                        "direction": "RW",
                        "visibility_class": "control",
                        "access_path": "mmio_direct",
                        "fields": [
                            { "name": "tx_start", "bits": [0, 0], "sv_signal": "ctrl_reg[0]", "c_accessor": "UART->CTRL.tx_start" }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();
        let cfg = VerifyConfig::from_toml(&format!(
            r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "ctxdsl"
files = ["a.ctxdsl"]

[alphabet]
strategy = "register_map"
register_map = "{rm_filename}"

[composition]
semantics = "asynchronous"
members = ["a"]
"#,
            rm_filename = "rm.json"
        ))
        .unwrap();
        let binding = AlphabetBinding::from_config(&cfg, temp.path()).unwrap();
        match binding {
            AlphabetBinding::RegisterMap {
                map,
                path,
                allow_peripheral_superset,
            } => {
                assert_eq!(map.peripheral, "UART");
                assert_eq!(map.registers.len(), 1);
                assert_eq!(path, rm_path);
                assert!(!allow_peripheral_superset);
            }
            _ => panic!("expected RegisterMap binding"),
        }
    }

    #[test]
    fn register_map_strategy_propagates_read_error() {
        let cfg = VerifyConfig::from_toml(
            r#"
[project]
name = "x"

[[sources]]
id = "a"
adapter = "ctxdsl"
files = ["a.ctxdsl"]

[alphabet]
strategy = "register_map"
register_map = "does_not_exist.json"

[composition]
semantics = "asynchronous"
members = ["a"]
"#,
        )
        .unwrap();
        let err = AlphabetBinding::from_config(&cfg, &PathBuf::from("/tmp")).unwrap_err();
        assert!(matches!(err, BindingError::RegisterMapReadFailed { .. }));
    }

    #[test]
    fn register_map_strategy_without_path_errors() {
        let alphabet = AlphabetSection {
            strategy: "register_map".to_string(),
            renamings: Vec::new(),
            register_map: None,
            allow_peripheral_superset: false,
        };
        let err =
            AlphabetBinding::from_alphabet_section(&alphabet, &PathBuf::from("/tmp")).unwrap_err();
        assert!(matches!(err, BindingError::RegisterMapStrategyWithoutPath));
    }

    // -----------------------------------------------------------------------
    // Rewriter
    // -----------------------------------------------------------------------

    #[test]
    fn empty_renamings_passes_through() {
        let ctxdsl = "context x { alphabet { label foo; } }";
        assert_eq!(apply_renamings_to_ctxdsl(ctxdsl, &BTreeMap::new()), ctxdsl);
    }

    #[test]
    fn rewrites_label_declaration() {
        let ctxdsl = "alphabet { label tx_start; label tx_busy; }";
        let out = apply_renamings_to_ctxdsl(
            ctxdsl,
            &renamings(&[
                ("tx_start", "wr_ctrl_tx_start"),
                ("tx_busy", "rd_status_tx_busy"),
            ]),
        );
        assert!(out.contains("label wr_ctrl_tx_start"));
        assert!(out.contains("label rd_status_tx_busy"));
        assert!(!out.contains("label tx_start;"));
        assert!(!out.contains("label tx_busy;"));
    }

    #[test]
    fn rewrites_transition_on_clause() {
        let ctxdsl = "transition S -> T on label tx_start;";
        let out =
            apply_renamings_to_ctxdsl(ctxdsl, &renamings(&[("tx_start", "wr_ctrl_tx_start")]));
        assert_eq!(out, "transition S -> T on label wr_ctrl_tx_start;");
    }

    #[test]
    fn rewrites_multi_label_transition() {
        let ctxdsl = "transition S -> T on label tx_start, label tx_busy;";
        let out = apply_renamings_to_ctxdsl(
            ctxdsl,
            &renamings(&[
                ("tx_start", "wr_ctrl_tx_start"),
                ("tx_busy", "rd_status_tx_busy"),
            ]),
        );
        assert_eq!(
            out,
            "transition S -> T on label wr_ctrl_tx_start, label rd_status_tx_busy;"
        );
    }

    #[test]
    fn does_not_rewrite_inside_other_identifiers() {
        // `tx_start_extended` is a longer identifier; the regex word
        // boundary must not match inside it.
        let ctxdsl = "label tx_start_extended; transition S -> T on label tx_start;";
        let out =
            apply_renamings_to_ctxdsl(ctxdsl, &renamings(&[("tx_start", "wr_ctrl_tx_start")]));
        assert!(out.contains("label tx_start_extended;"));
        assert!(out.contains("label wr_ctrl_tx_start"));
    }

    #[test]
    fn longer_renaming_keys_win_over_shorter_substring_keys() {
        // Both keys defined; the longer one must match first so
        // `prefix_long` doesn't get rewritten by the shorter `prefix`.
        let ctxdsl = "label prefix; label prefix_long;";
        let out = apply_renamings_to_ctxdsl(
            ctxdsl,
            &renamings(&[("prefix", "P"), ("prefix_long", "PL")]),
        );
        assert!(out.contains("label P;"));
        assert!(out.contains("label PL;"));
        assert!(!out.contains("P_long")); // shorter must not have eaten the prefix
    }

    #[test]
    fn rewriter_is_deterministic() {
        let ctxdsl = "alphabet { label a; label b; label c; } transition S -> T on label a, label b, label c;";
        let map = renamings(&[("a", "alpha"), ("b", "beta"), ("c", "gamma")]);
        let r1 = apply_renamings_to_ctxdsl(ctxdsl, &map);
        let r2 = apply_renamings_to_ctxdsl(ctxdsl, &map);
        assert_eq!(r1, r2);
        assert!(r1.contains("label alpha"));
        assert!(r1.contains("label beta"));
        assert!(r1.contains("label gamma"));
    }

    // -----------------------------------------------------------------------
    // Lint
    // -----------------------------------------------------------------------

    #[test]
    fn no_collisions_in_clean_ctxdsl() {
        let ctxdsl = "alphabet { label foo; } states { state S0; }";
        let collisions =
            lint_renamings_for_collisions(ctxdsl, &renamings(&[("foo", "canonical_foo")]));
        assert!(collisions.is_empty());
    }

    #[test]
    fn flags_state_name_collision() {
        let ctxdsl = "states {\n    state tx_start initial;\n    state Idle;\n}";
        let collisions =
            lint_renamings_for_collisions(ctxdsl, &renamings(&[("tx_start", "wr_ctrl_tx_start")]));
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].from, "tx_start");
        assert_eq!(collisions[0].to, "wr_ctrl_tx_start");
        assert_eq!(collisions[0].colliding_kind, "state");
        assert_eq!(collisions[0].source_line, 2);
    }

    #[test]
    fn flags_variable_name_collision() {
        let ctxdsl = "variables {\n    variable tx_busy : int = 0;\n}";
        let collisions =
            lint_renamings_for_collisions(ctxdsl, &renamings(&[("tx_busy", "rd_status_tx_busy")]));
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].colliding_kind, "variable");
        assert_eq!(collisions[0].source_line, 2);
    }

    #[test]
    fn ignores_label_declarations_and_references() {
        let ctxdsl = "alphabet { label tx_start; }\ntransitions {\n    transition S -> T on label tx_start;\n}";
        let collisions =
            lint_renamings_for_collisions(ctxdsl, &renamings(&[("tx_start", "wr_ctrl_tx_start")]));
        assert!(collisions.is_empty());
    }
}
