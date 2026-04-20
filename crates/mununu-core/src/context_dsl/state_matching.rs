use bitvec::prelude::{BitVec, Lsb0};

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};

/// Helper for matching state names (including unrolled state variants) and
/// constructing bitsets for matching states.
///
/// Supports two resolution strategies:
/// 1. **Structured matching** (preferred): checks `state_valuation()` for an exact
///    variable-name → value match. This avoids the underscore-delimiter ambiguity
///    in composite state names like `data_out_r_IDLE`.
/// 2. **String prefix matching** (fallback): matches `pattern` against state names
///    using exact or prefix comparison. Used when no structured valuations exist.
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
    /// Uses structured matching (via `state_valuation()`) when available, falling
    /// back to string prefix matching. This is the primary entry point for
    /// predicate resolution.
    pub(crate) fn create_bitset_for_pattern(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        pattern: &str,
    ) -> BitVec<usize, Lsb0> {
        // Try structured matching first if any state has valuations
        if clts.has_valuations()
            && let Some(bits) = Self::create_bitset_from_valuations(clts, pattern)
        {
            return bits;
        }

        // Fallback: string prefix matching
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

    /// Attempts structured matching: tries to parse the predicate pattern as
    /// `variable_value` and checks against the structured valuation data on each state.
    ///
    /// Returns `None` if the pattern cannot be interpreted as a variable-value pair
    /// (no state has a valuation matching any split of the pattern).
    fn create_bitset_from_valuations(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        pattern: &str,
    ) -> Option<BitVec<usize, Lsb0>> {
        // Collect all variable names from valuations to find valid splits
        let mut known_vars: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for state_id in clts.states() {
            if let Some(valuation) = clts.state_valuation(state_id) {
                for key in valuation.keys() {
                    known_vars.insert(key.as_str());
                }
            }
        }

        if known_vars.is_empty() {
            return None;
        }

        // Try to split the pattern into (variable_name, value) using known variables.
        // For a pattern like "fill_3", try "fill" + "3".
        // For a pattern like "data_out_r_IDLE", try "data_out_r" + "IDLE".
        // We try all possible splits and check if the variable name is known.
        let mut matched_var = None;
        let mut matched_val = None;

        for (i, _) in pattern.char_indices() {
            if i == 0 {
                continue;
            }
            if pattern.as_bytes().get(i.wrapping_sub(1)) == Some(&b'_') {
                let (var_candidate, val_with_underscore) = pattern.split_at(i);
                let var_candidate = var_candidate.trim_end_matches('_');
                if known_vars.contains(var_candidate) {
                    // Found a valid split
                    matched_var = Some(var_candidate);
                    matched_val = Some(val_with_underscore);
                    // Keep searching for longer variable names (prefer longest match)
                }
            }
        }

        let var_name = matched_var?;
        let var_value = matched_val?;

        let mut bits = BitVec::repeat(false, clts.state_count());
        let mut any_match = false;

        for state_id in clts.states() {
            if let Some(valuation) = clts.state_valuation(state_id)
                && let Some(state_val) = valuation.get(var_name)
                && state_val == var_value
            {
                bits.set(state_id.index(), true);
                any_match = true;
            }
        }

        if any_match { Some(bits) } else { None }
    }
}
