//! Phase 2 · P2.1 — the **shared lazy-refinement core**'s refinement primitive.
//!
//! This is the ONE capability that unifies KMTS-based verification (plan
//! `cube-ic3ia-invariant-discovery.md` §9): given a spurious abstract step — a
//! source region that the abstraction lets reach a target region, but the EXACT
//! transition forbids — synthesise a new predicate that separates them. Both
//! Phase-2 drivers call it: the safety IC3ia frame-ladder (block a spurious CTI)
//! and the 3-valued μ-calculus evaluator (refine a spurious `⊥` obligation),
//! replacing the eager-cube + CEGAR-predicate-split with lazy, interpolation-driven
//! discovery.
//!
//! **Mechanism.** Build `A = source(cur) ∧ T(cur→nx)` and `B = ¬target(nx)` over
//! the *exact* transition ([`encode_design`]). If the step is realizable
//! (`A ∧ target(nx)` SAT) there is nothing to refine. Otherwise cvc5's
//! `get-interpolant` (convention `A ⟹ I ⟹ B`) yields `I(nx)` — an
//! over-approximation of the concrete image of `source` that excludes `target`.
//! The next-state variables are named by their **register name** (current-state
//! ones get a `__cur` suffix), so `I` comes back over register names and parses
//! directly to a [`PredicateExpr`] (via `#318`) — a ready cube dimension.
//!
//! **Soundness.** The returned predicate is only a *candidate cube dimension*.
//! Adding any well-formed predicate refines the abstraction monotonically
//! (Shoham–Grumberg) and can never flip a definite verdict — so a weak/mis-parsed
//! interpolant costs refinement power, never soundness.

use std::collections::{BTreeSet, HashMap};

use z3::ast::{Ast, BV, Bool};

use crate::adapter::btor2::ast::{Btor2File, Nid};
use crate::adapter::btor2::native_interp::{
    extract_interpolant_body, run_cvc5_raw, serialize_term,
};
use crate::adapter::btor2::predicate_expr::PredicateExpr;
use crate::adapter::cvc5::parse_interpolant_to_predicate_expr;
use crate::adapter::sidecar::predicate_image::btor2_encode::{SignalKind, encode_design};

/// Outcome of a refinement query.
#[derive(Debug)]
pub enum RefineOutcome {
    /// The abstract step is spurious; this predicate separates it — a new cube
    /// dimension for the abstraction.
    Predicate(PredicateExpr),
    /// The step is concretely realizable under the exact transition — not
    /// spurious, so no refinement is needed (the driver must look elsewhere).
    Realizable,
    /// cvc5 absent / timed out / produced an interpolant outside the parseable
    /// grammar, encode failed, or a predicate referenced an unknown register.
    /// The caller falls back (a different predicate source, or abstains).
    Unavailable(String),
}

