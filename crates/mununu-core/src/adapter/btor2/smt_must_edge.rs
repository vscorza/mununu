//! R.2.5b session 2 (2026-06-08) — SMT-backed must-edge query.
//!
//! Replaces session 1's [`MustEdgeInference::SamplingConfluence`]
//! sampling-based promotion with a Z3 BV theory check. Given a source
//! cube + a target cube + the BTOR2 transition relation, the query
//! asks:
//!
//! ```text
//! ∀ (state ⊨ src_cube). ∀ inputs. (transition(state, inputs, next) ⟹ next ⊨ tgt_cube)
//! ```
//!
//! This is **stronger** than the standard KMTS must-edge definition
//! (∀ state ⊨ src. ∃ input. next ⊨ tgt) — the session 2 MVP proves
//! "deterministic transition into tgt regardless of input." The
//! stronger definition is sound for KMTS preservation (every promoted
//! must-edge is also a standard must-edge); the MVP simply produces
//! *fewer* must-edges than the standard form would. The standard
//! ∀∃ form is queued for the R.2.5b session-2 follow-up. Both forms
//! are sound; the MVP trades some precision for a simpler encoding.
//!
//! The query is encoded as the UNSAT check of the negation:
//!
//! ```text
//! src_constraints(state_curr) ∧ transition ∧ ¬tgt_constraints(state_next)
//! ```
//!
//! If Z3 returns UNSAT, no (state, input) combination exists that
//! starts in src and escapes tgt — the must-edge holds. If SAT,
//! some (state, input) combination escapes, so the must-edge does
//! not hold. UNKNOWN (timeout) is treated as "does not hold" —
//! conservative for soundness (UNDER-approximation of R_must is
//! sound for KMTS preservation).
//!
//! # SOUNDNESS
//!
//! - Every promoted Sharp edge is sound — Z3's UNSAT verdict on the
//!   negation guarantees the must condition (in the MVP's stronger
//!   form). Missing must-edges (Z3 returns SAT or UNKNOWN) are
//!   conservative — they leave the edge as MayOnly, which is the
//!   safe direction.
//! - The encoder ([`crate::adapter::sidecar::predicate_image::btor2_encode`])
//!   is exact for every operator it supports. Operators outside its
//!   support set are rejected at encode time; the must-edge check
//!   simply returns [`SmtMustVerdict::Unknown`] in that case.

use std::collections::HashMap;

use crate::adapter::btor2::ast::Nid;
use crate::adapter::sidecar::predicate_image::btor2_encode::{Btor2SmtView, SignalKind};

/// Verdict from a single SMT must-edge query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtMustVerdict {
    /// Z3 UNSAT on the negation — the must-edge condition holds
    /// (the MVP's stronger ∀∀ form). Caller promotes MayOnly →
    /// Sharp.
    Must,
    /// Z3 SAT on the negation — some (state, input) escapes the
    /// target cube. The must-edge does not hold (in the MVP form);
    /// caller leaves the edge as MayOnly.
    NotMust,
    /// Z3 UNKNOWN (timeout, unsupported operator, missing register)
    /// — the verdict is indeterminate. Caller treats as NotMust
    /// (conservative for must-edge soundness).
    Unknown,
}

/// Build a `state_curr` BV equality constraint for one predicate
/// over the given cube bit (`1` ⇒ predicate true ⇒ register == value;
/// `0` ⇒ predicate false ⇒ register != value).
///
/// Returns `None` when the predicate's register has no matching
/// state-cell BV in the encoded view (typically because the BTOR2
/// file does not declare the register or because it was rewritten
/// away by an earlier pass).
fn build_predicate_constraint(bv: &z3::ast::BV, value: u64, polarity: bool) -> z3::ast::Bool {
    let width = bv.get_size();
    // R.2.5b session 2 — mask the unsigned value to the BV width.
    // Widths < 64 truncate; width == 64 passes through.
    let mask: u64 = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let bits = value & mask;
    let val_bv = z3::ast::BV::from_u64(bits, width);
    let eq = bv.eq(&val_bv);
    if polarity { eq } else { eq.not() }
}

