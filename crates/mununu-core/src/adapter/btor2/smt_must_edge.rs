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
        let src_polarity = (src_bits >> i) & 1 == 1;
        let tgt_polarity = (tgt_bits >> i) & 1 == 1;
        // B.1 — compound-aware; a missing register stays a conservative Unknown.
        let (Some(src_c), Some(tgt_c)) = (
            build_pred_constraint(view, nid_map, pred, false, src_polarity),
            build_pred_constraint(view, nid_map, pred, true, tgt_polarity),
        ) else {
            return SmtMustVerdict::Unknown;
        };
        src_constraints.push(src_c);
        tgt_constraints.push(tgt_c);
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
    /// B.1 — when this predicate is a **compound** (a boolean combination of
    /// register comparisons rather than the simple `register == value` atom),
    /// the [`PredicateExpr`] to encode. `None` (the default) ⇒ the simple atom
    /// path, which uses `register()` / `value()` exactly as before. A compound
    /// predicate is still one cube dimension; only its truth encoding changes.
    fn expr(&self) -> Option<&crate::adapter::btor2::predicate_expr::PredicateExpr> {
        None
    }
}

impl PredicateLike for crate::adapter::btor2::kmts_lift::PredicateSpec {
    fn register(&self) -> &str {
        &self.register
    }
    fn value(&self) -> u64 {
        self.value
    }
}

/// B.1 — build one predicate's constraint over the source (`slot_next ==
/// false`, `state_curr` BVs) or target (`slot_next == true`, `state_next` BVs)
/// cube, compound-aware. For a simple predicate (`expr() == None`) this is
/// exactly [`build_predicate_constraint`] — behaviour-preserving. For a
/// compound it builds the recursive constraint via
/// [`crate::adapter::btor2::predicate_expr::PredicateExpr::build_constraint`]
/// over a register→BV lookup and applies the cube polarity. Returns `None` if
/// any referenced register is absent from the view (the caller picks the
/// conservative verdict: `May` for may-checks, `Unknown` for must-checks).
fn build_pred_constraint<P: PredicateLike>(
    view: &Btor2SmtView,
    nid_map: &HashMap<String, Nid>,
    pred: &P,
    slot_next: bool,
    polarity: bool,
) -> Option<z3::ast::Bool> {
    match pred.expr() {
        None => {
            let nid = *nid_map.get(pred.register())?;
            let bv = if slot_next {
                view.next_state(nid)?
            } else {
                view.curr_state(nid)?
            };
            Some(build_predicate_constraint(bv, pred.value(), polarity))
        }
        Some(e) => {
            let lookup = |reg: &str| -> Option<z3::ast::BV> {
                let nid = *nid_map.get(reg)?;
                let bv = if slot_next {
                    view.next_state(nid)?
                } else {
                    view.curr_state(nid)?
                };
                Some(bv.clone())
            };
            let raw = e.build_constraint(&lookup)?;
            Some(if polarity { raw } else { raw.not() })
        }
    }
}

/// R.2.5b session-2 follow-up (2026-06-09) — build per-predicate
/// constraints for both the source cube (over `state_curr` BVs) and
/// the target cube (over `state_next` BVs). Shared helper so the
/// standard ∀∃ check + the hyper-must check don't duplicate the
/// per-predicate plumbing. Returns `None` if any predicate's
/// register isn't in `nid_map` (caller treats as Unknown verdict).
fn build_src_tgt_constraints<P>(
    view: &Btor2SmtView,
    src_bits: u64,
    tgt_bits: u64,
    predicates: &[P],
    nid_map: &HashMap<String, Nid>,
) -> Option<(Vec<z3::ast::Bool>, Vec<z3::ast::Bool>)>
where
    P: PredicateLike,
{
    let mut src = Vec::with_capacity(predicates.len());
    let mut tgt = Vec::with_capacity(predicates.len());
    for (i, pred) in predicates.iter().enumerate() {
        let src_polarity = (src_bits >> i) & 1 == 1;
        let tgt_polarity = (tgt_bits >> i) & 1 == 1;
        // B.1 — compound-aware (simple atom is the `expr() == None` branch).
        src.push(build_pred_constraint(
            view,
            nid_map,
            pred,
            false,
            src_polarity,
        )?);
        tgt.push(build_pred_constraint(
            view,
            nid_map,
            pred,
            true,
            tgt_polarity,
        )?);
    }
    Some((src, tgt))
}

