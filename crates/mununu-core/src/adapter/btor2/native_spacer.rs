//! SPACER frontend — a btor2→CHC encoding decided by **Z3's SPACER**
//! (Fixedpoint/IC3-PDR) engine.
//!
//! **Ownership note (do not overclaim):** the model-checking *algorithm* here is
//! **Z3's**, not mununu's. Unlike native BMC / k-induction / McMillan interpolation —
//! where mununu drives the search loop and calls z3/cvc5 only as per-query oracles —
//! this module builds the Horn encoding and hands the *entire* IC3/PDR + interpolation
//! search to `z3::Fixedpoint::query` (SPACER runs the frames, generalization, and proof
//! obligations internally). So mununu owns the **encoding**, Z3 owns the **deciding**;
//! for engine-provenance accounting this is an *external algorithm* run in-process (the
//! same algorithm-ownership class as btormc/Pono, differing only in that Z3 is linked
//! rather than forked as a subprocess). It decides SAFE properties native k-induction
//! ([`crate::adapter::btor2::native_bmc::decide_bad_safety`]) leaves `Unknown` — the
//! *non-k-inductive* ones needing an inductive-invariant search — with no subprocess.
//!
//! # The Horn encoding
//!
//! The BTOR2 safety problem is posed as constrained Horn clauses over a single
//! invariant relation `Inv` (one bit-vector argument per state cell):
//!
//! ```text
//!   Init(s)  ∧ C(s)          ⟹ Inv(s)         -- initial states are reachable
//!   Inv(s) ∧ T(s,i,s') ∧ C(s) ⟹ Inv(s')        -- reachability is closed under T
//!   Inv(s) ∧ Bad(s)          ⟹ Err            -- a reachable bad state is an error
//! ```
//!
//! and `query(Err)`: **SAT** ⇒ `Err` is derivable ⇒ a bad state is reachable
//! ([`SafetyVerdict::Violated`]); **UNSAT** ⇒ SPACER found an inductive invariant
//! ([`SafetyVerdict::Safe`]); **Unknown** ⇒ timeout / gave up.
//!
//! SPACER gives no natural counterexample *depth* or inductive *k*, so those
//! fields carry `0` here (the verdict, not the witness, is the product).

use crate::adapter::btor2::ast::{Btor2File, Nid, Node};
use crate::adapter::btor2::native_bmc::SafetyVerdict;
use crate::adapter::sidecar::predicate_image::btor2_encode::{EncodeError, encode_design};
use z3::ast::{Ast, BV, Bool};
use z3::{FuncDecl, Sort};

/// Default wall-clock budget for a portfolio SPACER solve. Invariant discovery can
/// run longer than bounded BMC, so this is more generous than
/// [`native_bmc::DEFAULT_TIMEOUT_MS`](crate::adapter::btor2::native_bmc::DEFAULT_TIMEOUT_MS),
/// but still bounded — a timeout abstains ([`SafetyVerdict::Unknown`]), never a wrong
/// verdict.
pub const DEFAULT_TIMEOUT_MS: u32 = 10_000;

/// `a ⟹ b`. Must be a real `(=> a b)` (not the `¬a ∨ b` rewrite): SPACER inspects
/// the *syntactic* head of each rule and rejects an `(or …)` head as "not an
/// uninterpreted predicate", so the implication node is load-bearing.
fn implies(a: &Bool, b: &Bool) -> Bool {
    a.implies(b)
}

/// Conjunction of a slice of `Bool`s; the empty conjunction is `true`.
fn and_all(cs: &[Bool]) -> Bool {
    if cs.is_empty() {
        Bool::from_bool(true)
    } else {
        let refs: Vec<&Bool> = cs.iter().collect();
        Bool::and(&refs)
    }
}

