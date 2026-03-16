//! Transition key construction helpers for CLTS composition.
//!
//! This module centralises the logic for building canonicalised transition
//! descriptors (`TransitionKey`) used during product construction. Keys are
//! sorted and deduplicated by source/target/label-set/controllability so that
//! we can defer edge insertion and emit a compact, deterministic controller.

use std::rc::Rc;

use crate::clts::{DefaultStateIdx, StateId};

/// Deduplicated transition descriptor used to defer edge insertion until the
/// product traversal finishes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TransitionKey {
    pub(crate) source: StateId<DefaultStateIdx>,
    pub(crate) target: StateId<DefaultStateIdx>,
    pub(crate) labels: Rc<Vec<String>>,
    pub(crate) has_uncontrollable_labels: bool,
}

/// Lightweight builder for `TransitionKey` values.
#[derive(Debug, Default)]
pub(crate) struct TransitionKeyBuilder;

impl TransitionKeyBuilder {
    /// Creates a new transition descriptor anchored by the provided source,
    /// target, canonical label set, and whether it has uncontrollable labels.
    ///
    /// The `labels` vector is assumed to be already in canonical form
    /// (sorted and deduplicated). This is currently guaranteed by
    /// `ProductStateArena::intern_labels`.
    pub(crate) fn create_key(
        source: StateId<DefaultStateIdx>,
        target: StateId<DefaultStateIdx>,
        labels: Rc<Vec<String>>,
        has_uncontrollable: bool,
    ) -> TransitionKey {
        TransitionKey {
            source,
            target,
            labels,
            has_uncontrollable_labels: has_uncontrollable,
        }
    }
}
