//! R-F5.3a (2026-07-02) — a **BDD bit-blaster** for the BTOR2 transition
//! function, the first concrete slice of R-F5.3 (symbolic edge construction).
//!
//! # Why this exists
//!
//! The predicate-cube CEGAR path builds its may/must transition relation with
//! `O(2^2|P|)` Z3 queries (`kmts_lift` → `smt_must_edge`), which is the H.H.c
//! "29 min, no output" wall at `|P| ≈ 12`. R-F5's thesis (see
//! [`.claude/plans/r-f5-symbolic-bdd-track.md`]) is that the relation should be
//! built **symbolically** — as a BDD over the register/input bits — by
//! bit-blasting the BTOR2 transition function once, rather than sampled pair by
//! pair with SMT.
//!
//! This module is the foundation for that: it lifts each BTOR2 node to a
//! **bit-vector of BDDs** (`Vec<BDDFunction>`, LSB-first) over one BDD variable
//! per register bit + per input bit, using the same generic
//! [`BvTermBackend`] / [`walk_design`] seam the concrete evaluator and the Z3
//! encoder already share. The per-op BDD gadgets (ripple-carry adder, unsigned
//! comparator, mux, sign/zero extension, …) mirror the concrete
//! [`eval_op`](super::bit_blast) semantics bit-for-bit.
//!
//! # Correctness gate
//!
//! The bit-blaster is validated **cell-for-cell against the concrete
//! simulator** [`simulate_one_step`](super::bit_blast::simulate_one_step): for
//! every reachable (register, input) assignment, restricting the symbolic
//! next-state BDDs to that assignment must equal the concrete next state. The
//! differential is exhaustive on the small hand-built fixtures below, so a
//! semantic drift in any gadget is caught immediately — the oracle is the
//! same one the explicit path trusts.
//!
//! # Scope (R-F5.3a)
//!
//! Covers the FSM + counter core op set: `not/and/or/xor/nand/nor/xnor`,
//! `inc/dec/neg`, `add/sub`, `eq/neq/iff/implies`, unsigned compares
//! (`ult/ulte/ugt/ugte`), reductions (`redor/redand/redxor`),
//! `concat/slice/uext/sext`, and `ite`. Multiplication, division, variable
//! shifts, signed compares, and array ops are rejected with a clear error —
//! they are later R-F5.3 slices (the FSM+counter residual the H.H.c ceiling
//! needs does not use them).
//!
//! # R-F5.3b — predicate BDDs
//!
//! [`BddBitBlaster::predicate_bdd`] compiles a
//! [`PredicateExpr`](crate::adapter::btor2::predicate_expr::PredicateExpr) into
//! a single BDD over the register-bit variables — the characteristic function
//! of the register valuations that satisfy the predicate. Comparisons reuse the
//! R-F5.3a `eq_bits` / `uge` gadgets at a common (zero-extended) width, so the
//! BDD matches `PredicateExpr::eval` bit-for-bit — the differential oracle.
//! This is the bridge to predicate cubes: each `P_i` is one BDD, so a cube is
//! the conjunction of the `⟦P_i⟧` / `¬⟦P_i⟧`.
//!
//! # R-F5.3c — the abstract may relation (the H.H.c unblock)
//!
//! [`BddBitBlaster::abstract_may_relation`] builds the abstract **may**
//! transition relation over predicate cubes as one BDD over `2k` predicate
//! variables — `p_i` (present) and `p'_i` (next):
//!
//! ```text
//! A(x, p)      = ⋀_i (p_i  ⟺ ⟦P_i⟧(x))
//! A'(x, i, p') = ⋀_i (p'_i ⟺ ⟦P_i⟧(next(x, i)))
//! R_may(p, p') = ∃ (x ∪ i). A(x, p) ∧ A'(x, i, p')
//! ```
//!
//! `⟦P_i⟧(next(x, i))` is the R-F5.3b predicate BDD with every register bit
//! `substitute`d by that register's bit-blasted next-state function (held
//! registers keep their present var; inputs stay free and are quantified out
//! by the `apply_exists`). This is one `substitute` + one `apply_exists` per
//! predicate — **construction cost, not the `O(2^2|P|)` per-cube-pair SMT** the
//! explicit path pays. Validated cell-for-cell against a brute-force concrete
//! may-relation (`simulate_one_step` over every register + input assignment).
//!
//! # R-F5.3d — the must relation
//!
//! [`BddBitBlaster::abstract_relation`] additionally computes `R_must` under a
//! [`MustSemantics`]: `∀x. (A(x,p) → ∃i. A'(x,i,p'))` (the canonical KMTS
//! `∀∃` must-edge — every concrete state in the source cube has *some* input
//! landing in the target) or the stricter `∀(x∪i). (A → A')` (`∀∀`,
//! deterministic-into-target). Built with OxiDD `exists` / `forall` over the
//! input / register cubes — again no per-cube-pair SMT. Must-edges are
//! restricted to **feasible** source cubes (`∧ ∃x. A(x,p)`) so `R_must ⊆ R_may`
//! holds globally over the whole `2^k` cube space (an infeasible cube is
//! vacuously a ∀-must-edge to everything but has no may-edge; the lift never
//! materialises it). Validated cell-for-cell against the brute-force concrete
//! must-relation + the `R_must ⊆ R_may` invariant.
//!
//! # R-F5.4.1 — the symbolic mu-calculus evaluator
//!
//! [`AbstractRelation::evaluate`] runs the mu-calculus directly over
//! `R_may` / `R_must` by BDD image/preimage (box/diamond via
//! `apply_exists(And,…)` on the predicate-var frame) + μ/ν Kleene fixpoint over
//! the `(must, may)` [`TritBdd`], never materialising cube states. It is the
//! `SymbolicKmts` counterpart sourced from the BTOR2-derived relation rather
//! than a `Clts`; an atomic proposition `P_i`'s characteristic set is simply the
//! present predicate var `p_i`. Supports the audited-sound **bare** fragment
//! (`True`/`False`, predicates, `!`/`&&`/`||`, bare `[]`/`<>`, `mu`/`nu`);
//! guards / controllability / step-bounds are an honest error. Validated
//! cell-for-cell against `evaluate_tri` on the equivalent explicit cube-KMTS
//! (nested νμ included). R-F5.4.2 then wires this behind `--engine symbolic`.

use std::collections::HashMap;

use oxidd::bdd::{self, BDDFunction, BDDManagerRef};
use oxidd::{BooleanFunction, FunctionSubst, Manager, ManagerRef, Subst, VarNo};

use crate::adapter::btor2::ast::{Btor2File, ConstValue, Nid, Node, Op, Operand};
use crate::adapter::btor2::bit_blast::eval_const_value;
use crate::adapter::btor2::parser::bv_width;
use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr};
use crate::adapter::btor2::term_backend::{BvTermBackend, WalkError, walk_design};
// R-F5.4.1 — the symbolic mu-calculus evaluator reuses the (must, may) `TritBdd`
// pair + the crate `Trit` verdict; the mu-calculus `Node` is aliased to avoid a
// clash with the BTOR2 `Node` above.
use crate::mu_calculus::symbolic::TritBdd;
use crate::mu_calculus::trit::Trit;
use crate::mu_calculus::{Control, Formula, FormulaVarId, Guard, ModalKind, Node as MuNode};

/// A bit-vector of BDDs, LSB-first: index `b` is the BDD for bit `b`. Every
/// node's value carries exactly `width` bits.
type BitVec = Vec<BDDFunction>;

/// A register or input cell: its symbol, whether it is a `state` (vs `input`),
/// the fresh BDD variables standing for its bits (LSB-first), and their
/// variable numbers. The bit-blaster restricts these to concrete values for
/// the differential; R-F5.3c substitutes register vars with next-state
/// functions via the [`VarNo`]s.
struct Cell {
    symbol: String,
    is_state: bool,
    vars: BitVec,
    varnos: Vec<VarNo>,
}

/// A BDD bit-blaster over one BTOR2 file. Owns one OxiDD manager; every node's
/// value is a `Vec<BDDFunction>` over the register-bit + input-bit variables.
pub struct BddBitBlaster {
    _manager: BDDManagerRef,
    tt: BDDFunction,
    ff: BDDFunction,
    /// `Nid → value` binding store, filled by [`walk_design`].
    env: HashMap<Nid, BitVec>,
    /// Register + input cells, in file order (drives the restriction minterm).
    cells: Vec<Cell>,
    /// State symbol → its next-state BDD vector, from the `Node::Next` lines.
    next_funcs: HashMap<String, BitVec>,
}

