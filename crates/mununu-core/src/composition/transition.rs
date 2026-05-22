//! Transition key construction helpers for CLTS composition.
//!
//! This module centralises the logic for building canonicalised transition
//! descriptors (`TransitionKey`) used during product construction. Keys are
//! sorted and deduplicated by source/target/label-set/controllability so that
//! we can defer edge insertion and emit a compact, deterministic controller.

use std::rc::Rc;

use crate::clts::{DefaultStateIdx, StateId, TransitionModality};

/// Deduplicated transition descriptor used to defer edge insertion until the
/// product traversal finishes.
///
/// R.1 — Carries the KMTS [`TransitionModality`] computed per the
/// composition's per-axis-conjunction merge rule
/// (`docs/design/native-sv-abstraction.md` §6.5;
/// `docs/design/kmts-theory.md` §5.1):
///
/// - Synchronising step (both sides contribute): `lt.modality().merge(rt.modality())`.
/// - Interleaving step (only one side moves): that side's modality.
///
/// For Sharp-everywhere CLTSes (every legacy adapter today), the
/// merge is `Sharp ⊗ Sharp = Sharp` and the modality field is
/// effectively a no-op. The field becomes load-bearing the moment
/// R.2's BTOR2 → KMTS lifter starts producing `MayOnly` edges.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TransitionKey {
    pub(crate) source: StateId<DefaultStateIdx>,
    pub(crate) target: StateId<DefaultStateIdx>,
    pub(crate) labels: Rc<Vec<String>>,
    pub(crate) has_uncontrollable_labels: bool,
    pub(crate) modality: TransitionModality,
}

/// Lightweight builder for `TransitionKey` values.
#[derive(Debug, Default)]
pub(crate) struct TransitionKeyBuilder;

impl TransitionKeyBuilder {
    /// Creates a new transition descriptor anchored by the provided source,
    /// target, canonical label set, controllability flag, and KMTS modality.
    ///
    /// The `labels` vector is assumed to be already in canonical form
    /// (sorted and deduplicated). This is currently guaranteed by
    /// `ProductStateArena::intern_labels`.
    pub(crate) fn create_key(
        source: StateId<DefaultStateIdx>,
        target: StateId<DefaultStateIdx>,
        labels: Rc<Vec<String>>,
        has_uncontrollable: bool,
        modality: TransitionModality,
    ) -> TransitionKey {
        TransitionKey {
            source,
            target,
            labels,
            has_uncontrollable_labels: has_uncontrollable,
            modality,
        }
    }
}
