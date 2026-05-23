//! Resolve `.mununu.json` sidecar entries against a BTOR2 file's symbol
//! annotations, producing a NID-keyed [`FieldDomain`] map for the
//! BTOR2 reader's bit-blast loop.
//!
//! The sidecar carries signal **names** (the user-facing convention).
//! BTOR2 carries **NIDs**. Yosys's `write_btor` emits symbol
//! annotations (`5 state 4 cnt`) that bridge the two: this module reads
//! the symbol table, matches each sidecar entry to a NID, and resolves
//! the entry to a [`FieldDomain`] via [`super::resolve_to_field_domain`].
//!
//! # Status
//!
//! Phase 1 sub-deliverable 2 — the module surface is shipped here so
//! the BTOR2 bit-blaster can call into it; the bit-blaster wiring lands
//! in a follow-up commit.

use crate::adapter::domain::FieldDomain;
use crate::adapter::systemverilog::annotation::SvAnnotation;
use std::collections::HashMap;

/// Build a `name → NID` lookup from a BTOR2 symbol table.
///
/// A BTOR2 `state` or `input` line may include a trailing symbol
/// annotation produced by Yosys: `5 state 4 cnt`. The parser collects
/// these into `Map<NID, name>`; this function inverts the relationship
/// for sidecar lookup.
pub fn invert_symbol_table(symbols: &HashMap<i64, String>) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for (nid, name) in symbols {
        out.insert(name.clone(), *nid);
    }
    out
}

/// For each `signals[]` entry in the sidecar, find the matching BTOR2
/// state cell (via the symbol table) and resolve to a `FieldDomain`.
/// Returns a NID-keyed map suitable for the BTOR2 bit-blaster's
/// per-state-cell domain lookup. Sidecar entries that don't match any
/// BTOR2 state are silently dropped — the caller is expected to warn.
///
/// The same logic applies to `inputs[]` entries (which use the
/// resolver's input-shaped path); see [`build_input_field_domains`].
pub fn build_field_domains_for_btor2(
    annotation: &SvAnnotation,
    symbols: &HashMap<i64, String>,
) -> HashMap<i64, (FieldDomain, Vec<(String, i64)>)> {
    let name_to_nid = invert_symbol_table(symbols);
    let mut out = HashMap::new();
    for sig in &annotation.signals {
        if let Some(&nid) = name_to_nid.get(&sig.name) {
            let resolved = super::resolve_to_field_domain(sig, annotation);
            out.insert(nid, resolved);
        }
    }
    out
}

/// Same as [`build_field_domains_for_btor2`] but for input signals.
/// Inputs in the sidecar use [`InputAnnotation`](crate::adapter::systemverilog::annotation::InputAnnotation);
/// this function constructs the equivalent `SignalAnnotation` view and
/// delegates to the shared resolver.
pub fn build_input_field_domains(
    annotation: &SvAnnotation,
    symbols: &HashMap<i64, String>,
) -> HashMap<i64, (FieldDomain, Vec<(String, i64)>)> {
    use crate::adapter::systemverilog::annotation::SignalAnnotation;

    let name_to_nid = invert_symbol_table(symbols);
    let mut out = HashMap::new();
    for inp in &annotation.inputs {
        if let Some(&nid) = name_to_nid.get(&inp.name) {
            let sig_view = SignalAnnotation {
                name: inp.name.clone(),
                preserve: inp.preserve,
                abstraction: inp.abstraction.clone(),
                bound: inp.bound,
                variants: inp.variants.clone(),
                value_map: inp.value_map.clone(),
                combinational: false,
                init_policy: inp.init_policy,
                type_name: None,
                note: None,
            };
            let resolved = super::resolve_to_field_domain(&sig_view, annotation);
            out.insert(nid, resolved);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::systemverilog::annotation::{
        DiscoveredValues, SignalAbstraction, SignalAnnotation, SvAnnotation,
    };

    fn empty_annotation() -> SvAnnotation {
        SvAnnotation {
            schema: Some("mununu_sv_annotation_v1".into()),
            module: "test".into(),
            source: None,
            signals: vec![],
            inputs: vec![],
            controllable: vec![],
            properties: vec![],
            discovered_values: std::collections::HashMap::new(),
            parameters: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn invert_symbol_table_round_trips() {
        let mut symbols = HashMap::new();
        symbols.insert(5i64, "cnt".to_string());
        symbols.insert(7i64, "state".to_string());

        let name_to_nid = invert_symbol_table(&symbols);

        assert_eq!(name_to_nid.get("cnt"), Some(&5i64));
        assert_eq!(name_to_nid.get("state"), Some(&7i64));
        assert_eq!(name_to_nid.get("missing"), None);
    }

    #[test]
    fn build_field_domains_matches_named_signals() {
        let mut ann = empty_annotation();
        ann.signals.push(SignalAnnotation {
            name: "cnt".into(),
            preserve: true,
            abstraction: SignalAbstraction::BoundedCounter,
            bound: Some(7),
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: crate::adapter::systemverilog::annotation::InitPolicy::Inherit,
            type_name: None,
            note: None,
        });

        let mut symbols = HashMap::new();
        symbols.insert(5i64, "cnt".to_string());
        symbols.insert(99i64, "unrelated".to_string());

        let domains = build_field_domains_for_btor2(&ann, &symbols);
        assert_eq!(domains.len(), 1);
        let (fd, _vm) = domains.get(&5i64).expect("cnt should resolve");
        assert_eq!(fd.bound, Some(7));
    }

    #[test]
    fn unmatched_signals_are_silently_dropped() {
        let mut ann = empty_annotation();
        ann.signals.push(SignalAnnotation {
            name: "absent_register".into(),
            preserve: true,
            abstraction: SignalAbstraction::Boolean,
            bound: None,
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: crate::adapter::systemverilog::annotation::InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        let symbols: HashMap<i64, String> = HashMap::new();

        let domains = build_field_domains_for_btor2(&ann, &symbols);
        assert!(domains.is_empty());
    }

    #[test]
    fn discovered_values_carry_through() {
        let mut ann = empty_annotation();
        ann.signals.push(SignalAnnotation {
            name: "ptr".into(),
            preserve: true,
            abstraction: SignalAbstraction::Discover,
            bound: None,
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: crate::adapter::systemverilog::annotation::InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        ann.discovered_values.insert(
            "ptr".to_string(),
            DiscoveredValues {
                values: vec![
                    crate::adapter::systemverilog::annotation::DiscoveredValue {
                        name: "ZERO".into(),
                        value: 0,
                        from: None,
                    },
                    crate::adapter::systemverilog::annotation::DiscoveredValue {
                        name: "MAX".into(),
                        value: 7,
                        from: None,
                    },
                ],
                catch_all: "OTHER".into(),
            },
        );
        let mut symbols = HashMap::new();
        symbols.insert(11i64, "ptr".to_string());

        let domains = build_field_domains_for_btor2(&ann, &symbols);
        let (fd, vm) = domains.get(&11i64).expect("ptr should resolve");
        assert_eq!(fd.variants.as_ref().map(|v| v.len()), Some(3)); // ZERO, MAX, OTHER
        assert_eq!(vm.len(), 2);
    }
}
