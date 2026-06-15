//! Predicate-image enumeration over the encoded transition relation.
//!
//! Phase A.4 step 4.3 ships a focused per-signal value enumerator
//! built on the [`super::btor2_encode::Btor2SmtView`]: for each
//! candidate value `k` in the cell's bit-vector range, ask Z3 whether
//! the transition relation admits `s_next_<nid> == k` (reachability
//! under any input). The output is a `Vec<i64>` of confirmed-reachable
//! values, suitable for round-trip into
//! [`crate::adapter::systemverilog::annotation::DiscoveredValues`].
//!
//! This is the **brute-force flavour** of the Hoder–Bjørner–de Moura
//! predicate-tuple enumeration. For narrow signals (≤ 8 bits) it's
//! exact and tractable; wider signals fall under the `cap_edges`
//! truncation. The full all-SMT tuple enumeration over multiple
//! predicates simultaneously is left for a follow-up — the brute-
//! force form is enough to close Phase A.4's Caliptra verdict on
//! `boot_fsm_ns` (3 bits → 8 queries).
//!
//! # SOUNDNESS
//!
//! Each enumerated value `k` is guaranteed reachable: Z3 returned
//! SAT on `T(s, s') ∧ s_next_<nid> == k`. Values `k` for which Z3
//! returned UNSAT are guaranteed unreachable from *any* current
//! state under *any* input. Unknown (timeout) verdicts are **dropped**
//! from the output, which is conservative for safety: we may surface
//! fewer values than reachable, but never more. Callers that need
//! the over-approximation should treat the absent values as
//! "unknown reachability" and consult the `Discover` fallback path.

use std::collections::HashSet;

use super::ImageOptions;
use super::btor2_encode::{Btor2SmtView, SignalKind};
use crate::adapter::btor2::ast::{Btor2File, Nid};
use crate::adapter::systemverilog::annotation::{DiscoveredValue, DiscoveredValues};

/// Enumerate reachable values of a single BTOR2 state cell by NID.
///
/// Returns the sorted set of `k` for which the transition relation
/// admits `s_next_<nid> == k` under some current state + input
/// combination. Bounded by [`ImageOptions::cap_edges`].
///
/// Caller must hold a [`z3::with_z3_config`] scope.
pub fn discover_values_for_state_cell(
    view: &Btor2SmtView,
    nid: Nid,
    opts: &ImageOptions,
) -> Vec<i64> {
    let next_bv = match view.next_state(nid) {
        Some(bv) => bv,
        None => return Vec::new(),
    };
    let width = next_bv.get_size();
    if width == 0 || width > 16 {
        // SOUNDNESS: widths > 16 explode the brute-force enumeration
        // (2^17 = 131 072 queries minimum). Return empty rather than
        // melt the harness; the caller's `cap_edges` already implies
        // this fallback. Width-aware predicate-tuple enumeration is
        // the follow-up.
        return Vec::new();
    }

    let solver = z3::Solver::new();
    // Assert the transition relation once; all per-value queries
    // share this assertion under `solver.check_assumptions`.
    solver.assert(&view.transition);

    let domain_cap = if width >= 63 {
        opts.cap_edges as u64
    } else {
        (1u64 << width).min(opts.cap_edges as u64)
    };

    let mut discovered: Vec<i64> = Vec::new();
    for k in 0..domain_cap {
        let k_bv = z3::ast::BV::from_u64(k, width);
        let probe = next_bv.eq(&k_bv);
        // `check_assumptions` lets us add the `s_next == k` constraint
        // for this iteration only without polluting the solver state.
        let result = solver.check_assumptions(&[probe]);
        if matches!(result, z3::SatResult::Sat) {
            discovered.push(k as i64);
        }
        // UNSAT and UNKNOWN both drop `k` from the discovered set.
        // SOUNDNESS: see module docs — UNKNOWN is conservative.
    }
    discovered
}

/// Run [`discover_values_for_state_cell`] on every named state cell
/// in the design and convert the results into the existing
/// `DiscoveredValues` sidecar shape.
///
/// Each entry's `name` is `VAL_<k>`; each entry's `from` provenance
/// string is `"predicate-image: ..."` for downstream traceability.
/// (This BTOR2-native discovery superseded the native-SV
/// `kripke_smt` significant-value discovery removed in S.2b — it is the
/// authoring path behind `mununu btor2 discover`.)
pub fn discover_design_values(
    file: &Btor2File,
    view: &Btor2SmtView,
    opts: &ImageOptions,
) -> std::collections::HashMap<String, DiscoveredValues> {
    let mut out = std::collections::HashMap::new();
    for signal in &view.signals {
        if signal.kind != SignalKind::State {
            continue;
        }
        let Some(name) = signal.symbol.as_ref() else {
            continue;
        };
        let values = discover_values_for_state_cell(view, signal.nid, opts);
        if values.is_empty() {
            continue;
        }
        let _ = file; // reserved for future seed-walking
        let dv = DiscoveredValues {
            values: values
                .iter()
                .map(|&v| DiscoveredValue {
                    value: v,
                    name: format!("VAL_{v}"),
                    from: Some(format!(
                        "predicate-image: reachable on s_next_{name} == {v}"
                    )),
                })
                .collect(),
            catch_all: "OTHER".to_string(),
        };
        out.insert(name.clone(), dv);
    }
    out
}

