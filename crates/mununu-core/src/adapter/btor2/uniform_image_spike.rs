//! H.U.0 — uniform predicate-image spike (go/no-go validation; **test-only**).
//!
//! The H.E soundness review (2026-06-29) found that the production
//! predicate-image (`smt_must_edge::build_pred_constraint`) branches per
//! *atom kind* — a simple state atom, a free-input atom (source-pin /
//! target-free, H.B), a compound — and that combinational-of-input atoms had to
//! be SKIPPED because the per-kind path has no next-cycle term for them. The
//! principled fix (H.U) is a **uniform** image: every predicate is just a
//! comparison over an arbitrary *term*, encoded as
//!
//! - **source** = the term's value over the current cycle `(s, i)` — the
//!   encoder's existing per-node cache (`signal_bvs`), plus `state_curr` /
//!   `inputs` for leaves;
//! - **target** = the term's value over the next cycle `(s', i')`, where `s'`
//!   is `state_next` and `i'` are FRESH next-cycle inputs — the "primed node
//!   cache" ([`encode_primed`]).
//!
//! This spike implements that one rule for the **may** check (a clean `∃`) and
//! the **non-`i'`-dependent** part of the **must** check (the existing `∀∃`
//! shape, sound for state + combinational-of-state targets), and:
//!
//! 1. **differential-tests it against the current per-kind path on state-only
//!    predicates** — verdicts must match exactly (else the refactor would change
//!    verdicts), and
//! 2. **demonstrates the new capability from the same rule** — a precise may
//!    relation for a combinational-of-state predicate (where the per-kind path
//!    falls back to conservative `May`), and the free-input source-pin /
//!    target-free behaviour (H.B) re-expressed via the fresh `i'`.
//!
//! ## Go/no-go finding (the reason this module exists)
//!
//! **GO.** The primed node cache reproduces the per-kind path on state-only
//! (by construction: a register leaf's primed value *is* `state_next[reg]`), and
//! one rule subsumes the free-input and combinational-of-state cases. The single
//! remaining piece for H.U.1 is the **`i'`-dependent must** target (a free input
//! or a combinational-of-input at the consequent): its sound encoding is a
//! nested `∀ s ∈ src. ∃ (inputs, state_next). ∀ i'. (transition ∧ tgt(s', i'))`
//! — the innermost `∀ i'` is what makes "for all next-cycle inputs" sound, and
//! collapsing it into the outer `∀` block (the naive encoding) would make `i'`
//! *existential* in the must condition, which is UNSOUND (it would fabricate
//! must-edges). This spike therefore returns `Unknown` (conservative) for an
//! `i'`-dependent must target rather than risk that error; H.U.1 implements the
//! nested quantifier, classified by [`cone_reaches_input`], with the state-only
//! differential here + the H.O concrete oracle as the regression guards.

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
use crate::adapter::btor2::ast::Btor2File;
#[cfg(test)]
use crate::adapter::btor2::ast::Nid;
#[cfg(test)]
use crate::adapter::btor2::parser::cone_reaches_input;
#[cfg(test)]
use crate::adapter::btor2::smt_must_edge::{SmtMayVerdict, SmtMustVerdict};
#[cfg(test)]
use crate::adapter::sidecar::predicate_image::btor2_encode::{Btor2SmtView, PrimedEnv};

/// One predicate of the spike: `term ⋈ value`, the comparison `==` with the
/// per-cube polarity supplied separately. `term` is the BTOR2 nid of an
/// arbitrary node (a state cell, an input, or a combinational op).
#[cfg(test)]
struct Pred {
    nid: Nid,
    value: u64,
}

/// `(bv == value)`, negated when `polarity` is false (the cube's `¬predicate`).
/// Mirrors `smt_must_edge::build_predicate_constraint` (mask to width + `.eq()`).
#[cfg(test)]
fn pred_constraint(bv: &z3::ast::BV, value: u64, polarity: bool) -> z3::ast::Bool {
    let width = bv.get_size();
    let mask: u64 = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let v = z3::ast::BV::from_u64(value & mask, width);
    let eq = bv.eq(&v);
    if polarity { eq } else { eq.not() }
}

/// **The uniform rule, source side.** The term's value over the current cycle:
/// a state leaf (`state_curr`), an input leaf (`inputs`), or a combinational
/// op (`signal_bvs`, the encoder's per-node cache). One lookup, no per-kind
/// branch.
#[cfg(test)]
fn term_source_bv(view: &Btor2SmtView, nid: Nid) -> Option<&z3::ast::BV> {
    view.state_curr
        .get(&nid)
        .or_else(|| view.inputs.get(&nid))
        .or_else(|| view.signal_bvs.get(&nid))
}

