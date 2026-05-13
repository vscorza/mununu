//! Interleaved trace origin classifier — Document C task C3, slice 1.
//!
//! Counterexample / counterstrategy traces over a codesign-composed
//! model become **interleaved**: each transition came from either the
//! firmware side, the peripheral side, or the bus (a rendezvous on a
//! register-access label). When mununu reports such a trace, the
//! reader needs to know **which side fired which step** to make sense
//! of a race or a missed-acknowledgement bug.
//!
//! This module is the classifier. Given a label name and a
//! [`CouplingInfo`] describing the codesign-composed model's label
//! partition, it returns a [`TraceOrigin`] tag. Slice 1 ships the
//! classifier + a trace-list helper + a human-readable formatter.
//! Task C4 (`mununu codesign verify`) is the consumer that pulls a
//! trace out of the verifier and runs it through this module.
//!
//! ## Why a separate module
//!
//! The composition engine ([`crate::composition`]) produces flat
//! state-name traces — composed states look like `"Idle|Polling"` and
//! labels are bare strings. The composition engine deliberately does
//! not know about HW/SW semantics; that's a Doc C concern. This
//! module is the bridge: it brings the codesign vocabulary
//! (`rendezvous`, `peripheral`, `firmware`) to bear on those flat
//! traces *after* verification, without polluting the verifier.
//!
//! ## Soundness posture
//!
//! Classification is **lookup-based and total**:
//!   - If the label is in `rendezvous_labels` → `Bus`.
//!   - Else if the label is in `peripheral_internal_labels` → `Hw`.
//!   - Else if the label is in `firmware_internal_labels` → `Sw`.
//!   - Else → `Unknown`.
//!
//! Order matters because real codesign vocabularies have some overlap
//! (a peripheral-driven status read shows up in both sides' alphabets,
//! and it's deliberately a `Bus` event). The caller is responsible for
//! constructing label sets that respect this order; the helpers in
//! this module compute them correctly when given a register map.

use crate::codesign::coupling::register_map_labels;
use crate::codesign::register_map::RegisterMap;
use std::collections::HashSet;
use std::fmt;

/// Origin of a single trace step in a codesign-composed model.
///
/// The verifier produces flat traces; this enum is the codesign-layer
/// annotation a reader needs to see at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceOrigin {
    /// Firmware-side internal transition. Not visible to the peripheral.
    Sw,
    /// Peripheral RTL-side internal transition. Not visible to firmware.
    Hw,
    /// Rendezvous on a register-access label. Both sides synchronise.
    Bus,
    /// Could not be classified — the label is not in any of the
    /// caller-supplied label sets. Surfaces honestly rather than
    /// guessing.
    Unknown,
}

impl TraceOrigin {
    /// One-character display tag used in CLI / UI traces.
    pub fn tag(self) -> &'static str {
        match self {
            TraceOrigin::Sw => "SW",
            TraceOrigin::Hw => "HW",
            TraceOrigin::Bus => "BUS",
            TraceOrigin::Unknown => "?",
        }
    }
}

impl fmt::Display for TraceOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// What the trace classifier needs to know about the codesign-composed
/// model. The caller assembles these label sets from whichever sources
/// they have: the [`RegisterMap`] (for rendezvous labels), and the
/// firmware / peripheral CTXDSL alphabets (for the per-side internals).
#[derive(Debug, Clone, Default)]
pub struct CouplingInfo {
    /// Labels both sides synchronise on (output of
    /// [`register_map_labels`]). A trace step on any of these is a
    /// `Bus` event.
    pub rendezvous_labels: HashSet<String>,
    /// Labels visible only to the peripheral side — internal ticks,
    /// chaotic-stub housekeeping, peripheral-internal events. A trace
    /// step on any of these is a `Hw` event.
    pub peripheral_internal_labels: HashSet<String>,
    /// Labels visible only to the firmware side — internal control
    /// flow, polling counter ticks, ISR housekeeping. A trace step on
    /// any of these is a `Sw` event.
    pub firmware_internal_labels: HashSet<String>,
}

impl CouplingInfo {
    /// Build a [`CouplingInfo`] with just the rendezvous labels
    /// derived from a register map. Useful when the caller has no
    /// per-side internal vocabulary to declare yet — every non-bus
    /// trace step will classify as `Unknown` in that case. Callers
    /// typically extend the result by filling
    /// `peripheral_internal_labels` and `firmware_internal_labels`
    /// from the CTXDSL alphabets of the respective automata.
    pub fn from_register_map(rm: &RegisterMap) -> Self {
        let rendezvous_labels = register_map_labels(rm)
            .into_iter()
            .map(|l| l.name)
            .collect();
        Self {
            rendezvous_labels,
            peripheral_internal_labels: HashSet::new(),
            firmware_internal_labels: HashSet::new(),
        }
    }

