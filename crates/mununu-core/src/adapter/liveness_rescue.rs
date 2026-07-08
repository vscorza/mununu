//! P2 liveness-to-safety — reduce the **response** liveness property
//! `AG(a → AF b)` to a BTOR2 `bad` monitor and decide it with the reachability
//! portfolio at scale.
//!
//! # The property and why it reduces
//!
//! `AG(a → AF b)` — "whenever `a` holds, `b` is eventually reached on *every*
//! path" — is the canonical request/response (request/grant) liveness property.
//! In the modal-μ calculus it is
//!
//! ```text
//!   ν Z. ((¬a ∨ μ Y. (b ∨ [] Y)) ∧ [] Z)
//! ```
//!
//! a νμ alternation (`PropertyClass::Liveness`). The **exact** 3-valued KMTS engine
//! decides it directly but only within its enumeration cap; this module reduces it
//! to a single `bad`-reachability query the scalable portfolio (native ⊕ spacer ⊕
//! btormc ⊕ Pono) decides symbolically on wide designs, and the exact engine
//! cross-checks the small cases.
//!
//! # Liveness-to-safety (Biere–Artho–Schuppan)
//!
//! `AG(a → AF b)` **fails** iff the design has a *reachable* infinite path on which
//! some `a` is never followed by `b` — equivalently, a **reachable lasso** (a
//! reachable cycle) that contains a state where `a` holds and contains **no** state
//! where `b` holds (on the cycle the outstanding request is never granted). The l2s
//! construction ([`crate::adapter::btor2::l2s_monitor::emit_response_l2s_monitor`])
//! makes "a reachable `b`-free cycle containing an `a`" a `bad` state via a
//! nondeterministically-snapshotted shadow copy of the state, so:
//!
//! - `bad` **reachable** ⇒ such a lasso exists ⇒ `AG(a → AF b)` **VIOLATED**;
//! - `bad` **unreachable** (a portfolio safety proof) ⇒ no such lasso ⇒ **HOLDS**.
//!
//! # A note on `F` in mununu's LTL front-end
//!
//! mununu's LTL translator lowers `G(a → F b)` with the **diamond** reading of `F`
//! (`μ Y. (b ∨ <> Y)` = `EF`), giving `AG(a → EF b)` — the weaker *recoverability*
//! response ("`b` is reachable"), a νμ property that does **not** collapse to a
//! single safety query. This module targets the **box** reading `AF b`
//! (`μ Y. (b ∨ [] Y)` — "`b` on every path"), the true "always eventually granted"
//! liveness that l2s reduces. Author the property as the μ-calculus formula above
//! (box recursion in the inner `μ`), not via the LTL `F`.
//!
//! # Conservative by construction (soundness-critical)
//!
//! [`reduce_response_af`] returns `Some` ONLY for the exact single-atom shape above
//! with **unconstrained** boxes and both `a`, `b` single register-comparison atoms.
//! Every other shape returns `None` — a sound abstention (the caller leaves the
//! property to the exact engine), never a mis-reduction.
//!
//! # Status — validated reduction, wiring is the next slice (2026-07-08)
//!
//! [`response_liveness_rescue`] is proven correct end-to-end by the differential
//! tests below: on both a responder (HOLDS) and a staller (VIOLATED) the
//! l2s → portfolio verdict **matches** the exact 3-valued engine
//! ([`crate::adapter::btor2::symbolic_bitblast::exact_symbolic_verdict`]) — the
//! roadmap's "every reduced verdict is cross-checked by the exact engine". Routing
//! `verify_auto` to try this reduction when a liveness property escapes the exact
//! engine's enumeration cap (and the CLI/API/UI surface for it) is the next P2 slice,
//! mirroring how [`crate::adapter::reach_rescue`]'s reduction primitive landed and was
//! validated before any surface wiring.

use crate::adapter::btor2::l2s_monitor::emit_response_l2s_monitor;
use crate::adapter::btor2::parser;
use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr, parse_predicate_expr};
use crate::adapter::reach_portfolio::{ReachOutcome, ReachVerdict, decide_reach_portfolio};
use crate::mu_calculus::{Formula, FormulaVarId, Guard, ModalKind, Node, NodeId};

