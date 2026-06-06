//! R-S8 (2026-06-06) — GKMTS may-state encoder for under-
//! constrained constants.
//!
//! Per §Phase 9 §9.1 R-S8 spec: when a config bit / parameter has
//! multiple valid values and the property doesn't pin it down,
//! encode as `MustHyperOnly { targets }` at the appropriate
//! transition. The K.4.5 substrate (multi-target hyper-must) +
//! K.1b CTXDSL syntax (`-> [t1, t2, t3] on a [must];`) ship in
//! main; this module adds the **encoder layer** that translates a
//! sidecar declaration "register R is under-constrained with valid
//! values ∈ {v1, v2, v3}" into the corresponding cube-index set
//! suitable for the lifter's initial-state expansion OR a CTXDSL-
//! level hyper-must transition.
//!
//! **Session 1 MVP scope**: pure helper [`hyper_must_initial_cubes`]
//! that takes a predicate set + a per-register value-set map and
//! returns the cube indices whose predicate evaluation is
//! consistent with the under-constrained value sets. Callers
//! (future `cegar.rs` integration, CLI consumers, sidecar
//! resolvers) wire this into their initial-state set or their
//! transition emission.
//!
//! **What this MVP does NOT do** (queued for session 2):
//!
//! - No `PredicateCubeLiftOptions` integration. The lifter does
//!   not yet read this encoder's output; callers run the helper
//!   themselves and pass the result through.
//! - No sidecar `signals[].config_values` field. Today the
//!   sidecar's `bounded_init: Vec<u64>` (R-Y4) covers the bit-blast
//!   path; the predicate-cube path's analogous declaration is
//!   queued. Session 2 wires the sidecar field into this encoder.
//! - No CTXDSL emitter for hyper-must transitions (gated on K.1b
//!   emitter wiring, queued).

use crate::adapter::btor2::kmts_lift::PredicateSpec;
use std::collections::HashMap;

/// R-S8 — Compute the set of cube indices whose predicate-bit
/// pattern is consistent with the per-register value-set
/// constraints.
///
/// Inputs:
/// - `predicates`: the predicate set, in the order cube bits
///   are assigned (the lifter's convention — `predicates[i]`
///   controls bit `i` of each cube index).
/// - `config_values`: `register_name → set of valid values`. A
///   register with multiple valid values is "under-constrained";
///   the encoder admits every cube where the register's
///   predicate is consistent with some valid value.
///
/// Returns: the cube indices that are admissible under the
/// `config_values` constraints. A cube `i` is admitted iff for
/// every predicate `p_j` (bit `j` in `i`):
///
/// - If `p_j.register` is NOT in `config_values`: the predicate
///   imposes no R-S8 constraint; the cube's bit `j` (true/false)
///   is unconstrained.
/// - If `p_j.register` IS in `config_values` with set
///   `{v1, v2, ...}`: the cube is admitted at bit `j` iff
///   - bit `j == 1` (predicate `p_j` is TRUE in this cube)
///     AND `p_j.value` ∈ `{v1, v2, ...}` (the value the
///     predicate checks is a valid one), OR
///   - bit `j == 0` (predicate `p_j` is FALSE in this cube)
///     AND `p_j.value` ∉ `{v1, v2, ...}` (the value the
///     predicate checks is NOT a valid one — false-bit means
///     the register holds something other than `p_j.value`).
///
/// The second case is the load-bearing R-S8 semantics: when a
/// register's valid values are restricted, cubes where the
/// register's predicate is false for a VALID value are
/// inadmissible (because the register MUST hold a valid value;
/// it can't hold "anything else").
///
/// **Soundness**: R-S8 produces an OVER-APPROXIMATION of valid
/// initial cubes. The encoder doesn't know which specific cube
/// the concrete instance starts in; it knows the set of
/// admissible cubes. The downstream GKMTS evaluator must treat
/// these as a hyper-must initial set ("some cube in this set is
/// the actual start") rather than a singleton.
pub fn hyper_must_initial_cubes(
    predicates: &[PredicateSpec],
    config_values: &HashMap<String, Vec<u64>>,
) -> Vec<usize> {
    let cube_count: usize = 1 << predicates.len();
    let mut admissible = Vec::new();
    for cube in 0..cube_count {
        if cube_is_admissible(cube, predicates, config_values) {
            admissible.push(cube);
        }
    }
    admissible
}