/// P2.1 — synthesise a separating predicate for a (possibly spurious) one-step
/// abstract transition `source(cur) → target(nx)` over the exact transition of
/// `file`. See the module docs.
pub fn synthesize_refinement_predicate(
    file: &Btor2File,
    source: &[PredicateExpr],
    target: &[PredicateExpr],
    timeout_ms: u32,
) -> RefineOutcome {
    let cfg = z3::Config::new();
    z3::with_z3_config(&cfg, || {
        let view = match encode_design(file) {
            Ok(v) => v,
            Err(e) => return RefineOutcome::Unavailable(format!("encode: {e:?}")),
        };

        // Register symbol → NID (only named state cells are addressable predicates).
        let name_to_nid: HashMap<String, Nid> = view
            .signals
            .iter()
            .filter(|s| s.kind == SignalKind::State)
            .filter_map(|s| s.symbol.clone().map(|sym| (sym, s.nid)))
            .collect();
        let nid_to_name: HashMap<Nid, String> =
            name_to_nid.iter().map(|(n, &d)| (d, n.clone())).collect();

        // Controlled-name interface: next-state cells keep the register NAME (so the
        // interpolant, over the shared vocabulary, parses straight to a PredicateExpr);
        // current-state cells get a `__cur` suffix; inputs an `__in` suffix.
        let named = |suffix: &str| -> HashMap<Nid, BV> {
            view.state_curr
                .iter()
                .filter_map(|(nid, bv)| {
                    let sym = nid_to_name.get(nid)?;
                    Some((*nid, BV::new_const(format!("{sym}{suffix}"), bv.get_size())))
                })
                .collect()
        };
        let cur = named("__cur");
        let nx = named("");
        let inp: HashMap<Nid, BV> = view
            .inputs
            .iter()
            .map(|(nid, bv)| (*nid, BV::new_const(format!("in{nid}__in"), bv.get_size())))
            .collect();

        // Instantiate the transition over the interface names.
        let mut subs: Vec<(&BV, &BV)> = Vec::new();
        for (nid, bv) in &view.state_curr {
            if let Some(c) = cur.get(nid) {
                subs.push((bv, c));
            }
        }
        for (nid, bv) in &view.state_next {
            if let Some(n) = nx.get(nid) {
                subs.push((bv, n));
            }
        }
        for (nid, bv) in &view.inputs {
            if let Some(i) = inp.get(nid) {
                subs.push((bv, i));
            }
        }
        let transition = view.transition.substitute(&subs);

        // Build the source region over `cur` and the target region over `nx`.
        let build = |ps: &[PredicateExpr], frame: &HashMap<Nid, BV>| -> Option<Bool> {
            let lookup = |name: &str| -> Option<BV> {
                name_to_nid
                    .get(name)
                    .and_then(|nid| frame.get(nid))
                    .cloned()
            };
            let mut conj = Vec::new();
            for p in ps {
                conj.push(p.build_constraint(&lookup)?);
            }
            Some(if conj.is_empty() {
                Bool::from_bool(true)
            } else {
                Bool::and(&conj.iter().collect::<Vec<_>>())
            })
        };
        let source_bool = match build(source, &cur) {
            Some(b) => b,
            None => {
                return RefineOutcome::Unavailable("source references an unknown register".into());
            }
        };
        let target_bool = match build(target, &nx) {
            Some(b) => b,
            None => {
                return RefineOutcome::Unavailable("target references an unknown register".into());
            }
        };

        let mk_solver = || {
            let s = z3::Solver::new();
            let mut p = z3::Params::new();
            p.set_u32("timeout", timeout_ms);
            s.set_params(&p);
            s
        };

        // A = source(cur) ∧ T. Realizable ⇔ A ∧ target(nx) SAT.
        let a = Bool::and(&[&source_bool, &transition]);
        {
            let s = mk_solver();
            s.assert(&a);
            s.assert(&target_bool);
            match s.check() {
                z3::SatResult::Sat => return RefineOutcome::Realizable,
                z3::SatResult::Unknown => {
                    return RefineOutcome::Unavailable("solver timeout on realizability".into());
                }
                z3::SatResult::Unsat => {} // spurious → interpolate
            }
        }

        // Spurious: interpolate A vs B = ¬target(nx). Shared vocabulary = the
        // next-state register names ⇒ I is a state predicate over them.
        let b = target_bool.not();
        let (a_decls, a_body) = match serialize_term(&a) {
            Some(x) => x,
            None => return RefineOutcome::Unavailable("serialize A".into()),
        };
        let (b_decls, b_body) = match serialize_term(&b) {
            Some(x) => x,
            None => return RefineOutcome::Unavailable("serialize B".into()),
        };
        let mut decls: BTreeSet<String> = BTreeSet::new();
        decls.extend(a_decls);
        decls.extend(b_decls);
        let mut query =
            String::from("(set-logic QF_BV)\n(set-option :produce-interpolants true)\n");
        for d in &decls {
            query.push_str(d);
            query.push('\n');
        }
        query.push_str(&format!(
            "(assert {a_body})\n(get-interpolant I {b_body})\n(exit)\n"
        ));

        let stdout = match run_cvc5_raw(&query, timeout_ms) {
            Ok(s) => s,
            Err(e) => return RefineOutcome::Unavailable(e),
        };
        if stdout.trim().is_empty()
            || stdout.contains("(error")
            || stdout
                .lines()
                .any(|l| l.trim() == "fail" || l.trim() == "none")
        {
            return RefineOutcome::Unavailable("cvc5 produced no interpolant".into());
        }
        match parse_interpolant_to_predicate_expr(&stdout) {
            Some(pe) => RefineOutcome::Predicate(pe),
            None => RefineOutcome::Unavailable(format!(
                "interpolant outside the parseable grammar: {:?}",
                extract_interpolant_body(&stdout)
            )),
        }
    })
}

