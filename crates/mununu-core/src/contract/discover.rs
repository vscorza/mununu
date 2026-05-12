//! Discovery pipeline phase 1 — task A5 of
//! `docs/design/black-box-modules.md`.
//!
//! Phase 1 covers the first three fields of the contract object the
//! pipeline emits per black-box module:
//!
//! 1. **Interface alphabet** — the labels derived from the module's port
//!    list (or function signature in the software analogue).
//! 2. **Controllability classification** per label, computed via the
//!    shared `crate::controllability` helper (task A4).
//! 3. **Gap markers** — explicit `unknown { … }` regions describing what
//!    is *not* known about the module, so the user sees the soundness
//!    consequence of accepting the default chaotic stub.
//!
//! Phase 2 (task A6) adds discovered automaton fragments from
//! source-comment annotations and corpus lookups; phase 3 adds discovered
//! formulas. Both are out of scope for this module.
//!
//! The phase-1 deliverable is intentionally adapter-agnostic. A
//! `BlackBoxInterface` describes the bare minimum the adapter must hand
//! over (module name + ordered list of `PortDescriptor`s); the rest of
//! the pipeline is shared. Adapters supply this description today by
//! synthesising it from whatever language-specific representation they
//! have — SV port lists, MCP tool schemas, C function signatures, …

use crate::clts::LabelControllability;
use crate::contract::gap::{GapKind, GapMarker, GapMarkerReport};
use crate::controllability::{BoundaryDirection, classify_label};
use serde::{Deserialize, Serialize};

/// A single port / parameter / channel on a black-box module's interface.
///
/// The adapter-supplied description; `discover_phase1` translates this
/// into the contract-side `InterfaceLabel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDescriptor {
    /// Label name as it will appear in the CLTS alphabet.
    pub name: String,
    /// Direction relative to the module's boundary.
    pub direction: BoundaryDirection,
    /// Optional human-readable description (e.g. signal width, channel
    /// semantics, MCP tool docstring). Carried into the gap-marker
    /// metadata so the user sees the original intent.
    #[serde(default)]
    pub description: Option<String>,
}

/// What the adapter knows about a black-box module before phase 1 runs.
///
/// This is the smallest abstraction that lets the same discovery
/// pipeline serve SV, BTOR2, and software extraction. Each adapter is
/// responsible for filling this in from its own native representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlackBoxInterface {
    /// Module / component / class name.
    pub name: String,
    /// Ports in declaration order. The pipeline preserves order so the
    /// emitted alphabet is deterministic.
    pub ports: Vec<PortDescriptor>,
    /// Optional source location (file + line) for diagnostics.
    #[serde(default)]
    pub source_file: Option<String>,
    /// Optional source line for diagnostics.
    #[serde(default)]
    pub source_line: Option<u32>,
}

/// A single classified interface label emitted by phase 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceLabel {
    pub name: String,
    /// Controllability after applying the shared classifier + any
    /// adapter-supplied overrides.
    pub controllability: LabelControllability,
    /// Original direction reported by the adapter.
    pub direction: BoundaryDirection,
    pub description: Option<String>,
}

/// Output of phase 1 discovery for one black-box module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phase1Output {
    /// Module name, copied through.
    pub module: String,
    /// Classified interface labels.
    pub labels: Vec<InterfaceLabel>,
    /// Gap markers attached to this module (always emitted in phase 1 —
    /// the discovery pipeline has not yet learned anything about
    /// sequencing or formulas, so every black-box module starts with at
    /// least an `OutputSequencing` gap).
    pub gaps: GapMarkerReport,
}

/// Configuration knobs for `discover_phase1`.
#[derive(Debug, Clone, Default)]
pub struct DiscoverOptions<'a> {
    /// Per-label overrides — names listed here are forced to
    /// `Controllable` regardless of port direction. Used as the legacy
    /// escape hatch per Document A §4.ii.
    pub force_controllable: &'a [&'a str],
    /// Per-label overrides — names forced to `Uncontrollable`.
    pub force_uncontrollable: &'a [&'a str],
    /// Whether to emit a default `Fairness` gap marker as well as the
    /// usual `OutputSequencing` one. Adapters that know the module is
    /// purely combinational (no liveness story) can disable this.
    pub emit_fairness_gap: bool,
}

