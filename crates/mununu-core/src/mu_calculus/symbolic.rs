//! R-F5 — symbolic (BDD-backed) abstraction spike.
//!
//! The predicate-cube abstraction currently materializes 2^|P| explicit cube states
//! and builds the may/must transition relation with O(2^2|P|) SMT queries (see
//! `adapter/btor2/kmts_lift.rs`). This module is the de-risking spike for the
//! symbolic alternative (R-F5): represent state sets and the transition relation as
//! BDDs (via OxiDD) and evaluate the mu-calculus by fixpoint image/preimage, so the
//! cube space is never enumerated.
//!
//! Spike scope (this file): validate the OxiDD dependency + the BDD encoding of a
//! 3-valued (Kleene) state set as a `(must, may)` BDD pair, and the box preimage via
//! the fused relational product `∃next. R ∧ φ` — checked **cell-for-cell against the
//! explicit `evaluate_tri` evaluator** on hand-built KMTSes. This proves the
//! semantics + encoding before the full port (R-F5.1: BDD-backed `TritSet`; R-F5.2:
//! symbolic modal step in the evaluator; R-F5.3: symbolic edge construction from
//! BTOR2, the actual O(2^2|P|)-SMT-avoidance win).
//!
//! 3-valued box semantics (Bruns–Godefroid; mirrors `evaluator::modal_trit_core`):
//! for `[]φ`, `must = ∀ may-successors. φ.must` and `may = ∀ must-successors. φ.may`
//! — as preimages, `box.must = ¬∃next. R_may ∧ ¬φ.must[next]` and
//! `box.may = ¬∃next. R_must ∧ ¬φ.may[next]`.