    /// Add a peripheral-internal label.
    pub fn with_peripheral_label(mut self, label: impl Into<String>) -> Self {
        self.peripheral_internal_labels.insert(label.into());
        self
    }

    /// Add a firmware-internal label.
    pub fn with_firmware_label(mut self, label: impl Into<String>) -> Self {
        self.firmware_internal_labels.insert(label.into());
        self
    }

    /// Extend the peripheral-internal label set from any iterable of
    /// label names.
    pub fn extend_peripheral_labels<I, S>(mut self, labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for l in labels {
            self.peripheral_internal_labels.insert(l.into());
        }
        self
    }

    /// Extend the firmware-internal label set from any iterable of
    /// label names.
    pub fn extend_firmware_labels<I, S>(mut self, labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for l in labels {
            self.firmware_internal_labels.insert(l.into());
        }
        self
    }
}

/// Classify a single label by which side of the coupling it belongs
/// to.
///
/// Lookup order:
///   1. Rendezvous → `Bus` (shared-by-design takes precedence).
///   2. Peripheral-internal → `Hw`.
///   3. Firmware-internal → `Sw`.
///   4. None of the above → `Unknown`.
///
/// The order matters when label sets overlap: a label declared as
/// both peripheral-internal and rendezvous always classifies as
/// `Bus`. Callers who want to avoid overlap should ensure the three
/// sets are disjoint before passing them in.
pub fn classify_label(label: &str, info: &CouplingInfo) -> TraceOrigin {
    if info.rendezvous_labels.contains(label) {
        TraceOrigin::Bus
    } else if info.peripheral_internal_labels.contains(label) {
        TraceOrigin::Hw
    } else if info.firmware_internal_labels.contains(label) {
        TraceOrigin::Sw
    } else {
        TraceOrigin::Unknown
    }
}

/// Classify every label in a trace.
///
/// `labels[i]` is the label that drove the transition into `states[i+1]`
/// (matching the existing `LassoTraceApi.prefix_labels` /
/// `cycle_labels` convention from the API surface).
pub fn classify_trace_labels(labels: &[String], info: &CouplingInfo) -> Vec<TraceOrigin> {
    labels.iter().map(|l| classify_label(l, info)).collect()
}

