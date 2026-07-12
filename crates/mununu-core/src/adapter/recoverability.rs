//! P2 — recoverability `AG EF good` ("from every reachable state, can the design
//! still get back to a good state?"), the branching property SVA cannot express.
//!
//! # The property
//!
//! Recoverability is the CTL formula `AG EF good` — from every reachable state
//! (`AG`) there **exists** a path back to `good` (`EF`). In the modal-μ calculus it
//! is an alternating fixpoint (a greatest fixpoint wrapping a least fixpoint):
//!
//! ```text
//!   nu Y. ((mu X. (good || <> X)) && [] Y)
//! ```
//!
//! The `<>` (some-successor) inside the `[]` (all-successors) is the branching
//! content — it quantifies existentially over futures *inside* a universal envelope,
//! which is exactly what a linear formalism (LTL / SVA) cannot state. See
//! [`docs/design/recoverability-vs-sva.md`](../../../docs/design/recoverability-vs-sva.md).
//!
//! # How it is decided
//!
//! This module offers the ergonomic entry point: name the `good` atom, and it builds
//! the `AG EF good` formula and decides it with the **exact 3-valued symbolic
//! engine** ([`crate::adapter::btor2::symbolic_bitblast::exact_symbolic_verdict`]) —
//! sound at every alternation depth (Bruns–Godefroid), definite within the engine's
//! 40-bit cone cap. Over the cap it abstains (`Unknown`).
//!
//! For designs wider than the exact cap, the **predicate-cube + `smt-hyper-must`**
//! path (`mununu btor2 cegar --formula … --must-edge-inference smt-hyper-must`)
//! decides the same formula via abstraction — the path the V.7-c OpenTitan `csrng`
//! recoverability showcase uses.
//!
//! P2 Slice 1 wires that path in **automatically**: [`verify_recoverability`] tries
//! the exact engine first and, when it abstains (over the ~40-bit cone cap or an
//! unsupported construct), escalates to [`verify_recoverability_scalable`] — the
//! predicate-cube `smt-hyper-must` reduction — so the νμ property decides beyond 40
//! bits. The escalation mirrors the verify-auto safety-⊥ `reach_rescue` escalation.
//! Every scalable verdict is cross-checked against the exact engine on the small
//! fixtures (the differential-oracle soundness gate; see the module tests).

use std::collections::BTreeMap;

use crate::adapter::AdapterOptions;
use crate::adapter::btor2::cegar::{
    CegarOptions, LiftStrategy, PredicateSource, cegar_refine_loop, config_values_to_sidecar_json,
};
use crate::adapter::btor2::kmts_lift::{MayEdgeInference, MustEdgeInference, PredicateSpec};
use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr, parse_predicate_expr};
use crate::adapter::btor2::symbolic_bitblast::exact_symbolic_verdict;
use crate::mu_calculus::Environment;
use crate::mu_calculus::parser as mu_parser;
use crate::mu_calculus::trit::Trit;
use crate::verdict::PropertyVerdict;

/// Max CEGAR refinement iterations for the scalable recoverability path — a sensible
/// default: a pure state-atom `AG EF` target either decides at the seed abstraction or
/// after a few weakest-precondition splits.
const RECOVERABILITY_MAX_ITERATIONS: usize = 8;

/// Extract `state_register == constant` guard atoms from the design's `Eq` comparison nodes — the
/// decision conditions the control logic branches on. Seeds the datapath predicates a
/// datapath-DEPENDENT recoverability return needs (Class 2): `busy → done` gated on `data == K`
/// yields the atom `(data, K)`, where K is a design literal (not enumerable over `2^W`). Returns only
/// comparisons of a STATE register to a resolvable CONSTANT; deduped.
fn eq_guard_atoms(file: &crate::adapter::btor2::ast::Btor2File) -> Vec<(String, u64)> {
    use crate::adapter::btor2::ast::{Node, Op};
    let symbols = crate::adapter::btor2::parser::collect_symbols(file);
    let state_nids: std::collections::HashSet<i64> = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, Node::State { .. }))
        .map(|l| l.nid)
        .collect();
    let mut out: Vec<(String, u64)> = Vec::new();
    for line in &file.lines {
        let Node::Op {
            op: Op::Eq, args, ..
        } = &line.node
        else {
            continue;
        };
        if args.len() != 2 {
            continue;
        }
        // Either operand may be the register vs the constant.
        for (reg_nid, const_nid) in [
            (args[0].0.abs(), args[1].0.abs()),
            (args[1].0.abs(), args[0].0.abs()),
        ] {
            if state_nids.contains(&reg_nid)
                && let Some(sym) = symbols.get(&reg_nid)
                && let Some(k) =
                    crate::adapter::btor2::bit_blast::resolve_btor2_constant(file, const_nid)
                && !out.iter().any(|(s, v)| s == sym && *v == k)
            {
                out.push((sym.clone(), k));
            }
        }
    }
    out
}

/// N1 Class-2 RELATIONAL discovery — the datapath-dependent return whose guard compares two STATE
/// registers (`busy → done` gated on `data == target`), not a register to a constant. Extracts the
/// `(lhs, rhs)` register pairs from the design's `Eq` comparison nodes where BOTH operands are state
/// registers — the relational decision conditions the control logic branches on. The seeded relational
/// predicate lets the recoverability return decide at SCALE: the exact hyper-must edge preserves the
/// invariant (`data == target ⟹ data' == target'`) across the wide datapath WITHOUT concretising the
/// register values (which are not enumerable over `2^W`); the may side may UF-wrap the increments, but
/// that only pushes toward `⊥` (sound, per PR #302's universal ◇). Pairs are ordered (`lhs <= rhs`) for
/// deterministic dedup (`data == target` and `target == data` are the same predicate); reflexive
/// `r == r` is dropped (no information). Deduped.
fn eq_reg_guard_atoms(file: &crate::adapter::btor2::ast::Btor2File) -> Vec<(String, String)> {
    use crate::adapter::btor2::ast::{Node, Op};
    let symbols = crate::adapter::btor2::parser::collect_symbols(file);
    let state_nids: std::collections::HashSet<i64> = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, Node::State { .. }))
        .map(|l| l.nid)
        .collect();
    let mut out: Vec<(String, String)> = Vec::new();
    for line in &file.lines {
        let Node::Op {
            op: Op::Eq, args, ..
        } = &line.node
        else {
            continue;
        };
        if args.len() != 2 {
            continue;
        }
        let (a, b) = (args[0].0.abs(), args[1].0.abs());
        if a == b {
            continue; // reflexive `r == r` carries no information
        }
        if state_nids.contains(&a)
            && state_nids.contains(&b)
            && let (Some(sa), Some(sb)) = (symbols.get(&a), symbols.get(&b))
        {
            let (lhs, rhs) = if sa <= sb {
                (sa.clone(), sb.clone())
            } else {
                (sb.clone(), sa.clone())
            };
            if !out.iter().any(|(l, r)| l == &lhs && r == &rhs) {
                out.push((lhs, rhs));
            }
        }
    }
    out
}

/// N1 frontier (b′) — arithmetic-relational discovery: the datapath-dependent return whose guard is
/// `data == target + K` (a register compared to ANOTHER register plus a constant addend), not a bare
/// `register == register`. The value that unblocks the return (`target + K`) is neither a design literal
/// nor a syntactic single-register comparison, so the literal- and relational-guard extractions both
/// miss it — but the design DOES contain it as `eq(reg, add(reg, const))`. Extracting that pattern yields
/// the `PredicateExpr::CmpRegAddend` the return needs; like the relational case, the exact hyper-must
/// edge preserves the invariant (`data == target + K ⟹ data' == target' + K`, in mod-2^width BV) across
/// the wide increments without concretising the registers. Returns `(lhs_reg, rhs_reg, addend, width)`;
/// `width` is the register width (for the mod-2^width arithmetic the reset-cube eval needs). Skips the
/// degenerate `reg == reg + K` (same register). Deduped.
fn eq_reg_addend_guard_atoms(
    file: &crate::adapter::btor2::ast::Btor2File,
) -> Vec<(String, String, u64, u32)> {
    use crate::adapter::btor2::ast::{Node, Op};
    let symbols = crate::adapter::btor2::parser::collect_symbols(file);
    let state_nids: std::collections::HashSet<i64> = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, Node::State { .. }))
        .map(|l| l.nid)
        .collect();
    let state_width = |nid: i64| -> Option<u32> {
        match file.lookup(nid).map(|l| &l.node) {
            Some(Node::State { sort, .. }) => crate::adapter::btor2::parser::bv_width(file, *sort),
            _ => None,
        }
    };
    let mut out: Vec<(String, String, u64, u32)> = Vec::new();
    for line in &file.lines {
        let Node::Op {
            op: Op::Eq, args, ..
        } = &line.node
        else {
            continue;
        };
        if args.len() != 2 {
            continue;
        }
        // One operand is a state register `lhs`; the other is an `add(state, const)` node.
        for (reg_nid, other_nid) in [
            (args[0].0.abs(), args[1].0.abs()),
            (args[1].0.abs(), args[0].0.abs()),
        ] {
            if !state_nids.contains(&reg_nid) {
                continue;
            }
            let Some(lhs_sym) = symbols.get(&reg_nid) else {
                continue;
            };
            let Some(Node::Op {
                op: Op::Add,
                args: aa,
                ..
            }) = file.lookup(other_nid).map(|l| &l.node)
            else {
                continue;
            };
            if aa.len() != 2 {
                continue;
            }
            // The add is `rhs_state + const` (either operand order).
            for (rs_nid, ac_nid) in [
                (aa[0].0.abs(), aa[1].0.abs()),
                (aa[1].0.abs(), aa[0].0.abs()),
            ] {
                if state_nids.contains(&rs_nid)
                    && let Some(rhs_sym) = symbols.get(&rs_nid)
                    && rhs_sym != lhs_sym
                    && let Some(addend) =
                        crate::adapter::btor2::bit_blast::resolve_btor2_constant(file, ac_nid)
                    && let Some(width) = state_width(reg_nid)
                    && width > 0
                    && !out
                        .iter()
                        .any(|(l, r, a, _)| l == lhs_sym && r == rhs_sym && *a == addend)
                {
                    out.push((lhs_sym.clone(), rhs_sym.clone(), addend, width));
                }
            }
        }
    }
    out
}