impl BddBitBlaster {
    /// Bit-blast `file`: allocate one BDD variable per register/input bit,
    /// bind the leaves, walk every `Const`/`Op` node into a BDD vector, and
    /// collect the per-register next-state functions.
    pub fn build(file: &Btor2File) -> Result<Self, String> {
        // Pass 1 — collect register + input cells with their widths.
        let mut leaf_specs: Vec<(Nid, String, bool, u32)> = Vec::new();
        for line in &file.lines {
            let (sort, symbol, is_state, tag) = match &line.node {
                Node::State { sort, symbol } => (*sort, symbol.clone(), true, "state"),
                Node::Input { sort, symbol } => (*sort, symbol.clone(), false, "input"),
                _ => continue,
            };
            let width = bv_width(file, sort)
                .ok_or_else(|| format!("NID {}: {tag} has non-bitvec sort", line.nid))?;
            let symbol = symbol.unwrap_or_else(|| format!("{tag}_{}", line.nid));
            leaf_specs.push((line.nid, symbol, is_state, width));
        }

        let total_bits: u32 = leaf_specs.iter().map(|(_, _, _, w)| *w).sum();

        // Allocate the manager + one BDD variable per leaf bit.
        let manager = bdd::new_manager(1 << 16, 1 << 16, 1);
        let (all_vars, var_base, tt, ff) = manager.with_manager_exclusive(|m| {
            let range = m.add_vars(total_bits as VarNo);
            let vars: Vec<BDDFunction> = (0..total_bits)
                .map(|i| BDDFunction::var(m, range.start + i as VarNo).unwrap())
                .collect();
            (vars, range.start, BDDFunction::t(m), BDDFunction::f(m))
        });

        // Slice the flat variable pool out per cell, LSB-first.
        let mut env: HashMap<Nid, BitVec> = HashMap::new();
        let mut cells: Vec<Cell> = Vec::new();
        let mut cursor = 0usize;
        for (nid, symbol, is_state, width) in leaf_specs {
            let vars: BitVec = all_vars[cursor..cursor + width as usize].to_vec();
            let varnos: Vec<VarNo> = (0..width as usize)
                .map(|b| var_base + (cursor + b) as VarNo)
                .collect();
            cursor += width as usize;
            env.insert(nid, vars.clone());
            cells.push(Cell {
                symbol,
                is_state,
                vars,
                varnos,
            });
        }

        let mut blaster = BddBitBlaster {
            _manager: manager,
            tt,
            ff,
            env,
            cells,
            next_funcs: HashMap::new(),
        };

        // Pass 2 — walk the node DAG, evaluating every Const/Op into a BitVec.
        walk_design(file, &mut blaster).map_err(|e| match e {
            WalkError::NonBitvecSort(nid) => {
                format!("NID {nid}: non-bitvec sort in the transition cone")
            }
            WalkError::Unevaluated(nid) => format!("NID {nid}: operand unevaluated"),
            WalkError::Backend(msg) => msg,
        })?;

        // Pass 3 — collect the per-register next-state functions. A state's
        // symbol is looked up from its declaration.
        let mut nid_symbol: HashMap<Nid, String> = HashMap::new();
        for line in &file.lines {
            if let Node::State { symbol, .. } = &line.node {
                let sym = symbol
                    .clone()
                    .unwrap_or_else(|| format!("state_{}", line.nid));
                nid_symbol.insert(line.nid, sym);
            }
        }
        for line in &file.lines {
            if let Node::Next { state, value, .. } = &line.node {
                let Some(sym) = nid_symbol.get(state) else {
                    continue;
                };
                let next = blaster.resolve(*value)?;
                blaster.next_funcs.insert(sym.clone(), next);
            }
        }

        Ok(blaster)
    }

    /// Symbolically compute one clock step and restrict it to a concrete
    /// assignment: the BDD counterpart of
    /// [`simulate_one_step`](super::bit_blast::simulate_one_step). Returns each
    /// register symbol (that has a `Next` line) mapped to its next value.
    ///
    /// `assignment` maps every register + input symbol to its current value.
    /// Symbols absent from the map default to `0` (the `setundef -zero`
    /// discipline the concrete oracle also follows).
    pub fn eval_step(&self, assignment: &HashMap<String, u128>) -> HashMap<String, u128> {
        let minterm = self.minterm_for(assignment);
        self.next_funcs
            .iter()
            .map(|(sym, bits)| (sym.clone(), self.value_of(bits, &minterm)))
            .collect()
    }

    /// The full-assignment minterm over every register + input bit.
    fn minterm_for(&self, assignment: &HashMap<String, u128>) -> BDDFunction {
        let mut mt = self.tt.clone();
        for cell in &self.cells {
            let v = assignment.get(&cell.symbol).copied().unwrap_or(0);
            for (b, var) in cell.vars.iter().enumerate() {
                let lit = if (v >> b) & 1 == 1 {
                    var.clone()
                } else {
                    var.not().unwrap()
                };
                mt = mt.and(&lit).unwrap();
            }
        }
        mt
    }

    /// Read a bit-vector of BDDs at a full assignment: bit `b` is set iff the
    /// minterm implies `bits[b]` (`mt ⊑ f ⟺ mt ∧ f == mt`).
    fn value_of(&self, bits: &[BDDFunction], minterm: &BDDFunction) -> u128 {
        let mut val = 0u128;
        for (b, f) in bits.iter().enumerate() {
            if minterm.and(f).unwrap() == *minterm {
                val |= 1u128 << b;
            }
        }
        val
    }

    /// Resolve an operand to its BDD vector, applying the BTOR2 negative-NID
    /// bitwise-NOT shorthand (mirrors the concrete `read_operand`).
    fn resolve(&self, op: Operand) -> Result<BitVec, String> {
        let v = self
            .env
            .get(&op.nid())
            .ok_or_else(|| format!("operand {} unevaluated", op.nid()))?;
        if op.is_negated() {
            Ok(v.iter().map(|b| b.not().unwrap()).collect())
        } else {
            Ok(v.clone())
        }
    }

    // ---- Boolean gadgets (each mirrors a concrete `eval_op` arm) ----

    fn xor(&self, a: &BDDFunction, b: &BDDFunction) -> BDDFunction {
        // (a ∧ ¬b) ∨ (¬a ∧ b) — kept crate-portable (oxidd exposes `.and/.or/.not`).
        let lhs = a.and(&b.not().unwrap()).unwrap();
        let rhs = a.not().unwrap().and(b).unwrap();
        lhs.or(&rhs).unwrap()
    }

    /// Ripple-carry adder over `width` bits: returns `(sum, carry_out)`.
    /// Missing high bits of either operand read as `0`.
    fn add_bits(
        &self,
        a: &[BDDFunction],
        b: &[BDDFunction],
        cin: BDDFunction,
        width: u32,
    ) -> (BitVec, BDDFunction) {
        let mut carry = cin;
        let mut sum = Vec::with_capacity(width as usize);
        for i in 0..width as usize {
            let ai = a.get(i).cloned().unwrap_or_else(|| self.ff.clone());
            let bi = b.get(i).cloned().unwrap_or_else(|| self.ff.clone());
            let axb = self.xor(&ai, &bi);
            let s = self.xor(&axb, &carry);
            // carry' = (ai ∧ bi) ∨ (carry ∧ (ai ⊕ bi))
            let c = ai.and(&bi).unwrap().or(&carry.and(&axb).unwrap()).unwrap();
            sum.push(s);
            carry = c;
        }
        (sum, carry)
    }

    /// OR-reduce a bit-vector to a single BDD (`|x`).
    fn or_reduce(&self, bits: &[BDDFunction]) -> BDDFunction {
        let mut acc = self.ff.clone();
        for b in bits {
            acc = acc.or(b).unwrap();
        }
        acc
    }

    /// AND-reduce a bit-vector to a single BDD (`&x`).
    fn and_reduce(&self, bits: &[BDDFunction]) -> BDDFunction {
        let mut acc = self.tt.clone();
        for b in bits {
            acc = acc.and(b).unwrap();
        }
        acc
    }

    /// XOR-reduce a bit-vector to a single BDD (`^x`).
    fn xor_reduce(&self, bits: &[BDDFunction]) -> BDDFunction {
        let mut acc = self.ff.clone();
        for b in bits {
            acc = self.xor(&acc, b);
        }
        acc
    }

    /// Structural bit-equality: AND of per-bit XNOR. Returns a 1-bit vector.
    fn eq_bits(&self, a: &[BDDFunction], b: &[BDDFunction]) -> BitVec {
        let n = a.len().max(b.len());
        let mut acc = self.tt.clone();
        for i in 0..n {
            let ai = a.get(i).cloned().unwrap_or_else(|| self.ff.clone());
            let bi = b.get(i).cloned().unwrap_or_else(|| self.ff.clone());
            let xnor = self.xor(&ai, &bi).not().unwrap();
            acc = acc.and(&xnor).unwrap();
        }
        vec![acc]
    }

    /// Unsigned `a >= b`: the carry-out of `a + ¬b + 1` (borrow-free subtract).
    fn uge(&self, a: &[BDDFunction], b: &[BDDFunction], width: u32) -> BDDFunction {
        let not_b: BitVec = (0..width as usize)
            .map(|i| {
                b.get(i)
                    .cloned()
                    .unwrap_or_else(|| self.ff.clone())
                    .not()
                    .unwrap()
            })
            .collect();
        let (_, cout) = self.add_bits(a, &not_b, self.tt.clone(), width);
        cout
    }

    // ---- R-F5.3b: predicate BDDs over the register-bit variables ----

    /// The BDD variable vector for a register/input symbol, LSB-first, or
    /// `None` if the symbol names no cell.
    pub fn register_bits(&self, name: &str) -> Option<&BitVec> {
        self.cells
            .iter()
            .find(|c| c.symbol == name)
            .map(|c| &c.vars)
    }

    /// Zero-extend a bit-vector to `width` bits (high bits become `⊥`).
    fn zero_extend(&self, bits: &[BDDFunction], width: usize) -> BitVec {
        (0..width)
            .map(|i| bits.get(i).cloned().unwrap_or_else(|| self.ff.clone()))
            .collect()
    }

    /// A constant bit-vector of `value` at `width` bits, LSB-first.
    fn const_bits(&self, value: u128, width: usize) -> BitVec {
        (0..width)
            .map(|b| {
                if b < 128 && (value >> b) & 1 == 1 {
                    self.tt.clone()
                } else {
                    self.ff.clone()
                }
            })
            .collect()
    }