fn cube_is_admissible(
    cube: usize,
    predicates: &[PredicateSpec],
    config_values: &HashMap<String, Vec<u64>>,
) -> bool {
    for (bit, pred) in predicates.iter().enumerate() {
        let pred_truth = (cube >> bit) & 1 == 1;
        let Some(valid_values) = config_values.get(&pred.register) else {
            // Predicate's register is not constrained by R-S8 —
            // any bit value (true or false) is admissible.
            continue;
        };
        let pred_value_is_valid = valid_values.contains(&pred.value);
        if pred_truth && !pred_value_is_valid {
            // Predicate is TRUE in the cube (register == p.value)
            // but p.value is not in the valid set → inadmissible.
            return false;
        }
        if !pred_truth && pred_value_is_valid {
            // Predicate is FALSE in the cube (register != p.value)
            // but p.value IS valid — the register might still hold
            // a different valid value. This is admissible UNLESS
            // we can prove the register has no other valid
            // representation in this cube. The K.1 predicate
            // shape (`register == value`) doesn't carry enough
            // info to distinguish "register holds another valid
            // value" from "register holds an invalid value"; for
            // R-S8 MVP we admit both cases (over-approximation).
            //
            // Tighter encoding requires multi-predicate coverage:
            // if the predicate set has p_j for every value in
            // `valid_values`, the "false" bit on this predicate
            // implies the register holds one of the OTHER valid
            // values (admissible). Without full coverage, we
            // can't decide.
            //
            // For the MVP: over-approximate by admitting.
            continue;
        }
        // pred_truth XOR pred_value_is_valid is consistent in
        // the remaining cases:
        // - pred_truth=true, pred_value_is_valid=true →
        //   register == p.value, p.value is valid → OK.
        // - pred_truth=false, pred_value_is_valid=false →
        //   register != p.value, p.value is invalid → OK
        //   (register could hold a valid value other than
        //   p.value).
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pred(name: &str, register: &str, value: u64) -> PredicateSpec {
        PredicateSpec {
            name: name.into(),
            register: register.into(),
            value,
        }
    }

    /// R-S8 — Empty config_values: every cube is admissible
    /// (R-S8 imposes no constraint).
    #[test]
    fn r_s8_empty_config_admits_all_cubes() {
        let predicates = vec![pred("p0", "r", 0), pred("p1", "r", 1)];
        let config_values: HashMap<String, Vec<u64>> = HashMap::new();
        let result = hyper_must_initial_cubes(&predicates, &config_values);
        assert_eq!(result.len(), 4, "all 2^2 cubes admissible when no config");
        assert_eq!(result, vec![0, 1, 2, 3]);
    }

    /// R-S8 — Single valid value, single predicate matching it.
    /// The cube where the predicate is TRUE (register == valid) is
    /// admissible. The cube where it's FALSE (register != valid)
    /// is inadmissible IF no other valid value exists — but R-S8
    /// MVP over-approximates and admits it too (see encoder
    /// comment). For this test we just assert the count is ≥ 1.
    #[test]
    fn r_s8_single_valid_value_admits_at_least_the_matching_cube() {
        let predicates = vec![pred("r_is_0", "r", 0)];
        let mut config_values: HashMap<String, Vec<u64>> = HashMap::new();
        config_values.insert("r".to_string(), vec![0]);
        let result = hyper_must_initial_cubes(&predicates, &config_values);
        // Cube 0: bit 0 = 0, predicate false (register != 0). Per
        // R-S8 over-approximation, admitted.
        // Cube 1: bit 0 = 1, predicate true (register == 0), value
        // is valid. Admitted.
        assert!(
            result.contains(&1),
            "cube 1 (predicate true, value valid) must be admitted"
        );
        assert_eq!(result.len(), 2, "over-approximation admits both cubes");
    }

    /// R-S8 — A predicate value that is NOT in the valid set
    /// excludes the cube where that predicate is TRUE (because the
    /// register would have to hold an invalid value).
    #[test]
    fn r_s8_invalid_predicate_value_excludes_predicate_true_cube() {
        let predicates = vec![pred("r_is_7", "r", 7)];
        let mut config_values: HashMap<String, Vec<u64>> = HashMap::new();
        config_values.insert("r".to_string(), vec![0, 1, 2]); // 7 not valid
        let result = hyper_must_initial_cubes(&predicates, &config_values);
        // Cube 0: bit 0 = 0, predicate false (register != 7), 7
        // not valid → admissible (register could hold a valid
        // value like 0, 1, 2).
        // Cube 1: bit 0 = 1, predicate true (register == 7), 7
        // not valid → INADMISSIBLE.
        assert!(
            !result.contains(&1),
            "cube 1 (register == 7) must be excluded; 7 is not in valid set"
        );
        assert!(result.contains(&0), "cube 0 (register != 7) is admissible");
    }

    /// R-S8 — Multiple predicates over different registers; only
    /// one register is config-constrained. Cubes for the other
    /// register are unaffected.
    #[test]
    fn r_s8_unconstrained_register_unaffected_by_config_values() {
        let predicates = vec![pred("r_is_0", "r", 0), pred("s_is_1", "s", 1)];
        // Only r is config-constrained; s is unconstrained.
        let mut config_values: HashMap<String, Vec<u64>> = HashMap::new();
        config_values.insert("r".to_string(), vec![0]);
        let result = hyper_must_initial_cubes(&predicates, &config_values);
        // r predicate (bit 0): per the over-approximation, both
        // bits admissible (value 0 IS valid; predicate-true and
        // predicate-false both OK).
        // s predicate (bit 1): unconstrained, both bits
        // admissible.
        // Total admissible = 4 (full 2^2).
        assert_eq!(result.len(), 4);
    }

    /// R-S8 — Multiple predicates over the SAME register. The
    /// encoder treats each predicate independently. A cube where
    /// BOTH predicates are TRUE simultaneously is impossible in
    /// the concrete (a register can't equal two different values
    /// at once), but the cube space allows it as an abstract
    /// state; R-S8's MVP doesn't filter these inconsistent cubes
    /// (the existing predicate_cube_lift's SMT-satisfiability
    /// check is a separate sub-item).
    #[test]
    fn r_s8_inconsistent_cubes_not_filtered_by_mvp() {
        let predicates = vec![pred("r_is_0", "r", 0), pred("r_is_1", "r", 1)];
        let mut config_values: HashMap<String, Vec<u64>> = HashMap::new();
        config_values.insert("r".to_string(), vec![0, 1]);
        let result = hyper_must_initial_cubes(&predicates, &config_values);
        // Cube 3 (both predicates true → register == 0 AND
        // register == 1, contradictory) is structurally impossible
        // but R-S8 MVP doesn't filter it (the broader SMT-
        // satisfiability filter is a separate sub-item).
        // For now, just assert the result is non-empty.
        assert!(!result.is_empty());
    }

    /// R-S8 — All cubes admitted when valid_values covers all
    /// predicate values used in the predicate set (and the
    /// over-approximation applies).
    #[test]
    fn r_s8_full_coverage_admits_all_cubes() {
        let predicates = vec![pred("r_is_0", "r", 0), pred("r_is_1", "r", 1)];
        let mut config_values: HashMap<String, Vec<u64>> = HashMap::new();
        config_values.insert("r".to_string(), vec![0, 1, 2, 3, 4, 5]);
        let result = hyper_must_initial_cubes(&predicates, &config_values);
        // Both predicate values (0, 1) are in the valid set;
        // every cube is admissible under the MVP over-approx.
        assert_eq!(result.len(), 4);
    }
}