/// Decide `bad`-reachability of `file` with **Z3's SPACER** (Fixedpoint/CHC) engine.
/// mununu owns the btor2→CHC encoding; the IC3/PDR + interpolation model checking is
/// Z3's (see the module docs — this is an external algorithm run in-process, not an
/// in-house model checker). `timeout_ms` bounds the SPACER solve (a timeout returns
/// [`SafetyVerdict::Unknown`] — never a wrong verdict).
pub fn decide_bad_safety_spacer(
    file: &Btor2File,
    timeout_ms: Option<u32>,
) -> Result<SafetyVerdict, EncodeError> {
    let mut bad_ops: Vec<Nid> = Vec::new();
    let mut init_pairs: Vec<(Nid, Nid)> = Vec::new();
    let mut constraint_ops: Vec<Nid> = Vec::new();
    for l in &file.lines {
        match &l.node {
            Node::Bad { signal } => bad_ops.push(signal.nid()),
            Node::Init { state, value, .. } => init_pairs.push((*state, value.nid())),
            Node::Constraint { signal } => constraint_ops.push(signal.nid()),
            _ => {}
        }
    }
    if bad_ops.is_empty() {
        // No property ⇒ nothing can be violated.
        return Ok(SafetyVerdict::Safe { k: 0 });
    }

    let cfg = z3::Config::new();
    z3::with_z3_config(&cfg, || {
        let view = encode_design(file)?;

        // Deterministic state-cell ordering for the `Inv` relation's argument list.
        let mut states: Vec<Nid> = view.state_curr.keys().copied().collect();
        states.sort();
        if states.is_empty() {
            // A purely combinational design: `Inv` has no state; fall back is a
            // single-step check — but SPACER needs a state relation, so defer such
            // designs to BMC (return Unknown here rather than mis-encode).
            return Ok(SafetyVerdict::Unknown { k: 0 });
        }

        let sorts: Vec<Sort> = states
            .iter()
            .map(|n| Sort::bitvector(view.state_curr[n].get_size()))
            .collect();
        let sort_refs: Vec<&Sort> = sorts.iter().collect();
        let bool_sort = Sort::bool();
        let inv = FuncDecl::new("Inv", &sort_refs, &bool_sort);
        let err = FuncDecl::new("Err", &[], &bool_sort);

        let fp = z3::Fixedpoint::new();
        // Select SPACER — Z3's IC3/PDR + interpolation CHC engine — explicitly. The
        // Fixedpoint default is `auto-config`, which routes *these* bit-vector Horn
        // rules to SPACER anyway, but pinning it keeps the choice robust against
        // auto-config heuristics. NB: `engine` is a *Fixedpoint-object* parameter set
        // via `set_params`, NOT a Config/global one (setting `fp.engine` on the Config
        // is rejected by Z3 with a warning). Context-scoped — it does not perturb the
        // `Solver`-based native BMC / k-induction engines.
        let mut params = z3::Params::new();
        params.set_symbol("engine", "spacer");
        if let Some(ms) = timeout_ms {
            params.set_u32("timeout", ms);
        }
        fp.set_params(&params);
        fp.register_relation(&inv);
        fp.register_relation(&err);

        // `Inv` applied to the current / next state variables (in `states` order).
        // Inlined (not a `dyn Fn` picker) so the borrow is inferred against `view`
        // rather than erased to `'static` by the trait object.
        let inv_curr = {
            let args: Vec<&dyn Ast> = states
                .iter()
                .map(|n| &view.state_curr[n] as &dyn Ast)
                .collect();
            inv.apply(&args).as_bool().expect("Inv is a Bool relation")
        };
        let inv_next = {
            let args: Vec<&dyn Ast> = states
                .iter()
                .map(|n| &view.state_next[n] as &dyn Ast)
                .collect();
            inv.apply(&args).as_bool().expect("Inv is a Bool relation")
        };
        let err_app = err.apply(&[]).as_bool().expect("Err is a Bool relation");

        let one1 = BV::from_u64(1, 1);
        let curr_bv = |nid: &Nid| -> Option<&BV> {
            view.signal_bvs
                .get(nid)
                .or_else(|| view.state_curr.get(nid))
                .or_else(|| view.inputs.get(nid))
        };
        // Constraints (over the current state / inputs) — asserted on every rule.
        let constr: Vec<Bool> = constraint_ops
            .iter()
            .filter_map(|op| curr_bv(op).map(|bv| bv.eq(&one1)))
            .collect();
        let constr_conj = and_all(&constr);

        // Init(s): the design's `init` values pin the initialised cells; init-less
        // cells stay free (BTOR2 nondeterministic-init semantics).
        let init_conj: Vec<Bool> = init_pairs
            .iter()
            .filter_map(|(state, val)| {
                match (view.state_curr.get(state), view.signal_bvs.get(val)) {
                    (Some(sc), Some(vbv)) => Some(sc.eq(vbv)),
                    _ => None,
                }
            })
            .collect();
        let init_pred = and_all(&init_conj);

        // Bad(s): OR over the `bad` operands of `operand == 1`.
        let bad_disj: Vec<Bool> = bad_ops
            .iter()
            .filter_map(|op| curr_bv(op).map(|bv| bv.eq(&one1)))
            .collect();
        let bad_pred = if bad_disj.is_empty() {
            Bool::from_bool(false)
        } else {
            Bool::or(&bad_disj.iter().collect::<Vec<_>>())
        };

        // Quantifier bounds.
        let curr_bounds: Vec<&dyn Ast> = states
            .iter()
            .map(|n| &view.state_curr[n] as &dyn Ast)
            .collect();
        let mut all_bounds: Vec<&dyn Ast> = curr_bounds.clone();
        for bv in view.inputs.values() {
            all_bounds.push(bv as &dyn Ast);
        }
        for n in &states {
            all_bounds.push(&view.state_next[n] as &dyn Ast);
        }

        // Rule 1: Init(s) ∧ C(s) ⟹ Inv(s).
        let body1 = implies(&and_all(&[init_pred, constr_conj.clone()]), &inv_curr);
        fp.add_rule(&z3::ast::forall_const(&curr_bounds, &[], &body1), None);
        // Rule 2: Inv(s) ∧ T(s,i,s') ∧ C(s) ⟹ Inv(s').
        let body2 = implies(
            &and_all(&[
                inv_curr.clone(),
                view.transition.clone(),
                constr_conj.clone(),
            ]),
            &inv_next,
        );
        fp.add_rule(&z3::ast::forall_const(&all_bounds, &[], &body2), None);
        // Rule 3: Inv(s) ∧ Bad(s) ⟹ Err.
        let body3 = implies(&and_all(&[inv_curr, bad_pred]), &err_app);
        fp.add_rule(&z3::ast::forall_const(&curr_bounds, &[], &body3), None);

        // SPACER query semantics over the *exact* Horn encoding of the concrete
        // transition relation: SAT ⇒ `Err` derivable ⇒ a real Init →* bad path exists
        // ([`SafetyVerdict::Violated`]); UNSAT ⇒ SPACER exhibited an inductive
        // invariant excluding every bad state ([`SafetyVerdict::Safe`]).
        // SOUNDNESS: both definite verdicts are exact (no over/under-approximation —
        // Init pins only the design's `init` values, init-less cells stay free per
        // BTOR2 semantics). UNDEF (timeout / gave up) maps to `Unknown` — abstain,
        // never a wrong verdict, matching the native BMC / k-induction contract.
        Ok(match fp.query(&err_app) {
            z3::SatResult::Sat => SafetyVerdict::Violated { depth: 0 },
            z3::SatResult::Unsat => SafetyVerdict::Safe { k: 0 },
            z3::SatResult::Unknown => SafetyVerdict::Unknown { k: 0 },
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser;

    fn spacer(content: &str) -> SafetyVerdict {
        let file = parser::parse(content).expect("parse btor2");
        decide_bad_safety_spacer(&file, Some(10_000)).expect("spacer runs")
    }

    // `q` init 0, `next q = 1`, `bad = q`. Reachable at step 1.
    const REACH: &str = "1 sort bitvec 1\n2 zero 1\n3 one 1\n4 state 1 q\n5 init 1 4 2\n\
                         6 next 1 4 3\n7 bad 4\n";
    // `q` init 0, `next q = 0`, `bad = q`. Safe (invariant q==0).
    const SAFE: &str = "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 next 1 3 2\n\
                        6 bad 3\n";

    #[test]
    fn spacer_finds_violation() {
        assert_eq!(spacer(REACH), SafetyVerdict::Violated { depth: 0 });
    }

    #[test]
    fn spacer_proves_safe() {
        assert_eq!(spacer(SAFE), SafetyVerdict::Safe { k: 0 });
    }

    #[test]
    fn spacer_decides_a_safe_property_that_k_induction_abstains_on() {
        // An 8-bit counter that STALLS at 0: init 0, next = (c == 0) ? 0 : c+1,
        // bad = (c == 255). The reachable set is exactly {0}, so `c == 255` is
        // unreachable ⇒ SAFE. But *simple* k-induction (no loop-free constraint) can
        // build an unreachable free path 1 → 2 → … → 255 that stays ¬bad until the
        // last step, keeping the step case satisfiable for every depth below 255 — so
        // native k-induction ABSTAINS at its default cap. SPACER discovers the
        // inductive invariant `c == 0` and proves the property outright. This is the
        // gap the SPACER portfolio member exists to close.
        let stall = "1 sort bitvec 8\n2 zero 1\n3 one 1\n4 state 1 c\n5 init 1 4 2\n\
                     6 sort bitvec 1\n7 eq 6 4 2\n8 add 1 4 3\n9 ite 1 7 2 8\n\
                     10 next 1 4 9\n11 ones 1\n12 eq 6 4 11\n13 bad 12\n";
        let file = parser::parse(stall).expect("parse btor2");

        // Native k-induction abstains: its default cap (40) is far below depth 255.
        let native = crate::adapter::btor2::native_bmc::decide_bad_safety(&file, 40, Some(10_000))
            .expect("native k-induction runs");
        assert!(
            matches!(native, SafetyVerdict::Unknown { .. }),
            "expected native k-induction to abstain below the required depth, got {native:?}"
        );

        // SPACER decides it Safe via invariant discovery.
        assert_eq!(spacer(stall), SafetyVerdict::Safe { k: 0 });
    }
}