    /// An unsigned comparison of two equal-width bit-vectors, as a 1-bit BDD.
    /// Mirrors [`crate::adapter::btor2::predicate_expr`]'s `cmp_apply` (unsigned).
    fn cmp_bits(&self, a: &[BDDFunction], op: CmpOp, b: &[BDDFunction], width: u32) -> BDDFunction {
        match op {
            CmpOp::Eq => self.eq_bits(a, b).swap_remove(0),
            CmpOp::Ne => self.eq_bits(a, b).swap_remove(0).not().unwrap(),
            CmpOp::Ge => self.uge(a, b, width),
            CmpOp::Lt => self.uge(a, b, width).not().unwrap(),
            CmpOp::Le => self.uge(b, a, width), // a ≤ b ⟺ b ≥ a
            CmpOp::Gt => self.uge(b, a, width).not().unwrap(), // a > b ⟺ ¬(b ≥ a)
        }
    }

    /// R-F5.3b — compile a [`PredicateExpr`] into a single BDD over the
    /// present-state register-bit variables: the characteristic function of the
    /// set of concrete register valuations that satisfy the predicate.
    ///
    /// This is the bridge from the register-bit BDDs (R-F5.3a) to predicate
    /// cubes: each predicate `P_i` becomes one BDD `⟦P_i⟧(x)` over the leaf
    /// vars, so a cube (an assignment to the `|P|` predicates) is the
    /// conjunction of the `⟦P_i⟧` / `¬⟦P_i⟧`. R-F5.3c then builds the abstract
    /// may/must relation from these + the bit-blasted next-state functions.
    ///
    /// Comparisons are **unsigned** and evaluated at a common width (operands
    /// zero-extended), so the BDD matches [`PredicateExpr::eval`]'s `u128`
    /// semantics bit-for-bit — including when a literal exceeds the register
    /// width. `And`/`Or`/`Not` map to the BDD boolean ops.
    pub fn predicate_bdd(&self, expr: &PredicateExpr) -> Result<BDDFunction, String> {
        let unknown = |r: &str| format!("predicate references unknown register `{r}`");
        match expr {
            PredicateExpr::Cmp {
                register,
                op,
                value,
            } => {
                let reg = self
                    .register_bits(register)
                    .ok_or_else(|| unknown(register))?;
                // Compare at max(reg width, 64) so a u64 literal wider than the
                // register keeps its high bits (reg's zero-extend to those bits
                // is `0`, matching `eval`'s unmasked u128 comparison).
                let w = reg.len().max(64);
                let a = self.zero_extend(reg, w);
                let b = self.const_bits(*value as u128, w);
                Ok(self.cmp_bits(&a, *op, &b, w as u32))
            }
            PredicateExpr::CmpReg { lhs, op, rhs } => {
                let l = self.register_bits(lhs).ok_or_else(|| unknown(lhs))?;
                let r = self.register_bits(rhs).ok_or_else(|| unknown(rhs))?;
                let w = l.len().max(r.len());
                let a = self.zero_extend(l, w);
                let b = self.zero_extend(r, w);
                Ok(self.cmp_bits(&a, *op, &b, w as u32))
            }
            PredicateExpr::CmpRegAddend {
                lhs,
                op,
                rhs,
                addend,
                width: _,
            } => {
                let l = self.register_bits(lhs).ok_or_else(|| unknown(lhs))?;
                let r = self.register_bits(rhs).ok_or_else(|| unknown(rhs))?;
                // sum = (rhs + addend) mod 2^(rhs width) — wraps exactly as the
                // RTL `+` (the `bvadd` in `build_constraint`; the `width` field
                // is `eval`'s modulus and equals the rhs register width here).
                let rw = r.len();
                let addend_bits = self.const_bits(*addend as u128, rw);
                let (sum, _) = self.add_bits(r, &addend_bits, self.ff.clone(), rw as u32);
                let w = l.len().max(rw);
                let a = self.zero_extend(l, w);
                let b = self.zero_extend(&sum, w);
                Ok(self.cmp_bits(&a, *op, &b, w as u32))
            }
            PredicateExpr::And(a, b) => {
                Ok(self.predicate_bdd(a)?.and(&self.predicate_bdd(b)?).unwrap())
            }
            PredicateExpr::Or(a, b) => {
                Ok(self.predicate_bdd(a)?.or(&self.predicate_bdd(b)?).unwrap())
            }
            PredicateExpr::Not(a) => Ok(self.predicate_bdd(a)?.not().unwrap()),
        }
    }

    /// Evaluate a predicate BDD (over the register vars) at a concrete register
    /// assignment: `true` iff the assignment's minterm implies the BDD. The
    /// differential counterpart of [`PredicateExpr::eval`].
    pub fn eval_predicate_at(&self, bdd: &BDDFunction, assignment: &HashMap<String, u128>) -> bool {
        let mt = self.minterm_for(assignment);
        mt.and(bdd).unwrap() == mt
    }

    // ---- R-F5.3c: the abstract may relation over predicate cubes ----

    /// R-F5.3c — build the abstract **may** transition relation over predicate
    /// cubes, symbolically, without the `O(2^2|P|)` SMT the explicit path uses.
    ///
    /// Given predicates `P_0..P_{k-1}`, this returns an [`AbstractRelation`]
    /// whose `R_may` is a BDD over `2k` fresh predicate variables — `p_i`
    /// (present cube) and `p'_i` (next cube) — such that cube `c → c'` is a
    /// may-edge iff *some* concrete state in `c` can, under *some* input, step
    /// to a concrete state in `c'`:
    ///
    /// ```text
    /// A(x, p)      = ⋀_i (p_i  ⟺ ⟦P_i⟧(x))
    /// A'(x, i, p') = ⋀_i (p'_i ⟺ ⟦P_i⟧(next(x, i)))
    /// R_may(p, p') = ∃ (x ∪ i). A(x, p) ∧ A'(x, i, p')
    /// ```
    ///
    /// `⟦P_i⟧(next(x, i))` is the R-F5.3b predicate BDD with every register bit
    /// substituted by that register's bit-blasted next-state function (held
    /// registers keep their present var; inputs stay free and are quantified
    /// out). The whole relation is one `substitute` + one `apply_exists` per
    /// predicate — construction cost, not per-cube-pair SMT.
    pub fn abstract_may_relation(
        &self,
        predicates: &[PredicateExpr],
    ) -> Result<AbstractRelation, String> {
        self.abstract_relation(predicates, None)
    }

