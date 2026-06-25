//! B.1 — compound predicate expressions for the predicate-cube path.
//!
//! Today a cube/SMT predicate is a single atom `register == value`
//! ([`crate::adapter::btor2::kmts_lift::PredicateSpec`]). This module adds
//! **compound predicates** — a boolean combination of register comparisons,
//! e.g. `idle = cnt == 0 && en == 1`, evaluated as ONE cube dimension. A
//! compound predicate does not change `|P|` (it is still one cube bit); only
//! the function that decides that bit's truth changes from an equality test to
//! a recursive boolean evaluation.
//!
//! # The soundness obligation (the §4 PO)
//!
//! There are two evaluators that MUST compute the **same boolean function**
//! over any concrete state, or the cube abstraction is unsound:
//!
//! - [`PredicateExpr::eval`] — explicit evaluation over a concrete register
//!   valuation (the sampling / target-cube-truth path in `kmts_lift`).
//! - [`PredicateExpr::build_constraint`] — the Z3 `Bool` over a BV view (the
//!   SMT may/must-edge path in `smt_must_edge`).
//!
//! The `predicate_expr_eval_matches_smt` differential test enumerates atoms,
//! operators, and assignments and asserts `eval(e, s) == sat(build_constraint(e)
//! under s)`. With that, the cube preservation theorem (PO-1: `may ⊇ concrete`,
//! `must ⊆ concrete`) transfers to compound predicates unchanged — the KMTS
//! machinery is agnostic to whether a predicate is atomic or compound.
//!
//! # Width / masking convention
//!
//! `build_constraint` masks the comparison value to the BV width (matching the
//! simple-atom [`crate::adapter::btor2::smt_must_edge`] path). `eval` compares
//! the register's concrete value (already width-bounded when it comes from the
//! simulator) against the value as-is. The two agree whenever the comparison
//! value fits the register width — the same implicit assumption the existing
//! simple-atom cube path already makes (`next_v == pred.value as u128`).

use std::collections::BTreeSet;

/// Comparison operator for a predicate atom. Unsigned bit-vector semantics
/// (the register is an unsigned bit-vector value), matching the BTOR2 / Z3 BV
/// `bvult`/`bvule`/`bvugt`/`bvuge` operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A compound predicate expression: a boolean combination of register
/// comparisons. The leaf is a single `register <op> value` comparison; the
/// internal nodes are `And` / `Or` / `Not`. No arithmetic — that is the
/// register's job, not the predicate's (keeps the predicate layer inside the
/// audited modal fragment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateExpr {
    /// `register <op> value` (unsigned comparison).
    Cmp {
        register: String,
        op: CmpOp,
        value: u64,
    },
    And(Box<PredicateExpr>, Box<PredicateExpr>),
    Or(Box<PredicateExpr>, Box<PredicateExpr>),
    Not(Box<PredicateExpr>),
}

impl PredicateExpr {
    /// A simple `register == value` atom — the shape every existing
    /// `PredicateSpec` lowers to. Lets the cube path treat a simple predicate
    /// as a trivial compound without special-casing.
    pub fn eq(register: impl Into<String>, value: u64) -> Self {
        PredicateExpr::Cmp {
            register: register.into(),
            op: CmpOp::Eq,
            value,
        }
    }

    /// Explicit evaluation over a concrete register valuation. Registers absent
    /// from `regs` default to `0` — matching the cube lifter's `.unwrap_or(0)`
    /// convention for next-state registers that no transition wrote.
    pub fn eval(&self, regs: &std::collections::HashMap<String, u128>) -> bool {
        match self {
            PredicateExpr::Cmp {
                register,
                op,
                value,
            } => {
                let lhs = regs.get(register).copied().unwrap_or(0);
                let rhs = *value as u128;
                match op {
                    CmpOp::Eq => lhs == rhs,
                    CmpOp::Ne => lhs != rhs,
                    CmpOp::Lt => lhs < rhs,
                    CmpOp::Le => lhs <= rhs,
                    CmpOp::Gt => lhs > rhs,
                    CmpOp::Ge => lhs >= rhs,
                }
            }
            PredicateExpr::And(a, b) => a.eval(regs) && b.eval(regs),
            PredicateExpr::Or(a, b) => a.eval(regs) || b.eval(regs),
            PredicateExpr::Not(a) => !a.eval(regs),
        }
    }

