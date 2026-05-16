//! Label-alphabet reconciliation between firmware (C) and peripheral (SV).
//!
//! The codesign coupling fragment requires that the firmware-side
//! automaton (synthesised by [`crate::codesign::c_extract_llvm`]) and
//! the peripheral-side automaton (synthesised by the SV adapter when
//! given a register map) synchronise on the same rendezvous-label
//! alphabet. This module is the gate that catches alphabet drift
//! *before* composition silently over-approximates.
//!
//! ## Soundness
//!
//! A label-alphabet mismatch is a hard error, not a warning. The two
//! sides must declare exactly the same set of rendezvous labels for
//! the asynchronous composition (Doc C §C.5) to model the real bus
//! correctly:
//!
//! - **Firmware-only labels** mean the firmware drives an access the
//!   peripheral model never observes. Silently composing those would
//!   admit firmware traces that are physically impossible (the access
//!   either hits the peripheral or it does not — a phantom access is
//!   under-approximation, unsound for safety).
//! - **Peripheral-only labels** mean the peripheral asserts behaviour
//!   on a register the firmware never touches. Silently composing
//!   would block firmware progress on the missing label (no firmware
//!   transition fires on it), starving liveness without an
//!   accompanying spec assumption.
//!
//! Both directions of mismatch are surfaced as a structured
//! [`ReconcileMismatch`] so callers (CLI / HTTP / UI) can render the
//! offending labels to the user, who must fix the register-map sidecar
//! or the SV port-binding before re-running.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Successful reconciliation result. The `shared` field is the
/// (canonical) alphabet on which firmware and peripheral synchronise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciledAlphabet {
    /// The set of labels that appear in both extractions, in
    /// canonical sorted order. This is the alphabet the asynchronous
    /// composition uses.
    pub shared: Vec<String>,
}

/// Structured mismatch report. Empty `firmware_only` plus empty
/// `peripheral_only` would not be a mismatch — the constructor in
/// [`reconcile_label_alphabets`] only emits this variant when at
/// least one side is non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileMismatch {
    /// Labels emitted by the firmware extraction but absent from the
    /// peripheral extraction's alphabet.
    pub firmware_only: Vec<String>,
    /// Labels emitted by the peripheral extraction but absent from
    /// the firmware extraction's alphabet.
    pub peripheral_only: Vec<String>,
}

/// Reconciliation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileError {
    /// At least one label exists on exactly one side of the bus.
    Mismatch(ReconcileMismatch),
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconcileError::Mismatch(m) => {
                write!(
                    f,
                    "label-alphabet mismatch between firmware and peripheral extractions"
                )?;
                if !m.firmware_only.is_empty() {
                    write!(
                        f,
                        "; firmware-only labels: [{}]",
                        m.firmware_only.join(", ")
                    )?;
                }
                if !m.peripheral_only.is_empty() {
                    write!(
                        f,
                        "; peripheral-only labels: [{}]",
                        m.peripheral_only.join(", ")
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ReconcileError {}

/// Reconcile firmware and peripheral label alphabets.
///
/// Returns `Ok(ReconciledAlphabet)` exactly when the two sets are
/// equal. Otherwise returns `Err(ReconcileError::Mismatch { .. })`
/// listing the labels that appear on only one side.
///
/// Inputs are `BTreeSet`s rather than `Vec`s so duplicates within a
/// single extraction don't survive into the mismatch report.
pub fn reconcile_label_alphabets(
    firmware: &BTreeSet<String>,
    peripheral: &BTreeSet<String>,
) -> Result<ReconciledAlphabet, ReconcileError> {
    let firmware_only: Vec<String> = firmware.difference(peripheral).cloned().collect();
    let peripheral_only: Vec<String> = peripheral.difference(firmware).cloned().collect();

    if !firmware_only.is_empty() || !peripheral_only.is_empty() {
        return Err(ReconcileError::Mismatch(ReconcileMismatch {
            firmware_only,
            peripheral_only,
        }));
    }

    let shared: Vec<String> = firmware.iter().cloned().collect();
    Ok(ReconciledAlphabet { shared })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn matching_alphabets_reconcile() {
        let fw = set(&["wr_ctrl_tx_start", "rd_status_tx_busy"]);
        let p = set(&["wr_ctrl_tx_start", "rd_status_tx_busy"]);
        let r = reconcile_label_alphabets(&fw, &p).unwrap();
        assert_eq!(
            r.shared,
            vec![
                "rd_status_tx_busy".to_string(),
                "wr_ctrl_tx_start".to_string()
            ]
        );
    }

    #[test]
    fn firmware_only_label_reports_mismatch() {
        let fw = set(&["wr_ctrl_tx_start", "wr_data"]);
        let p = set(&["wr_ctrl_tx_start"]);
        let err = reconcile_label_alphabets(&fw, &p).unwrap_err();
        match err {
            ReconcileError::Mismatch(m) => {
                assert_eq!(m.firmware_only, vec!["wr_data".to_string()]);
                assert!(m.peripheral_only.is_empty());
            }
        }
    }

    #[test]
    fn peripheral_only_label_reports_mismatch() {
        let fw = set(&["wr_ctrl_tx_start"]);
        let p = set(&["wr_ctrl_tx_start", "rd_status_rx_ready"]);
        let err = reconcile_label_alphabets(&fw, &p).unwrap_err();
        match err {
            ReconcileError::Mismatch(m) => {
                assert!(m.firmware_only.is_empty());
                assert_eq!(m.peripheral_only, vec!["rd_status_rx_ready".to_string()]);
            }
        }
    }

    #[test]
    fn both_sides_mismatched() {
        let fw = set(&["wr_ctrl_tx_start", "wr_data"]);
        let p = set(&["wr_ctrl_tx_start", "rd_status_rx_ready"]);
        let err = reconcile_label_alphabets(&fw, &p).unwrap_err();
        match err {
            ReconcileError::Mismatch(m) => {
                assert_eq!(m.firmware_only, vec!["wr_data".to_string()]);
                assert_eq!(m.peripheral_only, vec!["rd_status_rx_ready".to_string()]);
            }
        }
    }

    #[test]
    fn empty_sets_reconcile() {
        let r = reconcile_label_alphabets(&BTreeSet::new(), &BTreeSet::new()).unwrap();
        assert!(r.shared.is_empty());
    }

    #[test]
    fn mismatch_display_lists_both_sides() {
        let err = ReconcileError::Mismatch(ReconcileMismatch {
            firmware_only: vec!["a".into(), "b".into()],
            peripheral_only: vec!["c".into()],
        });
        let s = format!("{err}");
        assert!(s.contains("firmware-only labels: [a, b]"));
        assert!(s.contains("peripheral-only labels: [c]"));
    }
}