    /// R-F5.3c/d — build the abstract may (and optionally must) transition
    /// relation over predicate cubes, symbolically. `R_may` is always computed;
    /// `R_must` is computed iff `must` is `Some`.
    ///
    /// With `A(x,p) = ⋀_i (p_i ⟺ ⟦P_i⟧(x))` and
    /// `A'(x,i,p') = ⋀_i (p'_i ⟺ ⟦P_i⟧(next(x,i)))`:
    ///
    /// ```text
    /// R_may(p, p')  = ∃ (x ∪ i). A ∧ A'                         (some x, some i)
    /// R_must_∀∃     = ∀ x. ( A(x,p) → ∃ i. A'(x,i,p') )         (every x, some i)
    /// R_must_∀∀     = ∀ (x ∪ i). ( A(x,p) → A'(x,i,p') )        (every x, every i)
    /// ```
    ///
    /// The `∀∃` form is the canonical KMTS must-edge (Bruns–Godefroid; the
    /// explicit path's [`MustEdgeInference::SmtPerTargetStandard`]); `∀∀` is
    /// the stricter deterministic-into-target form ([`MustEdgeInference::SmtPerTarget`]).
    /// `⟦P_i⟧(next(x,i))` is the R-F5.3b predicate BDD with every register bit
    /// substituted by that register's bit-blasted next-state function.
    ///
    /// [`MustEdgeInference::SmtPerTargetStandard`]: super::kmts_lift::MustEdgeInference::SmtPerTargetStandard
    /// [`MustEdgeInference::SmtPerTarget`]: super::kmts_lift::MustEdgeInference::SmtPerTarget
    pub fn abstract_relation(
        &self,
        predicates: &[PredicateExpr],
        must: Option<MustSemantics>,
    ) -> Result<AbstractRelation, String> {
        use oxidd::{BooleanFunctionQuant, BooleanOperator};

        let k = predicates.len();

        // Allocate 2k fresh predicate vars at the bottom of the order: p_i then p'_i.
        let (present, next, present_varnos) = self._manager.with_manager_exclusive(|m| {
            let range = m.add_vars(2 * k as VarNo);
            let present: Vec<BDDFunction> = (0..k)
                .map(|i| BDDFunction::var(m, range.start + i as VarNo).unwrap())
                .collect();
            let next: Vec<BDDFunction> = (0..k)
                .map(|i| BDDFunction::var(m, range.start + (k + i) as VarNo).unwrap())
                .collect();
            let present_varnos: Vec<VarNo> = (0..k as VarNo).map(|i| range.start + i).collect();
            (present, next, present_varnos)
        });
        // The cube of next predicate vars (to quantify out in a preimage).
        let mut next_cube = self.tt.clone();
        for v in &next {
            next_cube = next_cube.and(v).unwrap();
        }

        // The present→next substitution: each register bit var ↦ its
        // bit-blasted next-state function. Registers without a `Next` line
        // (held) and inputs are absent ⇒ left free (identity), which is the
        // correct next value for a held register and keeps inputs quantifiable.
        let mut sub_vars: Vec<VarNo> = Vec::new();
        let mut sub_repl: Vec<BDDFunction> = Vec::new();
        for cell in &self.cells {
            if !cell.is_state {
                continue;
            }
            if let Some(next_fn) = self.next_funcs.get(&cell.symbol) {
                for (vn, f) in cell.varnos.iter().zip(next_fn.iter()) {
                    sub_vars.push(*vn);
                    sub_repl.push(f.clone());
                }
            }
        }

        // Cubes: register vars, input vars, and their union.
        let mut reg_cube = self.tt.clone();
        let mut input_cube = self.tt.clone();
        for cell in &self.cells {
            for v in &cell.vars {
                if cell.is_state {
                    reg_cube = reg_cube.and(v).unwrap();
                } else {
                    input_cube = input_cube.and(v).unwrap();
                }
            }
        }
        let xi_cube = reg_cube.and(&input_cube).unwrap();

        // A(x, p) and A'(x, i, p').
        let mut a = self.tt.clone();
        let mut a_prime = self.tt.clone();
        for (i, expr) in predicates.iter().enumerate() {
            let pred = self.predicate_bdd(expr)?; // ⟦P_i⟧(x)
            let pred_next = if sub_vars.is_empty() {
                pred.clone()
            } else {
                pred.substitute(&Subst::new(sub_vars.clone(), sub_repl.clone()))
                    .unwrap()
            };
            // p_i ⟺ pred  and  p'_i ⟺ pred_next  (via XNOR).
            let iff_present = self.xor(&present[i], &pred).not().unwrap();
            let iff_next = self.xor(&next[i], &pred_next).not().unwrap();
            a = a.and(&iff_present).unwrap();
            a_prime = a_prime.and(&iff_next).unwrap();
        }

        // R_may = ∃(x ∪ i). A ∧ A'   (fused relational product).
        let r_may = a
            .apply_exists(BooleanOperator::And, &a_prime, &xi_cube)
            .unwrap();

        // R_must, when requested. `A → φ` is `¬A ∨ φ`.
        //
        // A vacuously-empty (unsatisfiable) source cube yields `¬A ≡ ⊤` there,
        // so the ∀ would make it a must-edge to *every* cube — breaking the KMTS
        // invariant `R_must ⊆ R_may` (an empty cube has no may-edge). We
        // intersect with the FEASIBLE present cubes `∃x. A(x,p)`: must-edges
        // only leave inhabited abstract states (the lift never materialises an
        // infeasible cube), so `R_must ⊆ R_may` holds globally over the whole
        // `2^k` cube space — which the R-F5.4.1 evaluator relies on
        // (`box.must ⊆ box.may`).
        let r_must = must.map(|sem| {
            let raw = match sem {
                MustSemantics::ForallExists => {
                    // ∀x. ( A(x,p) → ∃i. A'(x,i,p') )
                    let exists_i = a_prime.exists(&input_cube).unwrap();
                    a.not()
                        .unwrap()
                        .or(&exists_i)
                        .unwrap()
                        .forall(&reg_cube)
                        .unwrap()
                }
                MustSemantics::ForallForall => {
                    // ∀(x ∪ i). ( A(x,p) → A'(x,i,p') )
                    a.not()
                        .unwrap()
                        .or(&a_prime)
                        .unwrap()
                        .forall(&xi_cube)
                        .unwrap()
                }
            };
            let feasible_present = a.exists(&reg_cube).unwrap();
            raw.and(&feasible_present).unwrap()
        });

        Ok(AbstractRelation {
            num_predicates: k,
            present,
            next,
            present_varnos,
            next_cube,
            r_may,
            r_must,
            tt: self.tt.clone(),
            ff: self.ff.clone(),
        })
    }
}

/// R-F5.3d — which under-approximating must-edge semantics to compute for the
/// abstract relation. Mirrors the explicit path's must-edge options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MustSemantics {
    /// `∀x∈c. ∃i. next(x,i)∈c'` — the canonical KMTS must-edge (every concrete
    /// state in the source cube has *some* input that lands in the target).
    /// The more permissive of the two; the sound `[]`/`<>` under-approximation.
    ForallExists,
    /// `∀x∈c. ∀i. next(x,i)∈c'` — stricter: *every* input lands in the target
    /// (deterministic into the target cube). Every `∀∀` must-edge is also a
    /// `∀∃` must-edge.
    ForallForall,
}

/// R-F5.3c/d — the abstract may + must transition relation over predicate
/// cubes, as BDDs over `2k` predicate variables. A cube is an index in
/// `0..2^k`; bit `i` of the index is the truth of predicate `P_i`.
pub struct AbstractRelation {
    num_predicates: usize,
    /// Present-cube predicate variables `p_0..p_{k-1}`.
    present: Vec<BDDFunction>,
    /// Next-cube predicate variables `p'_0..p'_{k-1}`.
    next: Vec<BDDFunction>,
    /// Variable numbers of the present predicate vars (for the present→next
    /// rename in a preimage).
    present_varnos: Vec<VarNo>,
    /// The cube of next-cube predicate vars, to quantify out in a preimage.
    next_cube: BDDFunction,
    /// The may relation as a BDD over `(p, p')`.
    r_may: BDDFunction,
    /// The must relation over `(p, p')`, when a [`MustSemantics`] was requested.
    r_must: Option<BDDFunction>,
    tt: BDDFunction,
    ff: BDDFunction,
}

impl AbstractRelation {
    /// Number of predicates `k` (so `2^k` cubes).
    pub fn num_predicates(&self) -> usize {
        self.num_predicates
    }

    /// The may relation BDD over the present + next predicate variables.
    pub fn may(&self) -> &BDDFunction {
        &self.r_may
    }

    /// The must relation BDD, or `None` if no [`MustSemantics`] was requested.
    pub fn must(&self) -> Option<&BDDFunction> {
        self.r_must.as_ref()
    }

    /// The complete-assignment minterm for a cube index over the given
    /// predicate variables (`vars[i]` true iff bit `i` of `cube` is set).
    fn cube_minterm(&self, cube: usize, vars: &[BDDFunction]) -> BDDFunction {
        let mut m = self.tt.clone();
        for (i, v) in vars.iter().enumerate() {
            let lit = if (cube >> i) & 1 == 1 {
                v.clone()
            } else {
                v.not().unwrap()
            };
            m = m.and(&lit).unwrap();
        }
        m
    }

    /// Is `present_cube → next_cube` a may-edge? (its full `(p, p')` minterm
    /// implies `R_may`).
    pub fn may_holds(&self, present_cube: usize, next_cube: usize) -> bool {
        self.holds(&self.r_may, present_cube, next_cube)
    }

    /// Is `present_cube → next_cube` a must-edge? Panics if no [`MustSemantics`]
    /// was requested when the relation was built.
    pub fn must_holds(&self, present_cube: usize, next_cube: usize) -> bool {
        let r_must = self
            .r_must
            .as_ref()
            .expect("must_holds called on a relation built without a MustSemantics");
        self.holds(r_must, present_cube, next_cube)
    }

    /// Does the `(present_cube, next_cube)` pair belong to relation `r`?
    fn holds(&self, r: &BDDFunction, present_cube: usize, next_cube: usize) -> bool {
        let mt = self
            .cube_minterm(present_cube, &self.present)
            .and(&self.cube_minterm(next_cube, &self.next))
            .unwrap();
        mt.and(r).unwrap() == mt
    }

    // ---- R-F5.4.1: the symbolic mu-calculus evaluator over the relation ----

    /// `⊤` / `⊥` as `TritBdd`s over the present predicate vars (built directly
    /// from the manager's constants, since we have no `SymbolicContext`).
    fn tb_top(&self) -> TritBdd {
        TritBdd::from_parts(self.tt.clone(), self.tt.clone())
    }
    fn tb_bot(&self) -> TritBdd {
        TritBdd::from_parts(self.ff.clone(), self.ff.clone())
    }

    /// Rename a present-var function to the next-var frame (for a preimage).
    fn to_next(&self, f: &BDDFunction) -> BDDFunction {
        f.substitute(&Subst::new(self.present_varnos.clone(), self.next.clone()))
            .unwrap()
    }

    /// 3-valued box preimage of `phi` (Bruns–Godefroid): `box.must` uses
    /// `R_may`, `box.may` uses `R_must`. Mirrors [`crate::mu_calculus::symbolic`]'s
    /// `SymbolicKmts::box_pre` on this relation's own predicate-var frame.
    fn box_pre(&self, phi: &TritBdd, r_must: &BDDFunction) -> TritBdd {
        use oxidd::{BooleanFunctionQuant, BooleanOperator};
        let must_next = self.to_next(phi.must());
        let box_must = self
            .r_may
            .apply_exists(
                BooleanOperator::And,
                &must_next.not().unwrap(),
                &self.next_cube,
            )
            .unwrap()
            .not()
            .unwrap();
        let may_next = self.to_next(phi.may());
        let box_may = r_must
            .apply_exists(
                BooleanOperator::And,
                &may_next.not().unwrap(),
                &self.next_cube,
            )
            .unwrap()
            .not()
            .unwrap();
        TritBdd::from_parts(box_must, box_may)
    }