/// R.2.5b session-2 follow-up (2026-06-09) — conjoin a vec of
/// per-predicate constraints into a single Bool. Returns
/// `Bool::from_bool(true)` for an empty input (vacuous conjunction).
fn conj_bool(constraints: &[z3::ast::Bool]) -> z3::ast::Bool {
    if constraints.is_empty() {
        z3::ast::Bool::from_bool(true)
    } else {
        let refs: Vec<&z3::ast::Bool> = constraints.iter().collect();
        z3::ast::Bool::and(&refs)
    }
}

/// R.2.5b session-2 follow-up (2026-06-09) — collect the universal-
/// quantification bound vars (inputs + state_next BVs) for the ∀∃
/// must-edge check. Returns a Vec of cloned BVs whose references the
/// caller wraps as `&dyn Ast` for `forall_const`.
fn universal_bound_bvs(view: &Btor2SmtView) -> Vec<z3::ast::BV> {
    let mut bvs: Vec<z3::ast::BV> = Vec::new();
    for bv in view.inputs.values() {
        bvs.push(bv.clone());
    }
    for bv in view.state_next.values() {
        bvs.push(bv.clone());
    }
    bvs
}

/// R.2.5b session-2 follow-up (2026-06-09) — standard ∀∃ form of
/// the SMT must-edge query for one (source-cube, target-cube) pair.
///
/// **Semantics (per [`docs/design/kmts-theory.md`] standard KMTS):**
/// the must-edge `(src, label, tgt)` holds iff
///
/// ```text
/// ∀ state ⊨ src. ∃ inputs. ∃ state_next. (transition(state, inputs, state_next) ∧ state_next ⊨ tgt)
/// ```
///
/// This is **more permissive** than the MVP's ∀∀ form
/// ([`smt_per_target_must_check`]) — the ∀∀ form requires every
/// input combination to reach tgt; the ∀∃ standard form only
/// requires SOME input combination per concrete source state. Every
/// must-edge the ∀∀ form proves is also proved by the ∀∃ form, so
/// the ∀∃ form produces a STRICT SUPERSET of Sharp promotions.
///
/// **Encoding via Z3 quantifier alternation:**
///
/// Negation of the must-edge condition:
/// `∃ state ⊨ src. ∀ inputs. ∀ state_next. ¬(transition ∧ state_next ⊨ tgt)`
///
/// We build the inner formula and SAT-check:
/// `src_constraints(state_curr) ∧ forall_const([inputs, state_next], ¬(transition ∧ tgt_constraints))`
///
/// - SAT ⇒ negation holds ⇒ NotMust.
/// - UNSAT ⇒ negation refuted ⇒ Must.
/// - Unknown ⇒ Unknown (timeout / unsupported operator).
///
/// **Caller must hold a [`z3::with_z3_config`] scope.**
pub fn smt_per_target_must_check_standard<P>(
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
    let Some((src_constraints, tgt_constraints)) =
        build_src_tgt_constraints(view, src_bits, tgt_bits, predicates, nid_map)
    else {
        return SmtMustVerdict::Unknown;
    };

    // Inner body: ¬(transition ∧ ∧ tgt_i).
    let tgt_conj = conj_bool(&tgt_constraints);
    let inner_body = z3::ast::Bool::and(&[&view.transition, &tgt_conj]).not();

    // Universal bounds: every input BV + every state_next BV. The
    // state_next is determined by (state_curr, inputs) via transition;
    // universally quantifying over both is equivalent semantically
    // and is the cleanest encoding given the view's BV vocabulary.
    let bound_bvs = universal_bound_bvs(view);
    let bound_refs: Vec<&dyn z3::ast::Ast> =
        bound_bvs.iter().map(|bv| bv as &dyn z3::ast::Ast).collect();
    let universal = z3::ast::forall_const(&bound_refs, &[], &inner_body);

    let solver = z3::Solver::new();
    let mut params = z3::Params::new();
    params.set_u32("timeout", timeout_ms);
    solver.set_params(&params);

    // Assert src_constraints (state_curr is existentially free at
    // the top level — the outer ∃ in the negation).
    for c in &src_constraints {
        solver.assert(c);
    }
    solver.assert(&universal);

    match solver.check() {
        z3::SatResult::Unsat => SmtMustVerdict::Must,
        z3::SatResult::Sat => SmtMustVerdict::NotMust,
        z3::SatResult::Unknown => SmtMustVerdict::Unknown,
    }
}

