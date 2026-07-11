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
/// `good` must be a `REG == VALUE` equality atom; any other comparison (`<`, `>=`, …)
/// returns `Ok(Unknown)` (an honest abstain — the auto-seed pins/enumerates only `==`
/// targets). `extra_predicates` are additional abstraction predicates that refine the
/// cube (they do not enter the formula). Returns the canonical verdict at the design's
/// reset cube, or `Ok(Unknown)` when the abstraction cannot decide or the CEGAR loop
/// errors (a sound abstain — never a fabricated definite verdict).
pub fn verify_recoverability_scalable(
    btor2_content: &str,
    good: &str,
    extra_predicates: &[PredicateSpec],
) -> Result<PropertyVerdict, String> {
    // Parse `good` and require a `REG == VALUE` equality atom.
    let good_expr = parse_predicate_expr(good).map_err(|e| {
        format!("recoverability target `{good}` is not a register-comparison atom (`REG op VALUE`): {e:?}")
    })?;
    let (good_register, good_value) = match good_expr {
        PredicateExpr::Cmp {
            register,
            op: CmpOp::Eq,
            value,
        } => (register, value),
        // SOUNDNESS: the cube auto-seed pins each predicate register to its reset value
        // and reads the reset cube; that construction is only well-defined for an
        // equality (`==`) target. A non-equality `good` (e.g. `st < 3`) is an honest
        // abstain (Unknown), never a fabricated definite verdict.
        _ => return Ok(PropertyVerdict::Unknown),
    };

    // Parse the design early — needed both for the reset valuation and for the auto-seed candidate
    // values below.
    let file = crate::adapter::btor2::parser::parse(btor2_content)
        .map_err(|e| format!("recoverability cube path: parsing BTOR2: {}", e.message))?;
    let init_values: BTreeMap<String, u128> =
        crate::adapter::btor2::concrete_oracle::init_valuation(&file);

    // Seed predicates = [good] ++ [good register's OTHER control states] ++ extra. The `good`
    // predicate is named `good` so the formula atom resolves to it via the cube labelling; the
    // rest only refine the abstraction (they are not referenced by the formula).
    let good_spec = PredicateSpec {
        name: "good".to_string(),
        register: good_register.clone(),
        value: good_value,
    };
    let mut specs: Vec<PredicateSpec> = Vec::with_capacity(1 + extra_predicates.len());
    specs.push(good_spec);

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
    const MAX_AUTO_SEED: usize = 8;
    let mut candidate_values: Vec<u64> = vec![0, 1];
    for v in crate::adapter::btor2::bit_blast::collect_btor2_constants(&file) {
        if !candidate_values.contains(&v) {
            candidate_values.push(v);
        }
    }
    for v in candidate_values {
        // Cap the total predicate count at good (1) + MAX_AUTO_SEED to bound the cube space.
        if specs.len() > MAX_AUTO_SEED {
            break;
        }
        if v != good_value
            && !specs
                .iter()
                .any(|s| s.register == good_register && s.value == v)
        {
            specs.push(PredicateSpec {
                name: format!("state_{good_register}_eq_{v}"),
                register: good_register.clone(),
                value: v,
            });
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
    for (reg, k) in eq_guard_atoms(&file) {
        if specs.len() > MAX_AUTO_SEED {
            break;
        }
        if !specs.iter().any(|s| s.register == reg && s.value == k) {
            specs.push(PredicateSpec {
                name: format!("guard_{reg}_eq_{k}"),
                register: reg,
                value: k,
            });
        }
    }

    specs.extend(extra_predicates.iter().cloned());

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
    let config_entries: Vec<String> = specs
        .iter()
        .filter_map(|s| {
            init_values
                .get(&s.register)
                .map(|v| format!("{}={}", s.register, v))
        })
        .collect();
    let sidecar_json = config_values_to_sidecar_json(&config_entries)
        .map_err(|e| format!("recoverability cube path: building the config-values pin: {e}"))?;
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
    let env = Environment::new(1usize << specs.len());

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
    if !trace
        .final_predicates
        .iter()
        .all(|spec| init_values.contains_key(&spec.register))
    {
        return Ok(PropertyVerdict::Unknown);
    }

    // The design's initial cube: evaluate every FINAL predicate at the reset valuation,
    // in the lift's cube-bit order (`final_predicates[i]` ↔ bit `i`). Every final predicate
    // is now a pinned `register == value` state atom (guarded above), so its reset truth is
    // `init_values[register] == value` and the reset cube is input-independent.
    let mut init_cube = 0usize;
    for (i, spec) in trace.final_predicates.iter().enumerate() {
        if init_values.get(&spec.register).copied() == Some(spec.value as u128) {
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