/// N1 frontier (a) — the good register's cone-of-influence: the state registers whose values determine
/// when `good` is (re)reached — those reachable backward from the good register's next-state function,
/// plus the register itself. A guard atom over a register OUTSIDE this cone (a decoy comparison on an
/// unrelated register) cannot change the recoverability verdict, yet seeding it still doubles the cube
/// (`2^{|P|}`) — the un-directed eager extraction's scaling wall (a design with many unrelated guards
/// blows up). Restricting the guard-atom seeds to this cone is the property-directed discovery of
/// frontier (a): only the guards the return path actually branches on are seeded, at cone-of-influence
/// granularity (a sound over-approximation of the strictly per-⊥-classifying-transition guard set).
/// Returns the register NAMES in the cone; an EMPTY set (good register absent, or no restriction found)
/// means "do not restrict" so the caller keeps its prior, unfiltered behaviour.
fn good_register_coi(
    file: &crate::adapter::btor2::ast::Btor2File,
    good_register: &str,
) -> std::collections::HashSet<String> {
    use crate::adapter::btor2::parser::{
        collect_reachable_states_from, collect_symbols, find_next_value_operand,
    };
    let symbols = collect_symbols(file);
    let Some(seed_nid) = symbols
        .iter()
        .find_map(|(nid, name)| (name == good_register).then_some(*nid))
    else {
        return std::collections::HashSet::new(); // good register absent → no restriction
    };
    let mut reachable: std::collections::HashSet<i64> = std::collections::HashSet::new();
    reachable.insert(seed_nid);
    if let Some(next_value) = find_next_value_operand(file, seed_nid) {
        reachable.extend(collect_reachable_states_from(
            file,
            std::slice::from_ref(&next_value),
        ));
    }
    reachable
        .iter()
        .filter_map(|nid| symbols.get(nid).cloned())
        .collect()
}

/// N1 frontier (a) — the constant literals appearing in the good register's next-state cone (the values
/// its control logic assigns or branches on). Directs the control-state candidate pool: enumerating
/// `good == v` for EVERY global constant (the prior behaviour) seeds control predicates the good
/// register can never take — decoy constants that belong to unrelated registers — and each is a cube
/// dimension, so a design with many unrelated constants blows up the `2^{|P|}` all-pairs SMT even though
/// none of those predicates can change the verdict. Restricting to the good register's own cone keeps
/// the control-state seeding property-directed. An EMPTY result (good register absent / no `next`) means
/// "no cone info" so the caller falls back to `{0,1} ∪ global constants`.
fn good_next_cone_constants(
    file: &crate::adapter::btor2::ast::Btor2File,
    good_register: &str,
) -> Vec<u64> {
    use crate::adapter::btor2::ast::Node;
    use crate::adapter::btor2::parser::{collect_symbols, find_next_value_operand};
    let symbols = collect_symbols(file);
    let Some(seed_nid) = symbols
        .iter()
        .find_map(|(nid, name)| (name == good_register).then_some(*nid))
    else {
        return Vec::new();
    };
    let Some(next_value) = find_next_value_operand(file, seed_nid) else {
        return Vec::new();
    };
    let mut queue: std::collections::VecDeque<i64> =
        std::collections::VecDeque::from([next_value.0.abs()]);
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut consts: Vec<u64> = Vec::new();
    while let Some(nid) = queue.pop_front() {
        if !seen.insert(nid) {
            continue;
        }
        if let Some(v) = crate::adapter::btor2::bit_blast::resolve_btor2_constant(file, nid)
            && !consts.contains(&v)
        {
            consts.push(v);
        }
        if let Some(line) = file.lookup(nid)
            && let Node::Op { args, .. } = &line.node
        {
            for o in args {
                queue.push_back(o.0.abs());
            }
        }
    }
    consts
}

/// N1 ranking helper — the INPUT nids in the next-state cone of the given (good) registers: the inputs
/// that can affect the RANKING's evolution. The ∃-input enumeration pins only these; a real design
/// often carries wide inputs outside this cone (an FPV backdoor, a hardened counter's SECONDARY-register
/// or dead-output logic — e.g. OpenTitan `prim_count`) that would otherwise swamp the enumeration's bit
/// cap. Leaving an out-of-cone input FREE rather than pinning it is SOUND — pinning fewer inputs makes
/// the UNSAT check STRONGER (∀ over the free ones), so a certified verdict stays a real witness.
fn inputs_in_next_cone(
    file: &crate::adapter::btor2::ast::Btor2File,
    seed_state_nids: &[i64],
) -> std::collections::HashSet<i64> {
    use crate::adapter::btor2::ast::Node;
    let mut queue: std::collections::VecDeque<i64> = std::collections::VecDeque::new();
    for &s in seed_state_nids {
        if let Some(nv) = crate::adapter::btor2::parser::find_next_value_operand(file, s) {
            queue.push_back(nv.0.abs());
        }
    }
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut inputs: std::collections::HashSet<i64> = std::collections::HashSet::new();
    while let Some(nid) = queue.pop_front() {
        if !seen.insert(nid) {
            continue;
        }
        match file.lookup(nid).map(|l| &l.node) {
            Some(Node::Input { .. }) => {
                inputs.insert(nid);
            }
            Some(Node::Op { args, .. }) => {
                for o in args {
                    queue.push_back(o.0.abs());
                }
            }
            _ => {}
        }
    }
    inputs
}

/// N1 ranking certificate for `AG EF good` — the well-founded-descent / RANKING class the predicate
/// cube cannot capture. When `good` is reached by a monotone datapath descent (a down-counter to 0, a
/// timer, a drain), no bounded predicate set represents the descent, so the cube abstains (`Unknown`) —
/// yet the property HOLDS. This proves it directly over the EXACT transition (Podelski–Rybalchenko),
/// sidestepping the cube. For a candidate ranking δ — `good = (r == V)` → δ = `r - V` (a DESCENT of r to
/// V) or δ = `V - r` (an ASCENT of r to V); `good = (a == b)` → δ = `a - b` or `b - a` (unsigned BV
/// difference; both directions are tried) — it checks:
///   (2) **`transition ∧ ¬good ∧ δ_next ≥ δ_curr` is UNSAT** — i.e. EVERY transition out of a non-good
///       state STRICTLY decreases δ; combined with (1) δ ≥ 0 (trivial for an unsigned BV), δ cannot
///       decrease forever, so every path reaches good.
/// Two attempts, in order of cost:
///   **(A) all-path** — `transition ∧ ¬good ∧ (δ_next ≥ δ_curr)` UNSAT: EVERY input choice out of a
///   non-good state strictly decreases δ. No quantifier (inputs free), fast; proves the stronger
///   `AG AF good` ⊆ `AG EF good`. Decides deterministic descents (down-counters, timers).
///   **(B) some-path** — decides a relation drained by only SOME input (a FIFO's read-gated
///   `wptr == rptr`, `AG EF` but not `AG AF`, since the environment may write forever) which (A) cannot.
///   Rather than a flaky quantified-BV query, it ENUMERATES constant input valuations: if some fixed
///   input `v` makes `transition ∧ ¬good ∧ (inputs == v) ∧ (δ_next ≥ δ_curr)` UNSAT, the constant-`v`
///   path strictly descends δ from every non-good state, so it reaches good ⇒ `AG EF good`. Capped at a
///   few input bits (a drain is gated on narrow control inputs), a non-quantified fast path.
/// Each attempt runs first with the single-register measure δ1, then with LEXICOGRAPHIC measures
/// `(δ1, s)` — δ1 paired with each other register `s` as a tiebreaker — so a NESTED descent decides:
/// when δ1 alone does not strictly decrease every step (it holds while an inner counter runs down), the
/// tuple `(δ1, s)` does, and lex order on the tuple is well-founded (`non_dec` becomes
/// `¬[(δ1' < δ1) ∨ (δ1' == δ1 ∧ s' < s)]`).
/// With δ ≥ 0 (trivial for an unsigned BV) either certificate ⇒ `EF good` from every state ⇒ `AG EF good`,
/// so returning `Holds` is SOUND. Both correctly FAIL on wrap/overshoot (`cnt - 2` from an odd start
/// never hits 0 → the difference jumps up → SAT) and on non-monotone designs (a sound fall-through to the
/// cube — the certificate is sufficient, not necessary).
fn ranking_certificate_holds(
    file: &crate::adapter::btor2::ast::Btor2File,
    good_registers: &[String],
    good_value: Option<u64>,
) -> bool {
    let cfg = z3::Config::new();
    z3::with_z3_config(&cfg, || {
        let Ok(view) = crate::adapter::btor2::kmts_lift::encode_design_for_lift(file) else {
            return false;
        };
        let nid_map = crate::adapter::btor2::smt_must_edge::build_register_nid_map(&view);
        let mask = |w: u32| if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        // PRIMARY ranking components + the ¬good constraint + the good register NIDs. BOTH directions are
        // tried: a DESCENT `δ = r - V` (r counts DOWN to V) and an ASCENT `δ = V - r` (r counts UP to V).
        // Exactly one direction is a well-founded measure for a monotone move toward `good`; the other
        // wraps and fails harmlessly. For a relational good `a == b`, the two are `a - b` and `b - a`.
        let (primaries, not_good, good_nids): (
            Vec<(z3::ast::BV, z3::ast::BV)>,
            z3::ast::Bool,
            Vec<i64>,
        ) = match good_value {
            Some(v) => {
                let [r] = good_registers else { return false };
                let Some(&nid) = nid_map.get(r) else {
                    return false;
                };
                let (Some(rc), Some(rn)) = (view.curr_state(nid), view.next_state(nid)) else {
                    return false;
                };
                let w = rc.get_size();
                let vbv = z3::ast::BV::from_u64(v & mask(w), w);
                (
                    vec![
                        (rc.bvsub(&vbv), rn.bvsub(&vbv)), // descent δ = r - V
                        (vbv.bvsub(rc), vbv.bvsub(rn)),   // ascent  δ = V - r
                    ],
                    rc.eq(&vbv).not(),
                    vec![nid],
                )
            }
            None => {
                let [a, b] = good_registers else { return false };
                let (Some(&na), Some(&nb)) = (nid_map.get(a), nid_map.get(b)) else {
                    return false;
                };
                let (Some(ac), Some(an), Some(bc), Some(bn)) = (
                    view.curr_state(na),
                    view.next_state(na),
                    view.curr_state(nb),
                    view.next_state(nb),
                ) else {
                    return false;
                };
                if ac.get_size() != bc.get_size() {
                    return false;
                }
                (
                    vec![(ac.bvsub(bc), an.bvsub(bn)), (bc.bvsub(ac), bn.bvsub(an))],
                    ac.eq(bc).not(),
                    vec![na, nb],
                )
            }
        };

        // Lexicographic SECONDARY candidates: each OTHER state register's (curr, next) value — any BV is
        // a well-founded tiebreaker. Bounded + sorted-by-nid for a stable search order.
        const MAX_SECONDARIES: usize = 6;
        let mut sec_nids: Vec<i64> = view
            .state_curr
            .keys()
            .copied()
            .filter(|n| !good_nids.contains(n))
            .collect();
        sec_nids.sort_unstable();
        let secondaries: Vec<(z3::ast::BV, z3::ast::BV)> = sec_nids
            .iter()
            .take(MAX_SECONDARIES)
            .filter_map(|n| {
                Some((
                    view.state_curr.get(n)?.clone(),
                    view.state_next.get(n)?.clone(),
                ))
            })
            .collect();

        // Input-valuation enumeration setup (shared across measures). Only inputs in the state
        // registers' next-state cone are enumerated — out-of-cone inputs (an FPV backdoor, a dead
        // output's wide operand) are left FREE (sound: they cannot affect any δ_next) so they do not
        // swamp the enumeration's bit cap.
        let cone_inputs = inputs_in_next_cone(file, &good_nids);
        let input_bvs: Vec<z3::ast::BV> = view
            .inputs
            .iter()
            .filter(|(nid, _)| cone_inputs.contains(nid))
            .map(|(_, bv)| bv.clone())
            .collect();
        let total_input_bits: u32 = input_bvs.iter().map(|bv| bv.get_size()).sum();
        let can_enum = total_input_bits > 0 && total_input_bits <= 10;

        // For a candidate ranking measure, `non_dec` is "the measure did NOT strictly (lex-)decrease".
        //   - ALL-PATH (`AG AF good`): `transition ∧ ¬good ∧ non_dec` UNSAT ⇒ every input choice descends
        //     (down-counters, timers, deterministic nested loops). No quantifier.
        //   - SOME-PATH (`AG EF good`): ENUMERATE constant input valuations — if some fixed `v` makes
        //     `transition ∧ ¬good ∧ (inputs == v) ∧ non_dec` UNSAT, the constant-`v` path descends (a
        //     FIFO's read-gated drain). Non-quantified (z3's quantified-BV engine is version-flaky);
        //     capped at a few input bits (a drain is gated on narrow control inputs).
        // Either UNSAT ⇒ a strictly-descending, well-founded path reaches good ⇒ `AG EF good` (SOUND).
        let certifies = |non_dec: &z3::ast::Bool| -> bool {
            let mk_solver = || {
                let solver = z3::Solver::new();
                let mut params = z3::Params::new();
                params.set_u32("timeout", 5_000);
                solver.set_params(&params);
                solver
            };
            {
                let solver = mk_solver();
                solver.assert(&view.transition);
                solver.assert(&not_good);
                solver.assert(non_dec);
                if matches!(solver.check(), z3::SatResult::Unsat) {
                    return true;
                }
            }
            if can_enum {
                for v in 0u64..(1u64 << total_input_bits) {
                    let solver = mk_solver();
                    solver.assert(&view.transition);
                    solver.assert(&not_good);
                    let mut bit = 0u32;
                    for bv in &input_bvs {
                        let w = bv.get_size();
                        let m = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                        let vbv = z3::ast::BV::from_u64((v >> bit) & m, w);
                        let pin = bv.eq(&vbv);
                        solver.assert(&pin);
                        bit += w;
                    }
                    solver.assert(non_dec);
                    if matches!(solver.check(), z3::SatResult::Unsat) {
                        return true;
                    }
                }
            }
            false
        };

        // A ranking MEASURE is a list of components (curr, next), most-significant first. `non_decrease`
        // builds "the tuple did NOT strictly lex-decrease": fold least-significant up,
        //   acc_i = (next_i < curr_i) ∨ (next_i == curr_i ∧ acc_below),  base = false; then negate.
        let non_decrease = |measure: &[(&z3::ast::BV, &z3::ast::BV)]| -> z3::ast::Bool {
            let mut acc = z3::ast::Bool::from_bool(false);
            for &(curr, next) in measure.iter().rev() {
                let lt = next.bvult(curr);
                let eq = next.eq(curr);
                let tie = z3::ast::Bool::and(&[&eq, &acc]);
                acc = z3::ast::Bool::or(&[&lt, &tie]);
            }
            acc.not()
        };

        // Candidate measures, increasing complexity. MEASURE 1: single δ1. MEASURES 2..: each 2-tuple
        // `(δ1, s)` — decides a 2-level nested descent (δ1 holds while s counts down). FINAL: the FULL
        // tuple `(δ1, s1, …, sk)` over all secondaries in nid order — decides a k-LEVEL nested descent
        // (a 3-deep counter needs the 3-tuple; a 2-tuple over any single secondary fails). Lex order on
        // the tuple is well-founded, so the descent still reaches good (SOUND).
        for (d1_curr, d1_next) in &primaries {
            let d1 = (d1_curr, d1_next);
            if certifies(&non_decrease(&[d1])) {
                return true;
            }
            for (sc, sn) in &secondaries {
                if certifies(&non_decrease(&[d1, (sc, sn)])) {
                    return true;
                }
            }
            if secondaries.len() >= 2 {
                let mut full: Vec<(&z3::ast::BV, &z3::ast::BV)> = vec![d1];
                for (sc, sn) in &secondaries {
                    full.push((sc, sn));
                }
                if certifies(&non_decrease(&full)) {
                    return true;
                }
            }
        }
        false
    })
}