    /// 3-valued diamond preimage of `phi`: `dia.must` uses `R_must`, `dia.may`
    /// uses `R_may`.
    fn diamond_pre(&self, phi: &TritBdd, r_must: &BDDFunction) -> TritBdd {
        use oxidd::{BooleanFunctionQuant, BooleanOperator};
        let must_next = self.to_next(phi.must());
        let dia_must = r_must
            .apply_exists(BooleanOperator::And, &must_next, &self.next_cube)
            .unwrap();
        let may_next = self.to_next(phi.may());
        let dia_may = self
            .r_may
            .apply_exists(BooleanOperator::And, &may_next, &self.next_cube)
            .unwrap();
        TritBdd::from_parts(dia_must, dia_may)
    }

    /// R-F5.4.1 — evaluate a mu-calculus `formula` symbolically over this
    /// abstract relation, returning a `(must, may)` verdict over the present
    /// predicate cubes. `pred_names[i]` names predicate `P_i` (the atomic
    /// proposition `P_i` holds exactly on the cubes where bit `i` is set —
    /// i.e. its characteristic set is the present var `p_i`).
    ///
    /// Supports the **audited-sound bare fragment** the cube path evaluates:
    /// `True`/`False`, predicates, `!`/`&&`/`||`, bare `[]`/`<>`, `mu`/`nu`.
    /// A guarded / controllability / step-bounded modality is an honest error
    /// (as in [`crate::mu_calculus::symbolic`]'s `SymbolicKmts`). Requires a
    /// relation built *with* a [`MustSemantics`] (the modal steps read
    /// `R_must`).
    pub fn evaluate(&self, formula: &Formula, pred_names: &[String]) -> Result<TritBdd, String> {
        let r_must = self.r_must.as_ref().ok_or_else(|| {
            "AbstractRelation::evaluate requires a must relation — build with a MustSemantics"
                .to_string()
        })?;
        let name_to_idx: HashMap<&str, usize> = pred_names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let mut bindings: HashMap<FormulaVarId, TritBdd> = HashMap::new();
        self.eval_node(formula, formula.root(), &name_to_idx, r_must, &mut bindings)
    }

    fn eval_node(
        &self,
        f: &Formula,
        id: crate::mu_calculus::NodeId,
        names: &HashMap<&str, usize>,
        r_must: &BDDFunction,
        bindings: &mut HashMap<FormulaVarId, TritBdd>,
    ) -> Result<TritBdd, String> {
        Ok(match f.node(id) {
            MuNode::True => self.tb_top(),
            MuNode::False => self.tb_bot(),
            MuNode::Predicate(name) => match names.get(name.as_str()) {
                // The AP's characteristic set is the present predicate var; the
                // cube valuation is definite (Sharp), so must = may = p_i.
                Some(&i) => TritBdd::from_parts(self.present[i].clone(), self.present[i].clone()),
                None => self.tb_bot(),
            },
            MuNode::Variable(v) => bindings
                .get(v)
                .cloned()
                .ok_or_else(|| format!("unbound fixpoint variable {v:?}"))?,
            MuNode::Not(n) => self.eval_node(f, *n, names, r_must, bindings)?.not(),
            MuNode::And(a, b) => {
                let av = self.eval_node(f, *a, names, r_must, bindings)?;
                let bv = self.eval_node(f, *b, names, r_must, bindings)?;
                av.and(&bv)
            }
            MuNode::Or(a, b) => {
                let av = self.eval_node(f, *a, names, r_must, bindings)?;
                let bv = self.eval_node(f, *b, names, r_must, bindings)?;
                av.or(&bv)
            }
            MuNode::Modal {
                kind,
                guard,
                target,
            } => {
                if guard.control != Control::All {
                    return Err(
                        "symbolic cube evaluator: controllability guards (`ctrl`) unsupported"
                            .into(),
                    );
                }
                if guard.max_steps.is_some() {
                    return Err(
                        "symbolic cube evaluator: step-bounded modalities (`steps`) unsupported"
                            .into(),
                    );
                }
                if *guard != Guard::default() {
                    return Err(
                        "symbolic cube evaluator: label / state-var guards unsupported (bare `[]`/`<>` only)".into(),
                    );
                }
                let phi = self.eval_node(f, *target, names, r_must, bindings)?;
                match kind {
                    ModalKind::Box => self.box_pre(&phi, r_must),
                    ModalKind::Diamond => self.diamond_pre(&phi, r_must),
                }
            }
            MuNode::Mu { var, body } => {
                self.fixpoint(f, *var, *body, names, r_must, bindings, false)?
            }
            MuNode::Nu { var, body } => {
                self.fixpoint(f, *var, *body, names, r_must, bindings, true)?
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn fixpoint(
        &self,
        f: &Formula,
        var: FormulaVarId,
        body: crate::mu_calculus::NodeId,
        names: &HashMap<&str, usize>,
        r_must: &BDDFunction,
        bindings: &mut HashMap<FormulaVarId, TritBdd>,
        greatest: bool,
    ) -> Result<TritBdd, String> {
        let mut x = if greatest {
            self.tb_top()
        } else {
            self.tb_bot()
        };
        loop {
            bindings.insert(var, x.clone());
            let next = self.eval_node(f, body, names, r_must, bindings)?;
            if next.eq_set(&x) {
                bindings.remove(&var);
                return Ok(next);
            }
            x = next;
        }
    }

    /// The trit verdict of an evaluated formula at cube `cube`.
    pub fn verdict_at(&self, verdict: &TritBdd, cube: usize) -> Trit {
        let mt = self.cube_minterm(cube, &self.present);
        if mt.and(verdict.must()).unwrap() == mt {
            Trit::True
        } else if mt.and(verdict.may()).unwrap() == mt {
            Trit::Unknown
        } else {
            Trit::False
        }
    }
}

impl BvTermBackend for BddBitBlaster {
    type Value = BitVec;
    type Error = String;

    fn eval_const(&mut self, value: &ConstValue, width: u32) -> Result<Self::Value, Self::Error> {
        // Reuse the concrete constant semantics, then splat to per-bit BDDs.
        let bv = eval_const_value(value, width)?;
        Ok((0..width as usize)
            .map(|b| {
                if (bv.bits >> b) & 1 == 1 {
                    self.tt.clone()
                } else {
                    self.ff.clone()
                }
            })
            .collect())
    }

    fn eval_op(
        &mut self,
        _nid: Nid,
        op: Op,
        immediates: &[u32],
        args: &[Operand],
        width: u32,
    ) -> Result<Self::Value, Self::Error> {
        let read = |i: usize| -> Result<BitVec, String> { self.resolve(args[i]) };

        let result: BitVec = match op {
            // ---- Bitwise (result + operands all `width` bits) ----
            Op::Not => {
                let a = read(0)?;
                (0..width as usize).map(|i| a[i].not().unwrap()).collect()
            }
            Op::And => {
                let (a, b) = (read(0)?, read(1)?);
                (0..width as usize)
                    .map(|i| a[i].and(&b[i]).unwrap())
                    .collect()
            }
            Op::Or => {
                let (a, b) = (read(0)?, read(1)?);
                (0..width as usize)
                    .map(|i| a[i].or(&b[i]).unwrap())
                    .collect()
            }
            Op::Xor => {
                let (a, b) = (read(0)?, read(1)?);
                (0..width as usize)
                    .map(|i| self.xor(&a[i], &b[i]))
                    .collect()
            }
            Op::Nand => {
                let (a, b) = (read(0)?, read(1)?);
                (0..width as usize)
                    .map(|i| a[i].and(&b[i]).unwrap().not().unwrap())
                    .collect()
            }
            Op::Nor => {
                let (a, b) = (read(0)?, read(1)?);
                (0..width as usize)
                    .map(|i| a[i].or(&b[i]).unwrap().not().unwrap())
                    .collect()
            }
            Op::Xnor => {
                let (a, b) = (read(0)?, read(1)?);
                (0..width as usize)
                    .map(|i| self.xor(&a[i], &b[i]).not().unwrap())
                    .collect()
            }

            // ---- Arithmetic (two's-complement, wrap to `width`) ----
            Op::Add => {
                let (a, b) = (read(0)?, read(1)?);
                self.add_bits(&a, &b, self.ff.clone(), width).0
            }
            Op::Sub => {
                // a − b = a + ¬b + 1
                let (a, b) = (read(0)?, read(1)?);
                let not_b: BitVec = (0..width as usize).map(|i| b[i].not().unwrap()).collect();
                self.add_bits(&a, &not_b, self.tt.clone(), width).0
            }
            Op::Inc => {
                let a = read(0)?;
                self.add_bits(&a, &[], self.tt.clone(), width).0
            }
            Op::Dec => {
                // a − 1 = a + (all ones) + 0
                let a = read(0)?;
                let ones: BitVec = (0..width as usize).map(|_| self.tt.clone()).collect();
                self.add_bits(&a, &ones, self.ff.clone(), width).0
            }
            Op::Neg => {
                // −a = ¬a + 1
                let a = read(0)?;
                let not_a: BitVec = (0..width as usize).map(|i| a[i].not().unwrap()).collect();
                self.add_bits(&not_a, &[], self.tt.clone(), width).0
            }

            // ---- Equality / implication (1-bit result) ----
            Op::Eq | Op::Iff => {
                let (a, b) = (read(0)?, read(1)?);
                self.eq_bits(&a, &b)
            }
            Op::Neq => {
                let (a, b) = (read(0)?, read(1)?);
                vec![self.eq_bits(&a, &b)[0].not().unwrap()]
            }
            Op::Implies => {
                // ¬(|a) ∨ (|b)
                let (a, b) = (read(0)?, read(1)?);
                let a_bool = self.or_reduce(&a);
                let b_bool = self.or_reduce(&b);
                vec![a_bool.not().unwrap().or(&b_bool).unwrap()]
            }

            // ---- Unsigned comparisons (1-bit result) ----
            Op::Ugte => {
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.uge(&a, &b, w)]
            }
            Op::Ult => {
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.uge(&a, &b, w).not().unwrap()]
            }
            Op::Ugt => {
                // a > b ⟺ b < a ⟺ ¬(b ≥ a)
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.uge(&b, &a, w).not().unwrap()]
            }
            Op::Ulte => {
                // a ≤ b ⟺ b ≥ a
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.uge(&b, &a, w)]
            }

            // ---- Reductions (1-bit result) ----
            Op::Redor => vec![self.or_reduce(&read(0)?)],
            Op::Redand => vec![self.and_reduce(&read(0)?)],
            Op::Redxor => vec![self.xor_reduce(&read(0)?)],

            // ---- Structural rearrangement ----
            Op::Concat => {
                // {a, b}: b occupies the low bits, a the high bits.
                let (a, b) = (read(0)?, read(1)?);
                let mut out = b;
                out.extend(a);
                out.truncate(width as usize);
                out
            }
            Op::Slice => {
                // immediates = [upper, lower]; out bit i = a[lower + i].
                let a = read(0)?;
                let lower = *immediates.get(1).ok_or("slice missing lower")? as usize;
                (0..width as usize)
                    .map(|i| a.get(lower + i).cloned().unwrap_or_else(|| self.ff.clone()))
                    .collect()
            }
            Op::Uext => {
                // zero-extend to `width`.
                let a = read(0)?;
                (0..width as usize)
                    .map(|i| a.get(i).cloned().unwrap_or_else(|| self.ff.clone()))
                    .collect()
            }
            Op::Sext => {
                // sign-extend to `width` (replicate the MSB of `a`).
                let a = read(0)?;
                let msb = a.last().cloned().unwrap_or_else(|| self.ff.clone());
                (0..width as usize)
                    .map(|i| a.get(i).cloned().unwrap_or_else(|| msb.clone()))
                    .collect()
            }

            // ---- Conditional ----
            Op::Ite => {
                // c ? t : e — c is 1-bit.
                let c = read(0)?;
                let (t, e) = (read(1)?, read(2)?);
                let cond = c[0].clone();
                let ncond = cond.not().unwrap();
                (0..width as usize)
                    .map(|i| {
                        cond.and(&t[i])
                            .unwrap()
                            .or(&ncond.and(&e[i]).unwrap())
                            .unwrap()
                    })
                    .collect()
            }

            // Out of R-F5.3a scope — later slices / never in the FSM+counter core.
            other => {
                return Err(format!(
                    "operator {other:?} not yet supported in the R-F5.3a BDD bit-blaster"
                ));
            }
        };
        Ok(result)
    }