/// R.2.5b session-2 follow-up (2026-06-09) — SMT-backed hyper-must
/// check over a target SET T.
///
/// **Semantics (per [`docs/design/kmts-theory.md`] §7.4 generalised
/// KMTS hyper-must):** the hyper-must edge `(src, label, T)` holds iff
///
/// ```text
/// ∀ state ⊨ src. ∃ inputs. ∃ t ∈ T. ∃ state_next. (transition(state, inputs, state_next) ∧ state_next ⊨ t)
/// ```
///
/// The abstraction guarantees some t ∈ T is reached, but not
/// necessarily the same t across concrete states. This is the
/// standard hyper-must reading from Shoham–Grumberg LMCS 2007 §4.
///
/// **Encoding:** same shape as the per-target ∀∃ check, but with
/// the target-side constraint being a disjunction over the candidate
/// targets in `target_bits_set`:
///
/// `src_constraints ∧ forall_const([inputs, state_next], ¬(transition ∧ ⋁_t tgt_t_constraints))`
///
/// - SAT ⇒ some state in src can escape every t ∈ T ⇒ NotMust.
/// - UNSAT ⇒ every state in src has some (input, t ∈ T) witness ⇒ Must.
/// - Unknown ⇒ Unknown.
///
/// **Caller must hold a [`z3::with_z3_config`] scope.**
pub fn smt_hyper_must_check<P>(
    view: &Btor2SmtView,
    src_bits: u64,
    target_bits_set: &[u64],
    predicates: &[P],
    nid_map: &HashMap<String, Nid>,
    timeout_ms: u32,
) -> SmtMustVerdict
where
    P: PredicateLike,
{
    if target_bits_set.is_empty() {
        // Empty target set — no possible witnesses; trivially not a must.
        return SmtMustVerdict::NotMust;
    }

    // Build src constraints once (state_curr is shared across the
    // disjunction's target constraints).
    let Some((src_constraints, _placeholder)) = build_src_tgt_constraints(
        view, src_bits, 0, // tgt_bits placeholder — not used below; we rebuild per target
        predicates, nid_map,
    ) else {
        return SmtMustVerdict::Unknown;
    };

    // Build a target-conjunction per candidate, then OR them together.
    let mut tgt_disjuncts: Vec<z3::ast::Bool> = Vec::with_capacity(target_bits_set.len());
    for &tgt_bits in target_bits_set {
        let Some((_, tgt_constraints)) =
            build_src_tgt_constraints(view, src_bits, tgt_bits, predicates, nid_map)
        else {
            return SmtMustVerdict::Unknown;
        };
        tgt_disjuncts.push(conj_bool(&tgt_constraints));
    }
    let tgt_disjuncts_refs: Vec<&z3::ast::Bool> = tgt_disjuncts.iter().collect();
    let any_tgt = z3::ast::Bool::or(&tgt_disjuncts_refs);

    let inner_body = z3::ast::Bool::and(&[&view.transition, &any_tgt]).not();

    let bound_bvs = universal_bound_bvs(view);
    let bound_refs: Vec<&dyn z3::ast::Ast> =
        bound_bvs.iter().map(|bv| bv as &dyn z3::ast::Ast).collect();
    let universal = z3::ast::forall_const(&bound_refs, &[], &inner_body);

    let solver = z3::Solver::new();
    let mut params = z3::Params::new();
    params.set_u32("timeout", timeout_ms);
    solver.set_params(&params);

    for c in &src_constraints {
        solver.assert(c);
    }
    solver.assert(&universal);

    match solver.check() {
        z3::SatResult::Unsat => SmtMustVerdict::Must,
        z3::SatResult::Sat => SmtMustVerdict::NotMust,
        z3::SatResult::Unknown => SmtMustVerdict::Unknown,
    }
}

// ─────────────────────────────────────────────────────────────────────
// MIG-3 (S-track migration, 2026-06-13) — sound SMT may-edge query.
// ─────────────────────────────────────────────────────────────────────

