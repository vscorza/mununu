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
/// R.1 + R.4.5 — Carries the KMTS [`TransitionModality`] computed
/// per the composition's per-axis-conjunction merge rule
/// (`docs/design/native-sv-abstraction.md` §6.5 + §6.11;
/// `docs/design/kmts-theory.md` §5):
///
/// - Synchronising step (both sides contribute): `lt.modality().merge(rt.modality())`.
/// - Interleaving step (only one side moves): that side's modality.
///
/// For Sharp-everywhere CLTSes (every legacy adapter today), the
/// merge is `Sharp ⊗ Sharp = Sharp` and the modality field is
/// effectively a no-op. The field becomes load-bearing the moment
/// R.2's BTOR2 → KMTS lifter produces `MayOnly` edges or R.5 /
/// R.5b paths emit `MustHyperOnly` edges with hyper-target sets.
///
/// **R.4.5 note on hyper-targets:** when both sides synchronise on a
/// must-hyper-edge pair, the composed transition's hyper-target set
/// is the *Cartesian product* of the two sides' hyper-target sets,
/// projected through `ProductStateArena::ensure_state` to product
/// StateIds. That construction lives in `composition/mod.rs` because
/// it needs the product-state arena; this struct just carries the
/// already-merged modality value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TransitionKey {
    pub(crate) source: StateId<DefaultStateIdx>,
    pub(crate) target: StateId<DefaultStateIdx>,
    pub(crate) labels: Rc<Vec<String>>,
    pub(crate) has_uncontrollable_labels: bool,
    pub(crate) modality: TransitionModality<DefaultStateIdx>,
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
        modality: TransitionModality<DefaultStateIdx>,
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