#[cfg(test)]
mod tests {
    use oxidd::bdd::{self, BDDFunction};
    use oxidd::{
        BooleanFunction, BooleanFunctionQuant, BooleanOperator, FunctionSubst, Manager, ManagerRef,
        Subst, VarNo,
    };

    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, TransitionModality, Tristate};
    use crate::mu_calculus::evaluator::{Environment, evaluate_tri};
    use crate::mu_calculus::parser;
    use crate::mu_calculus::trit::Trit;

    /// A 3-valued state set as a `(must, may)` BDD pair over the present-state vars,
    /// with the KMTS invariant `must ⊑ may`.
    struct TritBdd {
        must: BDDFunction,
        may: BDDFunction,
    }

    /// A minimal symbolic KMTS over `k` present- + `k` next-state boolean vars.
    struct SymKmts {
        _manager: bdd::BDDManagerRef,
        present: Vec<BDDFunction>,
        next: Vec<BDDFunction>,
        present_varnos: Vec<VarNo>,
        next_cube: BDDFunction,
        tt: BDDFunction,
        ff: BDDFunction,
        r_may: BDDFunction,
        r_must: BDDFunction,
    }

    impl SymKmts {
        /// Build from `state_bits` and an edge list `(src, dst, is_must)`. `R_may` =
        /// all edges; `R_must` = the `is_must` (Sharp) edges only (`must ⊆ may`).
        fn new(state_bits: usize, edges: &[(usize, usize, bool)]) -> Self {
            let manager = bdd::new_manager(1 << 16, 1 << 16, 1);
            let (present, next, tt, ff) = manager.with_manager_exclusive(|m| {
                let vars = m.add_vars(2 * state_bits as VarNo);
                let mut present = Vec::with_capacity(state_bits);
                let mut next = Vec::with_capacity(state_bits);
                for i in 0..state_bits {
                    present.push(BDDFunction::var(m, vars.start + i as VarNo).unwrap());
                }
                for i in 0..state_bits {
                    next.push(BDDFunction::var(m, vars.start + (state_bits + i) as VarNo).unwrap());
                }
                (present, next, BDDFunction::t(m), BDDFunction::f(m))
            });
            let present_varnos: Vec<VarNo> = (0..state_bits as VarNo).collect();
            // Cube of the next-state vars (to quantify them out in the preimage).
            let mut next_cube = tt.clone();
            for v in &next {
                next_cube = next_cube.and(v).unwrap();
            }

            let minterm = |idx: usize, vars: &[BDDFunction], tt: &BDDFunction| -> BDDFunction {
                let mut m = tt.clone();
                for (b, v) in vars.iter().enumerate() {
                    let lit = if (idx >> b) & 1 == 1 {
                        v.clone()
                    } else {
                        v.not().unwrap()
                    };
                    m = m.and(&lit).unwrap();
                }
                m
            };

            // R(present, next) = ⋁ over edges of (src@present ∧ dst@next).
            let mut r_may = ff.clone();
            let mut r_must = ff.clone();
            for &(src, dst, is_must) in edges {
                let e = minterm(src, &present, &tt)
                    .and(&minterm(dst, &next, &tt))
                    .unwrap();
                r_may = r_may.or(&e).unwrap();
                if is_must {
                    r_must = r_must.or(&e).unwrap();
                }
            }

            SymKmts {
                _manager: manager,
                present,
                next,
                present_varnos,
                next_cube,
                tt,
                ff,
                r_may,
                r_must,
            }
        }

        fn present_minterm(&self, idx: usize) -> BDDFunction {
            let mut m = self.tt.clone();
            for (b, v) in self.present.iter().enumerate() {
                let lit = if (idx >> b) & 1 == 1 {
                    v.clone()
                } else {
                    v.not().unwrap()
                };
                m = m.and(&lit).unwrap();
            }
            m
        }

        /// Is state `idx` in the set `f`? (its present-minterm implies `f`.)
        fn holds_at(&self, f: &BDDFunction, idx: usize) -> bool {
            let mt = self.present_minterm(idx);
            mt.and(f).unwrap() == mt
        }

        /// Rename a present-var function to the next-var frame.
        fn to_next(&self, f: &BDDFunction) -> BDDFunction {
            let subst = Subst::new(self.present_varnos.clone(), self.next.clone());
            f.substitute(&subst).unwrap()
        }

        /// 3-valued box preimage of `phi` (over present vars).
        fn box_pre(&self, phi: &TritBdd) -> TritBdd {
            // box.must = ¬∃next. R_may ∧ ¬φ.must[next]
            let must_next = self.to_next(&phi.must);
            let ex_may = self
                .r_may
                .apply_exists(
                    BooleanOperator::And,
                    &must_next.not().unwrap(),
                    &self.next_cube,
                )
                .unwrap();
            let box_must = ex_may.not().unwrap();
            // box.may = ¬∃next. R_must ∧ ¬φ.may[next]
            let may_next = self.to_next(&phi.may);
            let ex_must = self
                .r_must
                .apply_exists(
                    BooleanOperator::And,
                    &may_next.not().unwrap(),
                    &self.next_cube,
                )
                .unwrap();
            let box_may = ex_must.not().unwrap();
            TritBdd {
                must: box_must,
                may: box_may,
            }
        }

        /// Greatest fixpoint `νX. (p ∧ []X)` — the AGp safety verdict.
        fn nu_p_and_box(&self, p: &TritBdd) -> TritBdd {
            let mut x = TritBdd {
                must: self.tt.clone(),
                may: self.tt.clone(),
            };
            loop {
                let bx = self.box_pre(&x);
                // truth-meet `p ⊓ box` = (must∧must, may∧may).
                let next = TritBdd {
                    must: p.must.and(&bx.must).unwrap(),
                    may: p.may.and(&bx.may).unwrap(),
                };
                if next.must == x.must && next.may == x.may {
                    return next;
                }
                x = next;
            }
        }
    }

    /// Build the explicit KMTS twin, evaluate `nu X. p and [] X`, and return the
    /// per-state trit verdicts (index 0..n).
    fn explicit_verdicts(edges: &[(usize, usize, bool)], p: &[Tristate], n: usize) -> Vec<Trit> {
        let mut b = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        for i in 0..n {
            b.state(format!("s{i}"));
        }
        b.initial("s0");
        let step = b.labels().intern(["step"]).unwrap();
        let ids: Vec<_> = (0..n)
            .map(|i| b.state_id_or_insert(format!("s{i}")).unwrap())
            .collect();
        for (i, &verdict) in p.iter().enumerate() {
            b.with_3valued_predicate(ids[i], "p", verdict);
        }
        for &(src, dst, is_must) in edges {
            let modality = if is_must {
                TransitionModality::Sharp
            } else {
                TransitionModality::MayOnly
            };
            b.transition_ids_with_modality(ids[src], &[step], ids[dst], modality);
        }
        let clts = b.build().expect("explicit KMTS builds");
        let formula = parser::parse("nu X. p and [] X").expect("formula parses");
        let env = Environment::new(clts.state_count());
        let verdict = evaluate_tri(&formula, &clts, &env).expect("evaluate_tri");
        (0..n).map(|i| verdict.verdict_at(i)).collect()
    }

    fn symbolic_verdicts(
        state_bits: usize,
        edges: &[(usize, usize, bool)],
        p: &[Tristate],
        n: usize,
    ) -> Vec<Trit> {
        let k = SymKmts::new(state_bits, edges);
        // Atomic predicate `p` as a (must, may) BDD pair over present vars.
        let mut p_must = k.ff.clone();
        let mut p_may = k.ff.clone();
        for (i, &verdict) in p.iter().enumerate() {
            let mt = k.present_minterm(i);
            match verdict {
                Tristate::KleeneT => {
                    p_must = p_must.or(&mt).unwrap();
                    p_may = p_may.or(&mt).unwrap();
                }
                Tristate::KleeneBot => {
                    p_may = p_may.or(&mt).unwrap();
                }
                Tristate::KleeneF => {}
            }
        }
        let pbdd = TritBdd {
            must: p_must,
            may: p_may,
        };
        let x = k.nu_p_and_box(&pbdd);
        (0..n)
            .map(|i| {
                if k.holds_at(&x.must, i) {
                    Trit::True
                } else if k.holds_at(&x.may, i) {
                    Trit::Unknown
                } else {
                    Trit::False
                }
            })
            .collect()
    }

    /// Sharp-only KMTS: a p-true 2-cycle {s0,s1} + a p-false self-loop s2.
    /// `AGp` is definitely true on the cycle, false at s2.
    #[test]
    fn spike_sharp_only_matches_evaluate_tri() {
        let edges = &[(0, 1, true), (1, 0, true), (2, 2, true)];
        let p = &[Tristate::KleeneT, Tristate::KleeneT, Tristate::KleeneF];
        let sym = symbolic_verdicts(2, edges, p, 3);
        let exp = explicit_verdicts(edges, p, 3);
        assert_eq!(
            exp,
            vec![Trit::True, Trit::True, Trit::False],
            "explicit AGp"
        );
        assert_eq!(sym, exp, "symbolic == evaluate_tri (Sharp-only)");
    }

    /// Add a `MayOnly` edge s0→s2 (to a p-false state). The may-edge poisons the box:
    /// s0 → ⊥, and via the s0↔s1 cycle s1 → ⊥ too; s2 stays False. This exercises the
    /// may/must SPLIT (the whole point of the 3-valued domain).
    #[test]
    fn spike_may_edge_bottoms_match_evaluate_tri() {
        let edges = &[(0, 1, true), (1, 0, true), (2, 2, true), (0, 2, false)];
        let p = &[Tristate::KleeneT, Tristate::KleeneT, Tristate::KleeneF];
        let sym = symbolic_verdicts(2, edges, p, 3);
        let exp = explicit_verdicts(edges, p, 3);
        assert_eq!(
            exp,
            vec![Trit::Unknown, Trit::Unknown, Trit::False],
            "explicit AGp with the MayOnly edge"
        );
        assert_eq!(sym, exp, "symbolic == evaluate_tri (may/must split)");
    }

    #[test]
    fn oxidd_smoke_boolean_algebra() {
        let manager = bdd::new_manager(1 << 12, 1 << 12, 1);
        let (x, y, tt, ff) = manager.with_manager_exclusive(|m| {
            let vars = m.add_vars(2);
            (
                BDDFunction::var(m, vars.start).unwrap(),
                BDDFunction::var(m, vars.start + 1).unwrap(),
                BDDFunction::t(m),
                BDDFunction::f(m),
            )
        });
        let nx = x.not().unwrap();
        assert!(x.and(&nx).unwrap() == ff, "x ∧ ¬x = ⊥");
        assert!(x.or(&nx).unwrap() == tt, "x ∨ ¬x = ⊤");
        let and = x.and(&y).unwrap();
        let or = x.or(&y).unwrap();
        assert!(and != or, "x ∧ y ≠ x ∨ y for distinct vars");
        let ny = y.not().unwrap();
        assert!(
            and.not().unwrap() == nx.or(&ny).unwrap(),
            "De Morgan: ¬(x ∧ y) = ¬x ∨ ¬y"
        );
    }
}
