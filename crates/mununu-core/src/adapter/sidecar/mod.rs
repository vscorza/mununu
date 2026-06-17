//! Shared `.mununu.json` sidecar resolver.
//!
//! The `.mununu.json` schema (defined in
//! [`crate::adapter::systemverilog::annotation`]) maps signal names to
//! abstraction strategies — boolean / bounded-counter / enum / discover.
//! The resolution from a `SignalAnnotation` JSON entry to a
//! [`FieldDomain`] is **format-agnostic**: it neither reads the SV AST
//! nor the BTOR2 file. Hosting it here lets the BTOR2 reader (and any
//! future adapter) consume the same sidecar without depending on the SV
//! adapter's internals.
//!
//! # Pipeline
//!
//! ```text
//!     .mununu.json (parsed → SvAnnotation)
//!                       │
//!                       ▼
//!     resolve_to_field_domain(SignalAnnotation, &SvAnnotation)
//!                       │
//!                       ▼
//!     (FieldDomain, value-name map)
//!                       │
//!                       ▼
//!     adapter-specific cross-product enumeration
//! ```
//!
//! # Per-deliverable status (Phase 1 RTL roadmap)
//!
//! - **Sub-deliverable 1 (this module)**: shared resolver lives here;
//!   the SV adapter delegates via [`crate::adapter::systemverilog::annotation::resolve_signal_domain`].
//! - **Sub-deliverable 2**: BTOR2 reader consumes this resolver via
//!   [`build_field_domains_for_btor2`] (below).

use crate::adapter::domain::{AbstractValue, AbstractionType, FieldDomain};
use crate::adapter::systemverilog::annotation::{
    SignalAbstraction, SignalAnnotation, SvAnnotation,
};

/// Resolve a sidecar `SignalAnnotation` into a [`FieldDomain`] plus a
/// value-name map (for [`AbstractionType::EnumValues`] domains).
///
/// `ann` is the parent annotation document — needed to look up
/// `discovered_values` for [`SignalAbstraction::Discover`] entries.
///
/// This function is the **single canonical resolver** for the sidecar
/// schema; the SV adapter delegates to it (see
/// `systemverilog::annotation::resolve_signal_domain`), and the BTOR2
/// reader does the same via [`build_field_domains_for_btor2`].
pub fn resolve_to_field_domain(
    sig: &SignalAnnotation,
    ann: &SvAnnotation,
) -> (FieldDomain, Vec<(String, i64)>) {
    if !sig.preserve {
        return (
            FieldDomain {
                name: sig.name.clone(),
                abstraction: AbstractionType::Ignored,
                bound: None,
                lower_bound: None,
                variants: None,
                initial: AbstractValue::Counter(0),
            },
            vec![],
        );
    }

    match &sig.abstraction {
        SignalAbstraction::Boolean => (
            FieldDomain {
                name: sig.name.clone(),
                abstraction: AbstractionType::Boolean,
                bound: None,
                lower_bound: None,
                variants: None,
                initial: AbstractValue::Bool(false),
            },
            vec![],
        ),

        SignalAbstraction::BoundedCounter => {
            // SOUNDNESS (sidecar-audit F3): a missing `bound` defaults to
            // 3 (saturating {0,1,2,≥3}). A bound smaller than the
            // property's value sensitivity UNDER-approximates the
            // reachable value set — high values saturate into one class,
            // so a safety property that distinguishes them can be missed.
            // Declare an explicit `bound` for value-sensitive properties.
            // (Legacy SV-specific variant scheduled for removal per
            // docs/abstraction.md — not a target for a smarter default.)
            let bound = sig.bound.unwrap_or(3);
            (
                FieldDomain {
                    name: sig.name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some(bound),
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            )
        }

        SignalAbstraction::Enum => {
            let mut variants = sig.variants.clone().unwrap_or_default();
            let mut value_map = Vec::new();

            if let Some(vm) = &sig.value_map {
                for entry in vm {
                    value_map.push((entry.name.clone(), entry.value));
                    if !variants.contains(&entry.name) {
                        variants.push(entry.name.clone());
                    }
                }
            }

            if variants.is_empty() {
                variants.push("OTHER".to_string());
            }

            (
                FieldDomain {
                    name: sig.name.clone(),
                    abstraction: AbstractionType::EnumValues,
                    bound: None,
                    lower_bound: None,
                    variants: Some(variants),
                    initial: AbstractValue::Variant(
                        sig.variants
                            .as_ref()
                            .and_then(|v| v.first().cloned())
                            .unwrap_or_else(|| "OTHER".to_string()),
                    ),
                },
                value_map,
            )
        }

        SignalAbstraction::Discover => {
            if let Some(discovered) = ann.discovered_values.get(&sig.name) {
                let mut variants: Vec<String> =
                    discovered.values.iter().map(|v| v.name.clone()).collect();
                variants.push(discovered.catch_all.clone());

                let value_map: Vec<(String, i64)> = discovered
                    .values
                    .iter()
                    .map(|v| (v.name.clone(), v.value))
                    .collect();

                (
                    FieldDomain {
                        name: sig.name.clone(),
                        abstraction: AbstractionType::EnumValues,
                        bound: None,
                        lower_bound: None,
                        variants: Some(variants.clone()),
                        initial: AbstractValue::Variant(
                            variants.first().cloned().unwrap_or_default(),
                        ),
                    },
                    value_map,
                )
            } else {
                // SOUNDNESS (sidecar-audit F2): a `Discover` signal with
                // no discovered values (SMT discovery skipped, or the
                // seeding stages R-S3/R-S4/R-S5/R-S7 did not fire)
                // collapses to `Ignored` — the cell is pinned to one value
                // and dropped from the state space. Over-approximation:
                // sound for safety (the model admits every concrete value
                // the cell could take), UNDER-approximation for liveness
                // (a progress path that depends on the cell deviating is
                // masked). `initial` is a placeholder — irrelevant for a
                // dropped cell. Future refinement: emit an AdapterWarning
                // naming the silently dropped signal.
                (
                    FieldDomain {
                        name: sig.name.clone(),
                        abstraction: AbstractionType::Ignored,
                        bound: None,
                        lower_bound: None,
                        variants: None,
                        initial: AbstractValue::Counter(0),
                    },
                    vec![],
                )
            }
        }

        SignalAbstraction::BitBlast => {
            // SOUNDNESS (sidecar-audit F3): same direction as
            // BoundedCounter above — a missing `bound` defaults to 15; too
            // small under-approximates the value set (unsound for
            // value-sensitive safety). Legacy SV-specific variant,
            // scheduled for removal per docs/abstraction.md.
            let bound = sig.bound.unwrap_or(15);
            (
                FieldDomain {
                    name: sig.name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some(bound),
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            )
        }

        SignalAbstraction::Ignored => (
            // SOUNDNESS (sidecar-audit F2/F4): the cell is pinned to a
            // single value and dropped from the state space —
            // over-approximation (sound for safety, under-approx for
            // liveness). `initial` is a placeholder, irrelevant for a
            // dropped cell.
            FieldDomain {
                name: sig.name.clone(),
                abstraction: AbstractionType::Ignored,
                bound: None,
                lower_bound: None,
                variants: None,
                initial: AbstractValue::Counter(0),
            },
            vec![],
        ),
    }
}

pub mod btor2_resolver;
pub mod predicate_image;