    /// All distinct register names referenced by the expression, sorted. Used
    /// to resolve + width-check every register a compound predicate touches
    /// (the simple atom touches exactly one; a compound may touch several).
    pub fn registers(&self) -> Vec<String> {
        let mut set = BTreeSet::new();
        self.collect_registers(&mut set);
        set.into_iter().collect()
    }

    fn collect_registers(&self, out: &mut BTreeSet<String>) {
        match self {
            PredicateExpr::Cmp { register, .. } => {
                out.insert(register.clone());
            }
            PredicateExpr::And(a, b) | PredicateExpr::Or(a, b) => {
                a.collect_registers(out);
                b.collect_registers(out);
            }
            PredicateExpr::Not(a) => a.collect_registers(out),
        }
    }

    /// SMT encoding: a Z3 `Bool` over the register BVs supplied by `lookup`
    /// (the caller maps a register name to its `state_curr` or `state_next` BV
    /// from the encoded view). Returns `None` if any referenced register is
    /// absent from the view — the caller treats that exactly as it treats a
    /// missing simple-atom register (an `Unknown` must-edge verdict).
    ///
    /// **Caller must hold a [`z3::with_z3_config`] scope.**
    pub fn build_constraint<F>(&self, lookup: &F) -> Option<z3::ast::Bool>
    where
        F: Fn(&str) -> Option<z3::ast::BV>,
    {
        match self {
            PredicateExpr::Cmp {
                register,
                op,
                value,
            } => {
                let bv = lookup(register)?;
                Some(cmp_constraint(&bv, *op, *value))
            }
            PredicateExpr::And(a, b) => {
                let ca = a.build_constraint(lookup)?;
                let cb = b.build_constraint(lookup)?;
                Some(z3::ast::Bool::and(&[&ca, &cb]))
            }
            PredicateExpr::Or(a, b) => {
                let ca = a.build_constraint(lookup)?;
                let cb = b.build_constraint(lookup)?;
                Some(z3::ast::Bool::or(&[&ca, &cb]))
            }
            PredicateExpr::Not(a) => Some(a.build_constraint(lookup)?.not()),
        }
    }
}

