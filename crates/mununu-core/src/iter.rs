//! Shared iterator traits for common patterns (states, transitions).
//!
//! These traits provide a small, generic vocabulary for iterating over
//! CLTS-like structures without forcing callers to know the concrete
//! container layout.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, IdStorage, StateId, Transition};

/// Boxed iterator over state identifiers.
pub type StateIterBox<'a, S> = Box<dyn Iterator<Item = StateId<S>> + 'a>;

/// Boxed iterator over `(state, outgoing transitions)` pairs.
pub type TransitionIterBox<'a, S, L> =
    Box<dyn Iterator<Item = (StateId<S>, &'a [Transition<S, L>])> + 'a>;

/// Generic iterator over state identifiers.
pub trait StateIterable<S: IdStorage> {
    /// Returns an iterator over all state identifiers.
    fn states_iter(&self) -> StateIterBox<'_, S>;
}

/// Generic iterator over (state, outgoing transitions) pairs.
pub trait TransitionIterable<S: IdStorage, L: IdStorage> {
    /// Returns an iterator over each state and its outgoing transitions.
    fn transitions_iter(&self) -> TransitionIterBox<'_, S, L>;
}

impl StateIterable<DefaultStateIdx> for Clts<DefaultStateIdx, DefaultLabelIdx> {
    #[inline]
    fn states_iter(&self) -> StateIterBox<'_, DefaultStateIdx> {
        Box::new(self.states())
    }
}

impl TransitionIterable<DefaultStateIdx, DefaultLabelIdx>
    for Clts<DefaultStateIdx, DefaultLabelIdx>
{
    #[inline]
    fn transitions_iter(&self) -> TransitionIterBox<'_, DefaultStateIdx, DefaultLabelIdx> {
        Box::new(self.state_outgoing_pairs())
    }
}
