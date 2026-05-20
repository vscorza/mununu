//! [`DepGraphBuilder`] implementation for extraction specs.
//!
//! Extraction specs (`*.espec.json`) describe state at two granularities:
//!
//! 1. **Source-anchored fields** in `state_fields[]` — name + abstraction
//!    declarations for class/struct members extracted from real source.
//!    Methods carry `guards[]` / `effects[]` JSON arrays that reference
//!    these field names; the dep graph reads those references back.
//! 2. **Declarative-only automata** in `model_config.automata[]` — fully
//!    spelled-out state names and transitions. Most production fixtures
//!    use *only* this form, leaving `state_fields[]` empty.
//!
//! This trait impl targets form (1). When `state_fields[]` is empty
//! the builder exposes an empty signal set, which makes
//! [`crate::adapter::partition::classify`] a no-op for declarative-only
//! specs — the right behaviour, since there is nothing per-field to
//! prune at the CLTS-state level.
//!
//! # SOUNDNESS — indirect references
//!
//! The `guards[]` / `effects[]` fields are populated by the upstream
//! `mununu-extract` tool walking the source AST. **Indirect writes via
//! pointers (`*p = x` in C, slice/index assignments through aliased
//! variables in Rust/TS/Python) are not resolved by the upstream tool
//! today** — the dep edge "field-targeted-by-`*p`" → "actually-written-field"
//! is missing. This under-represents the dep graph. Auto-COI will
//! correctly classify such a field as `Dropped` even though the
//! property semantically depends on it.
//!
//! Phase A.3 mitigates this with the `AdapterWarning` emitted at
//! translate time. The real fix is the follow-up plan
//! `phase-a3-followup-indirect-references.md`. Until then, callers
//! verifying C code with non-trivial pointer aliasing should either
//! (a) hand-author the dep graph via explicit `preserve: true` in the
//! sidecar (when step 3.5 lands) or (b) disable auto-partition via
//! `PartitionOptions::disabled = true`.

use std::collections::{HashMap, HashSet};

use super::ast::ExtractionSpec;
use crate::adapter::partition::DepGraphBuilder;

impl DepGraphBuilder for ExtractionSpec {
    fn build(&self) -> HashMap<String, HashSet<String>> {
        let known_fields: HashSet<String> =
            self.state_fields.iter().map(|f| f.id.clone()).collect();
        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();

        for method in &self.methods {
            // Two-pass: collect referenced fields under guards (reads)
            // and under effects (writes). Each effect-targeted field
            // gets every guard-referenced field as a dependency. The
            // SOUNDNESS impact of dropping indirect references is
            // documented at the module level.
            let mut guard_refs: HashSet<String> = HashSet::new();
            for g in &method.guards {
                collect_field_refs(g, &known_fields, &mut guard_refs);
            }
            for e in &method.effects {
                let mut effect_refs: HashSet<String> = HashSet::new();
                collect_field_refs(e, &known_fields, &mut effect_refs);
                for written in &effect_refs {
                    deps.entry(written.clone())
                        .or_default()
                        .extend(guard_refs.iter().cloned());
                    // Also let effects depend on each other when they
                    // appear together — captures the case where one
                    // method's body writes A and B simultaneously and
                    // a property atom over A pulls in B's data flow.
                    deps.entry(written.clone())
                        .or_default()
                        .extend(effect_refs.iter().cloned());
                }
            }
        }
        // Remove self-loops from the over-approximation (they reduce
        // precision without contributing to BFS reach).
        for (k, vs) in deps.iter_mut() {
            vs.remove(k);
        }
        deps
    }

    fn state_cells(&self) -> HashSet<String> {
        self.state_fields.iter().map(|f| f.id.clone()).collect()
    }