/// Build the Z3 `Bool` for one `register <op> value` atom over `bv`. Masks the
/// value to the BV width (matching
/// `smt_must_edge::build_predicate_constraint`).
fn cmp_constraint(bv: &z3::ast::BV, op: CmpOp, value: u64) -> z3::ast::Bool {
    let width = bv.get_size();
    let mask: u64 = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let val_bv = z3::ast::BV::from_u64(value & mask, width);
    match op {
        CmpOp::Eq => bv.eq(&val_bv),
        CmpOp::Ne => bv.eq(&val_bv).not(),
        CmpOp::Lt => bv.bvult(&val_bv),
        CmpOp::Le => bv.bvule(&val_bv),
        CmpOp::Gt => bv.bvugt(&val_bv),
        CmpOp::Ge => bv.bvuge(&val_bv),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn regs(pairs: &[(&str, u128)]) -> HashMap<String, u128> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn eval_simple_eq_atom() {
        let e = PredicateExpr::eq("cnt", 0);
        assert!(e.eval(&regs(&[("cnt", 0)])));
        assert!(!e.eval(&regs(&[("cnt", 1)])));
        // Absent register defaults to 0.
        assert!(e.eval(&regs(&[])));
    }

    #[test]
    fn eval_compound_and_or_not() {
        // idle = cnt == 0 && en == 1
        let idle = PredicateExpr::And(
            Box::new(PredicateExpr::eq("cnt", 0)),
            Box::new(PredicateExpr::eq("en", 1)),
        );
        assert!(idle.eval(&regs(&[("cnt", 0), ("en", 1)])));
        assert!(!idle.eval(&regs(&[("cnt", 0), ("en", 0)])));
        assert!(!idle.eval(&regs(&[("cnt", 3), ("en", 1)])));

        // busy = !(cnt == 0) || en == 0
        let busy = PredicateExpr::Or(
            Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("cnt", 0)))),
            Box::new(PredicateExpr::eq("en", 0)),
        );
        assert!(busy.eval(&regs(&[("cnt", 2), ("en", 1)])));
        assert!(busy.eval(&regs(&[("cnt", 0), ("en", 0)])));
        assert!(!busy.eval(&regs(&[("cnt", 0), ("en", 1)])));
    }

    #[test]
    fn eval_ordering_operators() {
        let lt = PredicateExpr::Cmp {
            register: "x".into(),
            op: CmpOp::Lt,
            value: 4,
        };
        assert!(lt.eval(&regs(&[("x", 3)])));
        assert!(!lt.eval(&regs(&[("x", 4)])));
        let ge = PredicateExpr::Cmp {
            register: "x".into(),
            op: CmpOp::Ge,
            value: 4,
        };
        assert!(ge.eval(&regs(&[("x", 4)])));
        assert!(!ge.eval(&regs(&[("x", 3)])));
    }

    #[test]
    fn registers_collects_all_referenced_sorted() {
        let e = PredicateExpr::And(
            Box::new(PredicateExpr::Or(
                Box::new(PredicateExpr::eq("b", 1)),
                Box::new(PredicateExpr::eq("a", 0)),
            )),
            Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("a", 2)))),
        );
        assert_eq!(e.registers(), vec!["a".to_string(), "b".to_string()]);
    }

    // The §4 soundness obligation: the explicit evaluator and the SMT
    // encoding compute the SAME boolean function over every assignment. If
    // they ever diverge, compound predicates are unsound on the cube path.
    #[test]
    fn predicate_expr_eval_matches_smt() {
        // Two 2-bit registers a, b ranging 0..=3. A spread of expressions
        // covering every operator + And/Or/Not nesting.
        let exprs: Vec<PredicateExpr> = vec![
            PredicateExpr::eq("a", 0),
            PredicateExpr::Cmp {
                register: "a".into(),
                op: CmpOp::Ne,
                value: 2,
            },
            PredicateExpr::Cmp {
                register: "a".into(),
                op: CmpOp::Lt,
                value: 2,
            },
            PredicateExpr::Cmp {
                register: "b".into(),
                op: CmpOp::Ge,
                value: 1,
            },
            PredicateExpr::And(
                Box::new(PredicateExpr::eq("a", 0)),
                Box::new(PredicateExpr::Cmp {
                    register: "b".into(),
                    op: CmpOp::Gt,
                    value: 1,
                }),
            ),
            PredicateExpr::Or(
                Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("a", 3)))),
                Box::new(PredicateExpr::Cmp {
                    register: "b".into(),
                    op: CmpOp::Le,
                    value: 1,
                }),
            ),
            PredicateExpr::Not(Box::new(PredicateExpr::And(
                Box::new(PredicateExpr::eq("a", 1)),
                Box::new(PredicateExpr::eq("b", 1)),
            ))),
        ];

        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            for e in &exprs {
                for a in 0u64..4 {
                    for b in 0u64..4 {
                        let want = e.eval(&regs(&[("a", a as u128), ("b", b as u128)]));

                        let a_bv = z3::ast::BV::new_const("a", 2);
                        let b_bv = z3::ast::BV::new_const("b", 2);
                        let lookup = |name: &str| -> Option<z3::ast::BV> {
                            match name {
                                "a" => Some(a_bv.clone()),
                                "b" => Some(b_bv.clone()),
                                _ => None,
                            }
                        };
                        let constraint = e.build_constraint(&lookup).expect("all regs present");

                        let solver = z3::Solver::new();
                        // Pin the assignment (named bindings, mirroring the
                        // smt_must_edge assert pattern).
                        let a_val = z3::ast::BV::from_u64(a, 2);
                        let b_val = z3::ast::BV::from_u64(b, 2);
                        let a_pin = a_bv.eq(&a_val);
                        let b_pin = b_bv.eq(&b_val);
                        solver.assert(&a_pin);
                        solver.assert(&b_pin);
                        solver.assert(&constraint);
                        let got = matches!(solver.check(), z3::SatResult::Sat);

                        assert_eq!(
                            want, got,
                            "eval/SMT disagree for {e:?} at a={a}, b={b}: eval={want}, smt={got}"
                        );
                    }
                }
            }
        });
    }
}