/// Build a register-name → state-cell-NID lookup from the encoded
/// view's signal table. The keys are the BTOR2 symbol strings
/// (matching [`crate::adapter::btor2::kmts_lift::PredicateSpec::register`]).
pub fn build_register_nid_map(view: &Btor2SmtView) -> HashMap<String, Nid> {
    let mut map = HashMap::new();
    for sig in &view.signals {
        if sig.kind == SignalKind::State
            && let Some(symbol) = &sig.symbol
        {
            map.insert(symbol.clone(), sig.nid);
        }
    }
    map
}

/// R.2.5b session 2 — run the SMT must-edge query for one
/// (source-cube, target-cube) pair.
///
/// `src_bits` and `tgt_bits` are bitmasks: bit `i` of `src_bits` is
/// `1` iff predicate `predicates[i]` holds in the source cube;
/// likewise for the target cube. The view's
/// [`Btor2SmtView::transition`] is conjoined with the source +
/// negated-target constraints; UNSAT proves the must-edge.
///
/// `nid_map` maps each predicate's `register` symbol to its
/// state-cell NID; predicates whose register is absent from the
/// map cause the query to return [`SmtMustVerdict::Unknown`].
///
/// `timeout_ms` is applied per-query via Z3's `solver.timeout`
/// parameter; UNKNOWN verdicts include timeout cases.
///
/// **Caller must hold a [`z3::with_z3_config`] scope.**
pub fn smt_per_target_must_check<P>(
    view: &Btor2SmtView,
    src_bits: u64,
    tgt_bits: u64,
    predicates: &[P],
    nid_map: &HashMap<String, Nid>,
    timeout_ms: u32,
) -> SmtMustVerdict
where
    P: PredicateLike,
{
    let mut src_constraints: Vec<z3::ast::Bool> = Vec::new();
    let mut tgt_constraints: Vec<z3::ast::Bool> = Vec::new();

    for (i, pred) in predicates.iter().enumerate() {
        let Some(&nid) = nid_map.get(pred.register()) else {
            return SmtMustVerdict::Unknown;
        };
        let Some(curr_bv) = view.curr_state(nid) else {
            return SmtMustVerdict::Unknown;
        };
        let Some(next_bv) = view.next_state(nid) else {
            return SmtMustVerdict::Unknown;
        };

        let src_polarity = (src_bits >> i) & 1 == 1;
        let tgt_polarity = (tgt_bits >> i) & 1 == 1;

        src_constraints.push(build_predicate_constraint(
            curr_bv,
            pred.value(),
            src_polarity,
        ));
        tgt_constraints.push(build_predicate_constraint(
            next_bv,
            pred.value(),
            tgt_polarity,
        ));
    }

    let solver = z3::Solver::new();
    let mut params = z3::Params::new();
    params.set_u32("timeout", timeout_ms);
    solver.set_params(&params);

    solver.assert(&view.transition);
    for c in &src_constraints {
        solver.assert(c);
    }
    // ¬tgt_constraints = ¬(∧ tgt_i) = ∨ ¬tgt_i
    let tgt_negs: Vec<z3::ast::Bool> = tgt_constraints.iter().map(|c| c.not()).collect();
    let tgt_negs_refs: Vec<&z3::ast::Bool> = tgt_negs.iter().collect();
    let neg_tgt = if tgt_negs_refs.is_empty() {
        // Empty predicate set — target cube is universal (all bits
        // 0 in a 0-predicate world). Then ¬tgt is `false`, so the
        // conjunction is unsatisfiable trivially → Must.
        z3::ast::Bool::from_bool(false)
    } else {
        z3::ast::Bool::or(&tgt_negs_refs)
    };
    solver.assert(&neg_tgt);

    match solver.check() {
        z3::SatResult::Unsat => SmtMustVerdict::Must,
        z3::SatResult::Sat => SmtMustVerdict::NotMust,
        z3::SatResult::Unknown => SmtMustVerdict::Unknown,
    }
}

/// Tiny trait surface so the must-edge check can consume either
/// `PredicateSpec` (from `kmts_lift`) or test-local predicate types
/// without taking on a kmts_lift dependency here.
pub trait PredicateLike {
    fn register(&self) -> &str;
    fn value(&self) -> u64;
}