/// Decide recoverability `AG EF (good)` of `btor2_content`, where `good` is a single
/// register-comparison atom string (`"state_q == 3"`).
///
/// Returns the canonical [`PropertyVerdict`]: `Holds` (every reachable state can
/// reach `good`), `Violated` (a reachable trap cannot), or `Unknown` (neither the
/// exact engine nor the cube + `smt-hyper-must` escalation could decide). Errors only
/// when `good` is not a parseable atom.
///
/// P2 Slice 1 — the exact engine is tried first; on a definite verdict it is returned
/// as before. When the exact engine abstains (over the ~40-bit cone cap or an
/// unsupported construct), the property is **escalated** to
/// [`verify_recoverability_scalable`] (the cube + `smt-hyper-must` reduction), so the
/// νμ recoverability property decides beyond 40 bits. The public signature is
/// unchanged; callers that want extra abstraction predicates use
/// [`verify_recoverability_with_predicates`].
pub fn verify_recoverability(btor2_content: &str, good: &str) -> Result<PropertyVerdict, String> {
    verify_recoverability_with_predicates(btor2_content, good, &[])
}

/// [`verify_recoverability`] with optional extra abstraction predicates for the
/// cube-path escalation. `extra_predicates` refine the predicate-cube abstraction (they
/// help the `smt-hyper-must` path decide) but do NOT appear in the `AG EF good` formula
/// — only `good` does. When `extra_predicates` is empty this is exactly
/// [`verify_recoverability`]. The exact engine (tried first) ignores the extras; they
/// matter only if it abstains and the cube path runs.
pub fn verify_recoverability_with_predicates(
    btor2_content: &str,
    good: &str,
    extra_predicates: &[PredicateSpec],
) -> Result<PropertyVerdict, String> {
    // Validate the atom up front for a target-specific error message (the µ-parser
    // would otherwise report it as a formula-syntax error).
    parse_predicate_expr(good).map_err(|e| {
        format!("recoverability target `{good}` is not a register-comparison atom (`REG op VALUE`): {e:?}")
    })?;

    // AG EF good = nu Y. ((mu X. (good || <> X)) && [] Y). The exact engine reads the
    // raw `REG == VALUE` atom directly.
    let formula_str = format!("nu Y. ((mu X. (({good}) || <> X)) && [] Y)");
    let formula = mu_parser::parse(&formula_str)
        .map_err(|e| format!("building the AG EF formula for `{good}`: {e:?}"))?;

    match exact_symbolic_verdict(btor2_content, &formula) {
        // The exact engine decided (definite at every alternation depth per
        // Bruns–Godefroid) — return it unchanged; the exact-only behaviour is
        // preserved when the exact engine gives a definite verdict.
        Ok(v) => Ok(PropertyVerdict::from(v)),
        // Over the ~40-bit cone cap or an unsupported construct: escalate to the
        // cube + `smt-hyper-must` path so the property still decides at scale
        // (slice-5b safety-⊥ escalation mirror). The scalable path itself abstains
        // (Unknown) if it cannot decide, so this is never less sound than the old
        // `Err(_) => Unknown` abstention.
        Err(_) => verify_recoverability_scalable(btor2_content, good, extra_predicates),
    }
}