/// Format an interleaved trace as human-readable lines.
///
/// One line per transition, with the origin tag in brackets followed
/// by the label and a `→` to the next state. Matches the rendering
/// shape from Doc C §C.4's worked example:
///
/// ```text
/// [SW]  poll_busy returns 0      → Polling|Idle
/// [HW]  tx_busy rises             → Polling|Busy_status
/// [BUS] wr_data_byte              → Sending|Busy_data
/// ```
///
/// `states[0]` is the initial state. `labels[i]` drove the transition
/// into `states[i + 1]`. The function tolerates a mismatched
/// `labels.len() != states.len() - 1` by stopping at the shorter of
/// the two — surfaces partial traces rather than panicking.
pub fn format_interleaved_trace(
    states: &[String],
    labels: &[String],
    info: &CouplingInfo,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if states.is_empty() {
        return out;
    }
    let _ = writeln!(out, "      {}", states[0]);
    let step_count = labels.len().min(states.len().saturating_sub(1));
    for i in 0..step_count {
        let label = &labels[i];
        let origin = classify_label(label, info);
        let next_state = &states[i + 1];
        let _ = writeln!(out, "[{}] {label:<30} → {next_state}", origin.tag());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codesign::register_map::{
        AccessPath, Field, Register, RegisterDirection, RegisterMap, VisibilityClass,
    };

    fn uart_map() -> RegisterMap {
        RegisterMap {
            peripheral: "UART_LITE".to_string(),
            base_address: "0x40010000".to_string(),
            description: None,
            contract_uri: None,
            registers: vec![
                Register {
                    name: "CTRL".to_string(),
                    offset: 0,
                    width_bits: 32,
                    direction: RegisterDirection::Rw,
                    visibility_class: VisibilityClass::Control,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![Field {
                        name: "tx_start".to_string(),
                        bits: [0, 0],
                        sv_signal: None,
                        c_accessor: None,
                        description: None,
                    }],
                },
                Register {
                    name: "STATUS".to_string(),
                    offset: 4,
                    width_bits: 32,
                    direction: RegisterDirection::Ro,
                    visibility_class: VisibilityClass::Status,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![Field {
                        name: "tx_busy".to_string(),
                        bits: [0, 0],
                        sv_signal: None,
                        c_accessor: None,
                        description: None,
                    }],
                },
            ],
        }
    }

    #[test]
    fn rendezvous_label_classifies_as_bus() {
        let info = CouplingInfo::from_register_map(&uart_map());
        assert_eq!(classify_label("rd_status_tx_busy", &info), TraceOrigin::Bus);
        assert_eq!(classify_label("wr_ctrl_tx_start", &info), TraceOrigin::Bus);
    }

    #[test]
    fn peripheral_internal_label_classifies_as_hw() {
        let info =
            CouplingInfo::from_register_map(&uart_map()).with_peripheral_label("sha_compute_tick");
        assert_eq!(classify_label("sha_compute_tick", &info), TraceOrigin::Hw);
    }

    #[test]
    fn firmware_internal_label_classifies_as_sw() {
        let info = CouplingInfo::from_register_map(&uart_map()).with_firmware_label("isr_dispatch");
        assert_eq!(classify_label("isr_dispatch", &info), TraceOrigin::Sw);
    }

    #[test]
    fn unknown_label_classifies_as_unknown() {
        let info = CouplingInfo::from_register_map(&uart_map());
        assert_eq!(
            classify_label("never_declared", &info),
            TraceOrigin::Unknown
        );
    }

    #[test]
    fn rendezvous_takes_precedence_over_internal_classification() {
        // A peripheral-side label that is *also* a rendezvous (e.g. the
        // user accidentally listed it in both sets) must classify as
        // `Bus`, not `Hw`. The classifier's order encodes that
        // rendezvous-by-design wins.
        let mut info = CouplingInfo::from_register_map(&uart_map());
        info.peripheral_internal_labels
            .insert("rd_status_tx_busy".to_string());
        assert_eq!(classify_label("rd_status_tx_busy", &info), TraceOrigin::Bus);
    }

    #[test]
    fn classify_trace_labels_per_step() {
        let info = CouplingInfo::from_register_map(&uart_map())
            .with_firmware_label("isr_dispatch")
            .with_peripheral_label("baud_tick");
        let labels: Vec<String> = vec![
            "isr_dispatch".to_string(),
            "rd_status_tx_busy".to_string(),
            "baud_tick".to_string(),
            "wr_ctrl_tx_start".to_string(),
            "phantom".to_string(),
        ];
        let origins = classify_trace_labels(&labels, &info);
        assert_eq!(
            origins,
            vec![
                TraceOrigin::Sw,
                TraceOrigin::Bus,
                TraceOrigin::Hw,
                TraceOrigin::Bus,
                TraceOrigin::Unknown,
            ]
        );
    }

    #[test]
    fn format_interleaved_trace_renders_one_line_per_step() {
        let info = CouplingInfo::from_register_map(&uart_map()).with_firmware_label("isr_dispatch");
        let states = vec!["S0".to_string(), "S1".to_string(), "S2".to_string()];
        let labels = vec!["isr_dispatch".to_string(), "rd_status_tx_busy".to_string()];
        let rendered = format_interleaved_trace(&states, &labels, &info);
        // Initial state present.
        assert!(rendered.contains("S0"));
        // Per-step tags + label + next state.
        assert!(rendered.contains("[SW]"));
        assert!(rendered.contains("isr_dispatch"));
        assert!(rendered.contains("→ S1"));
        assert!(rendered.contains("[BUS]"));
        assert!(rendered.contains("rd_status_tx_busy"));
        assert!(rendered.contains("→ S2"));
    }

    #[test]
    fn format_handles_empty_states() {
        let info = CouplingInfo::default();
        let out = format_interleaved_trace(&[], &[], &info);
        assert_eq!(out, "");
    }

    #[test]
    fn format_tolerates_label_count_one_short_of_states() {
        // states.len() = 3, labels.len() = 1 → should render initial
        // + 1 transition + skip the missing label rather than panic.
        let info = CouplingInfo::default();
        let states = vec!["S0".to_string(), "S1".to_string(), "S2".to_string()];
        let labels = vec!["a".to_string()];
        let rendered = format_interleaved_trace(&states, &labels, &info);
        assert!(rendered.contains("S0"));
        assert!(rendered.contains("→ S1"));
        assert!(!rendered.contains("→ S2"));
    }

    #[test]
    fn extend_helpers_populate_sets() {
        let info = CouplingInfo::from_register_map(&uart_map())
            .extend_peripheral_labels(["tick", "drain"])
            .extend_firmware_labels(["pop"]);
        assert_eq!(info.peripheral_internal_labels.len(), 2);
        assert_eq!(info.firmware_internal_labels.len(), 1);
        assert!(info.peripheral_internal_labels.contains("tick"));
        assert!(info.firmware_internal_labels.contains("pop"));
    }

    #[test]
    fn trace_origin_tag_strings_match_doc_c_convention() {
        // Doc C §C.4 worked example uses `[SW]`, `[HW]`, `[BUS]` tags.
        // The `?` tag is mununu-specific (no Doc-C precedent) so the
        // test is just that it doesn't collide with the three real
        // tags.
        assert_eq!(TraceOrigin::Sw.tag(), "SW");
        assert_eq!(TraceOrigin::Hw.tag(), "HW");
        assert_eq!(TraceOrigin::Bus.tag(), "BUS");
        assert_eq!(TraceOrigin::Unknown.tag(), "?");
    }
}