/// A single register-comparison atom `signal ⋈ value` (the `a` / `b` of the
/// response property).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom {
    /// The register (or input) the atom compares.
    pub signal: String,
    /// The comparison operator.
    pub op: CmpOp,
    /// The literal right-hand side.
    pub value: u128,
}

/// A reducible response-liveness property `AG(a → AF b)` over two single atoms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseAf {
    /// The antecedent `a` — the request.
    pub ante: Atom,
    /// The consequent `b` — the grant that must eventually follow on every path.
    pub cons: Atom,
}

/// Conservatively classify `formula` as the response-liveness shape
/// `ν Z. ((¬a ∨ μ Y. (b ∨ [] Y)) ∧ [] Z)` with single-atom `a`, `b`. Returns
/// `None` for every other shape (see the module docs) so the reduction never
/// mis-reduces a property it does not fully recognise.
pub fn reduce_response_af(formula: &Formula) -> Option<ResponseAf> {
    // Root must be `ν Z. BODY` (the outer `AG`).
    let Node::Nu { var: z, body } = formula.node(formula.root()) else {
        return None;
    };
    // BODY = `core ∧ [] Z`.
    let Node::And(l, r) = formula.node(*body) else {
        return None;
    };
    let (core_id, _boxz_id) = split_and_box_to_var(formula, *l, *r, *z)?;
    // core = `¬a ∨ (μ Y. (b ∨ [] Y))`.
    let Node::Or(cl, cr) = formula.node(core_id) else {
        return None;
    };
    let (neg_ante_id, af_id) = classify_or_notpred_mu(formula, *cl, *cr)?;
    // ¬a = `Not(Predicate(a))`.
    let Node::Not(a_pred_id) = formula.node(neg_ante_id) else {
        return None;
    };
    let ante = predicate_atom(formula, *a_pred_id)?;
    // AF b = `μ Y. (b ∨ [] Y)`.
    let Node::Mu {
        var: y,
        body: mbody,
    } = formula.node(af_id)
    else {
        return None;
    };
    let Node::Or(ml, mr) = formula.node(*mbody) else {
        return None;
    };
    let (cons_id, _boxy_id) = split_and_box_to_var(formula, *ml, *mr, *y)?;
    let cons = predicate_atom(formula, cons_id)?;
    Some(ResponseAf { ante, cons })
}

/// Return `(other, box_to_var)` iff exactly one of `l` / `r` is an unconstrained
/// `[] var` box and the other is not — the unambiguous "`X ∧ [] var`" /
/// "`X ∨ [] var`" split. `None` if neither or both look like the box recursion.
fn split_and_box_to_var(
    formula: &Formula,
    l: NodeId,
    r: NodeId,
    var: FormulaVarId,
) -> Option<(NodeId, NodeId)> {
    match (
        is_unconstrained_box_to_var(formula, l, var),
        is_unconstrained_box_to_var(formula, r, var),
    ) {
        (true, false) => Some((r, l)),
        (false, true) => Some((l, r)),
        _ => None,
    }
}

/// Return `(not_pred, mu)` iff exactly one of `l` / `r` is a `Not` node and the
/// other is a `Mu` fixpoint — the `¬a ∨ AF b` split. `None` otherwise.
fn classify_or_notpred_mu(formula: &Formula, l: NodeId, r: NodeId) -> Option<(NodeId, NodeId)> {
    let is_not = |id: NodeId| matches!(formula.node(id), Node::Not(_));
    let is_mu = |id: NodeId| matches!(formula.node(id), Node::Mu { .. });
    match (is_not(l) && is_mu(r), is_not(r) && is_mu(l)) {
        (true, false) => Some((l, r)),
        (false, true) => Some((r, l)),
        _ => None,
    }
}

/// A `[] var` box whose target is exactly `var` and whose guard is unconstrained
/// (a guarded box is a stronger property than the monitor checks).
fn is_unconstrained_box_to_var(formula: &Formula, id: NodeId, var: FormulaVarId) -> bool {
    matches!(
        formula.node(id),
        Node::Modal { kind: ModalKind::Box, guard, target }
            if *guard == Guard::default()
                && matches!(formula.node(*target), Node::Variable(v) if *v == var)
    )
}