/// Convert a list of discovered black-box interfaces into the adapter
/// sidecar files the rest of the contract subsystem consumes.
///
/// Used by adapters that have just detected one or more black-box
/// submodules during extraction (e.g., yosys frontend hitting
/// `(* blackbox *)`, or the custom-SV pipeline encountering an
/// instantiation not listed in its multi-module sidecar). The
/// adapter calls this helper to get a `Vec<AdapterSidecar>` it can
/// attach to its `AdapterOutput`; the caller (CLI / API) writes the
/// sidecars to disk next to the primary CTXDSL output.
///
/// For each black-box interface, two sidecars are emitted:
///   - `<module>.interface.json` — the `BlackBoxInterface` itself.
///   - `<module>.gap_report.json` — a `GapMarkerReport` with the
///     phase-1 default gaps (OutputSequencing covering outputs;
///     Fairness opt-in).
///
/// This is Document B § B.7.3's load-bearing helper: the moment the
/// contract subsystem stops being a separate JSON workflow and becomes
/// an automatic byproduct of extraction.
pub fn build_blackbox_sidecars(
    interfaces: &[BlackBoxInterface],
    options: &DiscoverOptions<'_>,
) -> Vec<crate::adapter::AdapterSidecar> {
    use crate::adapter::{AdapterSidecar, SidecarOrigin};

    let mut sidecars = Vec::with_capacity(interfaces.len() * 2);
    for iface in interfaces {
        let phase1 = discover_phase1(iface, options);

        // Interface JSON. Use `to_string_pretty` so the file is
        // human-readable and stable for diffing.
        if let Ok(interface_json) = serde_json::to_string_pretty(&iface) {
            sidecars.push(AdapterSidecar {
                filename: format!("{}.interface.json", sanitize_filename(&iface.name)),
                content: interface_json,
                origin: SidecarOrigin::BlackBoxInterface,
            });
        }

        // Gap-report JSON.
        if let Ok(gap_json) = serde_json::to_string_pretty(&phase1.gaps) {
            sidecars.push(AdapterSidecar {
                filename: format!("{}.gap_report.json", sanitize_filename(&iface.name)),
                content: gap_json,
                origin: SidecarOrigin::BlackBoxGapReport,
            });
        }
    }
    sidecars
}

