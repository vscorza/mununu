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
    ///
    /// SOUNDNESS: the result is masked to clear any OOB sink states (whose
    /// valuation carries the `$oob$ → "true"` marker). This implements the
    /// OOB-as-bottom semantics — user-declared predicates never match the OOB
    /// sink, so safety formulas referencing them correctly fail at any source
    /// state with a transition to OOB. The defensive mask here complements the
    /// final mask in `mu_calculus::evaluator::predicate_bits`.
    pub(crate) fn create_bitset_for_pattern(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        pattern: &str,
    ) -> BitVec<usize, Lsb0> {
        // Try structured matching first if any state has valuations
        let mut bits = if clts.has_valuations()
            && let Some(b) = Self::create_bitset_from_valuations(clts, pattern)
        {
            b
        } else {
            // Fallback: string prefix matching
            let mut b = BitVec::repeat(false, clts.state_count());
            for state_id in clts.states() {
                if let Some(state_name) = clts.state_name(state_id)
                    && Self::matches_pattern(pattern, state_name)
                {
                    b.set(state_id.index(), true);
                }
            }
            b
        };

        Self::clear_oob_bits(clts, &mut bits);
        bits
    }

    /// Clear bits for any state whose CLTS valuation carries the
    /// `__mununu_oob__ → "true"` out-of-bounds sink marker. Adapters set this
    /// marker when an abstract transition would have escaped the abstracted
    /// domain (see `adapter::systemverilog::kripke::OOB_STATE_KEY`).
    fn clear_oob_bits(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        bits: &mut BitVec<usize, Lsb0>,
    ) {
        for state_id in clts.states() {
            if let Some(val) = clts.state_valuation(state_id)
                && val.get("__mununu_oob__").map(|s| s.as_str()) == Some("true")
            {
                bits.set(state_id.index(), false);
            }
        }
    }

    /// Attempts structured matching: parses the predicate pattern as a
    /// **conjunction** of `variable_value` pairs and checks it against the
    /// structured valuation data on each state.
    ///
    /// Single pair (`fill_3` → `fill == 3`, `data_out_r_IDLE` →
    /// `data_out_r == IDLE`) is the common case. **Compound** patterns —
    /// `signal_T_state_VARIANT` → `signal == T ∧ state == VARIANT` — arise
    /// when the cross-product predicate names from the native pipeline are
    /// evaluated against the KMTS bit-blaster's per-cell valuations (which
    /// store each signal separately, unlike native's combined state
    /// names). The old single-pair parse turned
    /// `bvalid_r_T_state_ADDR_WAIT` into `bvalid_r == "T_state_ADDR_WAIT"`
    /// (never matched) → the predicate resolved to false → safety formulas
    /// became vacuous totality. (F2, S-track KMTS-fidelity 2026-06-14.)
    ///
    /// Returns `None` if the pattern is not a conjunction of known-variable
    /// assignments (the caller then falls back to string-name matching —
    /// the native cross-product-state-name path, which is unaffected
    /// because its valuations carry only the non-name-encoded variables).
    fn create_bitset_from_valuations(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        pattern: &str,
    ) -> Option<BitVec<usize, Lsb0>> {
        // Collect all variable names from valuations to find valid splits.
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

        let pairs = Self::split_compound_pairs(pattern, &known_vars)?;

        let mut bits = BitVec::repeat(false, clts.state_count());
        let mut any_match = false;
        for state_id in clts.states() {
            let Some(valuation) = clts.state_valuation(state_id) else {
                continue;
            };
            let all_match = pairs.iter().all(|(var, value)| {
                valuation
                    .get(*var)
                    .map(|sv| Self::value_matches(sv, value))
                    .unwrap_or(false)
            });
            if all_match {
                bits.set(state_id.index(), true);
                any_match = true;
            }
        }

        if any_match { Some(bits) } else { None }
    }

    /// Match a state's valuation string against a pattern value, with
    /// boolean normalization: the native cross-product naming uses `T` / `F`
    /// for boolean-true / -false, while the KMTS valuations store `1` / `0`.
    fn value_matches(state_val: &str, pattern_val: &str) -> bool {
        if state_val == pattern_val {
            return true;
        }
        match pattern_val {
            "T" => state_val == "1" || state_val.eq_ignore_ascii_case("true"),
            "F" => state_val == "0" || state_val.eq_ignore_ascii_case("false"),
            _ => false,
        }
    }

    /// Parse `pattern` as a sequence of `<known_var>_<value>` segments,
    /// where each value runs until the next `_<known_var>_` boundary (the
    /// last value runs to the end). Returns the `(var, value)` pairs, or
    /// `None` when the pattern does not start with a known variable (the
    /// native state-name case — falls back to string matching).
    ///
    /// Greedy longest-known-var match at each position. For
    /// `bvalid_r_T_state_ADDR_WAIT` with known vars `{bvalid_r, state}`:
    /// `[(bvalid_r, "T"), (state, "ADDR_WAIT")]`.
    fn split_compound_pairs<'a>(
        pattern: &'a str,
        known_vars: &std::collections::HashSet<&str>,
    ) -> Option<Vec<(&'a str, &'a str)>> {
        let bytes = pattern.as_bytes();
        // Longest known var that prefixes `pattern[pos..]` immediately
        // followed by a `_`.
        let var_at = |pos: usize| -> Option<&'a str> {
            let mut chosen: Option<&str> = None;
            for &kv in known_vars {
                if pattern[pos..].starts_with(kv)
                    && bytes.get(pos + kv.len()) == Some(&b'_')
                    && chosen.is_none_or(|c| kv.len() > c.len())
                {
                    chosen = Some(kv);
                }
            }
            chosen.map(|kv| &pattern[pos..pos + kv.len()])
        };

        let mut pairs = Vec::new();
        let mut pos = 0;
        while pos < pattern.len() {
            let var = var_at(pos)?;
            let val_start = pos + var.len() + 1; // skip "var_"
            // Find the next `_<known_var>_` boundary at/after val_start.
            let mut val_end = pattern.len();
            let mut scan = val_start;
            while scan < pattern.len() {
                if bytes[scan] == b'_' && var_at(scan + 1).is_some() {
                    val_end = scan;
                    break;
                }
                scan += 1;
            }
            pairs.push((var, &pattern[val_start..val_end]));
            pos = if val_end < pattern.len() {
                val_end + 1
            } else {
                pattern.len()
            };
        }

        if pairs.is_empty() { None } else { Some(pairs) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn kv(names: &[&str]) -> HashSet<&'static str> {
        // Leak to get 'static &str for the test's known-var set.
        names
            .iter()
            .map(|n| Box::leak(n.to_string().into_boxed_str()) as &'static str)
            .collect()
    }

    #[test]
    fn f2_split_single_pair() {
        // `fill_3` → fill == 3 (the common single-pair case).
        let pairs = StateNameMatcher::split_compound_pairs("fill_3", &kv(&["fill"])).unwrap();
        assert_eq!(pairs, vec![("fill", "3")]);
    }

    #[test]
    fn f2_split_single_pair_variant_value_with_underscore() {
        // `data_out_r_IDLE` → data_out_r == IDLE (value has no further var).
        let pairs = StateNameMatcher::split_compound_pairs("data_out_r_IDLE", &kv(&["data_out_r"]))
            .unwrap();
        assert_eq!(pairs, vec![("data_out_r", "IDLE")]);
    }

    #[test]
    fn f2_split_compound_two_pairs() {
        // The F2 case: bvalid_r_T_state_ADDR_WAIT →
        // [(bvalid_r, "T"), (state, "ADDR_WAIT")].
        let pairs = StateNameMatcher::split_compound_pairs(
            "bvalid_r_T_state_ADDR_WAIT",
            &kv(&["bvalid_r", "state"]),
        )
        .unwrap();
        assert_eq!(pairs, vec![("bvalid_r", "T"), ("state", "ADDR_WAIT")]);
    }

    #[test]
    fn f2_split_combinational_two_pairs() {
        // cwe1260: overlap_T_state_IDLE (overlap is a labeled combinational
        // signal) → [(overlap, "T"), (state, "IDLE")].
        let pairs = StateNameMatcher::split_compound_pairs(
            "overlap_T_state_IDLE",
            &kv(&["overlap", "state"]),
        )
        .unwrap();
        assert_eq!(pairs, vec![("overlap", "T"), ("state", "IDLE")]);
    }

    #[test]
    fn f2_split_native_state_name_returns_none() {
        // Native carries only `state` in valuations (bvalid_r is in the
        // state NAME). The compound parse finds no leading known-var →
        // None → caller falls back to string-name matching (unchanged).
        assert!(
            StateNameMatcher::split_compound_pairs("bvalid_r_T_state_ADDR_WAIT", &kv(&["state"]))
                .is_none()
        );
    }

    #[test]
    fn f2_value_matches_boolean_normalization() {
        // T/F ↔ 1/0 normalization for boolean valuations.
        assert!(StateNameMatcher::value_matches("1", "T"));
        assert!(StateNameMatcher::value_matches("0", "F"));
        assert!(StateNameMatcher::value_matches("true", "T"));
        assert!(StateNameMatcher::value_matches("false", "F"));
        assert!(StateNameMatcher::value_matches("ADDR_WAIT", "ADDR_WAIT"));
        assert!(!StateNameMatcher::value_matches("0", "T"));
        assert!(!StateNameMatcher::value_matches("IDLE", "ADDR_WAIT"));
    }
}
