//! R-F5.4.2 (2026-07-03) — the **symbolic predicate-cube engine**: compute the
//! per-cube 3-valued verdicts of a mu-calculus property via the R-F5.3/R-F5.4.1
//! symbolic BDD relation, instead of the explicit `predicate_cube_lift`
//! (`O(2^2|P|)` SMT edge construction + `2^|P|` cube materialisation).
//!
//! This is the integration seam between the symbolic engine
//! ([`crate::adapter::btor2::symbolic_bitblast`]) and the predicate-cube
//! surface the CEGAR / verify-auto path speaks in ([`PredicateSpec`] +
//! `compound_exprs` + a mu-calculus [`Formula`]). It:
//!
//! 1. bit-blasts the BTOR2 transition function to BDDs (R-F5.3a),
//! 2. compiles each [`PredicateSpec`] (or its compound
//!    [`PredicateExpr`](crate::adapter::btor2::predicate_expr::PredicateExpr))
//!    to a predicate BDD (R-F5.3b),
//! 3. builds the abstract may/must relation (R-F5.3c/d),
//! 4. evaluates the formula by BDD image/preimage + μ/ν fixpoint (R-F5.4.1),
//! 5. tallies the `{T, F, ⊥}` verdict over the **feasible** cubes (the cubes the
//!    explicit lift would materialise as abstract states).
//!
//! The verdict at each feasible cube is the same one `evaluate_tri` produces on
//! the explicitly-lifted KMTS (pinned by the R-F5.4.1 differential) — but built
//! without a single per-cube-pair SMT query, which is the H.H.c unblock.

use std::collections::HashMap;

use crate::adapter::btor2::predicate_expr::PredicateExpr;
use crate::adapter::btor2::symbolic_bitblast::{BddBitBlaster, MustSemantics};
use crate::adapter::{AdapterError, AdapterErrorKind};
use crate::mu_calculus::Formula;
use crate::mu_calculus::trit::Trit;

pub use crate::adapter::btor2::kmts_lift::PredicateSpec;

/// The per-cube verdict tally from the symbolic engine.
#[derive(Debug, Clone)]
pub struct SymbolicCubeVerdicts {
    /// Number of predicates `k` (the cube space is `2^k`, of which the feasible
    /// subset is materialisable).
    pub num_predicates: usize,
    /// `(cube_index, verdict)` for every **feasible** cube, ascending by index.
    /// Cube `c`'s bit `i` is the truth of `predicates[i]`.
    pub cube_verdicts: Vec<(usize, Trit)>,
    /// Count of definite-true (`KleeneT`) feasible cubes.
    pub definite_true: usize,
    /// Count of definite-false (`KleeneF`) feasible cubes.
    pub definite_false: usize,
    /// Count of indefinite (`KleeneBot`, ⊥) feasible cubes.
    pub bottom: usize,
}

fn ir_err(message: String) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::IrConsistencyError,
        message: format!("adapter/btor2/symbolic_engine: {message}"),
        location: None,
    }
}

/// Compile a [`PredicateSpec`] to a [`PredicateExpr`]: a compound expression
/// (keyed by the spec's `name` in `compound_exprs`) takes precedence; otherwise
/// the simple `register == value` atom.
fn spec_to_expr(
    spec: &PredicateSpec,
    compound_exprs: &HashMap<String, PredicateExpr>,
) -> PredicateExpr {
    compound_exprs
        .get(&spec.name)
        .cloned()
        .unwrap_or_else(|| PredicateExpr::eq(spec.register.clone(), spec.value))
}

