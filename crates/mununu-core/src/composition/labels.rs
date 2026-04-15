//! Label collection helpers for CLTS composition.
//!
//! This module centralises small utilities for extracting concrete label names
//! from transitions in a composition-friendly form.
//!
//! Currently this module is only used in unit tests; it is compiled behind
//! `cfg(test)` via the parent module declaration.

use std::collections::BTreeSet;

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, Transition};

/// Gathers the concrete label payload attached to a transition and returns it
/// as a sorted [`BTreeSet`].
///
/// This helper is primarily used in tests and diagnostics, where a stable
/// ordering of label names is desirable.
pub(crate) fn collect_transition_labels(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    transition: &Transition<DefaultStateIdx, DefaultLabelIdx>,
) -> BTreeSet<String> {
    let mut labels = BTreeSet::new();
    for label_id in transition.labels() {
        if let Some(payload) = clts.label_payload(*label_id) {
            labels.extend(payload.iter().cloned());
        }
    }
    labels
}