impl PredicateLike for crate::adapter::btor2::kmts_lift::PredicateSpec {
    fn register(&self) -> &str {
        &self.register
    }
    fn value(&self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::kmts_lift::PredicateSpec;
    use crate::adapter::btor2::parser::parse;
    use crate::adapter::sidecar::predicate_image::btor2_encode::encode_design;

    /// R.2.5b session 2 — deterministic transition `reg_a := 0`
    /// (input is dropped). Predicate `p:reg_a==0`. A transition
    /// from cube `{p}` (i.e. reg_a==0) goes deterministically
    /// to cube `{p}` again — Z3 must prove this Sharp.
    const DETERMINISTIC_ZERO_BTOR2: &str = "\
1 sort bitvec 1
2 state 1 reg_a
3 zero 1
4 init 1 2 3
5 next 1 2 3
";

    /// R.2.5b session 2 — input-driven transition `reg_a := in_a`.
    /// Predicate `p:reg_a==0`. From src cube `{p}`, the target
    /// depends on input: input=0 → tgt `{p}`; input=1 → tgt `{¬p}`.
    /// Z3 must conclude NotMust on the src→`{p}` query (input=1
    /// escapes).
    const INPUT_DRIVEN_BTOR2: &str = "\
1 sort bitvec 1
2 input 1 in_a
3 state 1 reg_a
4 zero 1
5 init 1 3 4
6 next 1 3 2
";

    fn run_check<F: FnOnce() -> SmtMustVerdict + Send + Sync>(f: F) -> SmtMustVerdict {
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, f)
    }

    #[test]
    fn smt_must_check_deterministic_zero_proves_must() {
        let file = parse(DETERMINISTIC_ZERO_BTOR2).expect("parse deterministic fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode deterministic");
            let nid_map = build_register_nid_map(&view);
            // src cube = {p}: bit 0 set → src_bits = 0b1.
            // tgt cube = {p}: bit 0 set → tgt_bits = 0b1.
            smt_per_target_must_check(&view, 0b1, 0b1, &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::Must,
            "deterministic reg_a := 0 with src=tgt={{p}} must prove Sharp; got {verdict:?}"
        );
    }

    #[test]
    fn smt_must_check_input_driven_rejects_must() {
        let file = parse(INPUT_DRIVEN_BTOR2).expect("parse input-driven fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode input-driven");
            let nid_map = build_register_nid_map(&view);
            // src cube = {p}: src_bits = 0b1.
            // tgt cube = {p}: tgt_bits = 0b1.
            // input=1 → reg_a:=1 → ¬p in next-state → escapes tgt.
            smt_per_target_must_check(&view, 0b1, 0b1, &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::NotMust,
            "input-driven reg_a := in_a with src=tgt={{p}} must reject Sharp; got {verdict:?}"
        );
    }

    #[test]
    fn smt_must_check_unknown_register_returns_unknown() {
        let file = parse(DETERMINISTIC_ZERO_BTOR2).expect("parse fixture");
        // Predicate references a register that doesn't exist
        // in the BTOR2 source.
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "ghost_reg".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode fixture");
            let nid_map = build_register_nid_map(&view);
            smt_per_target_must_check(&view, 0b1, 0b1, &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::Unknown,
            "missing register must yield Unknown; got {verdict:?}"
        );
    }

    #[test]
    fn build_register_nid_map_collects_state_signals() {
        let file = parse(INPUT_DRIVEN_BTOR2).expect("parse fixture");
        run_check(|| {
            let view = encode_design(&file).expect("encode fixture");
            let nid_map = build_register_nid_map(&view);
            assert!(
                nid_map.contains_key("reg_a"),
                "reg_a state-cell must appear; got keys: {:?}",
                nid_map.keys().collect::<Vec<_>>()
            );
            // Inputs must NOT appear (they are constants per step,
            // not predicate-targetable state cells).
            assert!(
                !nid_map.contains_key("in_a"),
                "input in_a must NOT appear; got keys: {:?}",
                nid_map.keys().collect::<Vec<_>>()
            );
            SmtMustVerdict::Must
        });
    }
}