/// R-F5.4.2 — evaluate `formula` over the predicate cube abstraction of the
/// BTOR2 design, symbolically. Returns the per-feasible-cube `{T, F, ⊥}`
/// verdict tally.
///
/// `must_semantics` selects the must-edge form ([`MustSemantics::ForallExists`]
/// is the canonical KMTS must-edge and the right default). The formula must be
/// in the audited-sound **bare** fragment (`True`/`False`, predicates,
/// `!`/`&&`/`||`, bare `[]`/`<>`, `mu`/`nu`); a guarded / controllability /
/// step-bounded modality is a hard error.
pub fn symbolic_cube_verdicts(
    btor2_content: &str,
    predicates: &[PredicateSpec],
    compound_exprs: &HashMap<String, PredicateExpr>,
    formula: &Formula,
    must_semantics: MustSemantics,
) -> Result<SymbolicCubeVerdicts, AdapterError> {
    let file = crate::adapter::btor2::parser::parse(btor2_content).map_err(|mut e| {
        e.message = format!("adapter/btor2/symbolic_engine: {}", e.message);
        e
    })?;

    let bb = BddBitBlaster::build(&file).map_err(ir_err)?;

    let exprs: Vec<PredicateExpr> = predicates
        .iter()
        .map(|spec| spec_to_expr(spec, compound_exprs))
        .collect();
    let names: Vec<String> = predicates.iter().map(|s| s.name.clone()).collect();

    let rel = bb
        .abstract_relation(&exprs, Some(must_semantics))
        .map_err(ir_err)?;
    let verdict = rel.evaluate(formula, &names).map_err(ir_err)?;

    let mut cube_verdicts = Vec::new();
    let (mut t, mut f, mut b) = (0usize, 0usize, 0usize);
    for cube in rel.feasible_cubes() {
        let v = rel.verdict_at(&verdict, cube);
        match v {
            Trit::True => t += 1,
            Trit::False => f += 1,
            Trit::Unknown => b += 1,
        }
        cube_verdicts.push((cube, v));
    }

    Ok(SymbolicCubeVerdicts {
        num_predicates: predicates.len(),
        cube_verdicts,
        definite_true: t,
        definite_false: f,
        bottom: b,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::predicate_expr::CmpOp;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, TransitionModality, Tristate};
    use crate::mu_calculus::evaluator::{Environment, evaluate_tri};

    // A 2-bit saturating counter with an enable — the R-F5.3/4 fixture.
    const SAT_COUNTER: &str = r#"
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 input 2 en
5 one 1
6 ones 1
7 add 1 3 5
8 eq 2 3 6
9 ite 1 8 3 7
10 ite 1 4 9 3
11 next 1 3 10
"#;

    /// Ground truth: build the explicit cube-KMTS over feasible cubes (from the
    /// concrete `simulate_one_step` relation) + AP labels, run `evaluate_tri`,
    /// and return the per-feasible-cube verdict list — fully independent of the
    /// symbolic engine under test.
    fn tri_oracle(
        btor2: &str,
        exprs: &[PredicateExpr],
        names: &[&str],
        states: &[(&str, u32)],
        formula_str: &str,
    ) -> Vec<(usize, Trit)> {
        use crate::adapter::btor2::bit_blast::simulate_one_step;
        use crate::adapter::btor2::parser;

        let file = parser::parse(btor2).expect("parse");
        let cube_of = |regs: &HashMap<String, u128>| -> usize {
            let mut c = 0usize;
            for (i, p) in exprs.iter().enumerate() {
                if p.eval(regs) {
                    c |= 1 << i;
                }
            }
            c
        };
        let total_state_bits: u32 = states.iter().map(|(_, w)| *w).sum();
        // Enumerate every concrete (register, input) → collect feasible cubes +
        // the may/must edges (∀∃ must: a cube-edge is must iff every state in the
        // source cube reaches the target under some input).
        let inputs: Vec<(String, u32)> = file_inputs(&file);
        let total_input_bits: u32 = inputs.iter().map(|(_, w)| *w).sum();
        let mut by_cube: HashMap<usize, Vec<std::collections::HashSet<usize>>> = HashMap::new();
        for scombo in 0..(1u128 << total_state_bits) {
            let mut regs: HashMap<String, u128> = HashMap::new();
            let mut off = 0u32;
            for (name, w) in states {
                let mask = (1u128 << w) - 1;
                regs.insert((*name).to_string(), (scombo >> off) & mask);
                off += w;
            }
            let present = cube_of(&regs);
            let mut reach = std::collections::HashSet::new();
            for icombo in 0..(1u128 << total_input_bits) {
                let mut inps: HashMap<String, u128> = HashMap::new();
                let mut ioff = 0u32;
                for (name, w) in &inputs {
                    let mask = (1u128 << w) - 1;
                    inps.insert(name.clone(), (icombo >> ioff) & mask);
                    ioff += w;
                }
                let next = simulate_one_step(&file, &regs, &inps).expect("step");
                let mut rn = regs.clone();
                rn.extend(next);
                reach.insert(cube_of(&rn));
            }
            by_cube.entry(present).or_default().push(reach);
        }

        let feasible: Vec<usize> = {
            let mut v: Vec<usize> = by_cube.keys().copied().collect();
            v.sort_unstable();
            v
        };
        let idx: HashMap<usize, usize> =
            feasible.iter().enumerate().map(|(j, &c)| (c, j)).collect();

        let mut b = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        for j in 0..feasible.len() {
            b.state(format!("c{j}"));
        }
        b.initial("c0");
        let step = b.labels().intern(["step"]).unwrap();
        let ids: Vec<_> = (0..feasible.len())
            .map(|j| b.state_id_or_insert(format!("c{j}")).unwrap())
            .collect();
        for (j, &ci) in feasible.iter().enumerate() {
            for (i, nm) in names.iter().enumerate() {
                let v = if (ci >> i) & 1 == 1 {
                    Tristate::KleeneT
                } else {
                    Tristate::KleeneF
                };
                b.with_3valued_predicate(ids[j], *nm, v);
            }
        }
        for (&ci, reaches) in &by_cube {
            let src = ids[idx[&ci]];
            // may target: union of all reach sets; must target: in every reach set.
            let mut may: std::collections::HashSet<usize> = Default::default();
            for r in reaches {
                may.extend(r);
            }
            for &cj in &may {
                let is_must = reaches.iter().all(|r| r.contains(&cj));
                let m = if is_must {
                    TransitionModality::Sharp
                } else {
                    TransitionModality::MayOnly
                };
                b.transition_ids_with_modality(src, &[step], ids[idx[&cj]], m);
            }
        }
        let clts = b.build().expect("clts");

        let formula = crate::mu_calculus::parser::parse(formula_str).expect("formula");
        let env = Environment::new(clts.state_count());
        let tri = evaluate_tri(&formula, &clts, &env).expect("evaluate_tri");
        feasible
            .iter()
            .enumerate()
            .map(|(j, &c)| (c, tri.verdict_at(j)))
            .collect()
    }

    fn file_inputs(file: &crate::adapter::btor2::ast::Btor2File) -> Vec<(String, u32)> {
        use crate::adapter::btor2::ast::Node;
        use crate::adapter::btor2::parser::bv_width;
        let mut out = Vec::new();
        for line in &file.lines {
            if let Node::Input { sort, symbol } = &line.node {
                let w = bv_width(file, *sort).unwrap_or(0);
                let name = symbol
                    .clone()
                    .unwrap_or_else(|| format!("input_{}", line.nid));
                out.push((name, w));
            }
        }
        out
    }

    #[test]
    fn rf5_4_2_symbolic_engine_matches_tri_via_predicate_specs() {
        // Predicates through the real API: p = (cnt == 0) as a simple spec,
        // q = (cnt >= 2) as a compound expr keyed by name.
        let specs = vec![
            PredicateSpec {
                name: "p".to_string(),
                register: "cnt".to_string(),
                value: 0,
            },
            PredicateSpec {
                name: "q".to_string(),
                register: "cnt".to_string(),
                value: 2, // unused (compound below), kept for the API shape
            },
        ];
        let mut compound: HashMap<String, PredicateExpr> = HashMap::new();
        compound.insert(
            "q".to_string(),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        );
        let exprs = [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ];

        let formulas = [
            "[] p",
            "<> q",
            "nu X. (not q) and [] X",
            "mu X. q or <> X",
            "nu Y. (mu X. (q or <> X)) and [] Y",
        ];

        for formula_str in formulas {
            let formula = crate::mu_calculus::parser::parse(formula_str).expect("formula");
            let got = symbolic_cube_verdicts(
                SAT_COUNTER,
                &specs,
                &compound,
                &formula,
                MustSemantics::ForallExists,
            )
            .expect("symbolic verdicts");

            let want = tri_oracle(SAT_COUNTER, &exprs, &["p", "q"], &[("cnt", 2)], formula_str);

            assert_eq!(
                got.cube_verdicts, want,
                "symbolic engine ≠ evaluate_tri for `{formula_str}`"
            );
            // Tally consistency.
            let (mut t, mut f, mut b) = (0, 0, 0);
            for (_, v) in &got.cube_verdicts {
                match v {
                    Trit::True => t += 1,
                    Trit::False => f += 1,
                    Trit::Unknown => b += 1,
                }
            }
            assert_eq!(
                (got.definite_true, got.definite_false, got.bottom),
                (t, f, b),
                "tally consistent for `{formula_str}`"
            );
        }
    }
}