/// **The uniform rule, target side.** The term's value over the next cycle
/// `(s', i')`: a state leaf (`state_next`), an input leaf (fresh `i'`), or a
/// combinational op (the primed node cache). Same shape as `term_source_bv`,
/// over the primed projection.
#[cfg(test)]
fn term_target_bv<'a>(
    view: &'a Btor2SmtView,
    primed: &'a PrimedEnv,
    nid: Nid,
) -> Option<&'a z3::ast::BV> {
    view.state_next
        .get(&nid)
        .or_else(|| primed.inputs.get(&nid))
        .or_else(|| primed.cache.get(&nid))
}

/// Uniform **may** check: `∃ s, i, s', i'. src(s,i) ∧ tgt(s',i') ∧ transition`.
/// The fresh `i'` are free (existential) consts, so a target predicate over an
/// input / combinational-of-input is satisfiable across input flavours — the
/// sound over-approximation (it can only ADD may-edges). An unresolved term →
/// conservative `May` (never drop an edge we cannot rule out), matching the
/// per-kind path.
#[cfg(test)]
fn uniform_may_check(
    view: &Btor2SmtView,
    primed: &PrimedEnv,
    src_bits: u64,
    tgt_bits: u64,
    preds: &[Pred],
    timeout_ms: u32,
) -> SmtMayVerdict {
    let mut constraints: Vec<z3::ast::Bool> = Vec::new();
    for (i, p) in preds.iter().enumerate() {
        let sp = (src_bits >> i) & 1 == 1;
        let tp = (tgt_bits >> i) & 1 == 1;
        let (Some(s), Some(t)) = (
            term_source_bv(view, p.nid),
            term_target_bv(view, primed, p.nid),
        ) else {
            return SmtMayVerdict::May;
        };
        constraints.push(pred_constraint(s, p.value, sp));
        constraints.push(pred_constraint(t, p.value, tp));
    }
    let solver = z3::Solver::new();
    let mut params = z3::Params::new();
    params.set_u32("timeout", timeout_ms);
    solver.set_params(&params);
    solver.assert(&view.transition);
    for c in &constraints {
        solver.assert(c);
    }
    match solver.check() {
        z3::SatResult::Sat => SmtMayVerdict::May,
        z3::SatResult::Unsat => SmtMayVerdict::NotMay,
        z3::SatResult::Unknown => SmtMayVerdict::May,
    }
}

