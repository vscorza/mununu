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
//! # Status — validated + user-surfaced (2026-07-08)
//!
//! [`response_liveness_rescue`] is proven correct end-to-end by the differential
//! tests below: on both a responder (HOLDS) and a staller (VIOLATED) the
//! l2s → portfolio verdict **matches** the exact 3-valued engine
//! ([`crate::adapter::btor2::symbolic_bitblast::exact_symbolic_verdict`]) — the
//! roadmap's "every reduced verdict is cross-checked by the exact engine".
//!
//! The reduction is user-invocable via [`response_liveness_rescue_atoms`] +
//! [`parse_response_atom`] across all three surfaces: CLI `mununu btor2
//! verify-liveness --request … --grant …`, API `POST /api/v1/btor2/verify-liveness`,
//! and the UI `runBtor2VerifyLiveness` client. Auto-routing `verify_auto` to *try*
//! this reduction when a translated SVA liveness property escapes the exact engine's
//! cap is a further follow-up (it needs a real SVA liveness target to validate under
//! the mununu-sva toolchain, per the [`crate::adapter::reach_rescue`] no-target
//! lesson).

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
    response_liveness_rescue_atoms(design_btor2, &r.ante, &r.cons, reset_pinned)
}

/// Like [`response_liveness_rescue`] but with the request / grant atoms given
/// directly (the caller asserts the `AG(ante → AF cons)` shape) — the entry point
/// for the `verify-liveness` surface, where the user names the two atoms rather than
/// authoring the full μ-calculus formula.
///
/// Returns `None` only when the l2s monitor cannot be built (an atom binds no
/// signal). See [`response_liveness_rescue`] for the `design_btor2` / `reset_pinned`
/// contract and the returned [`ReachOutcome`].
pub fn response_liveness_rescue_atoms(
    design_btor2: &str,
    ante: &Atom,
    cons: &Atom,
    reset_pinned: bool,
) -> Option<(LivenessVerdict, ReachOutcome)> {
    let monitored = emit_response_l2s_monitor(
        design_btor2,
        (&ante.signal, ante.op, ante.value),
        (&cons.signal, cons.op, cons.value),
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

/// Decide a **conjunction** of response-liveness properties `⋀_i AG(a_i → AF b_i)`
/// over one `design_btor2`, by deciding each response independently via
/// [`response_liveness_rescue_atoms`] and combining the verdicts:
///
/// - **any `Violated` ⇒ `Violated`** — a real `b_i`-free lasso through a pending `a_i`
///   is a genuine path violating the conjunction (the counterexample to conjunct `i`
///   is a counterexample to `⋀`), so `Violated` dominates.
/// - else **any `Inconclusive` ⇒ `Inconclusive`** — that conjunct might yet be
///   violated, so the conjunction cannot be declared to hold.
/// - else **all `Holds` ⇒ `Holds`**.
///
/// This is **correct by composition**: each single-response query is a sound + complete
/// l2s reduction (see [`response_liveness_rescue`]), a conjunction violation is exactly
/// *some* conjunct's violation (which its query finds), and each query still scales via
/// the reachability portfolio. It is the closed-system shape — verifying a
/// multi-guarantee controller (e.g. an arbiter's per-client no-starvation) in place,
/// where the environment is concrete and any GR(1) assumptions are already discharged.
/// The open-system Streett shape `(⋀ GF a_i) → (⋀ GF b_j)` — where live assumptions
/// couple the guarantees, so this N-query decomposition does NOT hold — is a separate
/// follow-up (the Emerson–Lei fair-cycle l2s).
///
/// Returns `None` (matching [`response_liveness_rescue_atoms`]) if `pairs` is empty or
/// any monitor cannot be built (an atom binds no signal). On `Some`, the second
/// component is the per-response [`ReachOutcome`] (same order as `pairs`) for diagnostics.
pub fn response_liveness_rescue_conjunction(
    design_btor2: &str,
    pairs: &[(Atom, Atom)],
    reset_pinned: bool,
) -> Option<(LivenessVerdict, Vec<ReachOutcome>)> {
    if pairs.is_empty() {
        return None;
    }
    let mut outcomes = Vec::with_capacity(pairs.len());
    let mut any_violated = false;
    let mut any_inconclusive = false;
    for (ante, cons) in pairs {
        let (verdict, outcome) =
            response_liveness_rescue_atoms(design_btor2, ante, cons, reset_pinned)?;
        match verdict {
            LivenessVerdict::Violated => any_violated = true,
            LivenessVerdict::Inconclusive => any_inconclusive = true,
            LivenessVerdict::Holds => {}
        }
        outcomes.push(outcome);
    }
    // Violated dominates Inconclusive dominates Holds (conjunction semantics).
    let verdict = if any_violated {
        LivenessVerdict::Violated
    } else if any_inconclusive {
        LivenessVerdict::Inconclusive
    } else {
        LivenessVerdict::Holds
    };
    Some((verdict, outcomes))
}

/// Parse a request/grant atom string (`"REG op VALUE"`, e.g. `"st == 2"`) into an
/// [`Atom`], for the CLI / API `verify-liveness` surface. Reuses the same predicate
/// grammar as the classifier, so a relational (`reg ⋈ reg`) or malformed atom is
/// rejected identically.
pub fn parse_response_atom(s: &str) -> Result<Atom, String> {
    match parse_predicate_expr(s) {
        Ok(PredicateExpr::Cmp {
            register,
            op,
            value,
        }) => Ok(Atom {
            signal: register,
            op,
            value: value as u128,
        }),
        Ok(_) => Err(format!(
            "`{s}` is not a single register-comparison atom (`REG op VALUE`); relational or \
             compound atoms are out of the response fragment"
        )),
        Err(e) => Err(format!("cannot parse response atom `{s}`: {e:?}")),
    }
}

/// Parse repeatable `"ANTE => CONS"` response strings (the `verify-liveness-all`
/// surface) into `(ante, cons)` [`Atom`] pairs for
/// [`response_liveness_rescue_conjunction`]. Each string is split on the first literal
/// `=>`; both sides are trimmed and parsed with [`parse_response_atom`]. Errors if a
/// string lacks `=>`, or either side is not a single register-comparison atom.
///
/// Reused by the CLI, HTTP API, and SV-direct surfaces so the `--response` grammar is
/// identical everywhere.
pub fn parse_response_pairs(responses: &[String]) -> Result<Vec<(Atom, Atom)>, String> {
    responses
        .iter()
        .map(|r| {
            let (ante, cons) = r.split_once("=>").ok_or_else(|| {
                format!(
                    "response `{r}` must contain `=>` separating the antecedent and consequent \
                     (e.g. `\"req == 1 => grant == 1\"`)"
                )
            })?;
            Ok((
                parse_response_atom(ante.trim())?,
                parse_response_atom(cons.trim())?,
            ))
        })
        .collect()
}

/// Render a list of `"ANTE => CONS"` response strings as the display property
/// `AG((a) -> AF (b)) && AG((c) -> AF (d))`, echoed for provenance across the
/// `verify-liveness-all` surfaces. Splits each string on the first `=>` (falling back
/// to the raw trimmed text when absent — a malformed entry is caught by
/// [`parse_response_pairs`], so this is display-only).
pub fn response_conjunction_property(responses: &[String]) -> String {
    responses
        .iter()
        .map(|r| match r.split_once("=>") {
            Some((ante, cons)) => format!("AG(({}) -> AF ({}))", ante.trim(), cons.trim()),
            None => r.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

// The surface verdict label comes from the canonical
// [`crate::verdict::PropertyVerdict`] (`From<LivenessVerdict>`), so
// `btor2 verify-liveness` reports the same `holds`/`violated`/`unknown` vocabulary as
// every other verify surface (`Inconclusive` folds to `unknown`).

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

    // The atoms-based surface entry (the CLI / API path) agrees with the formula path.
    #[test]
    fn atoms_entry_matches_formula_entry() {
        let ante = parse_response_atom("st == 1").expect("atom parses");
        let cons = parse_response_atom("st == 2").expect("atom parses");
        assert_eq!(ante.op, CmpOp::Eq);
        assert_eq!(cons.value, 2);
        let (v_r, _) = response_liveness_rescue_atoms(RESPONDER, &ante, &cons, false).expect("ok");
        assert_eq!(v_r, LivenessVerdict::Holds);
        let (v_s, _) = response_liveness_rescue_atoms(STALLER, &ante, &cons, false).expect("ok");
        assert_eq!(v_s, LivenessVerdict::Violated);
    }

    // --- Multi-response conjunction (correct by composition) ---

    // Two responses on RESPONDER, each individually Holds — req→grant and grant→idle —
    // so the conjunction Holds, and the conjunction verdict matches the composition of
    // the individual single-response verdicts.
    #[test]
    fn conjunction_all_hold_is_holds() {
        let p1 = (
            parse_response_atom("st == 1").unwrap(),
            parse_response_atom("st == 2").unwrap(),
        );
        let p2 = (
            parse_response_atom("st == 2").unwrap(),
            parse_response_atom("st == 0").unwrap(),
        );
        let (v1, _) = response_liveness_rescue_atoms(RESPONDER, &p1.0, &p1.1, false).unwrap();
        let (v2, _) = response_liveness_rescue_atoms(RESPONDER, &p2.0, &p2.1, false).unwrap();
        assert_eq!(v1, LivenessVerdict::Holds, "req→grant holds");
        assert_eq!(v2, LivenessVerdict::Holds, "grant→idle holds");
        let (v, outs) =
            response_liveness_rescue_conjunction(RESPONDER, &[p1, p2], false).expect("decides");
        assert_eq!(v, LivenessVerdict::Holds, "both hold ⇒ conjunction holds");
        assert_eq!(outs.len(), 2, "one outcome per response");
    }

    // On STALLER the req→grant response is Violated; conjoined with a trivially-holding
    // response, the conjunction is Violated (Violated dominates).
    #[test]
    fn conjunction_one_violated_is_violated() {
        let p_bad = (
            parse_response_atom("st == 1").unwrap(),
            parse_response_atom("st == 2").unwrap(),
        );
        let p_ok = (
            parse_response_atom("st == 0").unwrap(),
            parse_response_atom("st == 0").unwrap(),
        );
        let (vb, _) = response_liveness_rescue_atoms(STALLER, &p_bad.0, &p_bad.1, false).unwrap();
        assert_eq!(
            vb,
            LivenessVerdict::Violated,
            "req→grant violated on staller"
        );
        let (v, _) =
            response_liveness_rescue_conjunction(STALLER, &[p_ok, p_bad], false).expect("decides");
        assert_eq!(
            v,
            LivenessVerdict::Violated,
            "one violated conjunct ⇒ conjunction violated"
        );
    }

    #[test]
    fn conjunction_empty_is_none() {
        assert!(response_liveness_rescue_conjunction(RESPONDER, &[], false).is_none());
    }

    #[test]
    fn parse_response_atom_rejects_relational() {
        assert!(
            parse_response_atom("x == y").is_err(),
            "relational atom rejected"
        );
        assert!(
            parse_response_atom("not an atom !!").is_err(),
            "garbage rejected"
        );
    }

    // The surface label comes from the canonical PropertyVerdict — Inconclusive folds
    // to the shared `unknown`, not a bespoke `inconclusive`.
    #[test]
    fn liveness_verdict_maps_to_canonical_vocabulary() {
        use crate::verdict::PropertyVerdict;
        assert_eq!(
            PropertyVerdict::from(LivenessVerdict::Holds).as_str(),
            "holds"
        );
        assert_eq!(
            PropertyVerdict::from(LivenessVerdict::Violated).as_str(),
            "violated"
        );
        assert_eq!(
            PropertyVerdict::from(LivenessVerdict::Inconclusive).as_str(),
            "unknown"
        );
    }
}