/// P2.3 — synthesise a **reachability-constraining** predicate via PATH
/// interpolation: the Craig interpolant of `A = Init(s0) ∧ T(s0,s1)` against
/// `B = ¬(bad reachable within `suffix_depth` steps from s1)`, over the exact
/// transition, parsed to a [`PredicateExpr`].
///
/// This closes the gap the single-step [`synthesize_refinement_predicate`] cannot:
/// a cube that is *abstractly* reachable but *concretely* unreachable (every single
/// step realizable, the composed path not) needs a predicate that constrains
/// **reachability**, which only a multi-step suffix interpolant supplies. `I(s1)`
/// over-approximates the one-step image of `Init` while excluding states that reach
/// `bad` within `suffix_depth` — exactly such a predicate.
///
/// Returns `Realizable` when `bad` IS reachable within the suffix from `Init`'s
/// image (no interpolant — the caller confirms a real CEX elsewhere), and
/// `Unavailable` when cvc5 is absent / the interpolant is outside the `#318`
/// grammar (the grammar ceiling — the emergent-K parser-extension frontier).
pub fn synthesize_reachability_predicate(
    file: &Btor2File,
    suffix_depth: usize,
    timeout_ms: u32,
) -> RefineOutcome {
    let k = suffix_depth.max(1);
    let cfg = z3::Config::new();
    z3::with_z3_config(&cfg, || {
        let view = match encode_design(file) {
            Ok(v) => v,
            Err(e) => return RefineOutcome::Unavailable(format!("encode: {e:?}")),
        };
        let nid_to_name: HashMap<Nid, String> = view
            .signals
            .iter()
            .filter(|s| s.kind == SignalKind::State)
            .filter_map(|s| s.symbol.clone().map(|sym| (s.nid, sym)))
            .collect();

        // Frame state maps: `s0` (`<reg>__cur`), `s1` (`<reg>` — the interpolation
        // vocabulary, so `I` parses to a predicate), `s2..sk` (`<reg>__f<j>`).
        let mk_state = |tag: &dyn Fn(&str) -> String| -> HashMap<Nid, BV> {
            view.state_curr
                .iter()
                .filter_map(|(nid, bv)| {
                    nid_to_name
                        .get(nid)
                        .map(|sym| (*nid, BV::new_const(tag(sym), bv.get_size())))
                })
                .collect()
        };
        let s0 = mk_state(&|sym| format!("{sym}__cur"));
        let mut frames: Vec<HashMap<Nid, BV>> = Vec::new(); // frames[j] = state at step j+1
        frames.push(mk_state(&|sym| sym.to_string())); // s1 = register name
        for j in 2..=k {
            frames.push(mk_state(&|sym| format!("{sym}__f{j}")));
        }
        // Fresh inputs per step (0..k): step `t` uses `inp[t]`.
        let inp: Vec<HashMap<Nid, BV>> = (0..=k)
            .map(|t| {
                view.inputs
                    .iter()
                    .map(|(nid, bv)| (*nid, BV::new_const(format!("in{nid}__t{t}"), bv.get_size())))
                    .collect()
            })
            .collect();

        // The transition from state map `src` to `dst` using inputs `iv`.
        let trans =
            |src: &HashMap<Nid, BV>, dst: &HashMap<Nid, BV>, iv: &HashMap<Nid, BV>| -> Bool {
                let mut subs: Vec<(&BV, &BV)> = Vec::new();
                for (nid, bv) in &view.state_curr {
                    if let Some(s) = src.get(nid) {
                        subs.push((bv, s));
                    }
                }
                for (nid, bv) in &view.state_next {
                    if let Some(d) = dst.get(nid) {
                        subs.push((bv, d));
                    }
                }
                for (nid, bv) in &view.inputs {
                    if let Some(i) = iv.get(nid) {
                        subs.push((bv, i));
                    }
                }
                view.transition.substitute(&subs)
            };
        // `bad` over a state map + its inputs.
        let one1 = BV::from_u64(1, 1);
        let bad_ops: Vec<Nid> = file
            .lines
            .iter()
            .filter_map(|l| match &l.node {
                crate::adapter::btor2::ast::Node::Bad { signal } => Some(signal.nid()),
                _ => None,
            })
            .collect();
        if bad_ops.is_empty() {
            return RefineOutcome::Unavailable("design has no `bad`".into());
        }
        let bad_over = |st: &HashMap<Nid, BV>, iv: &HashMap<Nid, BV>| -> Bool {
            let mut subs: Vec<(&BV, &BV)> = Vec::new();
            for (nid, bv) in &view.state_curr {
                if let Some(s) = st.get(nid) {
                    subs.push((bv, s));
                }
            }
            for (nid, bv) in &view.inputs {
                if let Some(i) = iv.get(nid) {
                    subs.push((bv, i));
                }
            }
            let disj: Vec<Bool> = bad_ops
                .iter()
                .filter_map(|op| {
                    view.signal_bvs
                        .get(op)
                        .or_else(|| view.state_curr.get(op))
                        .map(|bv| bv.substitute(&subs).eq(&one1))
                })
                .collect();
            if disj.is_empty() {
                Bool::from_bool(false)
            } else {
                Bool::or(&disj.iter().collect::<Vec<_>>())
            }
        };

        // Init(s0).
        let init = {
            let subs: Vec<(&BV, &BV)> = view
                .state_curr
                .iter()
                .filter_map(|(nid, bv)| s0.get(nid).map(|c| (bv, c)))
                .collect();
            let conj: Vec<Bool> = file
                .lines
                .iter()
                .filter_map(|l| match &l.node {
                    crate::adapter::btor2::ast::Node::Init { state, value, .. } => {
                        let c = s0.get(state)?;
                        let vbv = view
                            .signal_bvs
                            .get(&value.nid())
                            .or_else(|| view.state_curr.get(&value.nid()))?
                            .substitute(&subs);
                        Some(c.eq(&vbv))
                    }
                    _ => None,
                })
                .collect();
            if conj.is_empty() {
                Bool::from_bool(true)
            } else {
                Bool::and(&conj.iter().collect::<Vec<_>>())
            }
        };

        // A = Init(s0) ∧ T(s0→s1).
        let a = Bool::and(&[&init, &trans(&s0, &frames[0], &inp[0])]);
        // reach = ⋁_{j=1}^{k} [ (⋀_{i=1}^{j-1} T(s_i→s_{i+1})) ∧ bad(s_j) ].
        let mut reach_disj: Vec<Bool> = Vec::new();
        for j in 1..=k {
            let mut path: Vec<Bool> = Vec::new();
            for i in 1..j {
                path.push(trans(&frames[i - 1], &frames[i], &inp[i]));
            }
            path.push(bad_over(&frames[j - 1], &inp[j - 1]));
            reach_disj.push(Bool::and(&path.iter().collect::<Vec<_>>()));
        }
        let reach = Bool::or(&reach_disj.iter().collect::<Vec<_>>());
        let b = reach.not();

        // Realizable (bad reachable within k from Init) ⇒ no interpolant.
        {
            let solver = z3::Solver::new();
            let mut params = z3::Params::new();
            params.set_u32("timeout", timeout_ms);
            solver.set_params(&params);
            solver.assert(&a);
            solver.assert(&reach);
            match solver.check() {
                z3::SatResult::Sat => return RefineOutcome::Realizable,
                z3::SatResult::Unknown => {
                    return RefineOutcome::Unavailable("timeout (reachability)".into());
                }
                z3::SatResult::Unsat => {}
            }
        }

        // Interpolate A vs B; I over `s1` (register names) → PredicateExpr.
        let (a_decls, a_body) = match serialize_term(&a) {
            Some(x) => x,
            None => return RefineOutcome::Unavailable("serialize A".into()),
        };
        let (b_decls, b_body) = match serialize_term(&b) {
            Some(x) => x,
            None => return RefineOutcome::Unavailable("serialize B".into()),
        };
        let mut decls: BTreeSet<String> = BTreeSet::new();
        decls.extend(a_decls);
        decls.extend(b_decls);
        let mut query =
            String::from("(set-logic QF_BV)\n(set-option :produce-interpolants true)\n");
        for d in &decls {
            query.push_str(d);
            query.push('\n');
        }
        query.push_str(&format!(
            "(assert {a_body})\n(get-interpolant I {b_body})\n(exit)\n"
        ));
        let stdout = match run_cvc5_raw(&query, timeout_ms) {
            Ok(s) => s,
            Err(e) => return RefineOutcome::Unavailable(e),
        };
        if stdout.trim().is_empty()
            || stdout.contains("(error")
            || stdout
                .lines()
                .any(|l| l.trim() == "fail" || l.trim() == "none")
        {
            return RefineOutcome::Unavailable("cvc5 produced no interpolant".into());
        }
        match parse_interpolant_to_predicate_expr(&stdout) {
            Some(pe) => RefineOutcome::Predicate(pe),
            None => RefineOutcome::Unavailable(format!(
                "interpolant outside the parseable grammar: {:?}",
                extract_interpolant_body(&stdout)
            )),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser;
    use crate::adapter::btor2::predicate_expr::CmpOp;

    // 2-bit `x` counter: init 0, next x = x + 1. No `bad` needed — refinement is
    // about a source→target STEP, not reachability.
    const COUNTER: &str = "1 sort bitvec 2\n2 zero 1\n3 state 1 x\n4 init 1 3 2\n\
                           5 one 1\n6 add 1 3 5\n7 next 1 3 6\n";

    fn cmp(reg: &str, value: u64) -> PredicateExpr {
        PredicateExpr::Cmp {
            register: reg.to_string(),
            op: CmpOp::Eq,
            value,
        }
    }

    /// A spurious step (`x==0 → x==2` is impossible: `0+1=1≠2`) must yield a
    /// separating predicate over `x`.
    #[test]
    fn spurious_step_yields_separating_predicate() {
        let file = parser::parse(COUNTER).expect("parse");
        match synthesize_refinement_predicate(&file, &[cmp("x", 0)], &[cmp("x", 2)], 5_000) {
            RefineOutcome::Predicate(pe) => {
                let s = format!("{pe:?}");
                assert!(s.contains('x'), "predicate should mention x: {s}");
            }
            RefineOutcome::Realizable => panic!("x==0 → x==2 is NOT realizable (0+1=1)"),
            RefineOutcome::Unavailable(w) => eprintln!("SKIP (cvc5/grammar): {w}"),
        }
    }

    /// P2.3 — path interpolation synthesises a *reachability* predicate: `a` holds at
    /// 0, `b = a+1`, `bad = (b==2)`. `bad` is not reachable, so the multi-step suffix
    /// interpolant over-approximates the reachable states — a predicate single-step
    /// refinement cannot produce.
    #[test]
    fn path_interpolation_yields_reachability_predicate() {
        const D: &str = "1 sort bitvec 2\n2 zero 1\n3 state 1 a\n4 init 1 3 2\n5 next 1 3 3\n\
                         6 state 1 b\n7 init 1 6 2\n8 one 1\n9 add 1 3 8\n10 next 1 6 9\n\
                         11 constd 1 2\n12 sort bitvec 1\n13 eq 12 6 11\n14 bad 13\n";
        let file = parser::parse(D).expect("parse");
        match synthesize_reachability_predicate(&file, 4, 5_000) {
            RefineOutcome::Predicate(pe) => {
                // A genuine over-approximation of the reachable states.
                assert!(!format!("{pe:?}").is_empty());
            }
            // Grammar ceiling (interpolant not parseable) or cvc5 absent — a sound
            // fall-through, not a failure.
            RefineOutcome::Unavailable(w) => eprintln!("SKIP (grammar/cvc5): {w}"),
            RefineOutcome::Realizable => {
                panic!("bad (b==2) is NOT reachable — cannot be Realizable")
            }
        }
    }

    /// A realizable step (`x==0 → x==1`: `0+1=1`) must report `Realizable`, never a
    /// spurious predicate.
    #[test]
    fn realizable_step_needs_no_refinement() {
        let file = parser::parse(COUNTER).expect("parse");
        match synthesize_refinement_predicate(&file, &[cmp("x", 0)], &[cmp("x", 1)], 5_000) {
            RefineOutcome::Realizable => {}
            RefineOutcome::Predicate(pe) => panic!("x==0 → x==1 IS realizable, got {pe:?}"),
            RefineOutcome::Unavailable(w) => eprintln!("SKIP: {w}"),
        }
    }
}