    fn input_ports(&self) -> HashSet<String> {
        // Extraction specs do not have "input ports" in the SV/BTOR2
        // sense. Alphabet labels (controllable / uncontrollable) live
        // in `model_config` and are events, not signals — they are
        // not candidates for the per-field partition. Return empty.
        HashSet::new()
    }
}

/// Recursively walk a JSON value collecting strings that match any
/// known field name. This is the only contract the trait impl has on
/// the opaque `guards` / `effects` JSON shape — string-typed leaves
/// that name a declared `state_fields[].id` count as references.
///
/// SOUNDNESS: missing references (e.g. an indirect pointer write that
/// the upstream extractor did not resolve to a concrete field name)
/// will not surface here. See the module-level SOUNDNESS block.
fn collect_field_refs(
    value: &serde_json::Value,
    known: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    match value {
        serde_json::Value::String(s) if known.contains(s) => {
            out.insert(s.clone());
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_field_refs(item, known, out);
            }
        }
        serde_json::Value::Object(obj) => {
            for v in obj.values() {
                collect_field_refs(v, known, out);
            }
            // Object keys that match field names also count as
            // references (common shape: `{ "field_name": { ... } }`).
            for k in obj.keys() {
                if known.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        _ => {}
    }
}

/// Best-effort property-atom extraction for extraction specs.
///
/// Walks `model_config.properties[].formula` strings looking for
/// declared state-field names. The mu-calculus parser is not invoked —
/// this is a substring scrape against the known field set, which is
/// the same approach the SV adapter takes
/// (`kripke::collect_property_signals_from_config`) and remains sound
/// because spurious matches only over-approximate the seed set.
pub fn extract_property_seeds(spec: &ExtractionSpec) -> HashSet<String> {
    let known: HashSet<String> = spec.state_fields.iter().map(|f| f.id.clone()).collect();
    if known.is_empty() {
        return HashSet::new();
    }
    let mut seeds = HashSet::new();
    for prop in &spec.model_config.properties {
        // Property may carry an inline `formula` body, a
        // `formula_template` alias, or a `template_ref` (the last is
        // handled downstream by template instantiation; its body is
        // not yet available at this point in the pipeline).
        let bodies = [prop.formula.as_deref(), prop.formula_template.as_deref()];
        for body in bodies.into_iter().flatten() {
            for field in &known {
                if formula_mentions_token(body, field) {
                    seeds.insert(field.clone());
                }
            }
        }
    }
    seeds
}

/// Substring match with word-boundary protection so that a field name
/// `id` does not spuriously match `valid` or `kid`.
fn formula_mentions_token(formula: &str, token: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let bytes = formula.as_bytes();
    let tlen = token.len();
    if tlen == 0 || tlen > bytes.len() {
        return false;
    }
    let mut i = 0;
    while i + tlen <= bytes.len() {
        if &bytes[i..i + tlen] == token.as_bytes() {
            let prev_ok = i == 0 || !is_word(bytes[i - 1] as char);
            let next_ok = i + tlen == bytes.len() || !is_word(bytes[i + tlen] as char);
            if prev_ok && next_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_spec_with_fields() -> ExtractionSpec {
        let json_text = r#"{
          "$schema": "extraction_spec_v1",
          "source": { "repo": "demo" },
          "state_fields": [
            { "id": "balance", "type": "u64" },
            { "id": "locked",  "type": "bool" },
            { "id": "owner",   "type": "address" }
          ],
          "methods": [
            { "id": "withdraw",
              "guards":  [ "locked" ],
              "effects": [ "balance" ] },
            { "id": "renounce",
              "guards":  [],
              "effects": [ "owner" ] }
          ],
          "model_config": {
            "context_name": "demo",
            "properties": [
              { "id": "balance_never_drained",
                "formula": "nu X. (balance && [] X)",
                "over": "demo" }
            ]
          }
        }"#;
        serde_json::from_str(json_text).expect("fixture must parse")
    }

    #[test]
    fn state_cells_lists_declared_fields() {
        let spec = make_spec_with_fields();
        let cells = spec.state_cells();
        assert_eq!(cells.len(), 3);
        assert!(cells.contains("balance"));
        assert!(cells.contains("locked"));
        assert!(cells.contains("owner"));
    }

    #[test]
    fn build_links_effect_to_guard() {
        let spec = make_spec_with_fields();
        let deps = spec.build();
        // `withdraw` reads `locked` and writes `balance` → balance depends on locked.
        let balance_deps = deps.get("balance").expect("balance has deps");
        assert!(balance_deps.contains("locked"), "got {balance_deps:?}");
        // `renounce` has no guards, so `owner` ends up with an empty
        // dep set (or no entry at all — both are valid).
        if let Some(owner_deps) = deps.get("owner") {
            assert!(owner_deps.is_empty());
        }
    }

    #[test]
    fn property_seeds_extract_field_names_from_formula() {
        let spec = make_spec_with_fields();
        let seeds = extract_property_seeds(&spec);
        // Property mentions `balance` only — the seed set must be {balance}.
        assert!(seeds.contains("balance"));
        assert_eq!(seeds.len(), 1, "got {seeds:?}");
    }

    #[test]
    fn partition_drops_unreached_field() {
        use crate::adapter::partition::{self, PartitionClass, PartitionOptions};
        let spec = make_spec_with_fields();
        let seeds = extract_property_seeds(&spec);
        let p = partition::classify(&spec, &seeds, &PartitionOptions::default());

        // `balance` is the seed → Kept.
        assert!(matches!(
            p.classes.get("balance"),
            Some(PartitionClass::Kept)
        ));
        // `withdraw` writes `balance` after reading `locked` → the dep
        // graph contains `balance → locked`. BFS from seed `balance`
        // reaches `locked`, so `locked` is also Kept.
        assert!(matches!(
            p.classes.get("locked"),
            Some(PartitionClass::Kept)
        ));
        // `owner` is written by `renounce` (no guards) and has no
        // path from `balance`; not in the property; must be Dropped.
        assert!(
            matches!(p.classes.get("owner"), Some(PartitionClass::Dropped { .. })),
            "owner classification was {:?}",
            p.classes.get("owner")
        );
    }

    #[test]
    fn declarative_only_spec_has_empty_state_cells() {
        // A spec with only model_config.automata (no state_fields) has
        // nothing per-field to partition. The builder must expose an
        // empty signal set; `classify` becomes a no-op.
        let json_text = r#"{
          "$schema": "extraction_spec_v1",
          "source": { "repo": "demo" },
          "model_config": {
            "context_name": "demo",
            "automata": [
              { "id": "A",
                "states": [ { "name": "S0", "initial": true } ],
                "transitions": [] }
            ]
          }
        }"#;
        let spec: ExtractionSpec = serde_json::from_str(json_text).unwrap();
        assert!(spec.state_cells().is_empty());
        assert!(spec.build().is_empty());
        assert!(extract_property_seeds(&spec).is_empty());
    }

    #[test]
    fn word_boundary_match() {
        assert!(formula_mentions_token("nu X. (!locked && [] X)", "locked"));
        assert!(!formula_mentions_token(
            "nu X. (!unlocked && [] X)",
            "locked"
        ));
        assert!(formula_mentions_token("[label] foo", "label"));
        assert!(!formula_mentions_token("[label_other] foo", "label"));
    }

    /// The opaque JSON walk must not crash on unexpected shapes —
    /// numbers, booleans, null, mixed arrays.
    #[test]
    fn collect_field_refs_is_total_over_json() {
        let known = ["balance"].into_iter().map(String::from).collect();
        let mut out = HashSet::new();
        collect_field_refs(
            &json!({"a": 1, "b": [null, true, 3.5, "balance", {"balance": "nested"}]}),
            &known,
            &mut out,
        );
        assert!(out.contains("balance"));
    }
}