/// Verdict of the SMT may-edge query ([`smt_per_target_may_check`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtMayVerdict {
    /// A concrete `(state ⊨ src, inputs, next ⊨ tgt)` witness exists
    /// (SAT): `src → tgt` is a sound may-edge. Also returned
    /// **conservatively on Unknown** (timeout) and when a predicate's
    /// register/handle is unresolved — the may relation must
    /// over-approximate, so only PROVEN-impossible edges are excluded.
    May,
    /// Z3 proved no witness exists (UNSAT): `src → tgt` is impossible
    /// and is excluded from the may relation.
    NotMay,
}

/// MIG-3 — the sound SMT may-edge query for one
/// (source-cube, target-cube) pair. This is the existential **dual** of
/// [`smt_per_target_must_check`]: it asserts
/// `transition ∧ src_constraints ∧ tgt_constraints` (target POSITIVE,
/// not negated) and returns [`SmtMayVerdict::May`] iff SAT — i.e. a
/// concrete `(s ⊨ src, inputs, s' ⊨ tgt)` witness exists. This is the
/// may-relation definition `R_may(b, b') ⟺ ∃ s ⊨ b, s' ⊨ b'. (s,s') ∈ R`.
///
/// **Why this is the sound replacement for sampling.** The
/// `predicate_cube_lift` sampling may-edges enumerate only a *subset*
/// of inputs per cube, so they can MISS a real may-edge → an
/// under-approximation of the may relation → unsound for safety (a
/// violation reachable only via an unsampled transition is lost). This
/// SMT query is exact: an edge is excluded ONLY when Z3 proves no
/// witness exists. On Unknown / unresolved predicates it conservatively
/// returns `May` (over-approximation).
///
/// `src_bits` / `tgt_bits` are predicate-polarity bitmasks (bit `i` =
/// predicate `predicates[i]` holds), exactly as in
/// [`smt_per_target_must_check`].
///
/// **Caller must hold a [`z3::with_z3_config`] scope.**
//
// Consumed by `predicate_cube_lift`'s `MayEdgeInference::SmtAllPairs`
// policy (MIG-3.2) — the sound all-pairs may relation.
pub fn smt_per_target_may_check<P>(
    view: &Btor2SmtView,
    src_bits: u64,
    tgt_bits: u64,
    predicates: &[P],
    nid_map: &HashMap<String, Nid>,
    timeout_ms: u32,
) -> SmtMayVerdict
where
    P: PredicateLike,
{
    let mut constraints: Vec<z3::ast::Bool> = Vec::new();

    for (i, pred) in predicates.iter().enumerate() {
        let src_polarity = (src_bits >> i) & 1 == 1;
        let tgt_polarity = (tgt_bits >> i) & 1 == 1;
        // B.1 — compound-aware. An unresolved register → conservatively include
        // the edge (over-approximation: never exclude an edge we can't rule out).
        let (Some(src_c), Some(tgt_c)) = (
            build_pred_constraint(view, nid_map, pred, false, src_polarity),
            build_pred_constraint(view, nid_map, pred, true, tgt_polarity),
        ) else {
            return SmtMayVerdict::May;
        };
        constraints.push(src_c);
        constraints.push(tgt_c);
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
        // Conservative: an undecided edge stays in the (over-approximate)
        // may relation.
        z3::SatResult::Unknown => SmtMayVerdict::May,
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

    // ---- B.1 — compound-predicate SMT-constraint path ----
    //
    // A `PredicateLike` whose `expr()` is a compound boolean over several
    // registers, exercising `build_pred_constraint`'s compound branch.
    use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr};

    struct CompoundPred {
        /// Vestigial for a compound (the simple `register()`/`value()` path is
        /// never taken when `expr()` is `Some`); a representative for display.
        reg0: String,
        e: PredicateExpr,
    }
    impl PredicateLike for CompoundPred {
        fn register(&self) -> &str {
            &self.reg0
        }
        fn value(&self) -> u64 {
            0
        }
        fn expr(&self) -> Option<&PredicateExpr> {
            Some(&self.e)
        }
    }

    // a := 0, b := 0 (deterministic). idle = (a == 0 && b == 0).
    const COMPOUND_DET_BTOR2: &str = "\
1 sort bitvec 1
2 state 1 a
3 state 1 b
4 zero 1
5 init 1 2 4
6 init 1 3 4
7 next 1 2 4
8 next 1 3 4
";

    // a := 0, b := in_b (b is input-driven). idle = (a == 0 && b == 0).
    const COMPOUND_INPUT_BTOR2: &str = "\
1 sort bitvec 1
2 input 1 in_b
3 state 1 a
4 state 1 b
5 zero 1
6 init 1 3 5
7 init 1 4 5
8 next 1 3 5
9 next 1 4 2
";

    fn idle_compound() -> PredicateExpr {
        // a == 0 && b == 0
        PredicateExpr::And(
            Box::new(PredicateExpr::Cmp {
                register: "a".into(),
                op: CmpOp::Eq,
                value: 0,
            }),
            Box::new(PredicateExpr::Cmp {
                register: "b".into(),
                op: CmpOp::Eq,
                value: 0,
            }),
        )
    }

    #[test]
    fn compound_predicate_deterministic_proves_must() {
        let file = parse(COMPOUND_DET_BTOR2).expect("parse compound-det fixture");
        let preds = vec![CompoundPred {
            reg0: "a".into(),
            e: idle_compound(),
        }];
        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode compound-det");
            let nid_map = build_register_nid_map(&view);
            // src cube {idle} (bit0=1) → tgt cube {idle} (bit0=1).
            smt_per_target_must_check(&view, 0b1, 0b1, &preds, &nid_map, 5_000)
        });
        assert_eq!(
            verdict,
            SmtMustVerdict::Must,
            "a:=0,b:=0 with idle=(a==0 && b==0), src=tgt={{idle}} must prove Sharp; got {verdict:?}"
        );
    }

    #[test]
    fn compound_predicate_honors_every_conjunct() {
        // b is input-driven, so the `&& b == 0` conjunct must make the must-edge
        // FAIL (input in_b=1 escapes the target). If the encoder ignored b and
        // only checked a==0, it would (wrongly) prove Must — see the contrast
        // assertion below.
        let file = parse(COMPOUND_INPUT_BTOR2).expect("parse compound-input fixture");
        let compound = vec![CompoundPred {
            reg0: "a".into(),
            e: idle_compound(),
        }];
        let compound_verdict = run_check(|| {
            let view = encode_design(&file).expect("encode compound-input");
            let nid_map = build_register_nid_map(&view);
            smt_per_target_must_check(&view, 0b1, 0b1, &compound, &nid_map, 5_000)
        });
        assert_eq!(
            compound_verdict,
            SmtMustVerdict::NotMust,
            "idle=(a==0 && b==0) with b:=in_b must reject Sharp (in_b=1 escapes); got {compound_verdict:?}"
        );

        // Contrast: the simple atom `a == 0` alone IS Must here (a:=0 always),
        // proving the compound's b-conjunct genuinely changed the verdict.
        let a_only = vec![PredicateSpec {
            name: "a_is_zero".into(),
            register: "a".into(),
            value: 0,
        }];
        let a_only_verdict = run_check(|| {
            let view = encode_design(&file).expect("encode compound-input");
            let nid_map = build_register_nid_map(&view);
            smt_per_target_must_check(&view, 0b1, 0b1, &a_only, &nid_map, 5_000)
        });
        assert_eq!(
            a_only_verdict,
            SmtMustVerdict::Must,
            "the simple atom a==0 alone is Must (a:=0); got {a_only_verdict:?}"
        );
    }

    // ---- R.2.5b session-2 follow-up (2026-06-09) ----
    //
    // Tests for the standard ∀∃ must-check + hyper-must helpers.
    //
    // The discriminating contrast vs the MVP ∀∀ check:
    // - ∀∀: deterministic into tgt regardless of input.
    // - ∀∃: for every state in src, SOME input reaches tgt.
    //
    // The INPUT_DRIVEN fixture (reg_a := in_a) is the canonical
    // discriminator: ∀∀ rejects the src=tgt={p} self-loop (input=1
    // escapes); ∀∃ ACCEPTS it (input=0 keeps reg_a==0). So the same
    // fixture that produced NotMust under the MVP produces Must
    // under the standard form — the strict-supremacy invariant.

    /// R.2.5b session-2 follow-up — on the input-driven fixture
    /// where input=0 keeps `reg_a==0` and input=1 sets `reg_a:=1`,
    /// the standard ∀∃ check proves the must-edge src=tgt={p}
    /// (for every state in src, the input=0 witness reaches tgt).
    /// The MVP ∀∀ check rejected this same edge.
    #[test]
    fn smt_standard_must_check_input_driven_proves_must() {
        let file = parse(INPUT_DRIVEN_BTOR2).expect("parse input-driven fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode input-driven");
            let nid_map = build_register_nid_map(&view);
            smt_per_target_must_check_standard(&view, 0b1, 0b1, &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::Must,
            "standard ∀∃: input=0 witness reaches src=tgt={{p}} from every state in src \
             ⇒ Must (more permissive than the MVP's ∀∀ form which rejected this)"
        );
    }

    /// R.2.5b session-2 follow-up — on the deterministic-zero
    /// fixture (always sets `reg_a:=0`), the standard ∀∃ check
    /// proves the must-edge (same as the MVP ∀∀ form). Confirms
    /// strict-supremacy: every ∀∀ Must is also a ∀∃ Must.
    #[test]
    fn smt_standard_must_check_deterministic_zero_proves_must() {
        let file = parse(DETERMINISTIC_ZERO_BTOR2).expect("parse deterministic fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode deterministic");
            let nid_map = build_register_nid_map(&view);
            smt_per_target_must_check_standard(&view, 0b1, 0b1, &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::Must,
            "deterministic reg_a := 0 with src=tgt={{p}} must prove ∀∃-Must too \
             (strict-supremacy invariant: every ∀∀-Must is also ∀∃-Must)"
        );
    }

    /// R.2.5b session-2 follow-up — when the src→tgt transition is
    /// IMPOSSIBLE under all inputs (e.g. src={p}, tgt={¬p}, with
    /// reg_a := 0 — every input keeps p true so tgt is never
    /// reached), the standard ∀∃ check returns NotMust (no input
    /// witness exists for any state in src).
    #[test]
    fn smt_standard_must_check_unreachable_target_returns_not_must() {
        let file = parse(DETERMINISTIC_ZERO_BTOR2).expect("parse deterministic fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode deterministic");
            let nid_map = build_register_nid_map(&view);
            // src cube = {p}: src_bits = 0b1.
            // tgt cube = {¬p}: tgt_bits = 0b0.
            // reg_a := 0 always; from src (reg_a==0), next is reg_a==0 ⇒ p stays true.
            // So tgt={¬p} is unreachable ⇒ NotMust.
            smt_per_target_must_check_standard(&view, 0b1, 0b0, &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::NotMust,
            "unreachable target ⇒ NotMust under standard ∀∃ form"
        );
    }

    /// R.2.5b session-2 follow-up — hyper-must on a 2-target set.
    /// On the input-driven fixture, hyper-must over {p, ¬p} covers
    /// both possible next-states (input=0 → p; input=1 → ¬p). For
    /// every state in src, some input reaches some target in the
    /// set ⇒ Must.
    #[test]
    fn smt_hyper_must_covers_full_target_set_proves_must() {
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
            // target set = [{p}, {¬p}] (bits 0b1 + 0b0).
            // Every input combo from src reaches some target in the set ⇒ Must.
            smt_hyper_must_check(&view, 0b1, &[0b1, 0b0], &predicates, &nid_map, 5_000)
        });

        assert_eq!(
            verdict,
            SmtMustVerdict::Must,
            "hyper-must over the full {{p, ¬p}} target set covers all input-driven \
             paths from src ⇒ Must"
        );
    }

    /// R.2.5b session-2 follow-up — hyper-must on a singleton
    /// (degenerates to the per-target ∀∃ check). Confirms the
    /// hyper-must helper agrees with `smt_per_target_must_check_standard`
    /// on cardinality-1 target sets.
    #[test]
    fn smt_hyper_must_singleton_agrees_with_per_target_standard() {
        let file = parse(INPUT_DRIVEN_BTOR2).expect("parse fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let (singleton_hyper, per_target) = run_two_checks(|| {
            let view = encode_design(&file).expect("encode fixture");
            let nid_map = build_register_nid_map(&view);
            let h = smt_hyper_must_check(&view, 0b1, &[0b1], &predicates, &nid_map, 5_000);
            let p =
                smt_per_target_must_check_standard(&view, 0b1, 0b1, &predicates, &nid_map, 5_000);
            (h, p)
        });

        assert_eq!(
            singleton_hyper, per_target,
            "hyper-must({{p}}) must agree with per-target ∀∃({{p}}) \
             (cardinality-1 reduction); got singleton={singleton_hyper:?}, per-target={per_target:?}"
        );
    }

    /// R.2.5b session-2 follow-up — hyper-must on an empty target
    /// set returns NotMust (no possible witnesses).
    #[test]
    fn smt_hyper_must_empty_set_returns_not_must() {
        let file = parse(INPUT_DRIVEN_BTOR2).expect("parse fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let verdict = run_check(|| {
            let view = encode_design(&file).expect("encode fixture");
            let nid_map = build_register_nid_map(&view);
            smt_hyper_must_check(&view, 0b1, &[], &predicates, &nid_map, 5_000)
        });

        assert_eq!(verdict, SmtMustVerdict::NotMust);
    }

    /// Run a closure inside the Z3 scope returning two verdicts.
    /// Local helper since `run_check` is single-verdict.
    fn run_two_checks<F: FnOnce() -> (SmtMustVerdict, SmtMustVerdict) + Send + Sync>(
        f: F,
    ) -> (SmtMustVerdict, SmtMustVerdict) {
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, f)
    }

    // ── MIG-3 — sound SMT may-edge query tests ──────────────────────

    /// MIG-3 — an input-driven transition `reg_a := in_a` admits BOTH
    /// targets from src cube `{p}` (reg_a==0): input=0 reaches `{p}`,
    /// input=1 reaches `{¬p}`. Both are sound may-edges (∃ witness),
    /// even though the must-check rejects src→`{p}` (input=1 escapes).
    /// This is the may/must distinction the sound may-edge captures and
    /// the sampling could miss.
    #[test]
    fn smt_may_check_input_driven_admits_both_targets() {
        let file = parse(INPUT_DRIVEN_BTOR2).expect("parse input-driven fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let cfg = z3::Config::new();
        let (to_p, to_not_p) = z3::with_z3_config(&cfg, || {
            let view = encode_design(&file).expect("encode input-driven");
            let nid_map = build_register_nid_map(&view);
            // src cube {p} = reg_a==0 (src_bits = 0b1).
            let to_p = smt_per_target_may_check(&view, 0b1, 0b1, &predicates, &nid_map, 5_000);
            let to_not_p = smt_per_target_may_check(&view, 0b1, 0b0, &predicates, &nid_map, 5_000);
            (to_p, to_not_p)
        });

        assert_eq!(
            to_p,
            SmtMayVerdict::May,
            "input=0 reaches reg_a==0, so src{{p}}→tgt{{p}} is a may-edge"
        );
        assert_eq!(
            to_not_p,
            SmtMayVerdict::May,
            "input=1 reaches reg_a==1, so src{{p}}→tgt{{¬p}} is a may-edge"
        );
    }

    /// MIG-3 — a deterministic transition `reg_a := 0` makes the target
    /// `{¬p}` (reg_a==1) UNREACHABLE from any source. The sound
    /// may-edge query EXCLUDES it (UNSAT → NotMay) — the key soundness
    /// property: only proven-impossible edges are excluded, never a
    /// reachable one (which sampling could miss).
    #[test]
    fn smt_may_check_excludes_proven_impossible_target() {
        let file = parse(DETERMINISTIC_ZERO_BTOR2).expect("parse deterministic fixture");
        let predicates = vec![PredicateSpec {
            name: "p".into(),
            register: "reg_a".into(),
            value: 0,
        }];

        let cfg = z3::Config::new();
        let (to_p, to_not_p) = z3::with_z3_config(&cfg, || {
            let view = encode_design(&file).expect("encode deterministic");
            let nid_map = build_register_nid_map(&view);
            let to_p = smt_per_target_may_check(&view, 0b1, 0b1, &predicates, &nid_map, 5_000);
            let to_not_p = smt_per_target_may_check(&view, 0b1, 0b0, &predicates, &nid_map, 5_000);
            (to_p, to_not_p)
        });

        assert_eq!(
            to_p,
            SmtMayVerdict::May,
            "reg_a:=0 reaches reg_a==0, so src{{p}}→tgt{{p}} is a may-edge"
        );
        assert_eq!(
            to_not_p,
            SmtMayVerdict::NotMay,
            "reg_a:=0 can NEVER reach reg_a==1, so src{{p}}→tgt{{¬p}} is excluded"
        );
    }
}