    fn bind(&mut self, nid: Nid, value: Self::Value) {
        self.env.insert(nid, value);
    }

    fn honor_init(&self) -> bool {
        // Register bits stand for their present-state variables, not init
        // values — the walk computes the transition function symbolically.
        false
    }

    fn read_operand(&self, op: Operand) -> Option<Self::Value> {
        self.resolve(op).ok()
    }

    fn uf_substitute(&mut self, _nid: Nid, _width: u32) -> Option<Self::Value> {
        // No uninterpreted-function abstraction in R-F5.3a.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::bit_blast::simulate_one_step;
    use crate::adapter::btor2::parser;

    /// Run the BDD bit-blaster against the concrete simulator over an
    /// exhaustive enumeration of the given (symbol, bit-width) leaves.
    fn assert_matches_simulator(
        src: &str,
        leaves: &[(&str, u32)],
        states: &[&str],
        inputs: &[&str],
    ) {
        let file = parser::parse(src).expect("parse fixture");
        let bb = BddBitBlaster::build(&file).expect("build bit-blaster");

        let total_bits: u32 = leaves.iter().map(|(_, w)| *w).sum();
        assert!(total_bits <= 16, "keep the exhaustive sweep small");

        for combo in 0..(1u128 << total_bits) {
            // Decode `combo` into a per-symbol value assignment.
            let mut assignment: HashMap<String, u128> = HashMap::new();
            let mut offset = 0u32;
            for (sym, w) in leaves {
                let mask = (1u128 << w) - 1;
                assignment.insert((*sym).to_string(), (combo >> offset) & mask);
                offset += w;
            }

            // Symbolic step.
            let symbolic = bb.eval_step(&assignment);

            // Concrete oracle.
            let regs: HashMap<String, u128> = states
                .iter()
                .map(|s| ((*s).to_string(), assignment[*s]))
                .collect();
            let inps: HashMap<String, u128> = inputs
                .iter()
                .map(|s| ((*s).to_string(), assignment[*s]))
                .collect();
            let concrete = simulate_one_step(&file, &regs, &inps).expect("concrete step");

            for st in states {
                assert_eq!(
                    symbolic.get(*st).copied(),
                    concrete.get(*st).copied(),
                    "next-state mismatch for `{st}` at assignment {assignment:?}"
                );
            }
        }
    }

    /// Saturating counter + companion register: exercises `state`, `one`,
    /// `ones`, `add`, `sub`, `eq`, `ult`, `and`, `ite`.
    #[test]
    fn rf5_3a_matches_simulator_saturating_counter() {
        let src = r#"
1 sort bitvec 3
2 sort bitvec 1
3 state 1 cnt
4 state 1 acc
5 input 2 en
6 one 1
7 ones 1
8 add 1 3 6
9 eq 2 3 7
10 ite 1 9 3 8
11 ite 1 5 10 3
12 next 1 3 11
13 sub 1 4 6
14 and 1 4 3
15 ult 2 4 3
16 ite 1 15 13 14
17 next 1 4 16
"#;
        assert_matches_simulator(
            src,
            &[("cnt", 3), ("acc", 3), ("en", 1)],
            &["cnt", "acc"],
            &["en"],
        );
    }

    /// Bit-manipulation heavy: exercises `not`, `inc`, `dec`, `neg`, `slice`,
    /// `uext`, `sext`, `concat`, `xor`, `or`, `redor`, `ite`. The next-state
    /// funnels every wide intermediate back to a 4-bit register, so the
    /// exhaustive check on `r'` transitively validates the whole cone.
    #[test]
    fn rf5_3a_matches_simulator_bit_manipulation() {
        let src = r#"
1 sort bitvec 4
2 sort bitvec 1
3 sort bitvec 2
4 sort bitvec 8
5 state 1 r
6 input 2 sel
7 not 1 5
8 inc 1 5
9 dec 1 5
10 neg 1 5
11 slice 3 5 3 2
12 uext 4 5 4
13 sext 4 5 4
14 concat 4 5 5
15 slice 1 14 7 4
16 xor 1 7 8
17 or 1 9 10
18 redor 2 5
19 ite 1 6 16 17
20 ite 1 18 15 19
21 next 1 5 20
"#;
        assert_matches_simulator(src, &[("r", 4), ("sel", 1)], &["r"], &["sel"]);
    }

    /// Unsigned comparator family + reductions on their own: `ult`, `ulte`,
    /// `ugt`, `ugte`, `neq`, `redand`, `redxor`. The seven 1-bit results are
    /// packed (via correctly-sized `concat`s) into two 3-bit words that an
    /// `ite` funnels back into the register, so the exhaustive check on `x'`
    /// validates every comparator.
    #[test]
    fn rf5_3a_matches_simulator_comparators() {
        let src = r#"
1 sort bitvec 3
2 sort bitvec 1
3 sort bitvec 2
4 state 1 x
5 input 1 y
6 ult 2 4 5
7 ulte 2 4 5
8 ugt 2 4 5
9 ugte 2 4 5
10 neq 2 4 5
11 redand 2 4
12 redxor 2 4
13 concat 3 6 7
14 concat 1 13 8
15 concat 3 9 10
16 concat 1 15 11
17 ite 1 12 14 16
18 next 1 4 17
"#;
        assert_matches_simulator(src, &[("x", 3), ("y", 3)], &["x"], &["y"]);
    }

    // ---- R-F5.3b: predicate BDDs vs `PredicateExpr::eval` ----

    /// Two 3-bit registers `a`, `b` with no transition logic — enough to hang
    /// predicate BDDs on the register-bit vars.
    fn two_register_blaster() -> BddBitBlaster {
        let src = r#"
1 sort bitvec 3
2 state 1 a
3 state 1 b
"#;
        let file = parser::parse(src).expect("parse");
        BddBitBlaster::build(&file).expect("build")
    }

    /// Exhaustively check that `predicate_bdd(expr)`, restricted to every
    /// (a, b) assignment, equals `expr.eval(regs)`.
    fn assert_predicate_matches_eval(bb: &BddBitBlaster, expr: &PredicateExpr) {
        let bdd = bb.predicate_bdd(expr).expect("compile predicate");
        for a in 0..8u128 {
            for b in 0..8u128 {
                let regs: HashMap<String, u128> = [("a".to_string(), a), ("b".to_string(), b)]
                    .into_iter()
                    .collect();
                assert_eq!(
                    bb.eval_predicate_at(&bdd, &regs),
                    expr.eval(&regs),
                    "predicate {expr:?} disagrees with eval at a={a} b={b}"
                );
            }
        }
    }

    #[test]
    fn rf5_3b_predicate_bdd_matches_eval_comparisons() {
        let bb = two_register_blaster();
        for op in [
            CmpOp::Eq,
            CmpOp::Ne,
            CmpOp::Lt,
            CmpOp::Le,
            CmpOp::Gt,
            CmpOp::Ge,
        ] {
            for value in 0..9u64 {
                // value 0..8 in range; value 8 exceeds the 3-bit register width.
                assert_predicate_matches_eval(
                    &bb,
                    &PredicateExpr::Cmp {
                        register: "a".to_string(),
                        op,
                        value,
                    },
                );
            }
        }
    }

    #[test]
    fn rf5_3b_predicate_bdd_matches_eval_literal_exceeds_width() {
        // A literal wider than the register: `a == 9` is always false, `a < 9`
        // always true — the zero-extend-to-common-width path must reproduce
        // `eval`'s unmasked u128 comparison.
        let bb = two_register_blaster();
        for (op, value) in [
            (CmpOp::Eq, 9u64),
            (CmpOp::Lt, 9),
            (CmpOp::Ge, 8),
            (CmpOp::Gt, 7),
        ] {
            assert_predicate_matches_eval(
                &bb,
                &PredicateExpr::Cmp {
                    register: "a".to_string(),
                    op,
                    value,
                },
            );
        }
    }

    #[test]
    fn rf5_3b_predicate_bdd_matches_eval_register_relations() {
        let bb = two_register_blaster();
        for op in [
            CmpOp::Eq,
            CmpOp::Ne,
            CmpOp::Lt,
            CmpOp::Le,
            CmpOp::Gt,
            CmpOp::Ge,
        ] {
            assert_predicate_matches_eval(
                &bb,
                &PredicateExpr::CmpReg {
                    lhs: "a".to_string(),
                    op,
                    rhs: "b".to_string(),
                },
            );
        }
    }

    #[test]
    fn rf5_3b_predicate_bdd_matches_eval_addend_wraps() {
        // `a == (b + k) mod 8` for every addend k — exercises the wrap boundary.
        let bb = two_register_blaster();
        for addend in 0..8u64 {
            assert_predicate_matches_eval(
                &bb,
                &PredicateExpr::CmpRegAddend {
                    lhs: "a".to_string(),
                    op: CmpOp::Eq,
                    rhs: "b".to_string(),
                    addend,
                    width: 3,
                },
            );
        }
        // A relational inequality with an addend too.
        assert_predicate_matches_eval(
            &bb,
            &PredicateExpr::CmpRegAddend {
                lhs: "a".to_string(),
                op: CmpOp::Ge,
                rhs: "b".to_string(),
                addend: 2,
                width: 3,
            },
        );
    }

    #[test]
    fn rf5_3b_predicate_bdd_matches_eval_boolean_combinators() {
        let bb = two_register_blaster();
        let a_lt_b = PredicateExpr::CmpReg {
            lhs: "a".to_string(),
            op: CmpOp::Lt,
            rhs: "b".to_string(),
        };
        let a_eq_3 = PredicateExpr::eq("a", 3);
        let b_ge_5 = PredicateExpr::Cmp {
            register: "b".to_string(),
            op: CmpOp::Ge,
            value: 5,
        };
        // (a < b ∧ a == 3) ∨ ¬(b ≥ 5)
        let compound = PredicateExpr::Or(
            Box::new(PredicateExpr::And(Box::new(a_lt_b), Box::new(a_eq_3))),
            Box::new(PredicateExpr::Not(Box::new(b_ge_5))),
        );
        assert_predicate_matches_eval(&bb, &compound);
    }

    // ---- R-F5.3c: abstract may relation vs brute-force concrete may-edges ----

    /// Build the symbolic abstract may relation and assert it equals the
    /// brute-force concrete may-relation: for every present cube `ci` and next
    /// cube `cj`, `ci → cj` is symbolically a may-edge iff *some* concrete
    /// state in `ci` steps (under *some* input) to a concrete state in `cj`.
    /// The brute force enumerates every (register, input) assignment and runs
    /// the concrete `simulate_one_step` — fully independent of the symbolic path.
    fn assert_may_relation_matches_bruteforce(
        src: &str,
        predicates: &[PredicateExpr],
        states: &[(&str, u32)],
        inputs: &[(&str, u32)],
    ) {
        let file = parser::parse(src).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let rel = bb
            .abstract_may_relation(predicates)
            .expect("build may relation");
        let k = predicates.len();

        let cube_of = |regs: &HashMap<String, u128>| -> usize {
            let mut c = 0usize;
            for (i, p) in predicates.iter().enumerate() {
                if p.eval(regs) {
                    c |= 1 << i;
                }
            }
            c
        };

        let total_state_bits: u32 = states.iter().map(|(_, w)| *w).sum();
        let total_input_bits: u32 = inputs.iter().map(|(_, w)| *w).sum();
        assert!(
            total_state_bits + total_input_bits <= 14,
            "keep the sweep small"
        );

        let mut expected: std::collections::HashSet<(usize, usize)> = Default::default();
        for scombo in 0..(1u128 << total_state_bits) {
            let mut regs: HashMap<String, u128> = HashMap::new();
            let mut off = 0u32;
            for (name, w) in states {
                let mask = (1u128 << w) - 1;
                regs.insert((*name).to_string(), (scombo >> off) & mask);
                off += w;
            }
            let present_cube = cube_of(&regs);
            for icombo in 0..(1u128 << total_input_bits) {
                let mut inps: HashMap<String, u128> = HashMap::new();
                let mut ioff = 0u32;
                for (name, w) in inputs {
                    let mask = (1u128 << w) - 1;
                    inps.insert((*name).to_string(), (icombo >> ioff) & mask);
                    ioff += w;
                }
                let next = simulate_one_step(&file, &regs, &inps).expect("concrete step");
                // Held registers keep their current value (BTOR2 convention).
                let mut regs_next = regs.clone();
                regs_next.extend(next);
                expected.insert((present_cube, cube_of(&regs_next)));
            }
        }

        for ci in 0..(1usize << k) {
            for cj in 0..(1usize << k) {
                assert_eq!(
                    rel.may_holds(ci, cj),
                    expected.contains(&(ci, cj)),
                    "may-edge {ci} -> {cj} disagrees with brute force"
                );
            }
        }
    }

    /// 2-bit saturating counter with an enable, predicates `{cnt == 0, cnt ≥ 2}`.
    /// Exercises the single-register substitution path.
    #[test]
    fn rf5_3c_may_relation_saturating_counter() {
        let src = r#"
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
        let predicates = [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ];
        assert_may_relation_matches_bruteforce(src, &predicates, &[("cnt", 2)], &[("en", 1)]);
    }

    /// Two 2-bit registers stepping together, with a **relational** predicate
    /// `cnt == acc` plus `cnt ≥ 2` and `acc == 0`. Exercises substitution over
    /// two registers that each carry a next-state function, and a `CmpReg`.
    #[test]
    fn rf5_3c_may_relation_two_registers_relational_predicate() {
        // cnt' = (cnt + 1) mod 4 ; acc' = en ? cnt : acc
        let src = r#"
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 state 1 acc
5 input 2 en
6 one 1
7 add 1 3 6
8 ite 1 5 3 4
9 next 1 3 7
10 next 1 4 8
"#;
        let predicates = [
            PredicateExpr::CmpReg {
                lhs: "cnt".to_string(),
                op: CmpOp::Eq,
                rhs: "acc".to_string(),
            },
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
            PredicateExpr::eq("acc", 0),
        ];
        assert_may_relation_matches_bruteforce(
            src,
            &predicates,
            &[("cnt", 2), ("acc", 2)],
            &[("en", 1)],
        );
    }

    // ---- R-F5.3d: abstract must relation vs brute-force concrete must-edges ----

    /// Build the symbolic abstract must relation under `semantics` and assert it
    /// equals the brute-force concrete must-relation. `∀∃`: `ci → cj` is a
    /// must-edge iff *every* concrete state in `ci` has *some* input landing in
    /// `cj`. `∀∀`: every state, under *every* input, lands in `cj`. An empty
    /// (unsatisfiable) source cube is vacuously a must-edge to every cube — the
    /// symbolic `∀` and the brute-force `.all()` over zero states agree.
    fn assert_must_relation_matches_bruteforce(
        src: &str,
        predicates: &[PredicateExpr],
        states: &[(&str, u32)],
        inputs: &[(&str, u32)],
        semantics: MustSemantics,
    ) {
        let file = parser::parse(src).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let rel = bb
            .abstract_relation(predicates, Some(semantics))
            .expect("build must relation");
        let k = predicates.len();

        let cube_of = |regs: &HashMap<String, u128>| -> usize {
            let mut c = 0usize;
            for (i, p) in predicates.iter().enumerate() {
                if p.eval(regs) {
                    c |= 1 << i;
                }
            }
            c
        };

        let total_state_bits: u32 = states.iter().map(|(_, w)| *w).sum();
        let total_input_bits: u32 = inputs.iter().map(|(_, w)| *w).sum();
        assert!(
            total_state_bits + total_input_bits <= 14,
            "keep the sweep small"
        );

        // Per concrete state: the set of next-cubes reachable over all inputs,
        // grouped by the state's present cube.
        let mut by_cube: HashMap<usize, Vec<std::collections::HashSet<usize>>> = HashMap::new();
        for scombo in 0..(1u128 << total_state_bits) {
            let mut regs: HashMap<String, u128> = HashMap::new();
            let mut off = 0u32;
            for (name, w) in states {
                let mask = (1u128 << w) - 1;
                regs.insert((*name).to_string(), (scombo >> off) & mask);
                off += w;
            }
            let present_cube = cube_of(&regs);
            let mut reach: std::collections::HashSet<usize> = Default::default();
            for icombo in 0..(1u128 << total_input_bits) {
                let mut inps: HashMap<String, u128> = HashMap::new();
                let mut ioff = 0u32;
                for (name, w) in inputs {
                    let mask = (1u128 << w) - 1;
                    inps.insert((*name).to_string(), (icombo >> ioff) & mask);
                    ioff += w;
                }
                let next = simulate_one_step(&file, &regs, &inps).expect("concrete step");
                let mut regs_next = regs.clone();
                regs_next.extend(next);
                reach.insert(cube_of(&regs_next));
            }
            by_cube.entry(present_cube).or_default().push(reach);
        }

        for ci in 0..(1usize << k) {
            for cj in 0..(1usize << k) {
                // An infeasible source cube (no concrete state maps to it) has
                // NO must-edge — the relation restricts must-edges to feasible
                // sources so `R_must ⊆ R_may` holds globally. A feasible cube
                // uses the ∀∃ / ∀∀ definition over its inhabitants.
                let expected = match by_cube.get(&ci) {
                    None => false,
                    Some(reaches) => match semantics {
                        MustSemantics::ForallExists => reaches.iter().all(|r| r.contains(&cj)),
                        MustSemantics::ForallForall => {
                            reaches.iter().all(|r| r.len() == 1 && r.contains(&cj))
                        }
                    },
                };
                assert_eq!(
                    rel.must_holds(ci, cj),
                    expected,
                    "must-edge {ci} -> {cj} ({semantics:?}) disagrees with brute force"
                );
            }
        }
    }

    const SATURATING_COUNTER_BTOR2: &str = r#"
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

    const TWO_REGISTER_BTOR2: &str = r#"
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 state 1 acc
5 input 2 en
6 one 1
7 add 1 3 6
8 ite 1 5 3 4
9 next 1 3 7
10 next 1 4 8
"#;

    #[test]
    fn rf5_3d_must_relation_saturating_counter() {
        let predicates = [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ];
        for sem in [MustSemantics::ForallExists, MustSemantics::ForallForall] {
            assert_must_relation_matches_bruteforce(
                SATURATING_COUNTER_BTOR2,
                &predicates,
                &[("cnt", 2)],
                &[("en", 1)],
                sem,
            );
        }
    }

    #[test]
    fn rf5_3d_must_relation_two_registers_relational_predicate() {
        let predicates = [
            PredicateExpr::CmpReg {
                lhs: "cnt".to_string(),
                op: CmpOp::Eq,
                rhs: "acc".to_string(),
            },
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
            PredicateExpr::eq("acc", 0),
        ];
        for sem in [MustSemantics::ForallExists, MustSemantics::ForallForall] {
            assert_must_relation_matches_bruteforce(
                TWO_REGISTER_BTOR2,
                &predicates,
                &[("cnt", 2), ("acc", 2)],
                &[("en", 1)],
                sem,
            );
        }
    }

    /// The KMTS invariant `R_must ⊆ R_may` over **feasible** source cubes: every
    /// must-edge out of a satisfiable cube is a may-edge (for both semantics).
    /// An infeasible (unsatisfiable) cube is excluded — it is vacuously a
    /// must-edge to every cube yet has no may-edge, and the downstream lift
    /// never materialises such a cube as an abstract state.
    #[test]
    fn rf5_3d_must_subseteq_may_over_feasible_cubes() {
        let predicates = [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ];
        let file = parser::parse(SATURATING_COUNTER_BTOR2).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        for sem in [MustSemantics::ForallExists, MustSemantics::ForallForall] {
            let rel = bb
                .abstract_relation(&predicates, Some(sem))
                .expect("relation");
            let k = rel.num_predicates();
            for ci in 0..(1usize << k) {
                // A feasible cube has at least one may-out-edge (every concrete
                // state steps somewhere); an infeasible cube has none.
                let feasible = (0..(1usize << k)).any(|cj| rel.may_holds(ci, cj));
                if !feasible {
                    continue;
                }
                for cj in 0..(1usize << k) {
                    if rel.must_holds(ci, cj) {
                        assert!(
                            rel.may_holds(ci, cj),
                            "must ⊄ may: {ci} -> {cj} ({sem:?}) is a must-edge but not a may-edge"
                        );
                    }
                }
            }
        }
    }

    // ---- R-F5.4.1: symbolic cube evaluator ≡ evaluate_tri ----

    /// Build the abstract relation, materialise the equivalent explicit
    /// cube-KMTS (feasible cubes as states, may/must edges + definite cube AP
    /// labels), and assert the symbolic evaluator agrees with `evaluate_tri`
    /// on every `formula_str` at every feasible cube.
    fn assert_symbolic_eval_matches_tri(
        src: &str,
        predicates: &[PredicateExpr],
        names: &[&str],
        states: &[(&str, u32)],
        formulas: &[&str],
    ) {
        use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, TransitionModality, Tristate};
        use crate::mu_calculus::evaluator::{Environment, evaluate_tri};

        let file = parser::parse(src).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let rel = bb
            .abstract_relation(predicates, Some(MustSemantics::ForallExists))
            .expect("relation");

        let cube_of = |regs: &HashMap<String, u128>| -> usize {
            let mut c = 0usize;
            for (i, p) in predicates.iter().enumerate() {
                if p.eval(regs) {
                    c |= 1 << i;
                }
            }
            c
        };

        // Feasible cubes: those inhabited by at least one concrete register state.
        let total_state_bits: u32 = states.iter().map(|(_, w)| *w).sum();
        let mut feasible_set: std::collections::BTreeSet<usize> = Default::default();
        for scombo in 0..(1u128 << total_state_bits) {
            let mut regs: HashMap<String, u128> = HashMap::new();
            let mut off = 0u32;
            for (name, w) in states {
                let mask = (1u128 << w) - 1;
                regs.insert((*name).to_string(), (scombo >> off) & mask);
                off += w;
            }
            feasible_set.insert(cube_of(&regs));
        }
        let feasible: Vec<usize> = feasible_set.into_iter().collect();
        let n = feasible.len();

        // Explicit cube-KMTS over the feasible cubes.
        let mut b = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        for j in 0..n {
            b.state(format!("c{j}"));
        }
        b.initial("c0");
        let step = b.labels().intern(["step"]).unwrap();
        let ids: Vec<_> = (0..n)
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
        for (j, &ci) in feasible.iter().enumerate() {
            for (l, &cj) in feasible.iter().enumerate() {
                if rel.must_holds(ci, cj) {
                    b.transition_ids_with_modality(
                        ids[j],
                        &[step],
                        ids[l],
                        TransitionModality::Sharp,
                    );
                } else if rel.may_holds(ci, cj) {
                    b.transition_ids_with_modality(
                        ids[j],
                        &[step],
                        ids[l],
                        TransitionModality::MayOnly,
                    );
                }
            }
        }
        let clts = b.build().expect("clts builds");

        let names_owned: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let env = Environment::new(clts.state_count());
        for formula_str in formulas {
            let formula = crate::mu_calculus::parser::parse(formula_str).expect("formula parses");
            let tri = evaluate_tri(&formula, &clts, &env).expect("evaluate_tri");
            let sym = rel
                .evaluate(&formula, &names_owned)
                .expect("symbolic evaluate");
            for (j, &ci) in feasible.iter().enumerate() {
                assert_eq!(
                    rel.verdict_at(&sym, ci),
                    tri.verdict_at(j),
                    "cube {ci} (state c{j}), formula `{formula_str}`"
                );
            }
        }
    }