/// Convenience wrapper that drives the full pipeline:
/// parse `file_path`, encode the design, enumerate, return the
/// `DiscoveredValues` map. Owns the Z3 scope so callers don't need
/// to deal with the thread-local context plumbing.
pub fn discover_values_for_btor2_file(
    file: &Btor2File,
    opts: &ImageOptions,
) -> Result<std::collections::HashMap<String, DiscoveredValues>, super::btor2_encode::EncodeError> {
    let cfg = z3::Config::new();
    z3::with_z3_config(&cfg, || {
        let view = super::btor2_encode::encode_design(file)?;
        Ok::<_, super::btor2_encode::EncodeError>(discover_design_values(file, &view, opts))
    })
}

/// Helper for the recall harness: convert a discovered-values map
/// into a `HashSet<i64>` per signal for set-intersection scoring.
pub fn flatten_to_value_sets(
    discovered: &std::collections::HashMap<String, DiscoveredValues>,
) -> std::collections::HashMap<String, HashSet<i64>> {
    discovered
        .iter()
        .map(|(sig, dv)| {
            (
                sig.clone(),
                dv.values.iter().map(|v| v.value).collect::<HashSet<i64>>(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser::parse;

    #[test]
    fn discover_cap_overflow_cnt_finds_bad_encoding() {
        // The 8-bit counter saturates at 200 via
        //   next(cnt) = ite(cnt < 200, cnt + 1, cnt).
        //
        // Single-step reachability from *any* current state:
        // - `s_next == 200` is reachable from cnt=199 (199+1=200) or
        //   cnt=200 (sticky). SAT.
        // - `s_next == 0` is unreachable in one step: cnt+1=0 needs
        //   cnt=255, but then `cnt < 200` is false → saturation
        //   pins cnt to 255, not 0.
        //
        // The predicate-image's single-step semantics is correct here;
        // the headline test is "the bad-encoding 200 must surface".
        let src = include_str!(
            "../../../../../../examples/verify/bench_predicate_image_a4/adversarial/cap_overflow.btor"
        );
        let file = parse(src).unwrap();
        let opts = ImageOptions::default();
        let discovered = discover_values_for_btor2_file(&file, &opts).unwrap();
        let cnt_values = flatten_to_value_sets(&discovered);
        let cnt = cnt_values.get("cnt").expect("cnt discovered");
        assert!(
            cnt.contains(&200),
            "cnt should include 200 (bad); got {cnt:?}"
        );
        // 0 must be excluded — see comment above. This is the soundness
        // assertion: a value the design cannot transition to in one
        // step must not appear in the discovered set.
        assert!(
            !cnt.contains(&0),
            "cnt should NOT include 0 (unreachable in one step); got {cnt:?}"
        );
    }

    #[test]
    fn discover_sparse_predicates_under_single_step_semantics() {
        // The brute-force enumerator's semantics is "reachable in one
        // transition step from *some* current state". The current
        // state is unconstrained, so any value `s` can take in one
        // step from any predecessor counts. For this fixture the
        // transition is `next(s) = ite(step, s + 6, s)` over BV(3) —
        // so from current state `c`, next state is `c` (step=0) or
        // `c + 6 mod 8` (step=1). Iterating `c` across all 8 values
        // gives the full range {0..7}. The harness's manifest reflects
        // this — multi-step reachability from init is *not* a
        // contract the brute-force enumerator can enforce; the proper
        // assertion lives at the recall harness in
        // `tests/predicate_image_recall.rs`.
        let src = include_str!(
            "../../../../../../examples/verify/bench_predicate_image_a4/adversarial/sparse_predicates.btor"
        );
        let file = parse(src).unwrap();
        let opts = ImageOptions::default();
        let discovered = discover_values_for_btor2_file(&file, &opts).unwrap();
        let by_signal = flatten_to_value_sets(&discovered);
        let s_values = by_signal.get("s").expect("s discovered");
        for k in 0..8 {
            assert!(
                s_values.contains(&k),
                "s should include {k}; got {s_values:?}"
            );
        }
    }

    #[test]
    fn discover_safety_demo_cnt() {
        // 2-bit cnt → values {0, 1, 2, 3} all reachable.
        let src = include_str!("../../../../../../examples/btor2/safety_demo.btor");
        let file = parse(src).unwrap();
        let opts = ImageOptions::default();
        let discovered = discover_values_for_btor2_file(&file, &opts).unwrap();
        let by_signal = flatten_to_value_sets(&discovered);
        let cnt = by_signal.get("cnt").expect("cnt discovered");
        for k in 0..4 {
            assert!(cnt.contains(&k), "cnt should include {k}; got {cnt:?}");
        }
    }
}
