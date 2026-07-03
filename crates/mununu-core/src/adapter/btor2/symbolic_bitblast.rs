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
//! the conjunction of the `⟦P_i⟧` / `¬⟦P_i⟧`. R-F5.3c then builds the abstract
//! may/must relation from these + the bit-blasted next-state functions by
//! existentially quantifying the register bits.

use std::collections::HashMap;

use oxidd::bdd::{self, BDDFunction, BDDManagerRef};
use oxidd::{BooleanFunction, Manager, ManagerRef, VarNo};

use crate::adapter::btor2::ast::{Btor2File, ConstValue, Nid, Node, Op, Operand};
use crate::adapter::btor2::bit_blast::eval_const_value;
use crate::adapter::btor2::parser::bv_width;
use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr};
use crate::adapter::btor2::term_backend::{BvTermBackend, WalkError, walk_design};

/// A bit-vector of BDDs, LSB-first: index `b` is the BDD for bit `b`. Every
/// node's value carries exactly `width` bits.
type BitVec = Vec<BDDFunction>;

/// A register or input cell: its BTOR2 NID + symbol + the fresh BDD variables
/// standing for its bits (LSB-first). The bit-blaster restricts these to
/// concrete values for the differential.
struct Cell {
    symbol: String,
    vars: BitVec,
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
        let mut leaf_specs: Vec<(Nid, String, u32)> = Vec::new();
        for line in &file.lines {
            let (sort, symbol, tag) = match &line.node {
                Node::State { sort, symbol } => (*sort, symbol.clone(), "state"),
                Node::Input { sort, symbol } => (*sort, symbol.clone(), "input"),
                _ => continue,
            };
            let width = bv_width(file, sort)
                .ok_or_else(|| format!("NID {}: {tag} has non-bitvec sort", line.nid))?;
            let symbol = symbol.unwrap_or_else(|| format!("{tag}_{}", line.nid));
            leaf_specs.push((line.nid, symbol, width));
        }

        let total_bits: u32 = leaf_specs.iter().map(|(_, _, w)| *w).sum();

        // Allocate the manager + one BDD variable per leaf bit.
        let manager = bdd::new_manager(1 << 16, 1 << 16, 1);
        let (all_vars, tt, ff) = manager.with_manager_exclusive(|m| {
            let range = m.add_vars(total_bits as VarNo);
            let vars: Vec<BDDFunction> = (0..total_bits)
                .map(|i| BDDFunction::var(m, range.start + i as VarNo).unwrap())
                .collect();
            (vars, BDDFunction::t(m), BDDFunction::f(m))
        });

        // Slice the flat variable pool out per cell, LSB-first.
        let mut env: HashMap<Nid, BitVec> = HashMap::new();
        let mut cells: Vec<Cell> = Vec::new();
        let mut cursor = 0usize;
        for (nid, symbol, width) in leaf_specs {
            let vars: BitVec = all_vars[cursor..cursor + width as usize].to_vec();
            cursor += width as usize;
            env.insert(nid, vars.clone());
            cells.push(Cell { symbol, vars });
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
}