    /// The full unguarded fragment (bare `[]`/`<>`, boolean ops, nested νμ) on
    /// the saturating counter, predicates `p = (cnt == 0)`, `q = (cnt ≥ 2)`.
    #[test]
    fn rf5_4_1_symbolic_eval_matches_tri_saturating_counter() {
        let predicates = [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ];
        let formulas = [
            "[] p",
            "<> p",
            "[] q",
            "<> q",
            "not q",
            "p or q",
            "p and q",
            "nu X. p and [] X",                   // AG p
            "mu X. q or <> X",                    // EF q
            "nu X. (not p) and [] X",             // AG ¬p
            "nu Y. (mu X. (q or <> X)) and [] Y", // AG EF q (nested νμ)
        ];
        assert_symbolic_eval_matches_tri(
            SATURATING_COUNTER_BTOR2,
            &predicates,
            &["p", "q"],
            &[("cnt", 2)],
            &formulas,
        );
    }

    /// Two registers stepping together, with a relational predicate — exercises
    /// a KMTS with `MayOnly` edges (input nondeterminism) so the `⊥` verdicts
    /// are non-trivial.
    #[test]
    fn rf5_4_1_symbolic_eval_matches_tri_two_registers() {
        let predicates = [
            PredicateExpr::CmpReg {
                lhs: "cnt".to_string(),
                op: CmpOp::Eq,
                rhs: "acc".to_string(),
            },
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
            PredicateExpr::eq("acc", 0),
        ];
        let formulas = [
            "[] p",
            "<> p",
            "<> (p and q)",
            "nu X. p and [] X",
            "mu X. r or <> X",
            "nu Y. (mu X. (p or <> X)) and [] Y",
        ];
        assert_symbolic_eval_matches_tri(
            TWO_REGISTER_BTOR2,
            &predicates,
            &["p", "q", "r"],
            &[("cnt", 2), ("acc", 2)],
            &formulas,
        );
    }
}