/// Uniform **must** check (spike scope): the standard `∀∃` form
/// `∀ s∈src. ∃ inputs, state_next. (transition ∧ tgt(s'))`, encoded as
/// `src(s) ∧ ∀[inputs, state_next]. ¬(transition ∧ tgt)` (SAT ⇒ NotMust).
///
/// **Sound scope.** A target predicate whose term depends on a next-cycle input
/// (`cone_reaches_input`, or the term IS an input) needs the nested `∀ i'` that
/// only H.U.1 implements; the spike returns `Unknown` (conservative) rather than
/// the naive collapse that would make `i'` existential (unsound). For state +
/// combinational-of-state targets the term is a function of `state_next` alone,
/// so this `∀∃` form is exact — identical to the per-kind path on state atoms.
#[cfg(test)]
fn uniform_must_check(
    file: &Btor2File,
    view: &Btor2SmtView,
    primed: &PrimedEnv,
    src_bits: u64,
    tgt_bits: u64,
    preds: &[Pred],
    timeout_ms: u32,
) -> SmtMustVerdict {
    let mut src: Vec<z3::ast::Bool> = Vec::new();
    let mut tgt: Vec<z3::ast::Bool> = Vec::new();
    for (i, p) in preds.iter().enumerate() {
        let sp = (src_bits >> i) & 1 == 1;
        let tp = (tgt_bits >> i) & 1 == 1;
        // i'-dependent target → out of spike scope (needs the nested ∀ i').
        if view.inputs.contains_key(&p.nid) || cone_reaches_input(file, p.nid) {
            return SmtMustVerdict::Unknown;
        }
        let (Some(s), Some(t)) = (
            term_source_bv(view, p.nid),
            term_target_bv(view, primed, p.nid),
        ) else {
            return SmtMustVerdict::Unknown;
        };
        src.push(pred_constraint(s, p.value, sp));
        tgt.push(pred_constraint(t, p.value, tp));
    }
    let tgt_conj = if tgt.is_empty() {
        z3::ast::Bool::from_bool(true)
    } else {
        z3::ast::Bool::and(&tgt.iter().collect::<Vec<_>>())
    };
    let inner_body = z3::ast::Bool::and(&[&view.transition, &tgt_conj]).not();

    // ∀ bound: transition inputs + state_next (the current per-kind path's bound;
    // no i' is added because every i'-dependent target was rejected above).
    let mut bound: Vec<z3::ast::BV> = Vec::new();
    for bv in view.inputs.values() {
        bound.push(bv.clone());
    }
    for bv in view.state_next.values() {
        bound.push(bv.clone());
    }
    let bound_refs: Vec<&dyn z3::ast::Ast> =
        bound.iter().map(|bv| bv as &dyn z3::ast::Ast).collect();
    let universal = z3::ast::forall_const(&bound_refs, &[], &inner_body);

    let solver = z3::Solver::new();
    let mut params = z3::Params::new();
    params.set_u32("timeout", timeout_ms);
    solver.set_params(&params);
    for c in &src {
        solver.assert(c);
    }
    solver.assert(&universal);
    match solver.check() {
        z3::SatResult::Unsat => SmtMustVerdict::Must,
        z3::SatResult::Sat => SmtMustVerdict::NotMust,
        z3::SatResult::Unknown => SmtMustVerdict::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser::parse;
    use crate::adapter::btor2::smt_must_edge::{
        PredicateLike, build_register_nid_map_with_inputs, smt_per_target_may_check,
        smt_per_target_must_check_standard,
    };
    use crate::adapter::sidecar::predicate_image::btor2_encode::{
        SignalKind, encode_design, encode_primed,
    };

    // A simple `register == value` predicate for the per-kind path differential.
    struct Spec {
        register: String,
        value: u64,
    }
    impl PredicateLike for Spec {
        fn register(&self) -> &str {
            &self.register
        }
        fn value(&self) -> u64 {
            self.value
        }
    }

    fn with_z3<R: Send + Sync>(f: impl FnOnce() -> R + Send + Sync) -> R {
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, f)
    }

    /// Find a term's nid by symbol: state / input via the nid-map, else a named
    /// combinational signal from the encoded `signals`.
    fn term_nid(view: &Btor2SmtView, nid_map: &HashMap<String, Nid>, name: &str) -> Nid {
        if let Some(n) = nid_map.get(name) {
            return *n;
        }
        view.signals
            .iter()
            .find(|s| s.kind == SignalKind::Combinational && s.symbol.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("term `{name}` not found"))
            .nid
    }

    // reg_a := 0 (deterministic). State-only.
    const DETERMINISTIC_ZERO: &str = "\
1 sort bitvec 1
2 state 1 reg_a
3 zero 1
4 init 1 2 3
5 next 1 2 3
";

    // reg_a := in_a (input-driven). Has a free input.
    const INPUT_DRIVEN: &str = "\
1 sort bitvec 1
2 input 1 in_a
3 state 1 reg_a
4 zero 1
5 init 1 3 4
6 next 1 3 2
";

    // reg stuck at 0; g = not(reg) is a combinational-of-state signal that never
    // changes (always 1). next(reg) = reg.
    const COMB_STUCK: &str = "\
1 sort bitvec 1
2 state 1 reg
3 zero 1
4 init 1 2 3
5 next 1 2 2
6 not 1 2 g_sig
";

    #[test]
    fn may_state_only_matches_per_kind_path() {
        // Differential: on a STATE predicate the uniform may verdict must equal
        // the per-kind `smt_per_target_may_check` for every polarity pair.
        for src in [DETERMINISTIC_ZERO, INPUT_DRIVEN] {
            let file = parse(src).expect("parse");
            with_z3(|| {
                let view = encode_design(&file).expect("encode");
                let primed = encode_primed(&file, &view).expect("primed");
                let nid_map = build_register_nid_map_with_inputs(&view);
                let reg_nid = term_nid(&view, &nid_map, "reg_a");
                let specs = vec![Spec {
                    register: "reg_a".into(),
                    value: 0,
                }];
                let preds = vec![Pred {
                    nid: reg_nid,
                    value: 0,
                }];
                for sb in 0..2u64 {
                    for tb in 0..2u64 {
                        let per_kind =
                            smt_per_target_may_check(&view, sb, tb, &specs, &nid_map, 5_000);
                        let uniform = uniform_may_check(&view, &primed, sb, tb, &preds, 5_000);
                        assert_eq!(
                            per_kind, uniform,
                            "may mismatch on state-only ({src:?} sb={sb} tb={tb})"
                        );
                    }
                }
            });
        }
    }

    #[test]
    fn must_state_only_matches_per_kind_path() {
        // Differential: on a STATE predicate the uniform must verdict must equal
        // the per-kind `smt_per_target_must_check_standard` for every polarity
        // pair. DETERMINISTIC_ZERO → Must on {p}→{p}; INPUT_DRIVEN → NotMust.
        for src in [DETERMINISTIC_ZERO, INPUT_DRIVEN] {
            let file = parse(src).expect("parse");
            with_z3(|| {
                let view = encode_design(&file).expect("encode");
                let primed = encode_primed(&file, &view).expect("primed");
                let nid_map = build_register_nid_map_with_inputs(&view);
                let reg_nid = term_nid(&view, &nid_map, "reg_a");
                let specs = vec![Spec {
                    register: "reg_a".into(),
                    value: 0,
                }];
                let preds = vec![Pred {
                    nid: reg_nid,
                    value: 0,
                }];
                for sb in 0..2u64 {
                    for tb in 0..2u64 {
                        let per_kind = smt_per_target_must_check_standard(
                            &view, sb, tb, &specs, &nid_map, 5_000,
                        );
                        let uniform =
                            uniform_must_check(&file, &view, &primed, sb, tb, &preds, 5_000);
                        assert_eq!(
                            per_kind, uniform,
                            "must mismatch on state-only ({src:?} sb={sb} tb={tb})"
                        );
                    }
                }
            });
        }
    }

    #[test]
    fn may_combinational_of_state_is_precise() {
        // `g = not(reg)`, reg stuck → g never changes (always 1). The uniform
        // rule gives the PRECISE may relation for the combinational term:
        //   {g} → {g}  is a may-edge (g stays 1);
        //   {g} → {¬g} is NOT a may-edge (g cannot become 0).
        // The per-kind path cannot build a constraint for a combinational term
        // (it is neither a state cell nor an input) → it conservatively returns
        // `May` for BOTH — so the uniform NotMay below is the new capability.
        let file = parse(COMB_STUCK).expect("parse");
        with_z3(|| {
            let view = encode_design(&file).expect("encode");
            let primed = encode_primed(&file, &view).expect("primed");
            let nid_map = build_register_nid_map_with_inputs(&view);
            let g = term_nid(&view, &nid_map, "g_sig");
            let preds = vec![Pred { nid: g, value: 1 }];
            // src {g} (g==1), tgt {g} (g==1): may-edge exists.
            assert_eq!(
                uniform_may_check(&view, &primed, 0b1, 0b1, &preds, 5_000),
                SmtMayVerdict::May,
                "g stays 1 → {{g}}→{{g}} is a may-edge"
            );
            // src {g} (g==1), tgt {¬g} (g==0): no witness → NotMay (precise).
            assert_eq!(
                uniform_may_check(&view, &primed, 0b1, 0b0, &preds, 5_000),
                SmtMayVerdict::NotMay,
                "g cannot become 0 → {{g}}→{{¬g}} is NOT a may-edge (per-kind path would say May)"
            );
        });
    }

    #[test]
    fn may_free_input_is_source_pinned_target_free() {
        // A predicate over the free input `in_a`: the uniform rule pins the
        // current input on the source and uses a FRESH i' on the target, so the
        // target is satisfiable for either flavour — H.B's source-pin /
        // target-free, re-expressed. Hence a may-edge exists to BOTH target
        // polarities from a fixed source.
        let file = parse(INPUT_DRIVEN).expect("parse");
        with_z3(|| {
            let view = encode_design(&file).expect("encode");
            let primed = encode_primed(&file, &view).expect("primed");
            let nid_map = build_register_nid_map_with_inputs(&view);
            let in_nid = term_nid(&view, &nid_map, "in_a");
            let preds = vec![Pred {
                nid: in_nid,
                value: 1,
            }];
            // src {in==1} → tgt {in'==1}: fresh i' can be 1 → May.
            assert_eq!(
                uniform_may_check(&view, &primed, 0b1, 0b1, &preds, 5_000),
                SmtMayVerdict::May
            );
            // src {in==1} → tgt {in'==0}: fresh i' can be 0 → May (target-free).
            assert_eq!(
                uniform_may_check(&view, &primed, 0b1, 0b0, &preds, 5_000),
                SmtMayVerdict::May,
                "fresh i' makes the input target free (H.B)"
            );
        });
    }

    #[test]
    fn must_input_target_is_deferred_not_unsound() {
        // The soundness guard: an `i'`-dependent must TARGET (a free-input
        // predicate at the consequent) is OUT of spike scope — it returns
        // `Unknown` (conservative) rather than the naive collapse that would
        // make i' existential (and fabricate must-edges). H.U.1 implements the
        // nested ∀ i' for this case.
        let file = parse(INPUT_DRIVEN).expect("parse");
        with_z3(|| {
            let view = encode_design(&file).expect("encode");
            let primed = encode_primed(&file, &view).expect("primed");
            let nid_map = build_register_nid_map_with_inputs(&view);
            let in_nid = term_nid(&view, &nid_map, "in_a");
            let preds = vec![Pred {
                nid: in_nid,
                value: 1,
            }];
            assert_eq!(
                uniform_must_check(&file, &view, &primed, 0b1, 0b1, &preds, 5_000),
                SmtMustVerdict::Unknown,
                "i'-dependent must target is conservatively deferred to H.U.1"
            );
        });
    }
}
