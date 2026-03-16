use bitvec::prelude::{BitVec, Lsb0};

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};

/// Helper for matching state names (including unrolled state variants) and
/// constructing bitsets for matching states.
pub(crate) struct StateNameMatcher;

impl StateNameMatcher {
    /// Returns `true` if `pattern` matches `state_name` either exactly or as a
    /// prefix for unrolled states.
    ///
    /// Matching strategy:
    /// - Exact match: `pattern == state_name`
    /// - Prefix match: `state_name` starts with `format!("{}_", pattern)`
    pub(crate) fn matches_pattern(pattern: &str, state_name: &str) -> bool {
        // First try exact match
        if pattern == state_name {
            return true;
        }

        // Then try prefix match for unrolled states, e.g.:
        // - "End" matches "End_x_0", "End_count_5", ...
        let prefix = format!("{}_", pattern);
        state_name.starts_with(&prefix)
    }

    /// Creates a bitset with bits set for all states whose names match `pattern`.
    ///
    /// This is the bitset-based counterpart to [`StateNameMatcher::find_matching_states`]
    /// and is used by predicate computation to support sidecar predicates that
    /// reference original (pre-unrolling) state names.
    pub(crate) fn create_bitset_for_pattern(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        pattern: &str,
    ) -> BitVec<usize, Lsb0> {
        let mut bits = BitVec::repeat(false, clts.state_count());
        for state_id in clts.states() {
            if let Some(state_name) = clts.state_name(state_id)
                && Self::matches_pattern(pattern, state_name)
            {
                bits.set(state_id.index(), true);
            }
        }
        bits
    }
}