/// P2 Slice 1 — decide recoverability `AG EF good` via the **predicate-cube +
/// `smt-hyper-must`** path, so the νμ property decides beyond the exact engine's
/// ~40-bit cone cap.
///
/// `good` is either a simple `REG == VALUE` equality atom OR a relational `REG == REG` atom (a
/// relational recoverability target — e.g. a FIFO's `AG EF (wptr == rptr)`, decided by the compound-good
/// machinery); any other comparison (`<`, `>=`, an addend form, …) returns `Ok(Unknown)` (an honest
/// abstain — the reset-cube construction is well-defined only for `==`). `extra_predicates` are
/// additional abstraction predicates that refine the cube (they do not enter the formula). Returns the
/// canonical verdict at the design's reset cube, or `Ok(Unknown)` when the abstraction cannot decide or
/// the CEGAR loop errors (a sound abstain — never a fabricated definite verdict). NOTE: a relation
/// reached by unbounded progress (a FIFO draining `wptr` up to `rptr` — independent counters) is the
/// ranking class and soundly abstains at scale; only invariant / bounded-must relations decide.
pub fn verify_recoverability_scalable(
    btor2_content: &str,
    good: &str,
    extra_predicates: &[PredicateSpec],
) -> Result<PropertyVerdict, String> {
    // Parse `good` and require a `REG == VALUE` equality atom.
    let good_expr = parse_predicate_expr(good).map_err(|e| {
        format!("recoverability target `{good}` is not a register-comparison atom (`REG op VALUE`): {e:?}")
    })?;
    // The good atom may be a simple `REG == VALUE` OR a relational `REG == REG`. The latter enables
    // relational recoverability targets — e.g. a FIFO's `AG EF (wptr == rptr)` ("always able to drain to
    // empty") — decided by the same relational compound-predicate machinery as frontier (b). Any other
    // comparison (`<`, `>=`, an addend form, …) is an honest abstain: the reset-cube construction below
    // is only well-defined for `==`.
    let (good_registers, good_simple): (Vec<String>, Option<(String, u64)>) = match good_expr {
        PredicateExpr::Cmp {
            register,
            op: CmpOp::Eq,
            value,
        } => (vec![register.clone()], Some((register, value))),
        PredicateExpr::CmpReg {
            ref lhs,
            op: CmpOp::Eq,
            ref rhs,
        } => (vec![lhs.clone(), rhs.clone()], None),
        _ => return Ok(PropertyVerdict::Unknown),
    };

    // Parse the design early — needed both for the reset valuation and for the auto-seed candidate
    // values below.
    let file = crate::adapter::btor2::parser::parse(btor2_content)
        .map_err(|e| format!("recoverability cube path: parsing BTOR2: {}", e.message))?;
    let init_values: BTreeMap<String, u128> =
        crate::adapter::btor2::concrete_oracle::init_valuation(&file);

    // N1 RANKING RESCUE: `AG EF good` may HOLD by a well-founded datapath descent (a down-counter to 0,
    // a timer, a deterministic drain) that the predicate cube cannot capture — the RANKING class. Prove
    // it directly over the exact transition BEFORE the cube; a sound sufficient condition (it establishes
    // the stronger `AG AF good`). If it fails, fall through to the cube (the certificate is sufficient,
    // not necessary).
    if ranking_certificate_holds(
        &file,
        &good_registers,
        good_simple.as_ref().map(|(_, v)| *v),
    ) {
        return Ok(PropertyVerdict::Holds);
    }

    // Seed predicates = [good] ++ [good register's OTHER control states] ++ extra. The `good`
    // predicate is named `good` so the formula atom resolves to it via the cube labelling; the
    // rest only refine the abstraction (they are not referenced by the formula).
    let mut specs: Vec<PredicateSpec> = Vec::with_capacity(1 + extra_predicates.len());
    // The `good` predicate (named `good` so the formula atom resolves to it). A SIMPLE target is a cube
    // dimension `register == value`; a RELATIONAL target is a COMPOUND predicate carried in
    // `good_compound` and added to `compound_seeds` below, so the reset cube evaluates the relation
    // (`lhs == rhs`) rather than a `register == value` atom.
    let good_compound: Option<(String, String, PredicateExpr)> = match &good_simple {
        Some((register, value)) => {
            specs.push(PredicateSpec {
                name: "good".to_string(),
                register: register.clone(),
                value: *value,
            });
            None
        }
        None => {
            let expr_str = format!("{} == {}", good_registers[0], good_registers[1]);
            let expr = parse_predicate_expr(&expr_str)
                .map_err(|e| format!("relational recoverability good `{expr_str}`: {e:?}"))?;
            Some(("good".to_string(), expr_str, expr))
        }
    };

    // N1 first increment — property-directed discovery (2026-07-11). Recoverability `EF good` must
    // DISTINGUISH the control states on the return path, not just `good` vs `!good`: a `good`-only
    // abstraction lumps every non-good control state into one cube whose must-successor self-loops,
    // so `EF good` is `⊥` (e.g. RESPONDER's `{st!=0}` lumps `req`/`grant` — it needs `st==1` and
    // `st==2`). Auto-seed `good_register == v` for each candidate value `v != good_value`, where the
    // candidate pool is `{0,1} ∪ the design's constant literals` (the FSM state encodings). This is
    // BOUNDED — `O(#constants)`, NOT `2^W` — so a wide `good` register does not blow up; capped for
    // safety. Directed by the `good` atom (the property), it recovers the control-return
    // recoverability class the coarse abstraction abstained on. Sound: adding predicates only
    // sharpens `⊥` toward a definite verdict (monotone refinement), never flips one.
    // Frontier (a): the constant pool is the good register's OWN cone constants (property-directed),
    // not every global constant — a design with many unrelated constants would otherwise seed
    // `good == decoy` predicates the good register can never take, inflating the cube. Fall back to the
    // global pool only when the cone is empty (good register absent / no `next`).
    const MAX_AUTO_SEED: usize = 8;
    for gr in &good_registers {
        let cone_constants = good_next_cone_constants(&file, gr);
        let const_pool = if cone_constants.is_empty() {
            crate::adapter::btor2::bit_blast::collect_btor2_constants(&file)
        } else {
            cone_constants
        };
        let mut candidate_values: Vec<u64> = vec![0, 1];
        for v in const_pool {
            if !candidate_values.contains(&v) {
                candidate_values.push(v);
            }
        }
        for v in candidate_values {
            // Cap the total dimension count at good + MAX_AUTO_SEED to bound the cube space.
            if specs.len() > MAX_AUTO_SEED {
                break;
            }
            // Skip the good value only for the SIMPLE good's OWN register (that value IS the good atom;
            // for a relational good every value is a legitimate discriminator).
            let is_good_val = good_simple
                .as_ref()
                .is_some_and(|(r, val)| r == gr && *val == v);
            if !is_good_val && !specs.iter().any(|s| s.register == *gr && s.value == v) {
                specs.push(PredicateSpec {
                    name: format!("state_{gr}_eq_{v}"),
                    register: gr.clone(),
                    value: v,
                });
            }
        }
    }

    // Class-2 (datapath-DEPENDENT return): seed the design's `register == constant` GUARD atoms — the
    // decision conditions the control branches on (e.g. `busy → done` only when `data == K`). When
    // the must-return reads a datapath predicate, control-state seeding alone leaves `EF good` at `⊥`;
    // the needed predicate is `data == K`, whose value K CANNOT be enumerated (`2^W`), but IS a design
    // literal read by a comparison node. Extracting those comparison atoms is the property-directed
    // discovery of the datapath predicate from the design's own guards (the shipped, eager form of the
    // lazy "discover from the ⊥ obligation" idea). Bounded by the number of comparison nodes; deduped
    // against what is already seeded; capped with the control seeding at MAX_AUTO_SEED.
    // N1 frontier (a) — property-directed guard discovery: restrict the guard-atom seeds to the good
    // register's cone-of-influence. A guard over a register the return path does not depend on (a decoy
    // comparison on an unrelated register) cannot change the verdict but still doubles the cube; without
    // this filter a design with many unrelated guards blows up the `2^{|P|}` all-pairs SMT. An empty
    // cone (good register absent / no `next`) means "no restriction" — prior behaviour preserved.
    let good_coi: std::collections::HashSet<String> = good_registers
        .iter()
        .flat_map(|gr| good_register_coi(&file, gr))
        .collect();
    let in_coi = |reg: &str| good_coi.is_empty() || good_coi.contains(reg);

    for (reg, k) in eq_guard_atoms(&file) {
        if specs.len() > MAX_AUTO_SEED {
            break;
        }
        if in_coi(&reg) && !specs.iter().any(|s| s.register == reg && s.value == k) {
            specs.push(PredicateSpec {
                name: format!("guard_{reg}_eq_{k}"),
                register: reg,
                value: k,
            });
        }
    }

    specs.extend(extra_predicates.iter().cloned());

    // Class-2 RELATIONAL return: seed the design's `register == register` guard atoms — the relational
    // decision conditions the control branches on (`busy → done` gated on `data == target`). These are
    // COMPOUND predicates (two registers, no literal), so they flow through the sidecar's
    // `compound_predicates`: the SMT hyper-must seam decides each cube's truth via the EXACT transition
    // (preserving the relational invariant across the wide datapath without concretising it), and the
    // reset cube below evaluates the relational expr rather than `register == value`. Bounded by the
    // number of comparison nodes; deduped implicitly (a `data == target` seed re-derives to the same
    // expr); capped WITH the other auto-seeds so the cube dimension count stays `≤ MAX_AUTO_SEED + 1`.
    // Built via `parse_predicate_expr` on the same string the sidecar carries, so the local expr used
    // for the reset-cube eval is byte-identical to the one the CEGAR lift parses.
    let mut compound_seeds: Vec<(String, String, PredicateExpr)> = Vec::new();
    // A RELATIONAL good target is the first compound predicate (named `good`; the formula references it).
    if let Some(gc) = good_compound {
        compound_seeds.push(gc);
    }
    for (lhs, rhs) in eq_reg_guard_atoms(&file) {
        if specs.len() + compound_seeds.len() > MAX_AUTO_SEED {
            break;
        }
        // Frontier (a): both operands must be in the good register's cone (a relational guard over
        // unrelated registers cannot change the verdict).
        if !(in_coi(&lhs) && in_coi(&rhs)) {
            continue;
        }
        let expr_str = format!("{lhs} == {rhs}");
        let Ok(expr) = parse_predicate_expr(&expr_str) else {
            continue;
        };
        compound_seeds.push((format!("rel_{lhs}_eq_{rhs}"), expr_str, expr));
    }

    // Frontier (b′) — arithmetic-relational returns `data == target + K` (the addend form the bare
    // relational extraction misses). Same compound-predicate path; the LOCAL expr carries the register
    // width so the reset-cube eval does the mod-2^width arithmetic the design's `+` wraps by (the sidecar
    // string parses to a width-agnostic BV form for the SMT seam, which is width-implicit).
    for (lhs, rhs, addend, width) in eq_reg_addend_guard_atoms(&file) {
        if specs.len() + compound_seeds.len() > MAX_AUTO_SEED {
            break;
        }
        if !(in_coi(&lhs) && in_coi(&rhs)) {
            continue;
        }
        let name = format!("rel_{lhs}_eq_{rhs}_plus_{addend}");
        if compound_seeds.iter().any(|(n, _, _)| n == &name) {
            continue;
        }
        let expr_str = format!("{lhs} == {rhs} + {addend}");
        let expr = PredicateExpr::CmpRegAddend {
            lhs: lhs.clone(),
            op: CmpOp::Eq,
            rhs: rhs.clone(),
            addend,
            width,
        };
        compound_seeds.push((name, expr_str, expr));
    }
    // name → expr, for the reset-cube evaluation + free-input guard of the compound predicates.
    let compound_map: std::collections::HashMap<String, PredicateExpr> = compound_seeds
        .iter()
        .map(|(name, _, expr)| (name.clone(), expr.clone()))
        .collect();

    // AG EF good, over the PREDICATE-NAME atom (the cube path resolves `good` to the
    // `good` predicate's 3-valued label, not a raw register comparison).
    let formula = mu_parser::parse("nu Y. ((mu X. (good || <> X)) && [] Y)")
        .map_err(|e| format!("building the AG EF cube formula: {e:?}"))?;

    // SOUNDNESS: pin each predicate's register to its BTOR2 reset value via
    // `config_values`, so the cube lift's initial cube is the design's reset state.
    // WITHOUT this pin the lift defaults its initial cube to all-false, which is NOT
    // the reset state and can falsely report VIOLATED — the pin is mandatory. A
    // predicate whose register has no `init_values` entry (e.g. a free input) is
    // silently skipped, which is correct (a free input has no reset value to pin).
    let mut config_entries: Vec<String> = specs
        .iter()
        .filter_map(|s| {
            init_values
                .get(&s.register)
                .map(|v| format!("{}={}", s.register, v))
        })
        .collect();
    // Pin the relational predicates' registers to their reset values too (both operands are states),
    // so the lift's initial cube is the design's reset state for the compound bits as well.
    for (_, _, expr) in &compound_seeds {
        for reg in expr.registers() {
            if let Some(v) = init_values.get(&reg) {
                let entry = format!("{reg}={v}");
                if !config_entries.contains(&entry) {
                    config_entries.push(entry);
                }
            }
        }
    }
    let base_sidecar_json = config_values_to_sidecar_json(&config_entries)
        .map_err(|e| format!("recoverability cube path: building the config-values pin: {e}"))?;
    // Inject the relational atoms as sidecar `compound_predicates` (the CEGAR loop reads them via
    // `sidecar_compound_predicates`, adds each as a cube dimension, and forces the compound-aware
    // SmtAllPairs+Eager lift). A `None` base (no config entries) still needs the compound array.
    let sidecar_json = if compound_seeds.is_empty() {
        base_sidecar_json
    } else {
        let mut val: serde_json::Value = match &base_sidecar_json {
            Some(s) => serde_json::from_str(s).map_err(|e| {
                format!("recoverability cube path: re-parsing the config-values sidecar: {e}")
            })?,
            None => serde_json::json!({ "module": "cegar", "source": "cegar.btor2" }),
        };
        let decls: Vec<serde_json::Value> = compound_seeds
            .iter()
            .map(|(name, expr, _)| {
                serde_json::json!({ "name": name, "expr": expr, "derived": false })
            })
            .collect();
        val["compound_predicates"] = serde_json::Value::Array(decls);
        Some(val.to_string())
    };
    let adapter_options = AdapterOptions {
        sidecar_json,
        ..Default::default()
    };

    // PERFORMANCE guard (2026-07-11, no longer a soundness band-aid): abstain on a UF-wrapping design
    // ONLY when the CALLER supplied extra predicates. Rationale:
    //   - Soundness is handled by the universal-hyper-must ◇ (PR #302): an inflated (havoc'd) target
    //     set only pushes ◇ toward `⊥`, never fabricates a `Holds`. So running the wrapped cube is
    //     always SOUND. This guard is purely about cost.
    //   - The AUTO path seeds only the good register's CONTROL states (above). Those predicates are
    //     over the controller, not the wide-op OUTPUT register, so the wide op stays on the may side
    //     (UF-wrapped) and never enters the cube successor / must query — the cube is small and FAST.
    //     Empirically it now DECIDES the wide-datapath control-return class (pos48 `Holds` ~1.4s,
    //     mult48 — a 48-bit MULTIPLIER — `Holds` ~0.12s; trap48 soundly `⊥`), where the exact BDD
    //     walls on the multiplier. This is the N1-first-increment payoff, so we RUN it.
    //   - A caller-supplied extra predicate MIGHT be over the wide-op output register (e.g.
    //     `data == 0`), which forces the wide op into the cube successor computation → the
    //     `O(2^{2|P|})` all-pairs SMT over wide arithmetic can be slow. Rather than risk a hang we
    //     ABSTAIN in that case (conservative; sound). A precise "abstain iff an extra predicate is in
    //     a wrapped-op's cone" is a follow-up; today "caller passed extras on a wrapped design" is the
    //     safe proxy.
    if !extra_predicates.is_empty()
        && !crate::adapter::btor2::bit_blast::collect_uf_wrapped_nids(&file, &adapter_options)
            .is_empty()
    {
        return Ok(PropertyVerdict::Unknown);
    }

    // Cube + smt-hyper-must, matching the verify_auto CegarOptions shape.
    let cegar_opts = CegarOptions {
        max_iterations: RECOVERABILITY_MAX_ITERATIONS,
        predicate_source: PredicateSource::WeakestPrecondition,
        max_cube_count: 1024,
        capture_approximants: false,
        enable_approximant_reuse: false,
        smart_uf_cap: true,
        lift_strategy: LiftStrategy::Eager,
        // The sound νμ hyper-must (GKMTS ∀∃ over the may-successor set) — definite
        // recoverability verdicts transfer to the concrete design (Bruns–Godefroid /
        // Shoham–Grumberg).
        must_edge_inference: MustEdgeInference::SmtHyperMust,
        // The sound all-pairs SMT may-relation (over-approximation).
        may_edge_inference: MayEdgeInference::SmtAllPairs,
        emit_ctxdsl: false,
    };
    // The cube space includes the compound (relational) predicates the CEGAR loop adds as dimensions.
    let env = Environment::new(1usize << (specs.len() + compound_seeds.len()));

    let trace = match cegar_refine_loop(
        &formula,
        btor2_content,
        specs.clone(),
        &env,
        &adapter_options,
        &cegar_opts,
    ) {
        Ok(t) => t,
        // SOUNDNESS: a CEGAR error (SMT failure, cube-cap overflow, …) is an honest
        // abstain — never a fabricated verdict, matching the exact verb's
        // abstain-on-error posture.
        Err(_) => return Ok(PropertyVerdict::Unknown),
    };

    // SOUNDNESS: the reset cube is well-defined only if EVERY final predicate's register
    // has a known reset value (is a pinned state cell). WeakestPrecondition refinement can
    // append a predicate over a free INPUT — e.g. `WP(st==0)` through `st' = ite(go,1,0)`
    // is `go==0` — whose reset truth is not fixed (the input is free at cycle 0). Reading a
    // single init-cube bit for such a predicate would silently pick one input flavour and
    // could return a wrong DEFINITE verdict, and at scale the exact engine abstains so
    // there is no cross-check to catch it. Rather than under-read (unsound) we ABSTAIN
    // (sound). Fully enumerating free-input initial flavours conjunctively (à la
    // verify_auto's `free_input_init_cubes`) is a completeness follow-up.
    if !trace.final_predicates.iter().all(|spec| {
        // A relational (compound) predicate is well-defined at reset iff EVERY register it references
        // has a reset value; a simple atom needs only its one register pinned.
        match compound_map.get(&spec.name) {
            Some(expr) => expr.registers().iter().all(|r| init_values.contains_key(r)),
            None => init_values.contains_key(&spec.register),
        }
    }) {
        return Ok(PropertyVerdict::Unknown);
    }

    // The design's initial cube: evaluate every FINAL predicate at the reset valuation,
    // in the lift's cube-bit order (`final_predicates[i]` ↔ bit `i`). Every final predicate
    // is now a pinned `register == value` state atom (guarded above), so its reset truth is
    // `init_values[register] == value` and the reset cube is input-independent.
    // A relational (compound) predicate's reset truth is the EXPR evaluated at the reset valuation
    // (e.g. `data == target`), NOT `register == value` — its `spec.register`/`spec.value` are only
    // placeholders. `eval` wants a `HashMap`, so build one view of the reset valuation.
    let init_map: std::collections::HashMap<String, u128> =
        init_values.iter().map(|(k, v)| (k.clone(), *v)).collect();
    let mut init_cube = 0usize;
    for (i, spec) in trace.final_predicates.iter().enumerate() {
        let holds = match compound_map.get(&spec.name) {
            Some(expr) => expr.eval(&init_map),
            None => init_values.get(&spec.register).copied() == Some(spec.value as u128),
        };
        if holds {
            init_cube |= 1 << i;
        }
    }

    Ok(match trace.final_verdict.verdict_at(init_cube) {
        Trit::False => PropertyVerdict::Violated,
        Trit::Unknown => PropertyVerdict::Unknown,
        Trit::True => PropertyVerdict::Holds,
    })
}