/// Make a filename-safe version of a module name. Lowercase ASCII +
/// digits + `_` + `-` are kept; everything else becomes `_`.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Run phase 1 discovery on a black-box interface. Always emits at least
/// one gap marker (chaotic-stub default).
pub fn discover_phase1(iface: &BlackBoxInterface, options: &DiscoverOptions<'_>) -> Phase1Output {
    // 1. Classify each port via the shared controllability helper.
    let labels: Vec<InterfaceLabel> = iface
        .ports
        .iter()
        .map(|port| {
            let controllability = classify_label(
                &port.name,
                port.direction,
                options.force_controllable,
                options.force_uncontrollable,
            );
            InterfaceLabel {
                name: port.name.clone(),
                controllability,
                direction: port.direction,
                description: port.description.clone(),
            }
        })
        .collect();

    // 2. Gap markers — at minimum, an `OutputSequencing` gap covering all
    //    outputs (the chaotic-stub default cannot prove liveness on
    //    output labels). Phase 2 will replace this with discovered
    //    automaton fragments when annotations / corpus entries exist.
    let mut gaps = GapMarkerReport::new();
    let output_labels: Vec<String> = labels
        .iter()
        .filter(|l| matches!(l.direction, BoundaryDirection::Output))
        .map(|l| l.name.clone())
        .collect();
    let source_location = match (iface.source_file.as_ref(), iface.source_line) {
        (Some(file), Some(line)) => Some(crate::contract::gap::SourceLocation {
            file: file.clone(),
            line,
        }),
        _ => None,
    };
    if !output_labels.is_empty() {
        gaps.push(GapMarker {
            module: iface.name.clone(),
            kind: GapKind::OutputSequencing,
            labels: output_labels,
            description: Some(format!(
                "Phase-1 discovery — no sequencing fragment yet for {}",
                iface.name
            )),
            source_location: source_location.clone(),
        });
    }
    if options.emit_fairness_gap {
        gaps.push(GapMarker {
            module: iface.name.clone(),
            kind: GapKind::Fairness,
            labels: vec![],
            description: Some("Phase-1 discovery — no fairness assumption authored.".to_string()),
            source_location,
        });
    }

    Phase1Output {
        module: iface.name.clone(),
        labels,
        gaps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, direction: BoundaryDirection) -> PortDescriptor {
        PortDescriptor {
            name: name.to_string(),
            direction,
            description: None,
        }
    }

    #[test]
    fn ports_classified_via_shared_rule() {
        let iface = BlackBoxInterface {
            name: "FifoIp".to_string(),
            ports: vec![
                port("push", BoundaryDirection::Input),
                port("pop", BoundaryDirection::Input),
                port("data_out", BoundaryDirection::Output),
                port("full", BoundaryDirection::Output),
                port("clk", BoundaryDirection::Input),
            ],
            source_file: None,
            source_line: None,
        };
        let out = discover_phase1(&iface, &DiscoverOptions::default());
        assert_eq!(out.module, "FifoIp");
        assert_eq!(out.labels.len(), 5);
        let by_name = |name: &str| {
            out.labels
                .iter()
                .find(|l| l.name == name)
                .expect("label exists")
        };
        assert_eq!(
            by_name("push").controllability,
            LabelControllability::Uncontrollable
        );
        assert_eq!(
            by_name("data_out").controllability,
            LabelControllability::Controllable
        );
        assert_eq!(
            by_name("full").controllability,
            LabelControllability::Controllable
        );
    }

    #[test]
    fn output_sequencing_gap_emitted_when_outputs_exist() {
        let iface = BlackBoxInterface {
            name: "SHA256".to_string(),
            ports: vec![
                port("din", BoundaryDirection::Input),
                port("hash_out", BoundaryDirection::Output),
                port("hash_valid", BoundaryDirection::Output),
            ],
            source_file: Some("rtl/sha.sv".to_string()),
            source_line: Some(42),
        };
        let out = discover_phase1(&iface, &DiscoverOptions::default());
        assert_eq!(out.gaps.len(), 1);
        let gap = &out.gaps.markers[0];
        assert_eq!(gap.module, "SHA256");
        assert_eq!(gap.kind, GapKind::OutputSequencing);
        assert_eq!(gap.labels.len(), 2);
        assert_eq!(gap.source_location.as_ref().map(|l| l.line), Some(42));
    }

    #[test]
    fn no_output_no_default_gap() {
        // Pure-input "sink" component — no outputs → no
        // OutputSequencing gap.
        let iface = BlackBoxInterface {
            name: "Sink".to_string(),
            ports: vec![
                port("din", BoundaryDirection::Input),
                port("strobe", BoundaryDirection::Input),
            ],
            source_file: None,
            source_line: None,
        };
        let out = discover_phase1(&iface, &DiscoverOptions::default());
        assert!(out.gaps.is_empty());
    }

    #[test]
    fn fairness_gap_when_opt_in() {
        let iface = BlackBoxInterface {
            name: "Arbiter".to_string(),
            ports: vec![
                port("req", BoundaryDirection::Input),
                port("grant", BoundaryDirection::Output),
            ],
            source_file: None,
            source_line: None,
        };
        let opts = DiscoverOptions {
            emit_fairness_gap: true,
            ..DiscoverOptions::default()
        };
        let out = discover_phase1(&iface, &opts);
        assert_eq!(out.gaps.len(), 2);
        assert_eq!(out.gaps.markers[0].kind, GapKind::OutputSequencing);
        assert_eq!(out.gaps.markers[1].kind, GapKind::Fairness);
    }

    #[test]
    fn build_sidecars_produces_interface_and_gap_files() {
        use crate::adapter::SidecarOrigin;

        let iface = BlackBoxInterface {
            name: "DDR3_PHY_V2".to_string(),
            ports: vec![
                port("clk", BoundaryDirection::Input),
                port("data_out", BoundaryDirection::Output),
            ],
            source_file: Some("rtl/vendor/ddr3.sv".to_string()),
            source_line: Some(10),
        };
        let sidecars = build_blackbox_sidecars(&[iface], &DiscoverOptions::default());
        assert_eq!(sidecars.len(), 2);
        assert_eq!(sidecars[0].origin, SidecarOrigin::BlackBoxInterface);
        assert_eq!(sidecars[0].filename, "DDR3_PHY_V2.interface.json");
        assert!(sidecars[0].content.contains("\"name\": \"DDR3_PHY_V2\""));
        assert_eq!(sidecars[1].origin, SidecarOrigin::BlackBoxGapReport);
        assert_eq!(sidecars[1].filename, "DDR3_PHY_V2.gap_report.json");
        // Phase-1 emits an OutputSequencing gap for any black box with
        // at least one output.
        assert!(sidecars[1].content.contains("output_sequencing"));
        assert!(sidecars[1].content.contains("data_out"));
    }

    #[test]
    fn build_sidecars_sanitises_filename_chars() {
        let iface = BlackBoxInterface {
            name: "vendor/IP::Block 1".to_string(),
            ports: vec![port("out", BoundaryDirection::Output)],
            source_file: None,
            source_line: None,
        };
        let sidecars = build_blackbox_sidecars(&[iface], &DiscoverOptions::default());
        assert_eq!(sidecars[0].filename, "vendor_IP__Block_1.interface.json");
    }

    #[test]
    fn build_sidecars_empty_input_yields_empty_output() {
        let sidecars = build_blackbox_sidecars(&[], &DiscoverOptions::default());
        assert!(sidecars.is_empty());
    }

    #[test]
    fn force_lists_override_direction() {
        let iface = BlackBoxInterface {
            name: "Quirky".to_string(),
            ports: vec![
                port("reset_n", BoundaryDirection::Input),
                port("status", BoundaryDirection::Output),
            ],
            source_file: None,
            source_line: None,
        };
        let opts = DiscoverOptions {
            force_controllable: &["reset_n"],
            force_uncontrollable: &["status"],
            ..DiscoverOptions::default()
        };
        let out = discover_phase1(&iface, &opts);
        let by = |name: &str| {
            out.labels
                .iter()
                .find(|l| l.name == name)
                .expect("label exists")
        };
        assert_eq!(
            by("reset_n").controllability,
            LabelControllability::Controllable
        );
        assert_eq!(
            by("status").controllability,
            LabelControllability::Uncontrollable
        );
    }
}