/// Parse a `Predicate` node as a single register-comparison atom. `None` for a
/// relational (`reg ⋈ reg`), compound, or unparseable atom.
fn predicate_atom(formula: &Formula, id: NodeId) -> Option<Atom> {
    let Node::Predicate(atom) = formula.node(id) else {
        return None;
    };
    match parse_predicate_expr(atom) {
        Ok(PredicateExpr::Cmp {
            register,
            op,
            value,
        }) => Some(Atom {
            signal: register,
            op,
            value: value as u128,
        }),
        _ => None,
    }
}

/// The response-liveness verdict, from the reachability portfolio's verdict on the
/// l2s `bad` monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessVerdict {
    /// `bad` proven unreachable ⇒ no `b`-free request-pending lasso ⇒ the response
    /// property holds.
    Holds,
    /// `bad` reachable ⇒ a reachable lasso leaves a request forever ungranted.
    Violated,
    /// Undecided by every portfolio member, or a soundness contradiction alarm —
    /// never a silently-picked side.
    Inconclusive,
}

/// Reduce a response-liveness property `AG(a → AF b)` to an l2s `bad` monitor over
/// `design_btor2` and decide it with the reachability portfolio.
///
/// Returns `None` when the property is not the reducible response shape (the caller
/// leaves it to the exact engine) or the monitor cannot be built (an atom binds no
/// signal). `design_btor2` MUST be the same reset/config-pinned BTOR2 the exact
/// engine reasoned over, so the reachable-state set matches; `reset_pinned` mirrors
/// that reset-gating so the atoms resolve through the async-reset mux consistently.
///
/// On `Some`, the second component is the raw [`ReachOutcome`] (which engines decided
/// each side) for diagnostics / a witness note.
pub fn response_liveness_rescue(
    design_btor2: &str,
    formula: &Formula,
    reset_pinned: bool,
) -> Option<(LivenessVerdict, ReachOutcome)> {
    let r = reduce_response_af(formula)?;
    let monitored = emit_response_l2s_monitor(
        design_btor2,
        (&r.ante.signal, r.ante.op, r.ante.value),
        (&r.cons.signal, r.cons.op, r.cons.value),
        reset_pinned,
    )
    .ok()?;
    let file = parser::parse(&monitored).ok()?;
    let outcome = decide_reach_portfolio(&file);
    let verdict = match outcome.verdict {
        ReachVerdict::Unreachable => LivenessVerdict::Holds,
        ReachVerdict::Reachable => LivenessVerdict::Violated,
        ReachVerdict::Unknown | ReachVerdict::Contradiction => LivenessVerdict::Inconclusive,
    };
    Some((verdict, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mu_calculus::parser as mu_parser;

    fn parse(s: &str) -> Formula {
        mu_parser::parse(s).unwrap_or_else(|e| panic!("parse `{s}`: {e:?}"))
    }

    // The canonical response shape, both atom orders inside the inner Or.
    #[test]
    fn reduces_response_af() {
        let f = parse("nu Z. ((!(req == 1) || mu Y. ((grant == 1) || [] Y)) && [] Z)");
        let r = reduce_response_af(&f).expect("reducible");
        assert_eq!(r.ante.signal, "req");
        assert_eq!(r.ante.op, CmpOp::Eq);
        assert_eq!(r.ante.value, 1);
        assert_eq!(r.cons.signal, "grant");
        assert_eq!(r.cons.op, CmpOp::Eq);
        assert_eq!(r.cons.value, 1);
    }

    // Order independence: `[] Z && core`, `[] Y || b`, `AF b || !a`.
    #[test]
    fn reduces_response_af_reordered() {
        let f = parse("nu Z. (([] Z) && ((mu Y. (([] Y) || (b != 0))) || !(a == 3)))");
        let r = reduce_response_af(&f).expect("reducible under reordering");
        assert_eq!(r.ante.signal, "a");
        assert_eq!(r.ante.value, 3);
        assert_eq!(r.cons.signal, "b");
        assert_eq!(r.cons.op, CmpOp::Ne);
    }

    // The DIAMOND response (mununu's LTL `G(a->F b)` = AG(a->EF b)) is NOT this
    // shape — the inner recursion is `<> Y`, not `[] Y`. Must abstain.
    #[test]
    fn rejects_diamond_ef_response() {
        let f = parse("nu Z. ((!(req == 1) || mu Y. ((grant == 1) || <> Y)) && [] Z)");
        assert!(reduce_response_af(&f).is_none());
    }

    // A bare AG invariant (no inner Mu) is not a response.
    #[test]
    fn rejects_ag_invariant() {
        let f = parse("nu Z. ((cnt == 3) && [] Z)");
        assert!(reduce_response_af(&f).is_none());
    }

    // A guarded outer box is a stronger property than the monitor checks.
    #[test]
    fn rejects_guarded_box() {
        let f = parse(
            "nu Z. ((!(req == 1) || mu Y. ((grant == 1) || [] Y)) && [(ctrl = controllable)] Z)",
        );
        assert!(reduce_response_af(&f).is_none());
    }

    // A relational consequent (`reg ⋈ reg`) is not a single literal atom.
    #[test]
    fn rejects_relational_atom() {
        let f = parse("nu Z. ((!(req == 1) || mu Y. ((x == y) || [] Y)) && [] Z)");
        assert!(reduce_response_af(&f).is_none());
    }

    // --- Differential correctness: l2s + portfolio vs the exact 3-valued engine ---
    //
    // Both atoms are STATE predicates (`st == k`) so the state-based exact engine can
    // serve as the ground-truth oracle. `go` is a free input (nondeterministic
    // idle→req choice) — allowed; it is not an atom.

    // 3-state responder: st 0=idle, 1=req, 2=grant. idle -go-> req; req -> grant
    // (deterministic); grant -> idle. AG((st==1) → AF (st==2)) HOLDS.
    const RESPONDER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 eq 2 3 4
10 eq 2 3 7
11 ite 1 6 7 4
12 ite 1 10 8 4
13 ite 1 9 11 12
14 next 1 3 13
";

    // 4-state staller: st 0=idle, 1=req, 3=stuck; 2=grant is UNREACHABLE. idle -go->
    // req; req -> stuck; stuck -> stuck. AG((st==1) → AF (st==2)) VIOLATED — a req
    // enters the stuck cycle and grant never comes.
    const STALLER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 constd 1 3
10 eq 2 3 4
11 eq 2 3 7
12 ite 1 6 7 4
13 ite 1 11 9 3
14 ite 1 10 12 13
15 next 1 3 14
";

    fn response_formula() -> Formula {
        parse("nu Z. ((!(st == 1) || mu Y. ((st == 2) || [] Y)) && [] Z)")
    }

    #[test]
    fn l2s_holds_matches_exact_on_responder() {
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        let f = response_formula();
        // Exact 3-valued engine — the ground-truth oracle.
        let exact = exact_symbolic_verdict(RESPONDER, &f).expect("exact decides");
        assert_eq!(
            exact,
            ExactVerdict::Holds,
            "responder: AG(req→AF grant) holds"
        );
        // l2s → portfolio must agree.
        let (v, out) = response_liveness_rescue(RESPONDER, &f, false).expect("reducible");
        assert_eq!(v, LivenessVerdict::Holds, "l2s outcome: {out:?}");
    }

    #[test]
    fn l2s_violated_matches_exact_on_staller() {
        use crate::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_symbolic_verdict};
        let f = response_formula();
        let exact = exact_symbolic_verdict(STALLER, &f).expect("exact decides");
        assert_eq!(
            exact,
            ExactVerdict::Violated,
            "staller: AG(req→AF grant) violated"
        );
        let (v, out) = response_liveness_rescue(STALLER, &f, false).expect("reducible");
        assert_eq!(v, LivenessVerdict::Violated, "l2s outcome: {out:?}");
    }
}