/// Parse an extra-abstraction-predicate triple `NAME:REGISTER=VALUE` (the surface
/// syntax shared by `btor2 verify-recoverability --predicate` and the API
/// `predicates` field) into a [`PredicateSpec`]. Same shape as `btor2 cegar`.
pub fn parse_extra_predicate(raw: &str) -> Result<PredicateSpec, String> {
    let (name, rest) = raw.split_once(':').ok_or_else(|| {
        format!("predicate spec '{raw}' missing ':' separator (expected NAME:REGISTER=VALUE)")
    })?;
    let (register, value_str) = rest.split_once('=').ok_or_else(|| {
        format!("predicate spec '{raw}' missing '=' separator (expected NAME:REGISTER=VALUE)")
    })?;
    let value: u64 = value_str
        .parse()
        .map_err(|e| format!("predicate spec '{raw}' has non-numeric value: {e}"))?;
    Ok(PredicateSpec {
        name: name.to_string(),
        register: register.to_string(),
        value,
    })
}

/// The `AG EF good` formula string this command decides, for provenance / echoing
/// on a surface (`AG EF (<good>)`).
pub fn recoverability_property_str(good: &str) -> String {
    format!("AG EF ({good})")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 3-state responder: st 0=idle, 1=req, 2=grant; idle -go-> req; req -> grant;
    // grant -> idle. Every reachable state can reach idle ⇒ AG EF (st==0) HOLDS.
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

    // 4-state staller: st 0=idle, 1=req, 3=stuck (absorbing); 2=grant unreachable.
    // idle -go-> req; req -> stuck; stuck -> stuck. The reachable `stuck` cannot get
    // back to idle ⇒ AG EF (st==0) VIOLATED.
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

    #[test]
    fn recoverable_design_holds() {
        assert_eq!(
            verify_recoverability(RESPONDER, "st == 0").expect("decides"),
            PropertyVerdict::Holds
        );
    }

    #[test]
    fn design_with_absorbing_trap_is_violated() {
        assert_eq!(
            verify_recoverability(STALLER, "st == 0").expect("decides"),
            PropertyVerdict::Violated
        );
    }

    #[test]
    fn malformed_target_errors() {
        assert!(verify_recoverability(RESPONDER, "not an atom !!").is_err());
    }

    #[test]
    fn property_string_echoes_the_target() {
        assert_eq!(recoverability_property_str("st == 0"), "AG EF (st == 0)");
    }

    // A ≥48-bit free-running counter (over the exact ~40-bit cone cap) whose value
    // gates a small 2-state FSM (`st`: 0=idle, 1=busy). The counter feeds st's
    // next-state (idle advances to busy only when `cnt == 0`), so the counter is IN
    // st's cone-of-influence → the exact engine over-caps. busy ALWAYS returns to idle,
    // so every reachable state can reach idle ⇒ AG EF (st==0) HOLDS.
    const WIDE_RECOVERABLE: &str = "\
1 sort bitvec 48
2 sort bitvec 2
3 sort bitvec 1
4 state 1 cnt
5 zero 1
6 init 1 4 5
7 inc 1 4
8 next 1 4 7
9 state 2 st
10 zero 2
11 init 2 9 10
12 one 2
13 eq 3 9 10
14 eq 3 4 5
15 ite 2 14 12 10
16 ite 2 13 15 10
17 next 2 9 16
";

    // Same 48-bit counter (over the exact cap) but st's next-state is `stuck (2)`
    // UNCONDITIONALLY — the `ite(cnt==0, 2, 2)` keeps the counter syntactically in st's
    // cone (so the exact engine over-caps) while every state moves to the absorbing
    // trap. From `stuck`, idle is unreachable ⇒ AG EF (st==0) VIOLATED.
    const WIDE_TRAP: &str = "\
1 sort bitvec 48
2 sort bitvec 2
3 sort bitvec 1
4 state 1 cnt
5 zero 1
6 init 1 4 5
7 inc 1 4
8 next 1 4 7
9 state 2 st
10 zero 2
11 init 2 9 10
12 constd 2 2
13 eq 3 4 5
14 ite 2 13 12 12
15 next 2 9 14
";

    /// The **exact-engine-only** verdict (no cube escalation) — the differential
    /// oracle. `Unknown` means the exact engine abstained (over-cap / unsupported).
    fn exact_verdict(btor2: &str, good: &str) -> PropertyVerdict {
        let formula_str = format!("nu Y. ((mu X. (({good}) || <> X)) && [] Y)");
        let formula = mu_parser::parse(&formula_str).expect("AG EF formula parses");
        match exact_symbolic_verdict(btor2, &formula) {
            Ok(v) => PropertyVerdict::from(v),
            Err(_) => PropertyVerdict::Unknown,
        }
    }

    // === P2 Slice 1 — the MANDATORY differential soundness gate ==================
    // The scalable cube + smt-hyper-must verdict MUST equal the exact-engine verdict
    // on the small fixtures the exact engine decides, in BOTH polarities. This is the
    // non-negotiable soundness assertion (the L5 differential-oracle rule).

    #[test]
    fn scalable_matches_exact_holds_polarity() {
        // Differential SOUNDNESS gate: the cube path agrees with the exact engine on RESPONDER
        // (both `Holds`). The N1-first-increment property-directed seeding (auto-seeding the good
        // register's other control states — `st==1`, `st==2`) splits the coarse `good`-vs-`!good`
        // abstraction so cube 1's must-self-loop resolves and `EF idle` is provably `Holds`. Before
        // the increment this abstained to `Unknown` (sound but imprecise). The gate stays soundness
        // (`agree or abstain, never contradict`) but is now met by AGREEMENT.
        let exact = exact_verdict(RESPONDER, "st == 0");
        let scalable = verify_recoverability_scalable(RESPONDER, "st == 0", &[]).expect("decides");
        assert_eq!(
            exact,
            PropertyVerdict::Holds,
            "exact decides RESPONDER Holds"
        );
        assert_eq!(
            scalable, exact,
            "cube path (with property-directed seeding) must DECIDE RESPONDER Holds, agreeing with exact"
        );
    }

    #[test]
    fn scalable_matches_exact_violated_polarity() {
        // STALLER: the reachable `stuck` state cannot get back to idle ⇒ Violated,
        // both engines.
        let exact = exact_verdict(STALLER, "st == 0");
        let scalable = verify_recoverability_scalable(STALLER, "st == 0", &[]).expect("decides");
        assert_eq!(
            exact,
            PropertyVerdict::Violated,
            "exact decides STALLER Violated"
        );
        assert_eq!(
            scalable, exact,
            "cube path must AGREE with the exact engine on STALLER (Violated)"
        );
    }

    // === The proof the slice decides AT SCALE ==================================
    // On a design whose cone exceeds the exact ~40-bit cap, the exact engine ABSTAINS
    // (Unknown) while the cube path DECIDES — in both polarities.

    #[test]
    fn wide_design_exact_abstains_but_cube_decides_holds() {
        // The 48-bit counter feeds st's cone ⇒ the exact engine over-caps (Unknown),
        // but the cube abstraction (which drops the counter) decides Holds.
        assert_eq!(
            exact_verdict(WIDE_RECOVERABLE, "st == 0"),
            PropertyVerdict::Unknown,
            "the exact engine must ABSTAIN on the wide (over-cap) design"
        );
        assert_eq!(
            verify_recoverability_scalable(WIDE_RECOVERABLE, "st == 0", &[]).expect("decides"),
            PropertyVerdict::Holds,
            "the cube path must DECIDE Holds where the exact engine abstains"
        );
    }

    #[test]
    fn wide_design_exact_abstains_but_cube_decides_violated() {
        assert_eq!(
            exact_verdict(WIDE_TRAP, "st == 0"),
            PropertyVerdict::Unknown,
            "the exact engine must ABSTAIN on the wide (over-cap) trap design"
        );
        assert_eq!(
            verify_recoverability_scalable(WIDE_TRAP, "st == 0", &[]).expect("decides"),
            PropertyVerdict::Violated,
            "the cube path must DECIDE Violated where the exact engine abstains"
        );
    }

    // === SOUNDNESS regression (2026-07-11): UF-wrap must NOT manufacture a spurious `Holds` =====
    // `data' = data + 2`, `data` init 1 ⇒ `data` stays ODD ⇒ `data==0` is unreachable ⇒ the
    // `busy --(data==0)--> done` escape never fires ⇒ `busy` is an absorbing trap ⇒ `AG EF idle`
    // is VIOLATED. At width 8 (cone under the exact cap) the exact engine proves it. At width 48
    // the wide `add` (> UF_WIDE_ADD_SUB_THRESHOLD = 32) is UF-wrapped; the pre-fix cube path
    // reported an unsound `Holds` (may-havoc manufactured the `data==0` escape). The fix ABSTAINS.
    const TRAP_UF_W8: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 8
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
8 zero 3
13 one 3
30 constd 3 2
9 init 3 7 13
10 input 2 start
11 one 1
12 constd 1 2
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 8
17 ite 1 10 11 5
25 ite 1 23 12 11
18 ite 1 15 25 5
19 ite 1 14 17 18
20 next 1 4 19
31 add 3 7 30
22 next 3 7 31
";
    const TRAP_UF_W48: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
8 zero 3
13 one 3
30 constd 3 2
9 init 3 7 13
10 input 2 start
11 one 1
12 constd 1 2
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 8
17 ite 1 10 11 5
25 ite 1 23 12 11
18 ite 1 15 25 5
19 ite 1 14 17 18
20 next 1 4 19
31 add 3 7 30
22 next 3 7 31
";

    // Wide (48-bit) datapath + control-return recoverability. `ctrl` idle(0)→busy(1)→done(2)→idle,
    // return is DATA-INDEPENDENT ⇒ `AG EF (ctrl==0)` HOLDS regardless of `data`. `data` is in `ctrl`'s
    // cone (idle→busy gated on `data==0`) so the exact engine over-caps; the wide op UF-wraps.
    const POS_W48: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
8 zero 3
9 init 3 7 8
10 input 2 start
11 one 1
12 constd 1 2
13 one 3
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 8
24 and 2 10 23
17 ite 1 24 11 5
18 ite 1 15 12 5
19 ite 1 14 17 18
20 next 1 4 19
21 add 3 7 13
22 next 3 7 21
";
    // Same control FSM, but `data' = data * data` — a 48-bit MULTIPLIER the exact BDD cannot build.
    const MULT_W48: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
8 zero 3
9 init 3 7 8
10 input 2 start
11 one 1
12 constd 1 2
13 one 3
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 8
24 and 2 10 23
17 ite 1 24 11 5
18 ite 1 15 12 5
19 ite 1 14 17 18
20 next 1 4 19
26 mul 3 7 7
22 next 3 7 26
";

    #[test]
    fn uf_wrap_recoverability_sound_and_decides_control_return() {
        // Ground truth (exact, width-8 cone under the cap): the odd-counter trap is VIOLATED.
        assert_eq!(
            exact_verdict(TRAP_UF_W8, "ctrl == 0"),
            PropertyVerdict::Violated,
            "exact engine (under cap) must prove the odd-counter trap VIOLATED"
        );

        // SOUNDNESS (PR #302 universal-◇): the wrapped trap is NEVER a spurious `Holds`. The auto
        // path now RUNS the wrapped cube (guard relaxed) and lands on a sound `⊥` — the coarse
        // abstraction can't prove the trap — but the universal ◇ forbids fabricating `Holds`.
        let trap = verify_recoverability_scalable(TRAP_UF_W48, "ctrl == 0", &[]).expect("decides");
        assert_ne!(
            trap,
            PropertyVerdict::Holds,
            "the wrapped trap must NEVER be a spurious Holds (got {trap:?})"
        );

        // PERF GUARD: a CALLER-supplied predicate on a wrapped design abstains (it may force the wide
        // op into the cube successor → the all-pairs-SMT cost). Empty extras (the auto path) runs.
        let extra = vec![parse_extra_predicate("dz:data=0").expect("parse")];
        assert_eq!(
            verify_recoverability_scalable(TRAP_UF_W48, "ctrl == 0", &extra).expect("abstains"),
            PropertyVerdict::Unknown,
            "a caller-supplied predicate on a wrapped design abstains (perf guard)"
        );

        // SCALE WIN — the N1 first increment (property-directed control-state seeding) DECIDES the
        // wide-datapath control-return class the exact BDD walls on. `data` is may-side (UF-wrapped,
        // sound over-approx); the seeded control predicates + exact-transition must decide the
        // datapath-independent return. Both `Holds`, including a 48-bit MULTIPLIER (`data*data`) whose
        // relation the exact BDD cannot even build.
        assert_eq!(
            verify_recoverability_scalable(POS_W48, "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Holds,
            "wide-add control-return recoverability decides Holds"
        );
        assert_eq!(
            verify_recoverability_scalable(MULT_W48, "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Holds,
            "48-bit MULTIPLIER control-return recoverability decides Holds (exact BDD walls)"
        );
    }

    // Class-2: datapath-DEPENDENT return. `busy → idle` only when `data == 7` (K=7, a 48-bit design
    // literal); `data == 7` is invariant (data' = data, init 7). `AG EF (ctrl==0)` HOLDS but needs the
    // `data == 7` predicate: control-state seeding alone (good register only) leaves it `⊥`, and K=7
    // cannot be enumerated (2^48 values). The guard-atom extraction reads `data == 7` off the design's
    // own comparison node and decides it.
    const CLASS2_DATADEP: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
30 constd 3 7
9 init 3 7 30
10 input 2 start
11 one 1
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 30
17 ite 1 10 11 5
25 ite 1 23 5 11
18 ite 1 15 25 5
19 ite 1 14 17 18
20 next 1 4 19
22 next 3 7 7
";

    #[test]
    fn class2_datapath_dependent_return_decides_via_guard_atoms() {
        // Class-2 discovery: the return reads a datapath predicate (`data == 7`) that control-state
        // seeding does not cover. The guard-atom extraction discovers it from the design's `eq`
        // comparison node (K=7, a 48-bit literal, unenumerable over `2^W`), and the datapath-dependent
        // recoverability decides `Holds`. (Diagnostic: `data==0`/`data==1` — what value-enumeration
        // would propose — leave it `⊥`; only `data==7`, the guard atom, decides.)
        assert_eq!(
            verify_recoverability_scalable(CLASS2_DATADEP, "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Holds,
            "datapath-dependent-return recoverability decides Holds via guard-atom discovery"
        );
    }

    // Class-2 RELATIONAL return. `busy → idle` (`ctrl' = ctrl & !(data==target)`) fires iff the two
    // 48-bit registers are equal; both init 0 and increment by 1, so `data == target` is INVARIANT →
    // `AG EF (ctrl==0)` HOLDS. But no `register == constant` predicate captures it: the guard compares
    // two STATE registers, and the constant K is not enumerable. `eq_reg_guard_atoms` reads the
    // relational atom `data == target` off the design's `eq` node; the exact hyper-must edge preserves
    // the invariant across the wide (UF-wrapped-on-the-may-side) `+1` increments WITHOUT concretising
    // the values, so the return decides at scale where the exact BDD walls.
    fn relational_w(width: u32, target_init_nid: &str) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 1 ctrl\n4 state 2 data\n\
             5 state 2 target\n6 zero 2\n7 one 2\n9 one 1\n10 init 1 3 9\n11 init 2 4 6\n\
             12 init 2 5 {target_init_nid}\n13 add 2 4 7\n14 add 2 5 7\n15 eq 1 4 5\n16 not 1 15\n\
             17 and 1 3 16\n18 next 1 3 17\n19 next 2 4 13\n20 next 2 5 14\n"
        )
    }

    #[test]
    fn class2_relational_return_decides_via_reg_eq_reg_atoms() {
        // POSITIVE (target init 0 == data init 0): `data == target` invariant → recoverable.
        let pos = relational_w(48, "6"); // nid 6 = zero
        assert_eq!(
            verify_recoverability_scalable(&pos, "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Holds,
            "48-bit RELATIONAL-return recoverability decides Holds via reg==reg atom discovery"
        );
        // NEGATIVE (target init 1 != data init 0): `data == target` NEVER holds → `busy` is a trap →
        // NOT recoverable. The relational predicate soundly reports Violated (not a spurious Holds).
        let neg = relational_w(48, "7"); // nid 7 = one
        assert_eq!(
            verify_recoverability_scalable(&neg, "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Violated,
            "the never-equal relational trap decides Violated"
        );

        // DIFFERENTIAL ORACLE: the same designs at 8-bit route through the EXACT BDD engine (below the
        // 40-bit cap) via the public verb. The narrow exact verdict must equal the wide cube verdict —
        // zero mismatches is the soundness gate for the relational seeding.
        assert_eq!(
            verify_recoverability(&relational_w(8, "6"), "ctrl == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle must agree with the 48-bit cube Holds"
        );
        assert_eq!(
            verify_recoverability(&relational_w(8, "7"), "ctrl == 0").expect("exact decides"),
            PropertyVerdict::Violated,
            "8-bit exact oracle must agree with the 48-bit cube Violated"
        );
    }

    // Frontier (b′) — arithmetic-relational return `data == target + 2` (`ctrl' = ctrl & !(data==target+2)`).
    // Both registers increment, so `data == target + 2` is INVARIANT when `data` inits `target + 2`. The
    // addend form is neither a literal K nor a bare `reg == reg` guard, so the earlier extractions miss
    // it; `eq_reg_addend_guard_atoms` reads it off `eq(data, add(target, 2))` and seeds the
    // `CmpRegAddend`, decided at scale by the exact must edge preserving the invariant across the wide
    // increments (mod-2^width).
    fn arith_rel_w(width: u32, data_init: u64) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 1 ctrl\n4 state 2 data\n\
             5 state 2 target\n6 zero 1\n7 one 1\n8 one 2\n9 constd 2 2\n10 constd 2 {data_init}\n\
             11 constd 2 3\n12 init 1 3 7\n13 init 2 4 10\n14 init 2 5 11\n15 add 2 4 8\n\
             16 add 2 5 8\n17 add 2 5 9\n18 eq 1 4 17\n19 not 1 18\n20 and 1 3 19\n21 next 1 3 20\n\
             22 next 2 4 15\n23 next 2 5 16\n"
        )
    }

    #[test]
    fn class2_frontier_bprime_arith_relational_return_decides_via_addend_atoms() {
        // POSITIVE (data init 5 == target(3)+2): `data == target + 2` invariant → recoverable.
        assert_eq!(
            verify_recoverability_scalable(&arith_rel_w(48, 5), "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Holds,
            "48-bit arithmetic-relational return decides Holds via addend-atom discovery"
        );
        // NEGATIVE (data init 2 != target(3)+2): the relation never holds → busy is a trap → Violated.
        assert_eq!(
            verify_recoverability_scalable(&arith_rel_w(48, 2), "ctrl == 0", &[]).expect("decides"),
            PropertyVerdict::Violated,
            "the never-equal arithmetic-relational trap decides Violated"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD) — must agree with the wide cube verdicts.
        assert_eq!(
            verify_recoverability(&arith_rel_w(8, 5), "ctrl == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle must agree with the 48-bit cube Holds"
        );
        assert_eq!(
            verify_recoverability(&arith_rel_w(8, 2), "ctrl == 0").expect("exact decides"),
            PropertyVerdict::Violated,
            "8-bit exact oracle must agree with the 48-bit cube Violated"
        );
    }

    // Frontier (a) — COI-directed seeding. A 2-state ctrl FSM whose return is gated on `data == 42`
    // (data'=data → invariant when `data` inits 42), PLUS a decoy register `mode` compared against
    // constants 2..9 in dead logic (`mode` is NOT in ctrl's cone-of-influence). The UN-directed eager
    // extraction seeds `good == v` for every global constant (2..9, 42) and every `eq` atom
    // (`mode==2..9`), inflating the cube to a >30s all-pairs-SMT hang. COI-directed seeding restricts
    // both the control-state constant pool and the guard atoms to ctrl's cone → seeds only `data == 42`
    // (+ good, + ctrl==1) → decides fast. `AG EF (ctrl==0)` HOLDS.
    fn manyguard_btor2(width: u32, data_init: u64) -> String {
        let mut l: Vec<String> = vec![
            "1 sort bitvec 4".into(),
            "2 sort bitvec 1".into(),
            format!("3 sort bitvec {width}"),
            "4 state 1 ctrl".into(),
            "5 state 3 data".into(),
            "6 state 1 mode".into(),
            "10 zero 1".into(),
            "11 one 1".into(),
            "12 constd 3 42".into(),            // K
            format!("13 constd 3 {data_init}"), // data reset value
        ];
        let mut dc: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        let mut nid = 20i64;
        for k in 2..10 {
            l.push(format!("{nid} constd 1 {k}"));
            dc.insert(k, nid);
            nid += 1;
        }
        l.push("14 init 1 4 11".into()); // ctrl = busy(1)
        l.push("15 init 3 5 13".into()); // data = data_init
        l.push(format!("16 init 1 6 {}", dc[&2])); // mode = 2
        // return: busy && data==42
        l.push(format!("{nid} eq 2 4 11"));
        let busy = nid;
        nid += 1;
        l.push(format!("{nid} eq 2 5 12"));
        let dk = nid;
        nid += 1;
        l.push(format!("{nid} and 2 {busy} {dk}"));
        let ret = nid;
        nid += 1;
        l.push(format!("{nid} ite 1 {ret} 10 4")); // busy&&data==42 -> idle(0) else stay
        let cn = nid;
        nid += 1;
        l.push(format!("{nid} next 1 4 {cn}"));
        nid += 1;
        l.push(format!("{nid} next 3 5 5")); // data' = data
        nid += 1;
        // decoy `mode` ring over eq(mode, 2..9) — dead logic, not in ctrl's cone
        let mut chain = 6i64;
        for k in 2..10 {
            let nxt = *dc.get(&(k + 1)).unwrap_or(&dc[&2]);
            l.push(format!("{nid} eq 2 6 {}", dc[&k]));
            let eqk = nid;
            nid += 1;
            l.push(format!("{nid} ite 1 {eqk} {nxt} {chain}"));
            chain = nid;
            nid += 1;
        }
        l.push(format!("{nid} next 1 6 {chain}"));
        l.join("\n")
    }

    #[test]
    fn class2_frontier_a_coi_directed_seeding_scales_past_decoy_guards() {
        // POSITIVE (data init 42): decides Holds fast — COI-direction drops the decoy `mode` guards and
        // the decoy control constants, so |P| stays small where the un-directed path blows up (>30s).
        assert_eq!(
            verify_recoverability_scalable(&manyguard_btor2(48, 42), "ctrl == 0", &[])
                .expect("decides"),
            PropertyVerdict::Holds,
            "COI-directed seeding decides the many-decoy-guard design (un-directed blows up)"
        );
        // NEGATIVE (data init 7 != 42): `data == 42` never holds → busy is a trap → not recoverable.
        assert_eq!(
            verify_recoverability_scalable(&manyguard_btor2(48, 7), "ctrl == 0", &[])
                .expect("decides"),
            PropertyVerdict::Violated,
            "the never-return many-guard variant decides Violated"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD) — must agree with the wide cube verdicts.
        assert_eq!(
            verify_recoverability(&manyguard_btor2(8, 42), "ctrl == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle must agree with the 48-bit cube Holds"
        );
        assert_eq!(
            verify_recoverability(&manyguard_btor2(8, 7), "ctrl == 0").expect("exact decides"),
            PropertyVerdict::Violated,
            "8-bit exact oracle must agree with the 48-bit cube Violated"
        );
    }

    // Relational recoverability TARGET — `AG EF (data == target)` where the good atom itself is a
    // register-vs-register relation, not `REG == VALUE`. data,target both init 0 and increment, so the
    // relation is INVARIANT and the property Holds; the target flows through the compound-good machinery
    // (the reset cube evaluates the relation). Decides at 48-bit where the exact engine walls. (Contrast:
    // a FIFO's `AG EF (wptr == rptr)` reaches the relation by DRAINING — independent counters — which is
    // the ranking class and correctly abstains at scale; this test covers the invariant case the cube
    // decides.)
    fn inv_rel_w(width: u32, target_init_nid: &str) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n4 state 2 data\n5 state 2 target\n6 zero 2\n\
             7 one 2\n13 init 2 4 6\n14 init 2 5 {target_init_nid}\n15 add 2 4 7\n16 add 2 5 7\n\
             22 next 2 4 15\n23 next 2 5 16\n"
        )
    }

    #[test]
    fn relational_recoverability_target_decides_at_scale() {
        // POSITIVE (target inits 0 == data): `data == target` invariant → Holds at 48-bit via the
        // relational target, where exact walls.
        assert_eq!(
            verify_recoverability(&inv_rel_w(48, "6"), "data == target").expect("decides"),
            PropertyVerdict::Holds,
            "48-bit invariant-relational recoverability target decides Holds"
        );
        // NEGATIVE (target inits 1 != data): never equal → Violated.
        assert_eq!(
            verify_recoverability(&inv_rel_w(48, "7"), "data == target").expect("decides"),
            PropertyVerdict::Violated,
            "the never-equal relational target decides Violated"
        );
        // DIFFERENTIAL ORACLE at 8-bit — must agree with the 48-bit verdicts.
        assert_eq!(
            verify_recoverability(&inv_rel_w(8, "6"), "data == target").expect("small decides"),
            PropertyVerdict::Holds
        );
        assert_eq!(
            verify_recoverability(&inv_rel_w(8, "7"), "data == target").expect("small decides"),
            PropertyVerdict::Violated
        );
    }

    // Ranking certificate — the well-founded-descent / RANKING class. A down-counter to 0: `cnt`
    // decrements (`cnt' = ite(cnt==0, 0, cnt - step)`), so `AG EF (cnt == 0)` HOLDS by a well-founded
    // descent, but no bounded predicate set captures the 2^W-step descent → the cube abstains. The
    // ranking certificate proves it over the exact transition (δ = cnt strictly decreases, bounded below).
    fn downcnt_w(width: u32, init: u128, step: u64) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 2 cnt\n4 zero 2\n5 constd 2 {step}\n\
             6 constd 2 {init}\n7 init 2 3 6\n8 eq 1 3 4\n9 sub 2 3 5\n10 ite 2 8 4 9\n11 next 2 3 10\n"
        )
    }

    #[test]
    fn ranking_certificate_decides_downcounter_at_scale() {
        // 48-bit down-counter to 0 (step 1): the cube abstains (ranking), the certificate decides Holds.
        assert_eq!(
            verify_recoverability_scalable(&downcnt_w(48, 140_737_488_355_328, 1), "cnt == 0", &[])
                .expect("decides"),
            PropertyVerdict::Holds,
            "48-bit down-counter recoverability decides Holds via the ranking certificate"
        );
        // OVERSHOOT (step 2 from an ODD start): cnt stays odd, wraps past 0, never == 0 → the property
        // does NOT hold. The certificate correctly FAILS (the BV difference jumps up at the wrap), so it
        // soundly abstains rather than fabricate Holds.
        assert_ne!(
            verify_recoverability_scalable(&downcnt_w(48, 140_737_488_355_327, 2), "cnt == 0", &[])
                .expect("decides"),
            PropertyVerdict::Holds,
            "the overshoot counter must NOT be a spurious Holds"
        );
        // DIFFERENTIAL ORACLE: the 8-bit down-counter routes through the exact BDD engine and agrees.
        assert_eq!(
            verify_recoverability(&downcnt_w(8, 200, 1), "cnt == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle agrees with the 48-bit ranking Holds"
        );
    }

    // ∃-input ranking — a SOME-path descent. `cnt` decrements only when a free input `dec` is 1
    // (`cnt' = ite(cnt==0, 0, ite(dec, cnt-1, cnt))`), so `AG EF (cnt == 0)` HOLDS (the `dec=1` path
    // drains) but NOT `AG AF` (`dec=0` forever holds). The all-path certificate (A) fails; the ∃-input
    // certificate (B) decides it — the shape of a FIFO drained by only some input (read, not write).
    fn nondet_drain_w(width: u32, init: u128) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 2 cnt\n4 zero 2\n5 one 2\n\
             6 constd 2 {init}\n7 init 2 3 6\n8 input 1 dec\n9 eq 1 3 4\n10 sub 2 3 5\n11 ite 2 8 10 3\n\
             12 ite 2 9 4 11\n13 next 2 3 12\n"
        )
    }

    #[test]
    fn ranking_certificate_exists_input_decides_nondeterministic_drain() {
        // 48-bit nondeterministic drain: AG EF (cnt==0) decides Holds via the ∃-input certificate (some
        // input path drains) where the all-path certificate fails (dec=0 holds cnt) and the cube abstains.
        assert_eq!(
            verify_recoverability_scalable(
                &nondet_drain_w(48, 140_737_488_355_328),
                "cnt == 0",
                &[]
            )
            .expect("decides"),
            PropertyVerdict::Holds,
            "48-bit nondeterministic drain decides Holds via the ∃-input ranking certificate"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD).
        assert_eq!(
            verify_recoverability(&nondet_drain_w(8, 200), "cnt == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle agrees with the 48-bit ∃-input ranking Holds"
        );
    }

    // Lexicographic ranking — a NESTED counter. `lo` cycles `lo_max → 0`; `hi` decrements ONLY when
    // `lo == 0`. `AG EF (hi == 0)` HOLDS, but δ = `hi` is NOT a valid ranking (hi holds while lo > 0, so
    // it does not strictly decrease every step). The lexicographic measure `(hi, lo)` decreases every
    // step (either `lo` down with `hi` fixed, or `hi` down when `lo` wraps) and is well-founded → decides.
    fn nested_w(width: u32, lo_max: u128, hi_init: u128) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 2 hi\n4 state 2 lo\n5 zero 2\n6 one 2\n\
             7 constd 2 {lo_max}\n8 constd 2 {hi_init}\n9 init 2 3 8\n10 init 2 4 7\n11 eq 1 4 5\n\
             12 sub 2 4 6\n13 ite 2 11 7 12\n14 next 2 4 13\n15 eq 1 3 5\n16 sub 2 3 6\n\
             17 ite 2 15 5 16\n18 ite 2 11 17 3\n19 next 2 3 18\n"
        )
    }

    #[test]
    fn ranking_certificate_decides_nested_counter_via_lexicographic() {
        // 48-bit nested counter: `hi` alone is not a ranking; the lexicographic `(hi, lo)` decides Holds
        // where the single-register certificate fails, the cube abstains, and exact BDD walls.
        assert_eq!(
            verify_recoverability_scalable(&nested_w(48, 100, 140_737_488_355_328), "hi == 0", &[])
                .expect("decides"),
            PropertyVerdict::Holds,
            "48-bit nested counter decides Holds via the lexicographic ranking certificate"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD).
        assert_eq!(
            verify_recoverability(&nested_w(8, 5, 200), "hi == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle agrees with the 48-bit lexicographic ranking Holds"
        );
    }

    // Multi-component (k-tuple) lexicographic — a 3-LEVEL nested counter. `lo` cycles; `mid` decrements
    // iff `lo==0`; `hi` decrements iff `lo==0 && mid==0`. `AG EF (hi==0)` HOLDS, but NO single register
    // and NO 2-tuple `(hi, s)` is a ranking (when lo counts down, `(hi, mid)` holds; at the mid step
    // `(hi, lo)` sees lo wrap up). Only the FULL 3-tuple `(hi, mid, lo)` decreases every step.
    fn nested3_w(width: u32, lo_max: u128, mid_max: u128, hi_init: u128) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 2 hi\n4 state 2 mid\n5 state 2 lo\n\
             6 zero 2\n7 one 2\n8 constd 2 {lo_max}\n9 constd 2 {mid_max}\n10 constd 2 {hi_init}\n\
             11 init 2 3 10\n12 init 2 4 9\n13 init 2 5 8\n14 eq 1 5 6\n15 sub 2 5 7\n16 ite 2 14 8 15\n\
             17 next 2 5 16\n18 eq 1 4 6\n19 sub 2 4 7\n20 ite 2 18 9 19\n21 ite 2 14 20 4\n\
             22 next 2 4 21\n23 and 1 14 18\n24 eq 1 3 6\n25 sub 2 3 7\n26 ite 2 24 6 25\n\
             27 ite 2 23 26 3\n28 next 2 3 27\n"
        )
    }

    #[test]
    fn ranking_certificate_decides_three_level_nested_via_full_tuple() {
        // 48-bit 3-level nested: only the full (hi, mid, lo) tuple decides — the single-register and
        // 2-tuple measures fail, the cube abstains, exact BDD walls.
        assert_eq!(
            verify_recoverability_scalable(
                &nested3_w(48, 50, 30, 140_737_488_355_328),
                "hi == 0",
                &[]
            )
            .expect("decides"),
            PropertyVerdict::Holds,
            "48-bit 3-level nested counter decides Holds via the full-tuple lexicographic ranking"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD).
        assert_eq!(
            verify_recoverability(&nested3_w(8, 3, 2, 100), "hi == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle agrees with the 48-bit full-tuple ranking Holds"
        );
    }

    // COI-filtered ∃-input enumeration — a drain gated on `dec` PLUS a DEAD wide input `junk` (declared,
    // never used). The enumeration pins only inputs in the good register's next-cone (`dec`); the dead
    // 48-bit input is left free (sound), so it does not swamp the enumeration's bit cap. This is the
    // shape a real hardened primitive (OpenTitan `prim_count`, with its secondary counter + FPV backdoor)
    // presents — inputs that do not touch the ranking register but would otherwise block the enumeration.
    fn dead_input_drain_w(width: u32, init: u128) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 2 cnt\n4 zero 2\n5 one 2\n\
             6 constd 2 {init}\n7 init 2 3 6\n8 input 1 dec\n9 input 2 junk\n10 eq 1 3 4\n\
             11 sub 2 3 5\n12 ite 2 8 11 3\n13 ite 2 10 4 12\n14 next 2 3 13\n"
        )
    }

    #[test]
    fn ranking_certificate_ignores_dead_wide_input() {
        // The dead 48-bit input would push the total enumerated input bits over the cap; the good-cone
        // filter excludes it, so the ∃-input drain (`dec`) still decides Holds.
        assert_eq!(
            verify_recoverability_scalable(
                &dead_input_drain_w(48, 140_737_488_355_328),
                "cnt == 0",
                &[]
            )
            .expect("decides"),
            PropertyVerdict::Holds,
            "the dead wide input is excluded from the ∃-input enumeration → decides Holds"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD).
        assert_eq!(
            verify_recoverability(&dead_input_drain_w(8, 200), "cnt == 0").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle agrees"
        );
    }

    // Ascent ranking — a saturating UP-counter to MAX = 2^W-1. `AG EF (cnt == MAX)` ("always able to fill
    // up") HOLDS by an ASCENT, but the descent measure δ = cnt - MAX wraps; the ASCENT measure δ = MAX -
    // cnt strictly decreases every step. The certificate tries both directions.
    fn upcnt_w(width: u32) -> String {
        format!(
            "1 sort bitvec 1\n2 sort bitvec {width}\n3 state 2 cnt\n4 ones 2\n5 one 2\n6 zero 2\n\
             7 init 2 3 6\n8 eq 1 3 4\n9 add 2 3 5\n10 ite 2 8 4 9\n11 next 2 3 10\n"
        )
    }

    #[test]
    fn ranking_certificate_decides_ascent_to_max() {
        // 48-bit up-counter to MAX: decides Holds via the ascent (δ = V - r) direction, where the descent
        // direction wraps, the cube abstains, and exact BDD walls.
        let max48: u128 = (1u128 << 48) - 1;
        assert_eq!(
            verify_recoverability_scalable(&upcnt_w(48), &format!("cnt == {max48}"), &[])
                .expect("decides"),
            PropertyVerdict::Holds,
            "48-bit up-counter to MAX decides Holds via the ascent ranking"
        );
        // DIFFERENTIAL ORACLE at 8-bit (exact BDD).
        assert_eq!(
            verify_recoverability(&upcnt_w(8), "cnt == 255").expect("exact decides"),
            PropertyVerdict::Holds,
            "8-bit exact oracle agrees with the 48-bit ascent ranking Holds"
        );
    }

    // === Auto-escalation: the public verb transparently uses the cube path ======

    #[test]
    fn verify_recoverability_auto_escalates_on_wide_design() {
        // The public `verify_recoverability` tries exact first; on the over-cap wide
        // designs it escalates to the cube path and returns the definite verdict.
        assert_eq!(
            verify_recoverability(WIDE_RECOVERABLE, "st == 0").expect("decides"),
            PropertyVerdict::Holds,
            "auto-escalation must yield the cube-decided Holds on the wide design"
        );
        assert_eq!(
            verify_recoverability(WIDE_TRAP, "st == 0").expect("decides"),
            PropertyVerdict::Violated,
            "auto-escalation must yield the cube-decided Violated on the wide trap"
        );
    }

    #[test]
    fn verify_recoverability_keeps_exact_verdict_when_exact_decides() {
        // When the exact engine decides (small fixtures), the escalation is a no-op —
        // the exact verdict is returned unchanged.
        assert_eq!(
            verify_recoverability(RESPONDER, "st == 0").expect("decides"),
            PropertyVerdict::Holds
        );
        assert_eq!(
            verify_recoverability(STALLER, "st == 0").expect("decides"),
            PropertyVerdict::Violated
        );
    }

    // === Abstains + surface plumbing ===========================================

    #[test]
    fn scalable_non_equality_target_abstains() {
        // A non-`==` good atom is an honest Unknown (the cube auto-seed only pins `==`
        // targets), never a fabricated definite verdict.
        assert_eq!(
            verify_recoverability_scalable(RESPONDER, "st < 3", &[]).expect("abstains cleanly"),
            PropertyVerdict::Unknown
        );
    }

    #[test]
    fn scalable_honors_extra_predicates() {
        // Extra abstraction predicates refine the cube without changing the (sound)
        // verdict on a design the seed already decides.
        let extra = vec![parse_extra_predicate("busy:st=1").expect("parses")];
        assert_eq!(
            verify_recoverability_scalable(RESPONDER, "st == 0", &extra).expect("decides"),
            PropertyVerdict::Holds
        );
    }

    #[test]
    fn parse_extra_predicate_parses_and_rejects() {
        assert_eq!(
            parse_extra_predicate("idle:state_q=3").expect("parses"),
            PredicateSpec {
                name: "idle".into(),
                register: "state_q".into(),
                value: 3,
            }
        );
        assert!(parse_extra_predicate("no_colon").is_err());
        assert!(parse_extra_predicate("n:reg").is_err());
        assert!(parse_extra_predicate("n:reg=notanumber").is_err());
    }
}
