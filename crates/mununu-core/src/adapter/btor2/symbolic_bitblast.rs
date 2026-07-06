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
//! present predicate var `p_i`. Supports the audited-sound fragment
//! (`True`/`False`, predicates, `!`/`&&`/`||`, `mu`/`nu`, bare `[]`/`<>`, and —
//! R-F5.5c — `req_cur`/`forb_cur`/`req_next`/`forb_next` state-predicate-guarded
//! modalities); label / controllability / step-bounded guards remain an honest
//! error (out-of-fragment over a predicate cube per
//! [`crate::mu_calculus::cube_modality_soundness_warnings`]). Validated
//! cell-for-cell against `evaluate_tri` on the equivalent explicit cube-KMTS
//! (nested νμ + guarded modalities included). R-F5.4.2 then wires this behind
//! `--engine symbolic`.

use std::collections::{BTreeMap, HashMap};

use oxidd::bdd::{self, BDDFunction, BDDManagerRef};
use oxidd::{BooleanFunction, FunctionSubst, Manager, ManagerRef, Subst, VarNo};

use crate::adapter::btor2::ast::{Btor2File, ConstValue, Nid, Node, Op, Operand};
use crate::adapter::btor2::bit_blast::eval_const_value;
use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr, parse_predicate_expr};
use crate::adapter::btor2::term_backend::{BvTermBackend, WalkError, walk_design};
// R-F5.4.1 — the symbolic mu-calculus evaluator reuses the (must, may) `TritBdd`
// pair + the crate `Trit` verdict; the mu-calculus `Node` is aliased to avoid a
// clash with the BTOR2 `Node` above.
use crate::mu_calculus::symbolic::TritBdd;
use crate::mu_calculus::trit::Trit;
use crate::mu_calculus::{Control, Formula, FormulaVarId, Guard, ModalKind, Node as MuNode};

/// Bit-count cap for the shared [`BddBitBlaster`]: a design whose cone exceeds this many
/// register+input bits is rejected before any BDD is built, so a caller (`sv verify-auto`)
/// degrades to a `Skipped` property rather than OoM. **Calibrated empirically at 40** — a cone
/// even a few bits wider can blow the BDD arena *during* `walk_design` and panic on a downstream
/// `unwrap`. Measured 2026-07-06: raising it to 56 made `prim_esc_receiver` (47-b cone) OoM-panic
/// mid-build instead of decide — its cone is NOT a compact FSM. Designs past the cap are covered by
/// the portfolio's other engines (the exact engine's cone for the same atom is often narrower — it
/// seeds only the formula atoms, not the wider auto-seeded cube-predicate set the symbolic engine
/// needs — so it decides where the cube engine cannot). Do not raise this without a `walk_design`-
/// internal node-budget guard: a post-walk node check cannot catch a mid-walk arena overflow.
const MAX_BITBLAST_BITS: u32 = 40;

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
    /// Named COMBINATIONAL signal (module output / named wire) → its BDD vector over the
    /// leaf vars, from `walk_design`. Lets a predicate bind to a combinational output
    /// (`depth_o`, `gnt_o`, …) that is not a state register — the atom's cone still seeds
    /// correctly (`resolve_atom_to_terminals` walks the output to its state/input terminals).
    named_signals: HashMap<String, BitVec>,
}

impl BddBitBlaster {
    /// Bit-blast `file`: allocate one BDD variable per register/input bit,
    /// bind the leaves, walk every `Const`/`Op` node into a BDD vector, and
    /// collect the per-register next-state functions.
    pub fn build(file: &Btor2File) -> Result<Self, String> {
        Self::build_with_keep(file, None)
    }

    /// R-F5.6 — build the exact bit-blaster, optionally restricted to a cone-of-influence
    /// KEEP-SET of leaf NIDs. A leaf (register/input) whose NID is NOT in `keep` is PINNED to
    /// a constant 0: it uses ZERO BDD variables, never appears in the state/input frame, and
    /// (for a register) never transitions — so `total_bits` (the bit cap) counts only the
    /// property's cone. `keep = None` bit-blasts the full design (the pre-R-F5.6 behaviour).
    ///
    /// SOUNDNESS: an out-of-cone leaf cannot influence any atom (the cone is closed under the
    /// data-flow dependency relation AND `constraint`/`fair`/`justice` coupling — see
    /// [`crate::adapter::btor2::dep_graph::cone_leaf_nids`]), so pinning it to any fixed value
    /// is verdict-preserving for the FULL mu-calculus over the atoms — an EXACT reduction, not
    /// an over/under-approximation. The dep graph over-approximates influence (spurious edges
    /// keep MORE), so the cone is a superset of the true cone: it can never drop a relevant
    /// leaf. `keep = None` is unchanged from before.
    pub fn build_with_keep(
        file: &Btor2File,
        keep: Option<&std::collections::HashSet<Nid>>,
    ) -> Result<Self, String> {
        // AR-S1 / D1.4 — the ONE canonical leaf enumeration + naming lives on the
        // STS-IR seam (`BtorSts::leaf_cells`). It resolves each state/input to its
        // user-visible name via `collect_symbols` (walking Yosys's `uext _ NID 0 NAME`
        // alias ops back to the underlying, often unnamed, state cell) through the
        // single `parser::resolve_leaf_symbol`. Without this the flattened yosys BTOR2
        // leaves registers unnamed (`state_<nid>`), so a predicate over a real register
        // name (`bit_cnt_q`) never binds; for a directly-named cell the resolved name
        // equals the raw symbol, so it is a no-op there. The bit-blaster no longer
        // re-walks `file.lines` or re-implements naming — its enumeration and the
        // seam's cannot drift (the #242 class), and Pass 3's `next_funcs` keying below
        // reuses these same names.
        let leaf_cells = crate::adapter::sts_ir::BtorSts::new(file).leaf_cells()?;
        // Pass 1 — register + input cells with their widths, in file order.
        let leaf_specs: Vec<(Nid, String, bool, u32)> = leaf_cells
            .iter()
            .map(|c| (c.nid, c.name.clone(), c.is_state, c.width))
            .collect();

        // R-F5.6 — a leaf is KEPT (a BDD variable) iff no keep-set is given, or its NID is in
        // the property cone. Out-of-cone leaves are pinned to constant 0 (zero variables).
        let is_kept = |nid: Nid| keep.is_none_or(|k| k.contains(&nid));

        // The bit cap now counts only the KEPT (cone) bits.
        let total_bits: u32 = leaf_specs
            .iter()
            .filter(|(nid, _, _, _)| is_kept(*nid))
            .map(|(_, _, _, w)| *w)
            .sum();

        // R-F5.6 guard — the bit-blaster builds BDDs over every KEPT register+input bit. With
        // the cone-of-influence keep-set the frame is the property's cone; without one it is
        // the whole design. On a real design whose CONE is still wide (hundreds of bits) the
        // BDD manager would OoM, so bail with a clean error above a conservative cap and let a
        // caller (`sv verify-auto`) degrade to a `Skipped` property rather than panic.
        if total_bits > MAX_BITBLAST_BITS {
            return Err(format!(
                "symbolic bit-blaster: design has {total_bits} register+input bits \
                 (> {MAX_BITBLAST_BITS}) after cone-of-influence restriction (R-F5.6) — the \
                 property's cone is too wide to bit-blast; use `--engine explicit`"
            ));
        }

        // Allocate the manager + one BDD variable per KEPT leaf bit. 2M inner nodes
        // (~32 MB arena, index manager) + a 512K apply cache — comfortably above
        // the toy fixtures' need and sized for a moderate (≤ `MAX_BITBLAST_BITS`)
        // cone; the manager is dropped per `BddBitBlaster`, so at most one
        // arena is live at a time.
        let manager = bdd::new_manager(1 << 21, 1 << 19, 1);
        let (all_vars, var_base, tt, ff) = manager.with_manager_exclusive(|m| {
            let range = m.add_vars(total_bits as VarNo);
            let vars: Vec<BDDFunction> = (0..total_bits)
                .map(|i| BDDFunction::var(m, range.start + i as VarNo).unwrap())
                .collect();
            (vars, range.start, BDDFunction::t(m), BDDFunction::f(m))
        });

        // Slice the flat variable pool out per KEPT cell (LSB-first); PIN each out-of-cone
        // leaf to a constant-0 BitVec (env only — never a `Cell`, so it is invisible to the
        // init BDD, the input cube, and the next-state substitution).
        let mut env: HashMap<Nid, BitVec> = HashMap::new();
        let mut cells: Vec<Cell> = Vec::new();
        let mut cursor = 0usize;
        for (nid, symbol, is_state, width) in leaf_specs {
            if is_kept(nid) {
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
            } else {
                // Pinned: constant 0, zero BDD variables. `walk_design` resolves the leaf to
                // this constant; downstream ops over it fold to constants (kept BDDs stay in
                // the cone frame). Not pushed to `cells`.
                env.insert(nid, vec![ff.clone(); width as usize]);
            }
        }

        let mut blaster = BddBitBlaster {
            _manager: manager,
            tt,
            ff,
            env,
            cells,
            next_funcs: HashMap::new(),
            named_signals: HashMap::new(),
        };

        // Pass 2 — walk the node DAG, evaluating every Const/Op into a BitVec.
        walk_design(file, &mut blaster).map_err(|e| match e {
            WalkError::NonBitvecSort(nid) => {
                format!("NID {nid}: non-bitvec sort in the transition cone")
            }
            WalkError::Unevaluated(nid) => format!("NID {nid}: operand unevaluated"),
            WalkError::Backend(msg) => msg,
        })?;

        // Pass 3 — collect the per-register next-state functions, keyed by the state's
        // symbol. AR-S1 / #242 drift guard: these names come from the SAME `leaf_cells`
        // enumeration as Pass 1, so `next_funcs.get(&cell.symbol)` in `exact_model()`
        // cannot miss and silently freeze a register at its init value (the #242
        // soundness bug: a false `EF`-VIOLATED / vacuous `AG AF`-HOLDS). Pinned
        // (out-of-cone) registers are SKIPPED — they are constants and must not transition.
        let nid_symbol: HashMap<Nid, String> = leaf_cells
            .iter()
            .filter(|c| c.is_state)
            .map(|c| (c.nid, c.name.clone()))
            .collect();
        for line in &file.lines {
            if let Node::Next { state, value, .. } = &line.node {
                if !is_kept(*state) {
                    continue; // pinned register — stays constant, no next-state function
                }
                let Some(sym) = nid_symbol.get(state) else {
                    continue;
                };
                let next = blaster.resolve(*value)?;
                blaster.next_funcs.insert(sym.clone(), next);
            }
        }

        // Pass 4 — named combinational signals (module outputs / named wires), so a predicate
        // can bind to a non-register signal (`depth_o`, `gnt_o`). Its BDD is already in `env`
        // (walk_design); the atom's cone still seeds via the output's terminal fan-in.
        for line in &file.lines {
            match &line.node {
                Node::Output {
                    symbol: Some(s),
                    signal,
                } => {
                    if let Some(bits) = blaster.env.get(&signal.nid()) {
                        blaster
                            .named_signals
                            .entry(s.clone())
                            .or_insert_with(|| bits.clone());
                    }
                }
                Node::Op {
                    symbol: Some(s), ..
                } => {
                    if let Some(bits) = blaster.env.get(&line.nid) {
                        blaster
                            .named_signals
                            .entry(s.clone())
                            .or_insert_with(|| bits.clone());
                    }
                }
                _ => {}
            }
        }

        Ok(blaster)
    }

    /// The BDD bit-vector for a predicate operand: a state/input register ([`register_bits`])
    /// or, failing that, a named combinational signal (module output / wire). `None` if neither
    /// names it.
    fn signal_bits(&self, name: &str) -> Option<&BitVec> {
        self.register_bits(name)
            .or_else(|| self.named_signals.get(name))
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

    /// Barrel shifter: shift `a` by the VARIABLE amount `s` (a `width`-bit BitVec).
    /// `left` ⇒ shift left (fill 0); else shift right, filling with 0 (logical) or the sign bit
    /// `a[width-1]` (`arith`). log-depth: stage `k` conditionally shifts by `2^k` on `s[k]`.
    /// A shift ≥ width falls out to the fill (the `sh < w`/`j+sh` bounds handle it).
    fn barrel_shift(
        &self,
        a: &[BDDFunction],
        s: &[BDDFunction],
        width: u32,
        left: bool,
        arith: bool,
    ) -> BitVec {
        let w = width as usize;
        let sign = if arith {
            a.get(w.wrapping_sub(1))
                .cloned()
                .unwrap_or_else(|| self.ff.clone())
        } else {
            self.ff.clone()
        };
        let mut cur: BitVec = (0..w)
            .map(|i| a.get(i).cloned().unwrap_or_else(|| self.ff.clone()))
            .collect();
        for (k, sk) in s.iter().enumerate() {
            let sh = 1usize.checked_shl(k as u32).unwrap_or(usize::MAX); // 2^k, saturating
            let nsk = sk.not().unwrap();
            cur = (0..w)
                .map(|j| {
                    // The bit selected when s[k] is set (shift by 2^k this stage).
                    let shifted = if left {
                        // left: result[j] = (j ≥ 2^k) ? cur[j − 2^k] : 0
                        if sh <= j {
                            cur[j - sh].clone()
                        } else {
                            self.ff.clone()
                        }
                    } else {
                        // right: result[j] = (j + 2^k < w) ? cur[j + 2^k] : fill
                        if sh < w && j + sh < w {
                            cur[j + sh].clone()
                        } else {
                            sign.clone()
                        }
                    };
                    // result[j] = s[k] ? shifted : cur[j]
                    sk.and(&shifted)
                        .unwrap()
                        .or(&nsk.and(&cur[j]).unwrap())
                        .unwrap()
                })
                .collect();
        }
        cur
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

    /// Signed (two's-complement) `a < b`: if the signs differ, the negative operand (MSB set)
    /// is the smaller; if the signs match, unsigned `<` decides. `slt = (a_msb ⊕ b_msb) ? a_msb
    /// : (a <u b)`, with `a <u b = ¬(a ≥u b)`.
    fn slt(&self, a: &[BDDFunction], b: &[BDDFunction], width: u32) -> BDDFunction {
        let w = width as usize;
        let amsb = a.get(w - 1).cloned().unwrap_or_else(|| self.ff.clone());
        let bmsb = b.get(w - 1).cloned().unwrap_or_else(|| self.ff.clone());
        let signs_differ = self.xor(&amsb, &bmsb);
        let ult = self.uge(a, b, width).not().unwrap(); // a <u b
        // signs_differ ? amsb : ult
        signs_differ
            .and(&amsb)
            .unwrap()
            .or(&signs_differ.not().unwrap().and(&ult).unwrap())
            .unwrap()
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
        let unknown = |r: &str| format!("predicate references unknown register/signal `{r}`");
        match expr {
            PredicateExpr::Cmp {
                register,
                op,
                value,
            } => {
                let reg = self
                    .signal_bits(register)
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
                let l = self.signal_bits(lhs).ok_or_else(|| unknown(lhs))?;
                let r = self.signal_bits(rhs).ok_or_else(|| unknown(rhs))?;
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
                let l = self.signal_bits(lhs).ok_or_else(|| unknown(lhs))?;
                let r = self.signal_bits(rhs).ok_or_else(|| unknown(rhs))?;
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

        // The FEASIBLE present cubes `∃x. A(x,p)` — the cubes some concrete
        // register state inhabits. Used to restrict `R_must` (below) and to
        // scope the verdict tally (R-F5.4.2) to materialisable abstract states.
        let feasible_present = a.exists(&reg_cube).unwrap();

        // R_must, when requested. `A → φ` is `¬A ∨ φ`.
        //
        // A vacuously-empty (unsatisfiable) source cube yields `¬A ≡ ⊤` there,
        // so the ∀ would make it a must-edge to *every* cube — breaking the KMTS
        // invariant `R_must ⊆ R_may` (an empty cube has no may-edge). We
        // intersect with `feasible_present`: must-edges only leave inhabited
        // abstract states (the lift never materialises an infeasible cube), so
        // `R_must ⊆ R_may` holds globally over the whole `2^k` cube space —
        // which the R-F5.4.1 evaluator relies on (`box.must ⊆ box.may`).
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
            raw.and(&feasible_present).unwrap()
        });

        Ok(AbstractRelation {
            num_predicates: k,
            feasible_present,
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
    /// The feasible present cubes `∃x. A(x,p)` — a BDD over the present
    /// predicate vars marking the cubes inhabited by some concrete state.
    feasible_present: BDDFunction,
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

    /// Is cube `cube` feasible — inhabited by at least one concrete state?
    /// Infeasible cubes are never materialised as abstract states.
    pub fn is_feasible(&self, cube: usize) -> bool {
        let mt = self.cube_minterm(cube, &self.present);
        mt.and(&self.feasible_present).unwrap() == mt
    }

    /// The feasible cube indices, ascending.
    pub fn feasible_cubes(&self) -> Vec<usize> {
        (0..(1usize << self.num_predicates))
            .filter(|&c| self.is_feasible(c))
            .collect()
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
    ///
    /// `r_may` / `r_must` are passed explicitly (rather than reading
    /// `self.r_may`) so a guarded modality can supply the guard-restricted
    /// relations from [`Self::guarded_relations`] (R-F5.5c). A bare modality
    /// passes `&self.r_may` and the relation's own `r_must`.
    fn box_pre(&self, phi: &TritBdd, r_may: &BDDFunction, r_must: &BDDFunction) -> TritBdd {
        use oxidd::{BooleanFunctionQuant, BooleanOperator};
        let must_next = self.to_next(phi.must());
        let box_must = r_may
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
    /// uses `R_may`. `r_may` / `r_must` are passed explicitly for the same
    /// guard-restriction reason as [`Self::box_pre`] (R-F5.5c).
    fn diamond_pre(&self, phi: &TritBdd, r_may: &BDDFunction, r_must: &BDDFunction) -> TritBdd {
        use oxidd::{BooleanFunctionQuant, BooleanOperator};
        let must_next = self.to_next(phi.must());
        let dia_must = r_must
            .apply_exists(BooleanOperator::And, &must_next, &self.next_cube)
            .unwrap();
        let may_next = self.to_next(phi.may());
        let dia_may = r_may
            .apply_exists(BooleanOperator::And, &may_next, &self.next_cube)
            .unwrap();
        TritBdd::from_parts(dia_must, dia_may)
    }

    /// R-F5.5c — the may/must relations RESTRICTED to a guard's matching
    /// transitions: `R ∧ cur_ok(x) ∧ next_ok(x')`, where `cur_ok` / `next_ok`
    /// are the `req_*` / `forb_*` state-predicate filters over the present /
    /// next predicate vars. Each guard predicate names a `P_i` in `names`; its
    /// characteristic set is the present var `present[i]` (current) or its
    /// `to_next` image (next). Mirrors [`crate::mu_calculus::symbolic`]'s
    /// `SymbolicKmts::guarded_relations` on this relation's cube frame.
    ///
    /// A required predicate absent from `names` (not part of the cube's
    /// abstraction) contributes `⊥` (no cube matches → the modal is vacuous
    /// there), exactly as the explicit `guard_matches` filter; a forbidden
    /// absent predicate is vacuously satisfied (no constraint, since `¬⊥ = ⊤`).
    ///
    /// Label / controllability / step-bounded guards are NOT handled here —
    /// they are rejected up front in [`Self::eval_node`] as out-of-fragment
    /// over a predicate cube (see
    /// [`crate::mu_calculus::cube_modality_soundness_warnings`]).
    fn guarded_relations(
        &self,
        guard: &Guard,
        names: &HashMap<&str, usize>,
        r_must: &BDDFunction,
    ) -> (BDDFunction, BDDFunction) {
        // `present[i]` if the predicate is in the cube's set; `ff` otherwise
        // (a required-but-absent predicate makes the guard vacuous; a
        // forbidden-but-absent one is vacuously satisfied since `¬ff = tt`).
        let pred_set = |name: &str| -> BDDFunction {
            match names.get(name) {
                Some(&i) => self.present[i].clone(),
                None => self.ff.clone(),
            }
        };
        let mut c = self.tt.clone();
        for v in &guard.current.required {
            c = c.and(&pred_set(v)).unwrap();
        }
        for v in &guard.current.forbidden {
            c = c.and(&pred_set(v).not().unwrap()).unwrap();
        }
        for v in &guard.next.required {
            c = c.and(&self.to_next(&pred_set(v))).unwrap();
        }
        for v in &guard.next.forbidden {
            c = c.and(&self.to_next(&pred_set(v).not().unwrap())).unwrap();
        }
        (self.r_may.and(&c).unwrap(), r_must.and(&c).unwrap())
    }

    /// R-F5.4.1 — evaluate a mu-calculus `formula` symbolically over this
    /// abstract relation, returning a `(must, may)` verdict over the present
    /// predicate cubes. `pred_names[i]` names predicate `P_i` (the atomic
    /// proposition `P_i` holds exactly on the cubes where bit `i` is set —
    /// i.e. its characteristic set is the present var `p_i`).
    ///
    /// Supports the **audited-sound fragment** the cube path evaluates:
    /// `True`/`False`, predicates, `!`/`&&`/`||`, `mu`/`nu`, bare `[]`/`<>`, and
    /// (R-F5.5c) modalities guarded by `req_cur`/`forb_cur`/`req_next`/`forb_next`
    /// state predicates (each names a `P_i` in `pred_names`; see
    /// [`Self::guarded_relations`]). A label / controllability / step-bounded
    /// modality remains an honest error — out-of-fragment over a predicate cube
    /// (see [`crate::mu_calculus::cube_modality_soundness_warnings`]). Requires a
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
                // Out-of-fragment over a predicate cube (see
                // `cube_modality_soundness_warnings`): controllability needs a
                // player partition the plain verification cube lacks; a bounded
                // step is not may/must-filtered; a label guard is vacuous because
                // the cube collapses every concrete action onto its own label.
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
                if !guard.labels.is_empty() {
                    return Err(
                        "symbolic cube evaluator: label guards unsupported over a predicate cube \
                         (the cube carries no label structure — use `req_cur`/`req_next` state \
                         guards, or a bare `[]`/`<>`)"
                            .into(),
                    );
                }
                // R-F5.5c — the sound in-fragment guards over a cube are the
                // `req_cur`/`forb_cur`/`req_next`/`forb_next` state-predicate
                // filters. `guarded_relations` returns `(r_may, r_must)`
                // unchanged for a bare (all-empty) guard, so this path is a
                // no-op for `[]`/`<>`.
                let (gr_may, gr_must) = self.guarded_relations(guard, names, r_must);
                let phi = self.eval_node(f, *target, names, r_must, bindings)?;
                match kind {
                    ModalKind::Box => self.box_pre(&phi, &gr_may, &gr_must),
                    ModalKind::Diamond => self.diamond_pre(&phi, &gr_may, &gr_must),
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
        // Wide binary literals (> 128 bits) are built DIRECTLY from the bit string, bypassing the
        // u128 `eval_const_value` (which ERRORS on a > 128-bit `Bin` — u128 overflow). Such consts
        // occur out of the property cone (verdict-irrelevant, cone widths ≤ the bit cap), but a
        // hard error would fail the whole exact MC; representing them per-bit keeps the build
        // going. `s` is the btor2 MSB-first bit string; bit `b` (LSB-first) is `s[len-1-b]`,
        // zero-extended past the string. Found: keymgr_ctrl's 256-bit key-state constants.
        if let ConstValue::Bin(s) = value {
            let bytes = s.as_bytes();
            let n = bytes.len();
            return Ok((0..width as usize)
                .map(|b| {
                    if b < n && bytes[n - 1 - b] == b'1' {
                        self.tt.clone()
                    } else {
                        self.ff.clone()
                    }
                })
                .collect());
        }
        // Non-`Bin` constants (Zero/One/Ones/Dec) fit the concrete u128 semantics. `bv.bits` is a
        // `u128`, so bit `b ≥ 128` is 0 (and `>> b` would PANIC): guard it.
        let bv = eval_const_value(value, width)?;
        Ok((0..width as usize)
            .map(|b| {
                if b < 128 && (bv.bits >> b) & 1 == 1 {
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
            Op::Mul => {
                // Schoolbook shift-and-add: Σ_i (b[i] ? (a << i) : 0), truncated to `width`.
                // O(width²) BDD adds — fine at the ≤ MAX_BITBLAST_BITS cone widths. Both
                // operands + the result are `width` bits; the product wraps mod 2^width.
                let (a, b) = (read(0)?, read(1)?);
                let w = width as usize;
                let mut acc: BitVec = vec![self.ff.clone(); w];
                for i in 0..w {
                    let bi = b.get(i).cloned().unwrap_or_else(|| self.ff.clone());
                    // Partial product: (a << i) masked by b[i]. Bit j = (j ≥ i) ? a[j−i] ∧ b[i] : 0.
                    let partial: BitVec = (0..w)
                        .map(|j| {
                            if j >= i {
                                a.get(j - i)
                                    .cloned()
                                    .unwrap_or_else(|| self.ff.clone())
                                    .and(&bi)
                                    .unwrap()
                            } else {
                                self.ff.clone()
                            }
                        })
                        .collect();
                    acc = self.add_bits(&acc, &partial, self.ff.clone(), width).0;
                }
                acc
            }

            // ---- Variable shifts (barrel shifter, result + operands all `width` bits) ----
            Op::Sll => {
                let (a, s) = (read(0)?, read(1)?);
                self.barrel_shift(&a, &s, width, true, false)
            }
            Op::Srl => {
                let (a, s) = (read(0)?, read(1)?);
                self.barrel_shift(&a, &s, width, false, false)
            }
            Op::Sra => {
                let (a, s) = (read(0)?, read(1)?);
                self.barrel_shift(&a, &s, width, false, true)
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

            // ---- Signed comparisons (two's-complement, 1-bit result) ----
            Op::Slt => {
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.slt(&a, &b, w)]
            }
            Op::Sgt => {
                // a > b ⟺ b < a
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.slt(&b, &a, w)]
            }
            Op::Sgte => {
                // a ≥ b ⟺ ¬(a < b)
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.slt(&a, &b, w).not().unwrap()]
            }
            Op::Slte => {
                // a ≤ b ⟺ ¬(b < a)
                let (a, b) = (read(0)?, read(1)?);
                let w = a.len().max(b.len()) as u32;
                vec![self.slt(&b, &a, w).not().unwrap()]
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

/// D1 — the EXACT (full-state, 2-valued) transition model for symbolic
/// μ-calculus model checking, with NO predicate abstraction. The state is the
/// register-bit valuation itself; inputs are free (nondeterminism). A formula's
/// satisfying set is a single [`BDDFunction`] over the register-bit vars, and the
/// modal pre-image substitutes each state bit by its next-state function then
/// quantifies the input bits (`∃` for `⟨⟩`, `∀` for `[]`). Because there is no
/// abstraction the verdict is 2-valued and definite (no `⊥`); the cost is bounded
/// by BDD size, not by the `2^|Registers|` explicit-state cap. Reuses the exact
/// same substitution [`BddBitBlaster::abstract_relation`] builds, applied to the
/// formula BDD directly rather than to predicate vars.
pub struct ExactModel {
    /// State-bit varno → its next-state function. Held registers (no `Next` line)
    /// are omitted (identity ⇒ held value); inputs are never state, never
    /// substituted (they remain free to be quantified).
    sub_vars: Vec<VarNo>,
    sub_repl: Vec<BDDFunction>,
    /// The cube of input-bit vars, quantified in the modal pre-image.
    input_cube: BDDFunction,
    tt: BDDFunction,
    ff: BDDFunction,
}

impl BddBitBlaster {
    /// Build the [`ExactModel`] for full-state 2-valued μ-calculus MC (D1). Pinned
    /// inputs (config concretization / reset-gating) are already constants in the
    /// bit-blasted design, so the `∃`/`∀` over the *remaining* free inputs is exact
    /// for the pinned model.
    pub fn exact_model(&self) -> ExactModel {
        // State bit → next-state function (the same sub the abstract relation uses).
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
        // The input-bit cube to quantify in the modal pre-image.
        let mut input_cube = self.tt.clone();
        for cell in &self.cells {
            if !cell.is_state {
                for v in &cell.vars {
                    input_cube = input_cube.and(v).unwrap();
                }
            }
        }
        ExactModel {
            sub_vars,
            sub_repl,
            input_cube,
            tt: self.tt.clone(),
            ff: self.ff.clone(),
        }
    }

    /// D1.3/D1.5 — the BDD of the design's **initial state**: EVERY state register
    /// is pinned to its reset value — its BTOR2 `init <state> <value>` bits, or **0
    /// when it has no `init` line** (the `setundef -zero` model convention Yosys +
    /// verify-auto's `state_cell_init_values` use). Pinning the undefined-reset
    /// registers to 0 (rather than leaving them free) is what keeps `AG …` from
    /// seeing unreachable stuck states: the verdict is checked from the *actual*
    /// modelled reset state, not an over-broad free-init set. `Holds` iff that
    /// initial state satisfies the property.
    pub fn initial_state_bdd(&self, file: &Btor2File) -> BDDFunction {
        // State-line nid → the value operand of its `init` line (if any).
        let mut init_value_nid: HashMap<Nid, Nid> = HashMap::new();
        for line in &file.lines {
            if let Node::Init { state, value, .. } = &line.node {
                init_value_nid.insert(*state, value.nid());
            }
        }
        // State-line nid → the symbol `build()` assigned it. AR-S1 / #242 drift
        // guard: names go through the SAME `parser::resolve_leaf_symbol` the seam's
        // `leaf_cells` enumeration (and hence `build()`) uses, else the cell lookup
        // below misses on flattened yosys output (cell = `bit_cnt_d`, raw State
        // symbol = none → `state_<nid>`) and the register is left unconstrained — a
        // silently over-broad init.
        let symbols = crate::adapter::btor2::parser::collect_symbols(file);
        let mut sym_of_nid: HashMap<Nid, String> = HashMap::new();
        for line in &file.lines {
            if let Node::State { symbol, .. } = &line.node {
                let sym = crate::adapter::btor2::parser::resolve_leaf_symbol(
                    &symbols, line.nid, symbol, "state",
                );
                sym_of_nid.insert(line.nid, sym);
            }
        }
        let cell_of_sym: HashMap<&str, &Cell> = self
            .cells
            .iter()
            .filter(|c| c.is_state)
            .map(|c| (c.symbol.as_str(), c))
            .collect();

        let mut init = self.tt.clone();
        for line in &file.lines {
            if !matches!(line.node, Node::State { .. }) {
                continue;
            }
            let Some(sym) = sym_of_nid.get(&line.nid) else {
                continue;
            };
            let Some(cell) = cell_of_sym.get(sym.as_str()) else {
                continue;
            };
            // Reset-value bits: the `init` line's value BDD, or all-`⊥` (every bit
            // 0) for a register with no `init` line.
            let value_bits: Vec<BDDFunction> = init_value_nid
                .get(&line.nid)
                .and_then(|vn| self.env.get(vn))
                .cloned()
                .unwrap_or_else(|| vec![self.ff.clone(); cell.vars.len()]);
            for (var, val) in cell.vars.iter().zip(value_bits.iter()) {
                // state_bit ⟺ value_bit  =  ¬(state_bit XOR value_bit)
                let iff = self.xor(var, val).not().unwrap();
                init = init.and(&iff).unwrap();
            }
        }
        init
    }
}

impl ExactModel {
    /// `⊤` / `⊥` over the state-var frame (for fixpoint seeds + tests).
    pub fn tt(&self) -> &BDDFunction {
        &self.tt
    }
    pub fn ff(&self) -> &BDDFunction {
        &self.ff
    }

    /// `φ` pushed one step forward: each state bit ↦ its next-state function ⇒ a
    /// BDD over `(state, input)` = "the successor of this state under this input
    /// satisfies `φ`". Simultaneous substitution ⇒ the concrete next-state
    /// relation. `pub` so the D1.8 lasso extractor can pick a `φ`-preserving input.
    pub fn to_next(&self, phi: &BDDFunction) -> BDDFunction {
        if self.sub_vars.is_empty() {
            phi.clone()
        } else {
            phi.substitute(&Subst::new(self.sub_vars.clone(), self.sub_repl.clone()))
                .unwrap()
        }
    }

    /// `⟨⟩φ` — some successor (over some input) satisfies `φ`: `∃i. to_next(φ)`.
    pub fn diamond_pre(&self, phi: &BDDFunction) -> BDDFunction {
        use oxidd::BooleanFunctionQuant;
        self.to_next(phi).exists(&self.input_cube).unwrap()
    }

    /// `[]φ` — all successors (over every input) satisfy `φ`: `∀i. to_next(φ)`.
    pub fn box_pre(&self, phi: &BDDFunction) -> BDDFunction {
        use oxidd::BooleanFunctionQuant;
        self.to_next(phi).forall(&self.input_cube).unwrap()
    }

    /// D1.2 — evaluate a **2-valued** μ-calculus `formula` over the exact model,
    /// returning the BDD of states that satisfy it. No abstraction ⇒ the answer is
    /// definite (no `⊥`); the whole modal-μ fragment is decided exactly, bounded
    /// only by BDD size. `atoms` maps each predicate-atom name to its full-state
    /// BDD (the caller builds these via [`BddBitBlaster::predicate_bdd`]); an
    /// unresolved atom is `⊥` (empty set), never silently true.
    ///
    /// Modalities are **bare** `[]`/`<>` in D1.2 (the fragment the liveness
    /// showcase uses). Guarded / controllability / step-bounded modalities are an
    /// honest error — state-predicate guards over the exact relation are a D1.2b
    /// follow-up (they mirror [`AbstractRelation::guarded_relations`], 2-valued).
    pub fn evaluate(
        &self,
        formula: &Formula,
        atoms: &HashMap<&str, BDDFunction>,
    ) -> Result<BDDFunction, String> {
        let mut bindings: HashMap<FormulaVarId, BDDFunction> = HashMap::new();
        self.eval_node(formula, formula.root(), atoms, &mut bindings)
    }

    /// Evaluate a specific sub-node `id` of `formula` to its state-set BDD (fresh
    /// bindings). D1.8b uses it to get the target-predicate BDD of a detected `AF p`.
    /// Sound only when `id` has no free fixpoint variables (the `AF` target doesn't).
    pub fn eval_at(
        &self,
        formula: &Formula,
        id: crate::mu_calculus::NodeId,
        atoms: &HashMap<&str, BDDFunction>,
    ) -> Result<BDDFunction, String> {
        let mut bindings: HashMap<FormulaVarId, BDDFunction> = HashMap::new();
        self.eval_node(formula, id, atoms, &mut bindings)
    }

    fn eval_node(
        &self,
        f: &Formula,
        id: crate::mu_calculus::NodeId,
        atoms: &HashMap<&str, BDDFunction>,
        bindings: &mut HashMap<FormulaVarId, BDDFunction>,
    ) -> Result<BDDFunction, String> {
        Ok(match f.node(id) {
            MuNode::True => self.tt.clone(),
            MuNode::False => self.ff.clone(),
            MuNode::Predicate(name) => atoms
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(|| self.ff.clone()),
            MuNode::Variable(v) => bindings
                .get(v)
                .cloned()
                .ok_or_else(|| format!("unbound fixpoint variable {v:?}"))?,
            MuNode::Not(n) => self.eval_node(f, *n, atoms, bindings)?.not().unwrap(),
            MuNode::And(a, b) => {
                let av = self.eval_node(f, *a, atoms, bindings)?;
                let bv = self.eval_node(f, *b, atoms, bindings)?;
                av.and(&bv).unwrap()
            }
            MuNode::Or(a, b) => {
                let av = self.eval_node(f, *a, atoms, bindings)?;
                let bv = self.eval_node(f, *b, atoms, bindings)?;
                av.or(&bv).unwrap()
            }
            MuNode::Modal {
                kind,
                guard,
                target,
            } => {
                if *guard != Guard::default() {
                    return Err(
                        "exact μ-calculus MC (D1.2): only bare `[]`/`<>` modalities are supported \
                         yet — guarded / controllability / step-bounded modalities are a D1.2b \
                         follow-up"
                            .into(),
                    );
                }
                let phi = self.eval_node(f, *target, atoms, bindings)?;
                match kind {
                    ModalKind::Box => self.box_pre(&phi),
                    ModalKind::Diamond => self.diamond_pre(&phi),
                }
            }
            MuNode::Mu { var, body } => self.fixpoint(f, *var, *body, atoms, bindings, false)?,
            MuNode::Nu { var, body } => self.fixpoint(f, *var, *body, atoms, bindings, true)?,
        })
    }

    /// Kleene iteration for a least (`greatest=false`, from `⊥`) or greatest
    /// (`greatest=true`, from `⊤`) fixpoint. Convergence is exact set equality —
    /// ROBDDs are canonical, so `==` is the fixpoint test, and over a finite state
    /// space it converges in ≤ |states| steps (the iteration *is* the ranking).
    fn fixpoint(
        &self,
        f: &Formula,
        var: FormulaVarId,
        body: crate::mu_calculus::NodeId,
        atoms: &HashMap<&str, BDDFunction>,
        bindings: &mut HashMap<FormulaVarId, BDDFunction>,
        greatest: bool,
    ) -> Result<BDDFunction, String> {
        let mut x = if greatest {
            self.tt.clone()
        } else {
            self.ff.clone()
        };
        loop {
            bindings.insert(var, x.clone());
            let next = self.eval_node(f, body, atoms, bindings)?;
            if next == x {
                bindings.remove(&var);
                return Ok(next);
            }
            x = next;
        }
    }
}

/// D1.3 — the definite verdict of exact symbolic model checking: a property
/// `Holds` iff every initial state satisfies it, else it is `Violated` (there is a
/// reachable-from-init counterexample class). 2-valued — the exact engine never
/// returns `⊥`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactVerdict {
    Holds,
    Violated,
}

/// D1.4 — rewrite every register name in a [`PredicateExpr`] through `resolve`
/// (the `BtorSts::resolve_register` alias→canonical map), so an atom over a
/// user-visible name binds against the bit-blasted state cell.
fn resolve_predicate_expr_registers(
    expr: &PredicateExpr,
    resolve: &impl Fn(&str) -> String,
) -> PredicateExpr {
    match expr {
        PredicateExpr::Cmp {
            register,
            op,
            value,
        } => PredicateExpr::Cmp {
            register: resolve(register),
            op: *op,
            value: *value,
        },
        PredicateExpr::CmpReg { lhs, op, rhs } => PredicateExpr::CmpReg {
            lhs: resolve(lhs),
            op: *op,
            rhs: resolve(rhs),
        },
        PredicateExpr::CmpRegAddend {
            lhs,
            op,
            rhs,
            addend,
            width,
        } => PredicateExpr::CmpRegAddend {
            lhs: resolve(lhs),
            op: *op,
            rhs: resolve(rhs),
            addend: *addend,
            width: *width,
        },
        PredicateExpr::And(a, b) => PredicateExpr::And(
            Box::new(resolve_predicate_expr_registers(a, resolve)),
            Box::new(resolve_predicate_expr_registers(b, resolve)),
        ),
        PredicateExpr::Or(a, b) => PredicateExpr::Or(
            Box::new(resolve_predicate_expr_registers(a, resolve)),
            Box::new(resolve_predicate_expr_registers(b, resolve)),
        ),
        PredicateExpr::Not(a) => {
            PredicateExpr::Not(Box::new(resolve_predicate_expr_registers(a, resolve)))
        }
    }
}

/// R-F5.6 — collect every register name a [`PredicateExpr`] references (already resolved to
/// canonical cell names). These are the seed atoms for the cone-of-influence keep-set that
/// restricts the bit-blaster to the property's cone (used by both the exact engine and, via
/// `symbolic_engine`, the symbolic cube-BDD engine).
pub(crate) fn collect_predicate_registers(
    expr: &PredicateExpr,
    out: &mut std::collections::HashSet<String>,
) {
    match expr {
        PredicateExpr::Cmp { register, .. } => {
            out.insert(register.clone());
        }
        PredicateExpr::CmpReg { lhs, rhs, .. } | PredicateExpr::CmpRegAddend { lhs, rhs, .. } => {
            out.insert(lhs.clone());
            out.insert(rhs.clone());
        }
        PredicateExpr::And(a, b) | PredicateExpr::Or(a, b) => {
            collect_predicate_registers(a, out);
            collect_predicate_registers(b, out);
        }
        PredicateExpr::Not(a) => collect_predicate_registers(a, out),
    }
}

/// Directly decide whether ANY `bad` property of a BTOR2 design is REACHABLE from the reset
/// state — the native HWMCC safety question (a reachable `bad` = the safety property is VIOLATED,
/// "SAT" in competition terms; unreachable = "UNSAT" / safe). This bridges an ARBITRARY `bad`
/// circuit (not just a register-value predicate, which [`exact_symbolic_verdict`] requires) to the
/// exact engine: it ORs the `bad` nodes' 1-bit BDDs and evaluates `EF(bad) = μY. (bad ∨ ◇Y)` over
/// the full bit-blasted transition relation, then tests whether the modelled reset state lies in
/// it. The exact analogue of btormc's bad-reachability — the oracle the reachability differential
/// + the portfolio coverage study compare against.
///
/// Returns `Ok(true)` (reachable / SAT / unsafe), `Ok(false)` (unreachable / UNSAT / safe), or an
/// `Err` (undecided) when the design is over the bit cap, carries an unsupported op, or has
/// `constraint`/`fair` lines. **The constraint guard is a soundness requirement:** this engine
/// does not restrict reachability to constraint-satisfying runs, so on a constrained design it
/// would OVER-approximate (report a spuriously-reachable `bad` that btormc, honouring the
/// constraint, calls safe). Refusing to decide there keeps every verdict it DOES emit sound.
pub fn exact_bad_reachable(btor2_content: &str) -> Result<bool, String> {
    use crate::mu_calculus::parser as mu_parser;
    let file = crate::adapter::btor2::parser::parse(btor2_content)
        .map_err(|e| format!("exact bad-reachability: parse: {}", e.message))?;

    // Soundness guard: we do not model `constraint` (assume) / `fair` — a design carrying them
    // could have a `bad` reachable in OUR unconstrained relation but unreachable under btormc's
    // constrained one. Refuse to decide rather than emit an over-approximate verdict.
    if file
        .lines
        .iter()
        .any(|l| matches!(l.node, Node::Constraint { .. } | Node::Fair { .. }))
    {
        return Err(
            "exact bad-reachability: design has `constraint`/`fair` lines (not modelled) — \
             undecided rather than over-approximate"
                .into(),
        );
    }

    let bad_ops: Vec<Operand> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Bad { signal } => Some(*signal),
            _ => None,
        })
        .collect();
    if bad_ops.is_empty() {
        return Err("exact bad-reachability: the design has no `bad` property".into());
    }

    let bb = BddBitBlaster::build(&file)?; // cap-guarded: a wide design ⇒ Err ⇒ undecided
    // OR the `bad` nodes' 1-bit BDDs into a single "some bad holds" state set.
    let mut bad = bb.ff.clone();
    for op in &bad_ops {
        let bits = bb.resolve(*op)?;
        let b = bits
            .into_iter()
            .next()
            .ok_or_else(|| "exact bad-reachability: a `bad` signal has zero width".to_string())?;
        bad = bad.or(&b).unwrap();
    }

    let model = bb.exact_model();
    let formula = mu_parser::parse("mu Y. (BAD or <> Y)").expect("EF(bad) formula parses");
    let mut atoms: HashMap<&str, BDDFunction> = HashMap::new();
    atoms.insert("BAD", bad);
    let reach = model.evaluate(&formula, &atoms)?;
    let init = bb.initial_state_bdd(&file);
    Ok(init.and(&reach).unwrap() != bb.ff)
}

/// D1.3 — exact full-state symbolic μ-calculus model checking end-to-end: parse the
/// BTOR2, bit-blast it, evaluate `formula` over the exact model (2-valued, no
/// abstraction), and return the initial-state verdict. Atoms in `formula` are
/// register-comparison predicates (`(reg op val)`) resolved via
/// [`parse_predicate_expr`] → [`BddBitBlaster::predicate_bdd`]; an atom that does
/// not parse is an error (never silently `⊥`). Bounded by BDD size, not by the
/// explicit-state cap: decides the whole modal-μ fragment (safety, `AG EF`,
/// `AF`-liveness, GR(1)) exactly, where predicate abstraction may return `⊥`.
pub fn exact_symbolic_verdict(
    btor2_content: &str,
    formula: &Formula,
) -> Result<ExactVerdict, String> {
    exact_symbolic_verdict_with_witness(btor2_content, formula).map(|(v, _)| v)
}

/// D1.8b — like [`exact_symbolic_verdict`], but on a `Violated` verdict for a bare
/// `AF p`-shaped property (`μX. (p ∨ [] X)`) it also returns a concrete
/// [`StallLasso`] counterexample: the reachable `¬p` stall witnessing that liveness
/// fails from the reset state. The witness is `None` when the verdict is `Holds`,
/// when `formula` is not that shape, or when the stall is not reachable *at* the
/// initial state (a reachable-but-not-initial stall — the `AG AF` case — is a future
/// extension of [`BddBitBlaster::exact_stall_lasso`]).
pub fn exact_symbolic_verdict_with_witness(
    btor2_content: &str,
    formula: &Formula,
) -> Result<(ExactVerdict, Option<StallLasso>), String> {
    use crate::adapter::sts_ir::SymbolicTransitionSystem;
    let file = crate::adapter::btor2::parser::parse(btor2_content)
        .map_err(|e| format!("adapter/btor2/exact MC: {}", e.message))?;

    // Register-name resolution: a user-visible name (`bit_cnt_q`) maps to the
    // canonical state-cell name the bit-blast binds against (`bit_cnt_d` after
    // yosys async2sync/flatten aliasing) — the same `BtorSts::resolve_register`
    // the cube path uses. Idempotent on names that are already canonical.
    let sts = crate::adapter::sts_ir::BtorSts::new(&file);
    let resolve = |name: &str| -> String {
        sts.resolve_register(name)
            .unwrap_or_else(|| name.to_string())
    };

    // R-F5.6 — resolve each distinct formula atom to a canonical [`PredicateExpr`] BEFORE
    // building the bit-blaster, and collect the register names they reference. Those seed the
    // cone-of-influence keep-set so the bit-blaster bit-blasts only the property's cone
    // (out-of-cone registers/inputs pinned to constants) instead of the whole design — the
    // R-F5.6 scaling fix. An atom-free formula (no seeds) keeps the full-design behaviour.
    let mut exprs: Vec<(String, PredicateExpr)> = Vec::new();
    let mut seed_regs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for node in formula.nodes() {
        if let MuNode::Predicate(name) = node {
            if exprs.iter().any(|(n, _)| n == name) {
                continue;
            }
            let expr = parse_predicate_expr(name)
                .map_err(|e| format!("exact MC: atom `{name}` is not a predicate: {e}"))?;
            let expr = resolve_predicate_expr_registers(&expr, &resolve);
            collect_predicate_registers(&expr, &mut seed_regs);
            exprs.push((name.clone(), expr));
        }
    }
    let seed_atoms: Vec<String> = seed_regs.into_iter().collect();
    let keep_set = (!seed_atoms.is_empty())
        .then(|| crate::adapter::btor2::dep_graph::cone_leaf_nids(&file, &seed_atoms));
    let bb = BddBitBlaster::build_with_keep(&file, keep_set.as_ref())?;
    let exact = bb.exact_model();

    // Resolve each atom's full-state BDD against the (cone-restricted) bit-blaster.
    let mut resolved: Vec<(String, BDDFunction)> = Vec::new();
    for (name, expr) in &exprs {
        let bdd = bb.predicate_bdd(expr)?;
        resolved.push((name.clone(), bdd));
    }
    let atoms: HashMap<&str, BDDFunction> = resolved
        .iter()
        .map(|(n, b)| (n.as_str(), b.clone()))
        .collect();

    let sat = exact.evaluate(formula, &atoms)?;
    let init = bb.initial_state_bdd(&file);
    // Holds iff init ⊆ sat, i.e. no initial state violates φ.
    let violating = init.and(&sat.not().unwrap()).unwrap();
    if violating == *exact.ff() {
        return Ok((ExactVerdict::Holds, None));
    }
    // Violated — attach a stall lasso when the property is a liveness shape (bare
    // `AF p` or `AG AF p`). The target `p` is evaluated from the same atom set;
    // `exact_reachable_stall_lasso` finds a stall reachable from reset (subsuming the
    // stall-at-reset case), so it covers both shapes. Best-effort: `None` if the shape
    // isn't recognised or no stall is reachable.
    // First the `AF`/`AG AF` stall lasso; failing that, the `AG EF p` recoverability trap path
    // (a reachable state from which `p` is unreachable). Best-effort: `None` if neither shape
    // matches or the witness region is unreachable.
    let witness = detect_af_target(formula)
        .and_then(|p_id| exact.eval_at(formula, p_id, &atoms).ok())
        .and_then(|p_bdd| bb.exact_reachable_stall_lasso(&file, &p_bdd))
        .or_else(|| {
            detect_ag_ef_target(formula)
                .and_then(|p_id| exact.eval_at(formula, p_id, &atoms).ok())
                .and_then(|p_bdd| bb.exact_reachable_trap_path(&file, &p_bdd))
        });
    Ok((ExactVerdict::Violated, witness))
}

/// D1.8b/b-2 — detect a liveness shape and return the node id of its target `p`.
/// Recognised (both disjunct/conjunct orders; modalities must be bare `[]`):
/// - bare `AF p` = `μX. (p ∨ [] X)`,
/// - `AG AF p` = `νY. ((μX. (p ∨ [] X)) ∧ [] Y)`.
///
/// `None` for any other shape.
fn detect_af_target(formula: &Formula) -> Option<crate::mu_calculus::NodeId> {
    let root = formula.root();
    // Bare `AF p`.
    if let Some(p) = af_target_at(formula, root) {
        return Some(p);
    }
    // `AG AF p` = `νY. (AF_p ∧ [] Y)`: strip the `AG` wrapper, then match `AF p`.
    let MuNode::Nu { var, body } = formula.node(root) else {
        return None;
    };
    let MuNode::And(a, b) = formula.node(*body) else {
        return None;
    };
    let af = if is_box_over_var(formula, *a, var) {
        *b
    } else if is_box_over_var(formula, *b, var) {
        *a
    } else {
        return None;
    };
    af_target_at(formula, af)
}

/// Match `μX. (p ∨ [] X)` at `node` → the node id of `p` (the non-recursive disjunct).
fn af_target_at(
    formula: &Formula,
    node: crate::mu_calculus::NodeId,
) -> Option<crate::mu_calculus::NodeId> {
    let MuNode::Mu { var, body } = formula.node(node) else {
        return None;
    };
    let MuNode::Or(a, b) = formula.node(*body) else {
        return None;
    };
    if is_box_over_var(formula, *a, var) {
        Some(*b)
    } else if is_box_over_var(formula, *b, var) {
        Some(*a)
    } else {
        None
    }
}

/// Is `node` a bare `[] v` (unguarded box over the fixpoint variable `v`)?
fn is_box_over_var(
    formula: &Formula,
    node: crate::mu_calculus::NodeId,
    v: &crate::mu_calculus::FormulaVarId,
) -> bool {
    matches!(
        formula.node(node),
        MuNode::Modal { kind: ModalKind::Box, guard, target }
            if *guard == Guard::default()
                && matches!(formula.node(*target), MuNode::Variable(w) if w == v)
    )
}

/// Is `node` a bare `<> v` (unguarded diamond over the fixpoint variable `v`)?
fn is_diamond_over_var(
    formula: &Formula,
    node: crate::mu_calculus::NodeId,
    v: &crate::mu_calculus::FormulaVarId,
) -> bool {
    matches!(
        formula.node(node),
        MuNode::Modal { kind: ModalKind::Diamond, guard, target }
            if *guard == Guard::default()
                && matches!(formula.node(*target), MuNode::Variable(w) if w == v)
    )
}

/// P3 — detect `AG EF p = νY. ((μX. (p ∨ <> X)) ∧ [] Y)` (the recoverability shape) and return
/// the node id of its target `p`. Mirrors [`detect_af_target`] but the inner μ uses a bare
/// DIAMOND (`<> X` = `∃input`), not a box. `None` for any other shape.
fn detect_ag_ef_target(formula: &Formula) -> Option<crate::mu_calculus::NodeId> {
    let root = formula.root();
    let MuNode::Nu { var, body } = formula.node(root) else {
        return None;
    };
    let MuNode::And(a, b) = formula.node(*body) else {
        return None;
    };
    let ef = if is_box_over_var(formula, *a, var) {
        *b
    } else if is_box_over_var(formula, *b, var) {
        *a
    } else {
        return None;
    };
    ef_target_at(formula, ef)
}

/// Match `μX. (p ∨ <> X)` (= `EF p`) at `node` → the node id of `p` (the non-recursive
/// disjunct). The diamond twin of [`af_target_at`].
fn ef_target_at(
    formula: &Formula,
    node: crate::mu_calculus::NodeId,
) -> Option<crate::mu_calculus::NodeId> {
    let MuNode::Mu { var, body } = formula.node(node) else {
        return None;
    };
    let MuNode::Or(a, b) = formula.node(*body) else {
        return None;
    };
    if is_diamond_over_var(formula, *a, var) {
        Some(*b)
    } else if is_diamond_over_var(formula, *b, var) {
        Some(*a)
    } else {
        None
    }
}

/// D1.8c — build the sound μ-calculus formula that verifies the GR(1) response
/// property `GF assume → GF guarantee` over the **exact** engine. `assume` and
/// `guarantee` are predicate expressions (e.g. `"req == 1"`).
///
/// The property FAILS iff the system has a reachable **bad cycle** — a cycle that
/// hits `assume` infinitely often (`GF assume`) yet avoids `guarantee` forever
/// (`FG ¬guarantee`). The generated formula is `¬EF badcycle`, where
///
/// ```text
/// badcycle = νZ. μY. ((assume ∧ ¬guarantee ∧ ◇Z) ∨ (¬guarantee ∧ ◇Y))
/// EF X     = μW. (X ∨ ◇W)
/// ```
///
/// `badcycle` is the Emerson–Lei fair-cycle set — an infinite `¬guarantee` path
/// recurring through `assume` — and `◇` is `∃input` in the exact model, so this reads
/// "∃ a bad input sequence". Fed to [`exact_symbolic_verdict`] (or a
/// `@mununu_guarantee` annotation on the exact-symbolic verify-auto path), the exact
/// engine reports **Holds** iff the reset state cannot reach any bad cycle — i.e. the
/// GR(1) property genuinely holds on all paths (a *definite*, sound liveness verdict
/// where the existential LTL `GF` translation `AG EF` and predicate abstraction do
/// not).
///
/// Scope: a single `assume`/`guarantee` pair (the GR(1) response core). Generalised
/// (Streett) multi-fairness `⋀ GF aᵢ → ⋀ GF bⱼ` is a future extension.
pub fn gr1_response_formula(assume: &str, guarantee: &str) -> String {
    // badcycle = νZ. μY. ((A ∧ ¬B ∧ ◇Z) ∨ (¬B ∧ ◇Y))
    let bad = format!(
        "nu Z. (mu Y. (((({assume}) and (not ({guarantee}))) and <> Z) \
         or ((not ({guarantee})) and <> Y)))"
    );
    // ¬EF badcycle = ¬ μW. (badcycle ∨ ◇W)
    format!("not (mu W. (({bad}) or <> W))")
}

/// D1.8 — a **stall lasso**: the concrete counterexample witness for a `Violated`
/// `AF p` (all-paths-eventually-`p`) property from the exact engine. `AF p` fails
/// exactly when some initial state can avoid `p` forever — reach a `¬p` cycle
/// staying in `¬p` (the "stall"). The lasso is that path: a `prefix` from the
/// initial state to the cycle entry, then the repeating `cycle`. Each state is a
/// concrete valuation of the state registers.
#[derive(Debug, Clone)]
pub struct StallLasso {
    /// States from the initial state up to (excluding) the cycle entry. Empty when
    /// the initial state is itself on the cycle.
    pub prefix: Vec<BTreeMap<String, u128>>,
    /// The repeating `¬p` cycle; `cycle[0]` is the state the last one steps back to.
    /// Empty only in the (guarded) non-total-model deadlock case.
    pub cycle: Vec<BTreeMap<String, u128>>,
    /// P3 — the INPUT assignment that drives each transition of the concatenated
    /// `prefix ++ cycle` path (`inputs[i]` is the input at path-state `i`), for RTL replay.
    /// Empty when a builder does not record inputs (e.g. hand-authored test lassos); the
    /// [`StallLasso`] identity ignores it (see the manual `PartialEq`), so equality is over
    /// the state path only.
    pub inputs: Vec<BTreeMap<String, u128>>,
}

// StallLasso identity is the STATE path (prefix ++ cycle); `inputs` is replay metadata that
// does not affect equality (a lasso witnessing the same path is the same witness).
impl PartialEq for StallLasso {
    fn eq(&self, other: &Self) -> bool {
        self.prefix == other.prefix && self.cycle == other.cycle
    }
}
impl Eq for StallLasso {}

impl BddBitBlaster {
    /// D1.8 — extract a [`StallLasso`] witnessing that `AF p` is **Violated** from the
    /// design's initial state, or `None` when `AF p` holds (no reachable stall).
    ///
    /// `p` is the target-predicate BDD (build it with [`predicate_bdd`], over state
    /// registers). The method computes `stall = EG ¬p = νZ. (¬p ∧ ◇Z)` — the states
    /// with an infinite `p`-avoiding path, exactly `¬⟦AF p⟧` — over the exact model.
    /// If the initial state ([`initial_state_bdd`]) lies in `stall`, it walks a
    /// concrete path inside `stall`: at each step it greedily picks (from the BDD) an
    /// input whose successor stays in `stall`, advances with the concrete
    /// [`eval_step`] simulator, and stops when a state repeats — the cycle. Bounded by
    /// the stall's size, so it terminates.
    ///
    /// The witness is **self-validating**: replay it through `eval_step` and confirm
    /// every state is `¬p` and the cycle closes.
    ///
    /// [`predicate_bdd`]: BddBitBlaster::predicate_bdd
    /// [`initial_state_bdd`]: BddBitBlaster::initial_state_bdd
    /// [`eval_step`]: BddBitBlaster::eval_step
    pub fn exact_stall_lasso(&self, file: &Btor2File, p: &BDDFunction) -> Option<StallLasso> {
        let exact = self.exact_model();
        let stall = self.eg_not_p(&exact, p);
        // A stall state that is also initial ⇒ AF p is violated from reset.
        let init = self.initial_state_bdd(file);
        let bad = init.and(&stall).unwrap();
        if bad == self.ff {
            return None;
        }
        let s = self.pick_state_assignment(&bad);
        Some(self.walk_stall_cycle(&exact, &stall, s, Vec::new(), Vec::new()))
    }

    /// D1.8b-2 — extract a [`StallLasso`] witnessing that `AG AF p` is **Violated**:
    /// a `¬p` stall is *reachable* from the reset state (not necessarily at reset).
    /// Generalises [`exact_stall_lasso`] (which requires the stall *at* reset): the
    /// lasso's `prefix` is now the reset→stall reach path followed by the `¬p` cycle.
    /// `None` when no stall is reachable (`AG AF p` holds).
    ///
    /// Layered reachability: `L_0 = EG¬p`, `L_k = L_{k-1} ∨ ◇L_{k-1}` (states that can
    /// reach a stall in ≤ k steps). `init ∩ L_∞ ≠ ∅` iff a stall is reachable. The
    /// reach phase descends the layers — each step lands one layer closer via a
    /// concrete [`eval_step`] — then the shared cycle walk closes the `¬p` cycle.
    ///
    /// [`exact_stall_lasso`]: BddBitBlaster::exact_stall_lasso
    /// [`eval_step`]: BddBitBlaster::eval_step
    pub fn exact_reachable_stall_lasso(
        &self,
        file: &Btor2File,
        p: &BDDFunction,
    ) -> Option<StallLasso> {
        let exact = self.exact_model();
        let stall = self.eg_not_p(&exact, p);
        if stall == self.ff {
            return None;
        }
        // Reach layers: L_0 = stall, L_k = L_{k-1} ∨ ◇L_{k-1}.
        let mut layers = vec![stall.clone()];
        loop {
            let prev = layers.last().unwrap();
            let next = prev.or(&exact.diamond_pre(prev)).unwrap();
            if &next == prev {
                break;
            }
            layers.push(next);
        }
        let can_reach = layers.last().unwrap().clone();
        let init = self.initial_state_bdd(file);
        let bad = init.and(&can_reach).unwrap();
        if bad == self.ff {
            return None;
        }
        // Reach phase: from an initial state that can reach a stall, descend one layer
        // per step until inside the stall, recording the reset→stall prefix.
        let mut s = self.pick_state_assignment(&bad);
        let mut prefix: Vec<BTreeMap<String, u128>> = Vec::new();
        let mut inputs: Vec<BTreeMap<String, u128>> = Vec::new();
        for _ in 0..1_000_000 {
            if self.state_minterm(&s).and(&stall).unwrap() != self.ff {
                break; // reached the stall
            }
            prefix.push(s.clone());
            // s ∈ L_k \ L_{k-1} (k minimal) ⇒ s ∈ ◇L_{k-1}: a successor lands in L_{k-1}.
            let k = (1..layers.len())
                .find(|&k| self.state_minterm(&s).and(&layers[k]).unwrap() != self.ff)
                .unwrap_or(1);
            let good = self
                .state_minterm(&s)
                .and(&exact.to_next(&layers[k - 1]))
                .unwrap();
            if good == self.ff {
                return Some(StallLasso {
                    prefix,
                    cycle: Vec::new(),
                    inputs,
                });
            }
            let full = self.pick_full_assignment(&good);
            inputs.push(self.input_assignment(&full));
            let mut ns = s.clone();
            for (reg, val) in self.eval_step(&full) {
                ns.insert(reg, val);
            }
            s = ns;
        }
        Some(self.walk_stall_cycle(&exact, &stall, s, prefix, inputs))
    }

    /// P3 — extract a concrete path witnessing that `AG EF p` is **Violated**: a state
    /// reachable from reset from which `p` is UNREACHABLE (the "trap" `¬EF p`), plus the
    /// reset→trap prefix. `¬EF p` is absorbing (if you cannot reach `p`, no successor can
    /// either), so the witness needs no cycle walk — the trap state is a single-state
    /// absorbing `cycle`. `None` when the trap is unreachable (`AG EF p` holds).
    ///
    /// Reuses the layered reachability of [`exact_reachable_stall_lasso`] with `trap = ¬EF p`
    /// in place of `stall = EG¬p`; the reach phase descends one layer per concrete
    /// [`eval_step`] until inside the trap.
    ///
    /// [`exact_reachable_stall_lasso`]: BddBitBlaster::exact_reachable_stall_lasso
    /// [`eval_step`]: BddBitBlaster::eval_step
    pub fn exact_reachable_trap_path(
        &self,
        file: &Btor2File,
        p: &BDDFunction,
    ) -> Option<StallLasso> {
        let exact = self.exact_model();
        let trap = self.not_ef_p(&exact, p);
        if trap == self.ff {
            return None; // EF p holds everywhere ⇒ AG EF p holds
        }
        // Reach layers: L_0 = trap, L_k = L_{k-1} ∨ ◇L_{k-1}.
        let mut layers = vec![trap.clone()];
        loop {
            let prev = layers.last().unwrap();
            let next = prev.or(&exact.diamond_pre(prev)).unwrap();
            if &next == prev {
                break;
            }
            layers.push(next);
        }
        let init = self.initial_state_bdd(file);
        let bad = init.and(layers.last().unwrap()).unwrap();
        if bad == self.ff {
            return None; // the trap is unreachable ⇒ AG EF p holds
        }
        // Reach phase: descend one layer per step until inside the trap, recording the prefix.
        let mut s = self.pick_state_assignment(&bad);
        let mut prefix: Vec<BTreeMap<String, u128>> = Vec::new();
        let mut inputs: Vec<BTreeMap<String, u128>> = Vec::new();
        for _ in 0..1_000_000 {
            if self.state_minterm(&s).and(&trap).unwrap() != self.ff {
                // Reached the trap: `s` witnesses `¬EF p`; it is absorbing (a 1-state cycle).
                return Some(StallLasso {
                    prefix,
                    cycle: vec![s],
                    inputs,
                });
            }
            prefix.push(s.clone());
            let k = (1..layers.len())
                .find(|&k| self.state_minterm(&s).and(&layers[k]).unwrap() != self.ff)
                .unwrap_or(1);
            let good = self
                .state_minterm(&s)
                .and(&exact.to_next(&layers[k - 1]))
                .unwrap();
            if good == self.ff {
                return Some(StallLasso {
                    prefix,
                    cycle: Vec::new(),
                    inputs,
                });
            }
            let full = self.pick_full_assignment(&good);
            inputs.push(self.input_assignment(&full));
            let mut ns = s.clone();
            for (reg, val) in self.eval_step(&full) {
                ns.insert(reg, val);
            }
            s = ns;
        }
        None
    }

    /// `¬EF p = ¬(μX. (p ∨ ◇X))` over the exact model — the "trap" region: states from which
    /// `p` is unreachable. `◇` is `∃input` (`diamond_pre`), so `EF p` is the states with SOME
    /// path to `p`. Least fixpoint from ⊥.
    fn not_ef_p(&self, exact: &ExactModel, p: &BDDFunction) -> BDDFunction {
        let mut ef = self.ff.clone();
        loop {
            let next = p.or(&exact.diamond_pre(&ef)).unwrap();
            if next == ef {
                break;
            }
            ef = next;
        }
        ef.not().unwrap()
    }

    /// `stall = EG ¬p = νZ. (¬p ∧ ◇Z)` over the exact model — the states with an
    /// infinite `p`-avoiding path (`¬⟦AF p⟧`). Greatest fixpoint from ⊤.
    fn eg_not_p(&self, exact: &ExactModel, p: &BDDFunction) -> BDDFunction {
        let not_p = p.not().unwrap();
        let mut stall = self.tt.clone();
        loop {
            let next = not_p.and(&exact.diamond_pre(&stall)).unwrap();
            if next == stall {
                break;
            }
            stall = next;
        }
        stall
    }

    /// P3 — the INPUT part of a full (state+input) assignment: the values of the design's
    /// input cells only. This is the input that drives the transition, recorded for RTL replay.
    fn input_assignment(&self, full: &HashMap<String, u128>) -> BTreeMap<String, u128> {
        self.cells
            .iter()
            .filter(|c| !c.is_state)
            .filter_map(|c| full.get(&c.symbol).map(|v| (c.symbol.clone(), *v)))
            .collect()
    }

    /// Walk a concrete `¬p` path inside `stall` from `s` until a state repeats — the
    /// cycle. `prefix` (the already-walked reach path) is prepended to the result; any
    /// pre-cycle stall states join it. `inputs` accumulates the input driving each recorded
    /// transition (for RTL replay). Each step greedily picks a stall-preserving input and
    /// advances via [`eval_step`]. Bounded by the stall's size; a deadlock yields an open lasso.
    ///
    /// [`eval_step`]: BddBitBlaster::eval_step
    fn walk_stall_cycle(
        &self,
        exact: &ExactModel,
        stall: &BDDFunction,
        mut s: BTreeMap<String, u128>,
        mut prefix: Vec<BTreeMap<String, u128>>,
        mut inputs: Vec<BTreeMap<String, u128>>,
    ) -> StallLasso {
        let mut cyc: Vec<BTreeMap<String, u128>> = Vec::new();
        for _ in 0..1_000_000 {
            if let Some(j) = cyc.iter().position(|prev| *prev == s) {
                let cycle = cyc.split_off(j);
                prefix.extend(cyc); // pre-cycle stall states → prefix
                return StallLasso {
                    prefix,
                    cycle,
                    inputs,
                };
            }
            cyc.push(s.clone());
            // The successors-in-stall set for state = s (inputs free) is
            // `state_minterm(s) ∧ to_next(stall)` — non-empty because s ∈ stall =
            // ¬p ∧ ◇stall guarantees a stall-successor under some input.
            let good = self.state_minterm(&s).and(&exact.to_next(stall)).unwrap();
            if good == self.ff {
                prefix.extend(cyc);
                return StallLasso {
                    prefix,
                    cycle: Vec::new(),
                    inputs,
                };
            }
            // Pick a full (state = s, input) assignment, step concretely, and keep
            // held registers (no `Next` line) at their current value.
            let full = self.pick_full_assignment(&good);
            inputs.push(self.input_assignment(&full));
            let mut ns = s.clone();
            for (reg, val) in self.eval_step(&full) {
                ns.insert(reg, val);
            }
            s = ns;
        }
        prefix.extend(cyc);
        StallLasso {
            prefix,
            cycle: Vec::new(),
            inputs,
        }
    }

    /// The minterm fixing only the STATE bits to `state` (input bits left free).
    fn state_minterm(&self, state: &BTreeMap<String, u128>) -> BDDFunction {
        let mut mt = self.tt.clone();
        for cell in &self.cells {
            if !cell.is_state {
                continue;
            }
            let v = state.get(&cell.symbol).copied().unwrap_or(0);
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

    /// Greedily pick one satisfying STATE assignment from a non-empty `set` (state
    /// registers only; inputs left free).
    fn pick_state_assignment(&self, set: &BDDFunction) -> BTreeMap<String, u128> {
        self.pick_assignment(set, true).into_iter().collect()
    }

    /// Greedily pick one satisfying FULL assignment (state + input) from `set`.
    fn pick_full_assignment(&self, set: &BDDFunction) -> HashMap<String, u128> {
        self.pick_assignment(set, false)
    }

    /// Greedy minterm pick: for each selected cell's bits, keep the value that leaves
    /// the residual set non-empty. `state_only` restricts to state cells.
    fn pick_assignment(&self, set: &BDDFunction, state_only: bool) -> HashMap<String, u128> {
        let mut cur = set.clone();
        let mut out: HashMap<String, u128> = HashMap::new();
        for cell in &self.cells {
            if state_only && !cell.is_state {
                continue;
            }
            let mut val = 0u128;
            for (b, var) in cell.vars.iter().enumerate() {
                let with1 = cur.and(var).unwrap();
                if with1 != self.ff {
                    cur = with1;
                    val |= 1u128 << b;
                } else {
                    cur = cur.and(&var.not().unwrap()).unwrap();
                }
            }
            out.insert(cell.symbol.clone(), val);
        }
        out
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

    /// Gated counter: `cnt` decrements only when the *untracked* register `tb`
    /// is 1, else holds; `tb' = setb` (free input). Mirrors uart_tx's `bit_cnt_q`
    /// gated by the `tick_baud_q` register. The predicate set is only `cnt == 0`
    /// (so `tb` is untracked). With the state `(cnt=1, tb=1)` FORCED to `cnt=0`
    /// regardless of input, the `∀∃` must-edge `{cnt≠0} → {cnt≠0}` must NOT hold
    /// (that state escapes). This is the exact structure where the verify-auto
    /// symbolic engine returned an (apparently unsound) definite verdict on
    /// `AG AF (bit_cnt==0)` — the brute-force oracle is ground truth.
    const GATED_COUNTER_BTOR2: &str = r#"
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 state 2 tb
5 input 2 setb
6 one 1
7 zero 1
8 eq 2 3 7
9 not 2 8
10 and 2 4 9
11 sub 1 3 6
12 ite 1 10 11 3
13 next 1 3 12
14 next 2 4 5
"#;

    #[test]
    fn rf5_soundness_gated_counter_must_relation_matches_bruteforce() {
        let predicates = [PredicateExpr::eq("cnt", 0)];
        for sem in [MustSemantics::ForallExists, MustSemantics::ForallForall] {
            assert_must_relation_matches_bruteforce(
                GATED_COUNTER_BTOR2,
                &predicates,
                &[("cnt", 2), ("tb", 1)],
                &[("setb", 1)],
                sem,
            );
        }
    }

    // ---- D1.1: exact full-state 2-valued modal pre-image vs brute force ----

    /// The `ExactModel`'s `box_pre`/`diamond_pre` over the exact (input-
    /// nondeterministic) concrete relation, checked cell-for-cell against a
    /// brute-force concrete pre-image. `φ` is the satisfying set of `pred` (a
    /// full-state BDD via `predicate_bdd`); the brute force enumerates every
    /// `(state, input)`, simulates one step, and forms `{x : ∃i. pred(next(x,i))}`
    /// (⟨⟩) and `{x : ∀i. pred(next(x,i))}` ([]) as BDDs of state minterms. BDD
    /// equality is exact (ROBDD canonical).
    fn assert_exact_modal_matches_bruteforce(
        src: &str,
        pred: &PredicateExpr,
        states: &[(&str, u32)],
        inputs: &[(&str, u32)],
    ) {
        let file = parser::parse(src).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let exact = bb.exact_model();
        let phi = bb.predicate_bdd(pred).expect("phi bdd");
        let dia = exact.diamond_pre(&phi);
        let boxed = exact.box_pre(&phi);

        // The minterm BDD (over state vars) for one register valuation.
        let state_minterm = |regs: &HashMap<String, u128>| -> BDDFunction {
            let mut m = bb.tt.clone();
            for cell in &bb.cells {
                if !cell.is_state {
                    continue;
                }
                let val = regs.get(&cell.symbol).copied().unwrap_or(0);
                for (b, var) in cell.vars.iter().enumerate() {
                    let lit = if (val >> b) & 1 == 1 {
                        var.clone()
                    } else {
                        var.not().unwrap()
                    };
                    m = m.and(&lit).unwrap();
                }
            }
            m
        };

        let total_state_bits: u32 = states.iter().map(|(_, w)| *w).sum();
        let total_input_bits: u32 = inputs.iter().map(|(_, w)| *w).sum();
        assert!(
            total_state_bits + total_input_bits <= 14,
            "keep the sweep small"
        );

        let mut expected_dia = bb.ff.clone();
        let mut expected_box = bb.ff.clone();
        for scombo in 0..(1u128 << total_state_bits) {
            let mut regs: HashMap<String, u128> = HashMap::new();
            let mut off = 0u32;
            for (name, w) in states {
                let mask = (1u128 << w) - 1;
                regs.insert((*name).to_string(), (scombo >> off) & mask);
                off += w;
            }
            let mut any = false;
            let mut all = true;
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
                let sat = pred.eval(&regs_next);
                any = any || sat;
                all = all && sat;
            }
            let m = state_minterm(&regs);
            if any {
                expected_dia = expected_dia.or(&m).unwrap();
            }
            if all {
                expected_box = expected_box.or(&m).unwrap();
            }
        }
        assert!(
            dia == expected_dia,
            "diamond_pre disagrees with brute force"
        );
        assert!(boxed == expected_box, "box_pre disagrees with brute force");
    }

    #[test]
    fn d1_1_exact_modal_gated_counter() {
        // The gated counter (cnt gated by untracked tb) — the exact model sees tb,
        // so the pre-image is precise where predicate abstraction was ⊥.
        assert_exact_modal_matches_bruteforce(
            GATED_COUNTER_BTOR2,
            &PredicateExpr::eq("cnt", 0),
            &[("cnt", 2), ("tb", 1)],
            &[("setb", 1)],
        );
    }

    #[test]
    fn d1_1_exact_modal_saturating_counter() {
        for pred in [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ] {
            assert_exact_modal_matches_bruteforce(
                SATURATING_COUNTER_BTOR2,
                &pred,
                &[("cnt", 2)],
                &[("en", 1)],
            );
        }
    }

    // ---- D1.2: exact full-state 2-valued μ-evaluator vs evaluate_tri ----

    /// The exact `ExactModel::evaluate` (2-valued, full-state BDD) agrees with the
    /// reference `evaluate_tri` on the equivalent **Sharp concrete** Kripke
    /// structure (states = concrete register valuations, Sharp edges over every
    /// input). Both are exact ⇒ definite; a state is in the exact set iff
    /// `evaluate_tri` returns `True` there. Includes `AF`/`AG AF` — the liveness
    /// the *abstraction* path answers ⊥ but the exact path decides.
    fn assert_exact_eval_matches_tri(
        src: &str,
        predicates: &[(&str, PredicateExpr)],
        states: &[(&str, u32)],
        inputs: &[(&str, u32)],
        formulas: &[&str],
    ) {
        use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, TransitionModality, Tristate};
        use crate::mu_calculus::evaluator::{Environment, evaluate_tri};
        use crate::mu_calculus::trit::Trit;

        let file = parser::parse(src).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let exact = bb.exact_model();
        let atoms: HashMap<&str, BDDFunction> = predicates
            .iter()
            .map(|(n, e)| (*n, bb.predicate_bdd(e).expect("pred bdd")))
            .collect();

        let total_state_bits: u32 = states.iter().map(|(_, w)| *w).sum();
        let total_input_bits: u32 = inputs.iter().map(|(_, w)| *w).sum();
        assert!(
            total_state_bits + total_input_bits <= 14,
            "keep the sweep small"
        );
        let n = 1usize << total_state_bits;

        let decode = |combo: u128| -> HashMap<String, u128> {
            let mut regs = HashMap::new();
            let mut off = 0u32;
            for (name, w) in states {
                let mask = (1u128 << w) - 1;
                regs.insert((*name).to_string(), (combo >> off) & mask);
                off += w;
            }
            regs
        };
        let encode = |regs: &HashMap<String, u128>| -> usize {
            let mut combo = 0usize;
            let mut off = 0u32;
            for (name, w) in states {
                let v = regs.get(*name).copied().unwrap_or(0) as usize;
                combo |= (v & ((1usize << w) - 1)) << off;
                off += w;
            }
            combo
        };

        // Explicit Sharp Kripke structure over the 2^bits concrete states.
        let mut b = Clts::<DefaultStateIdx, DefaultLabelIdx>::builder();
        for j in 0..n {
            b.state(format!("s{j}"));
        }
        b.initial("s0");
        let step = b.labels().intern(["step"]).unwrap();
        let ids: Vec<_> = (0..n)
            .map(|j| b.state_id_or_insert(format!("s{j}")).unwrap())
            .collect();
        for (j, &id) in ids.iter().enumerate() {
            let regs = decode(j as u128);
            for (name, e) in predicates {
                let v = if e.eval(&regs) {
                    Tristate::KleeneT
                } else {
                    Tristate::KleeneF
                };
                b.with_3valued_predicate(id, *name, v);
            }
        }
        for (j, &id) in ids.iter().enumerate() {
            let regs = decode(j as u128);
            for icombo in 0..(1u128 << total_input_bits) {
                let mut inps = HashMap::new();
                let mut ioff = 0u32;
                for (name, w) in inputs {
                    let mask = (1u128 << w) - 1;
                    inps.insert((*name).to_string(), (icombo >> ioff) & mask);
                    ioff += w;
                }
                let next = simulate_one_step(&file, &regs, &inps).expect("step");
                let mut regs_next = regs.clone();
                regs_next.extend(next);
                let jt = encode(&regs_next);
                b.transition_ids_with_modality(id, &[step], ids[jt], TransitionModality::Sharp);
            }
        }
        let clts = b.build().expect("clts builds");

        let state_minterm = |regs: &HashMap<String, u128>| -> BDDFunction {
            let mut m = bb.tt.clone();
            for cell in &bb.cells {
                if !cell.is_state {
                    continue;
                }
                let val = regs.get(&cell.symbol).copied().unwrap_or(0);
                for (bit, var) in cell.vars.iter().enumerate() {
                    let lit = if (val >> bit) & 1 == 1 {
                        var.clone()
                    } else {
                        var.not().unwrap()
                    };
                    m = m.and(&lit).unwrap();
                }
            }
            m
        };

        let env = Environment::new(clts.state_count());
        for fs in formulas {
            let formula = crate::mu_calculus::parser::parse(fs).expect("formula parses");
            let tri = evaluate_tri(&formula, &clts, &env).expect("evaluate_tri");
            let ex = exact.evaluate(&formula, &atoms).expect("exact evaluate");
            for j in 0..n {
                let regs = decode(j as u128);
                let m = state_minterm(&regs);
                let in_exact = m.and(&ex).unwrap() == m;
                let tri_true = tri.verdict_at(j) == Trit::True;
                assert_eq!(
                    in_exact,
                    tri_true,
                    "formula `{fs}` at s{j} ({regs:?}): exact∈={in_exact} tri={:?}",
                    tri.verdict_at(j)
                );
            }
        }
    }

    #[test]
    fn d1_2_exact_eval_gated_counter_matches_tri() {
        // `cnt` gated by untracked `tb`: AF(cnt==0) is definite-False at states that
        // can loop with tb=0 (setb kept 0), definite elsewhere — the exact path
        // decides it where abstraction was ⊥.
        assert_exact_eval_matches_tri(
            GATED_COUNTER_BTOR2,
            &[("p", PredicateExpr::eq("cnt", 0))],
            &[("cnt", 2), ("tb", 1)],
            &[("setb", 1)],
            &[
                "[] p",
                "<> p",
                "not p",
                "nu X. (p and [] X)",                   // AG p
                "mu X. (p or <> X)",                    // EF p
                "mu X. (p or [] X)",                    // AF p  (the liveness)
                "nu X. ((mu Y. (p or [] Y)) and [] X)", // AG AF p
                "nu Y. ((mu X. (p or <> X)) and [] Y)", // AG EF p
            ],
        );
    }

    #[test]
    fn d1_2_exact_eval_saturating_counter_matches_tri() {
        assert_exact_eval_matches_tri(
            SATURATING_COUNTER_BTOR2,
            &[
                ("p", PredicateExpr::eq("cnt", 0)),
                (
                    "q",
                    PredicateExpr::Cmp {
                        register: "cnt".to_string(),
                        op: CmpOp::Ge,
                        value: 2,
                    },
                ),
            ],
            &[("cnt", 2)],
            &[("en", 1)],
            &[
                "[] q",
                "<> q",
                "p or q",
                "mu X. (q or [] X)",                    // AF q
                "nu X. ((mu Y. (q or [] Y)) and [] X)", // AG AF q
                "nu Y. ((mu X. (q or <> X)) and [] Y)", // AG EF q
            ],
        );
    }

    // ---- D1.3: init-state verdict + the exact_symbolic_verdict entry point ----

    /// A 2-bit wrapping up-counter with an enable, RESET to 0 via a BTOR2 `init`
    /// line — so `initial_state_bdd` pins `cnt == 0` and the verdict is a real
    /// initial-state check.
    const RESET_COUNTER_BTOR2: &str = r#"
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 input 2 en
5 one 1
6 zero 1
7 add 1 3 5
8 ite 1 4 7 3
9 next 1 3 8
10 init 1 3 6
"#;

    #[test]
    fn d1_3_exact_verdict_reset_counter() {
        // Verdicts are AT the initial state (cnt == 0), computed exactly.
        let cases = [
            ("(cnt == 0)", ExactVerdict::Holds), // init is 0
            ("not (cnt == 0)", ExactVerdict::Violated),
            ("[] (cnt == 0)", ExactVerdict::Violated), // successor cnt=1 possible
            ("mu X. ((cnt == 3) or <> X)", ExactVerdict::Holds), // EF cnt==3 (reachable)
            ("mu X. ((cnt == 3) or [] X)", ExactVerdict::Violated), // AF cnt==3 (en=0 stalls at 0)
            (
                "nu Y. ((mu X. ((cnt == 0) or <> X)) and [] Y)", // AG EF cnt==0 (wraps back)
                ExactVerdict::Holds,
            ),
        ];
        for (fs, expected) in cases {
            let formula = crate::mu_calculus::parser::parse(fs).expect("formula parses");
            let v = exact_symbolic_verdict(RESET_COUNTER_BTOR2, &formula).expect("verdict");
            assert_eq!(v, expected, "formula `{fs}`");
        }
    }

    /// Without an `init` line the initial-state set is unconstrained (every state),
    /// so `Holds` means the property holds from EVERY state. `AG AF (cnt==0)` on
    /// the *saturating* counter is `Violated` (cnt saturates at 3 and never returns
    /// to 0) — a definite verdict the abstraction path could only give as ⊥.
    #[test]
    fn d1_3_exact_verdict_no_init_is_global() {
        let f = crate::mu_calculus::parser::parse("nu X. ((mu Y. ((cnt == 0) or [] Y)) and [] X)")
            .expect("formula");
        assert_eq!(
            exact_symbolic_verdict(SATURATING_COUNTER_BTOR2, &f).expect("verdict"),
            ExactVerdict::Violated,
        );
    }

    // Frozen-register regression (2026-07-05) — a register that LOADS a nonzero
    // value from a free input `ld` (`reg_next = ld ? 10 : reg`). Two shapes that
    // MUST decide identically:
    //   (A) the state line carries the symbol directly (`state 2 bit_cnt_q`);
    //   (B) the state line is UNNAMED and the user-visible name survives only on a
    //       `uext _ NID 0 NAME` alias (`state 2` + `uext 2 4 0 bit_cnt_q`) — the
    //       exact shape yosys emits after flatten + async2sync + dffunmap.
    // The bit-blaster names its cells via `collect_symbols` (alias-aware), so both
    // cells are named `bit_cnt_q`. The bug: `next_funcs` was keyed off the RAW
    // state symbol (`state_<nid>` for (B)), so `next_funcs.get(&cell.symbol)` missed
    // and (B)'s register was left with NO next-state function — silently FROZEN at
    // its init value. `EF(bit_cnt_q != 0)` then read `Violated` (a reachable state
    // reported unreachable) for (B) while (A) correctly read `Holds`. From any
    // state, asserting `ld` reaches `bit_cnt_q = 10`, so the sound verdict is
    // `Holds` for BOTH.
    const FROZEN_REG_ATOM_ON_STATE: &str = r#"
1 sort bitvec 1
2 sort bitvec 4
3 input 1 ld
4 state 2 bit_cnt_q
5 const 2 0000
6 const 2 1010
7 ite 2 3 6 4
8 next 2 4 7
"#;
    const FROZEN_REG_ATOM_ON_UEXT_ALIAS: &str = r#"
1 sort bitvec 1
2 sort bitvec 4
3 input 1 ld
4 state 2
5 const 2 0000
6 const 2 1010
7 ite 2 3 6 4
8 next 2 4 7
9 uext 2 4 0 bit_cnt_q
"#;

    #[test]
    fn exact_next_func_binds_to_uext_aliased_register_not_frozen() {
        // The regression: named-state and uext-aliased-state must decide the SAME.
        let ef = crate::mu_calculus::parser::parse("mu Y. ((not (bit_cnt_q == 0)) or <> Y)")
            .expect("EF formula");
        let named = exact_symbolic_verdict(FROZEN_REG_ATOM_ON_STATE, &ef).expect("verdict");
        let aliased = exact_symbolic_verdict(FROZEN_REG_ATOM_ON_UEXT_ALIAS, &ef).expect("verdict");
        assert_eq!(
            named,
            ExactVerdict::Holds,
            "EF(bit_cnt_q != 0) is reachable (assert `ld`), so a named state decides Holds"
        );
        assert_eq!(
            aliased, named,
            "a uext-ALIASED state must decide identically to a named one — not a frozen \
             register (Violated) from a `next_funcs` key that missed the alias resolution"
        );
        // The AF-liveness companion also agrees: with `ld` free, `ld=0` forever holds
        // the loaded value, so `AG AF(bit_cnt_q == 0)` is a real Violated on BOTH.
        let agaf = crate::mu_calculus::parser::parse(
            "nu X. ((mu Y. ((bit_cnt_q == 0) or [] Y)) and [] X)",
        )
        .expect("AGAF formula");
        assert_eq!(
            exact_symbolic_verdict(FROZEN_REG_ATOM_ON_UEXT_ALIAS, &agaf).expect("verdict"),
            exact_symbolic_verdict(FROZEN_REG_ATOM_ON_STATE, &agaf).expect("verdict"),
            "AG AF must also decide identically for named vs uext-aliased state"
        );
    }

    /// A binary constant WIDER than 128 bits (out of the property cone) must not fail the exact
    /// MC — it is built per-bit from the string, not parsed to u128. `wide` is a 130-bit const
    /// register the FSM never reads; the property is over the 1-bit `fsm`. Regression for the
    /// keymgr_ctrl "bad binary literal" Skip (256-bit key-state constants).
    #[test]
    fn exact_verdict_tolerates_over_128_bit_binary_constant() {
        // 130-bit `wide` const held in a register out of the `fsm` cone; `fsm` toggles 0↔1.
        let bits130 = "1".to_string() + &"0".repeat(129); // 130-char MSB-first binary literal
        let btor2 = format!(
            "1 sort bitvec 1\n2 sort bitvec 130\n3 const 2 {bits130}\n4 state 1 fsm\n5 state 2 wide\n\
             6 not 1 4\n7 next 1 4 6\n8 next 2 5 3\n9 zero 1\n10 init 1 4 9\n11 init 2 5 3\n"
        );
        let formula = crate::mu_calculus::parser::parse("mu Y. ((fsm == 1) or <> Y)")
            .expect("EF formula parses");
        // Must DECIDE (not error on the 130-bit const): fsm reaches 1 ⇒ EF(fsm==1) holds.
        assert_eq!(
            exact_symbolic_verdict(&btor2, &formula),
            Ok(ExactVerdict::Holds),
            "a > 128-bit out-of-cone binary constant must not fail the exact MC"
        );
    }

    /// `exact_bad_reachable` — a reachable `bad` (a 3-bit counter reaches `0b111`) is SAT.
    /// The btor2tools `count2` shape: init 0, `next = s + 1`, `bad = (s == 7)`.
    #[test]
    fn exact_bad_reachable_true_on_reaching_counter() {
        const COUNT2: &str = r#"
1 sort bitvec 3
2 zero 1
3 state 1
4 init 1 3 2
5 one 1
6 add 1 3 5
7 next 1 3 6
8 ones 1
9 sort bitvec 1
10 eq 9 3 8
11 bad 10
"#;
        assert_eq!(
            exact_bad_reachable(COUNT2),
            Ok(true),
            "a 3-bit counter from 0 reaches 0b111 ⇒ bad is reachable (SAT)"
        );
    }

    /// `exact_bad_reachable` — an unreachable `bad` is UNSAT: a register held at 0 can never
    /// equal 1, so `bad = (s == 1)` is unreachable from the reset state.
    #[test]
    fn exact_bad_reachable_false_on_stuck_register() {
        const STUCK: &str = r#"
1 sort bitvec 1
2 zero 1
3 state 1 s
4 init 1 3 2
5 next 1 3 3
6 one 1
7 eq 1 3 6
8 bad 7
"#;
        assert_eq!(
            exact_bad_reachable(STUCK),
            Ok(false),
            "s is held at 0 ⇒ (s == 1) is never reachable (UNSAT / safe)"
        );
    }

    /// `exact_bad_reachable` — the soundness guard: a design with a `constraint` line is
    /// UNDECIDED (Err), never an over-approximate verdict (this engine does not restrict
    /// reachability to constraint-satisfying runs).
    #[test]
    fn exact_bad_reachable_refuses_constrained_design() {
        const CONSTRAINED: &str = r#"
1 sort bitvec 1
2 zero 1
3 state 1 s
4 init 1 3 2
5 next 1 3 3
6 one 1
7 eq 1 3 6
8 bad 7
9 input 1 c
10 constraint 9
"#;
        assert!(
            exact_bad_reachable(CONSTRAINED).is_err(),
            "a constrained design must be undecided, not over-approximated"
        );
    }

    /// D1.7 — the exact engine DEGRADES GRACEFULLY on a design too large to
    /// bit-blast. `BddBitBlaster::build` rejects a design whose register+input
    /// bit count exceeds `MAX_BITBLAST_BITS` (40) with a clean `Err` — and does so
    /// BEFORE allocating the BDD manager, so there is no OoM / hang. `build` is the
    /// first thing `exact_symbolic_verdict` calls, so the whole exact path returns
    /// `Err` (which `verify_auto`'s exact branch maps to a `Skipped` property, per
    /// `e2e_sysrst`-style graceful degradation). This locks that OoM-safety
    /// guarantee for the exact path as a fast, non-docker regression.
    #[test]
    fn d1_7_exact_verdict_over_bit_cap_degrades_gracefully() {
        // One 48-bit register (48 > 40 register+input bits), no inputs. The
        // formula is immaterial: the cap fires in `build`, before atom
        // resolution or fixpoint evaluation.
        const WIDE_BTOR2: &str = r#"
1 sort bitvec 48
2 sort bitvec 1
3 state 1 wide
4 one 1
5 add 1 3 4
6 next 1 3 5
"#;
        let formula = crate::mu_calculus::parser::parse("(wide == 0)").expect("formula parses");
        let err = exact_symbolic_verdict(WIDE_BTOR2, &formula)
            .expect_err("48-bit design exceeds MAX_BITBLAST_BITS (40) → clean Err, not OoM");
        assert!(
            err.contains("register+input bits") && err.contains("48"),
            "the error must name the bit count + cap so a caller can degrade to \
             Skipped; got: {err}"
        );
    }

    /// R-F5.6 — cone-of-influence restriction lifts the bit cap when the property's cone is
    /// small even though the FULL design is over 40 bits. `fsm` (2-bit) cycles 1→2→3→0→…; `wide`
    /// (45-bit) is an out-of-cone counter `fsm` never reads. The full build hits the cap (47 >
    /// 40), but `exact_symbolic_verdict` restricts to the cone of `fsm == 0` = {fsm} (pinning
    /// `wide` to a constant) and DECIDES `EF (fsm == 0)` = Holds. Locks that (a) COI is wired
    /// into the exact path, and (b) pinning the out-of-cone datapath is verdict-preserving —
    /// the whole point of R-F5.6, as a fast non-docker regression.
    #[test]
    fn rf5_6_coi_lifts_bit_cap_on_out_of_cone_datapath() {
        const FSM_PLUS_WIDE_CTR: &str = r#"
1 sort bitvec 2
2 sort bitvec 45
3 state 1 fsm
4 one 1
5 add 1 3 4
6 next 1 3 5
7 zero 1
8 init 1 3 4
9 state 2 wide
10 one 2
11 add 2 9 10
12 next 2 9 11
"#;
        // The full design is 2 + 45 = 47 bits — over the cap.
        let file = parser::parse(FSM_PLUS_WIDE_CTR).expect("parse");
        let full_err = BddBitBlaster::build(&file).err();
        assert!(
            full_err.as_ref().is_some_and(|e| e.contains("47")),
            "full 47-bit build must still hit the cap; got {full_err:?}",
        );
        // The exact verdict restricts to the {fsm} cone (2 bits), pinning `wide` → decidable.
        // `fsm` inits to 1 and cycles to 0, so `EF (fsm == 0)` holds from the initial state.
        let formula =
            crate::mu_calculus::parser::parse("mu Y. ((fsm == 0) or <> Y)").expect("formula");
        let verdict = exact_symbolic_verdict(FSM_PLUS_WIDE_CTR, &formula)
            .expect("COI restricts to the 2-bit fsm cone ⇒ decidable, not capped");
        assert_eq!(
            verdict,
            ExactVerdict::Holds,
            "fsm cycles to 0 ⇒ EF(fsm==0) holds; the 45-bit out-of-cone `wide` is pinned",
        );
    }

    /// The bit-blasted `Mul` (shift-and-add) over a SYMBOLIC operand (not a constant fold):
    /// `p' = x * 3` on a 4-bit input `x`. `EF (p == 6)` holds (x=2 ⇒ 6 mod 16), so the
    /// multiplier is exercised end-to-end. Locks the `Op::Mul` arm that unblocks the
    /// prim_packer_fifo / prim_fifo_sync drainability properties in the corpus.
    #[test]
    fn mul_bit_blaster_shift_and_add() {
        const MUL_BTOR2: &str = r#"
1 sort bitvec 4
2 input 1 x
3 const 1 0011
4 mul 1 2 3
5 state 1 p
6 next 1 5 4
7 const 1 0000
8 init 1 5 7
"#;
        let formula =
            crate::mu_calculus::parser::parse("mu Y. ((p == 6) or <> Y)").expect("formula");
        assert_eq!(
            exact_symbolic_verdict(MUL_BTOR2, &formula).expect("mul decides"),
            ExactVerdict::Holds,
            "p' = x*3; x=2 ⇒ p==6 reachable ⇒ EF(p==6) holds",
        );
    }

    /// The bit-blasted variable `Srl` barrel shifter: `p' = 12 >> s` over a 4-bit shift input
    /// `s`. `12 >> s ∈ {12,6,3,1,0}`, so `EF(p==3)` holds (s=2) but `EF(p==5)` is VIOLATED (5 is
    /// never reachable) — a precise check of the shift amount/direction/fill. Unblocks the
    /// prim_packer_fifo / prim_fifo_sync drainability cones (which use `Srl`).
    #[test]
    fn srl_bit_blaster_barrel_shift() {
        const SRL_BTOR2: &str = r#"
1 sort bitvec 4
2 const 1 1100
3 input 1 s
4 srl 1 2 3
5 state 1 p
6 next 1 5 4
7 const 1 0000
8 init 1 5 7
"#;
        let reachable =
            crate::mu_calculus::parser::parse("mu Y. ((p == 3) or <> Y)").expect("formula");
        assert_eq!(
            exact_symbolic_verdict(SRL_BTOR2, &reachable).expect("srl decides"),
            ExactVerdict::Holds,
            "12 >> 2 == 3 ⇒ EF(p==3) holds",
        );
        let unreachable =
            crate::mu_calculus::parser::parse("mu Y. ((p == 5) or <> Y)").expect("formula");
        assert_eq!(
            exact_symbolic_verdict(SRL_BTOR2, &unreachable).expect("srl decides"),
            ExactVerdict::Violated,
            "12 >> s is never 5 ⇒ EF(p==5) is violated (validates exact shift semantics)",
        );
    }

    /// The bit-blasted signed `Slt`: `p' = (x <s 0)` over a 4-bit input `x`. `EF(p==1)` holds
    /// because x ∈ {8..15} are NEGATIVE two's-complement (−8..−1) — a verdict an UNSIGNED
    /// comparison would get wrong (x <u 0 is never true ⇒ it would report Violated). Unblocks
    /// prim_fifo_sync (its drainability cone uses `Slt`).
    #[test]
    fn slt_bit_blaster_signed_less_than() {
        const SLT_BTOR2: &str = r#"
1 sort bitvec 1
2 sort bitvec 4
3 input 2 x
4 const 2 0000
5 slt 1 3 4
6 state 1 p
7 next 1 6 5
8 const 1 0
9 init 1 6 8
"#;
        let formula =
            crate::mu_calculus::parser::parse("mu Y. ((p == 1) or <> Y)").expect("formula");
        assert_eq!(
            exact_symbolic_verdict(SLT_BTOR2, &formula).expect("slt decides"),
            ExactVerdict::Holds,
            "x in 8..15 are signed-negative so x <s 0 and EF(p==1) holds (unsigned would fail)",
        );
    }

    /// A predicate binds to a named COMBINATIONAL signal, not just a state register: `o = p + 1`
    /// is a module OUTPUT (no `next` line), and `EF(o == 3)` holds (p reaches 2 ⇒ o = 3). Locks
    /// the `named_signals` binding that unblocks corpus atoms over combinational outputs
    /// (`depth_o`, `gnt_o`) which are not post-synthesis registers.
    #[test]
    fn predicate_binds_combinational_output() {
        const COMB_BTOR2: &str = r#"
1 sort bitvec 4
2 state 1 p
3 one 1
4 add 1 2 3
5 output 4 o
6 next 1 2 4
7 const 1 0000
8 init 1 2 7
"#;
        let formula =
            crate::mu_calculus::parser::parse("mu Y. ((o == 3) or <> Y)").expect("formula");
        assert_eq!(
            exact_symbolic_verdict(COMB_BTOR2, &formula).expect("combinational atom binds"),
            ExactVerdict::Holds,
            "o = p+1 is a combinational output; p reaches 2 ⇒ o==3 ⇒ EF(o==3) holds",
        );
    }

    /// P3 — the trap-path witness for a `Violated` `AG EF (st==0)` recoverability property.
    /// `st` cycles 0→1→2→0, but `esc` drives it to the TERMINAL trap `st==3` (self-loop), from
    /// which `st==0` is unreachable — so `AG EF (st==0)` is Violated, and the witness is a
    /// concrete reset→trap path ending in the absorbing trap state. Self-validating: it replays
    /// the prefix through `eval_step` and confirms the final state is the trap `st==3`.
    #[test]
    fn p3_ag_ef_trap_path_witness() {
        const TRAP_FSM: &str = r#"
1 sort bitvec 1
2 sort bitvec 2
3 input 1 esc
4 state 2 st
5 const 2 11
6 const 2 00
7 one 2
8 add 2 4 7
9 const 2 10
10 eq 1 4 9
11 ite 2 10 6 8
12 eq 1 4 5
13 ite 2 12 5 11
14 ite 2 3 5 13
15 next 2 4 14
16 init 2 4 6
"#;
        let formula =
            crate::mu_calculus::parser::parse("nu Y. ((mu X. ((st == 0) or <> X)) and [] Y)")
                .expect("formula");
        let (verdict, witness) =
            exact_symbolic_verdict_with_witness(TRAP_FSM, &formula).expect("exact verdict");
        assert_eq!(
            verdict,
            ExactVerdict::Violated,
            "st==3 is a reachable terminal trap ⇒ AG EF (st==0) is Violated",
        );
        let w = witness.expect("a Violated AG EF must yield a reachable-trap-path witness");
        // The witness ends in the absorbing trap `st==3` (its single-state cycle).
        let trap_state = w
            .cycle
            .last()
            .or_else(|| w.prefix.last())
            .expect("non-empty witness");
        assert_eq!(
            trap_state.get("st").copied(),
            Some(3),
            "the witness ends in the terminal trap st==3; got {trap_state:?}",
        );
        // The prefix starts at the reset state (st==0) — a genuine reset→trap path.
        if let Some(first) = w.prefix.first() {
            assert_eq!(
                first.get("st").copied(),
                Some(0),
                "the witness prefix starts at the reset state st==0",
            );
        }
        // SELF-VALIDATE the recorded inputs: replaying `inputs[i]` from `prefix[i]` via the
        // concrete eval_step must land on the next path state — i.e. the witness's input
        // sequence genuinely reproduces the reset→trap path (the basis for RTL replay).
        assert_eq!(
            w.inputs.len(),
            w.prefix.len(),
            "one recorded input per prefix transition",
        );
        let file = parser::parse(TRAP_FSM).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let path: Vec<_> = w.prefix.iter().chain(w.cycle.iter()).collect();
        for (i, inp) in w.inputs.iter().enumerate() {
            // full = state(path[i]) + input(inp)
            let mut full: HashMap<String, u128> =
                path[i].iter().map(|(k, v)| (k.clone(), *v)).collect();
            for (k, v) in inp {
                full.insert(k.clone(), *v);
            }
            let stepped = bb.eval_step(&full);
            let next_st = stepped.get("st").copied().unwrap_or(path[i]["st"]);
            assert_eq!(
                Some(next_st),
                path[i + 1].get("st").copied(),
                "replaying input {i} from st={} must reach the next path state",
                path[i]["st"],
            );
        }
    }

    /// D1.8 — the stall-lasso witness for a `Violated` `AF (cnt==3)` on the reset
    /// counter. `en=0` holds `cnt`, so from the reset state (`cnt=0`) there is a
    /// `¬(cnt==3)` path that never reaches 3 — `AF (cnt==3)` fails, and the witness
    /// is a concrete lasso. The test is **self-validating**: it replays every lasso
    /// edge through the concrete `eval_step` simulator (there must be an input
    /// realising each step) and confirms every state is `¬p` and the cycle closes.
    #[test]
    fn d1_8_exact_stall_lasso_reset_counter() {
        let file = parser::parse(RESET_COUNTER_BTOR2).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let p = bb
            .predicate_bdd(&PredicateExpr::eq("cnt", 3))
            .expect("predicate cnt==3");

        let lasso = bb
            .exact_stall_lasso(&file, &p)
            .expect("AF(cnt==3) is Violated on the reset counter → a stall lasso exists");
        assert!(!lasso.cycle.is_empty(), "the stall must close into a cycle");

        // The witness starts at the reset state (cnt = 0).
        let first = lasso
            .prefix
            .first()
            .or_else(|| lasso.cycle.first())
            .expect("non-empty lasso");
        assert_eq!(
            first.get("cnt"),
            Some(&0),
            "lasso starts at the reset state"
        );

        // Every lasso state avoids p (cnt != 3).
        for st in lasso.prefix.iter().chain(lasso.cycle.iter()) {
            assert_ne!(st.get("cnt"), Some(&3), "every stall state is ¬(cnt==3)");
        }

        // Self-validate: every consecutive edge (including the cycle's closing edge
        // back to cycle[0]) is realised by SOME concrete input via eval_step.
        let step_exists = |from: &BTreeMap<String, u128>, to: &BTreeMap<String, u128>| -> bool {
            (0..=1u128).any(|en| {
                let mut asg: HashMap<String, u128> =
                    from.iter().map(|(k, v)| (k.clone(), *v)).collect();
                asg.insert("en".to_string(), en);
                let next = bb.eval_step(&asg);
                next.get("cnt").copied().unwrap_or(0) == *to.get("cnt").unwrap()
            })
        };
        let mut path: Vec<&BTreeMap<String, u128>> =
            lasso.prefix.iter().chain(lasso.cycle.iter()).collect();
        // Close the loop: the last cycle state steps back to cycle[0].
        path.push(&lasso.cycle[0]);
        for w in path.windows(2) {
            assert!(
                step_exists(w[0], w[1]),
                "lasso edge {:?} -> {:?} must be a real transition",
                w[0],
                w[1]
            );
        }
    }

    /// D1.8 — `AF (cnt==0)` HOLDS on the reset counter from cnt=0 (it's already 0),
    /// so there is no stall and no lasso.
    #[test]
    fn d1_8_exact_stall_lasso_none_when_af_holds() {
        let file = parser::parse(RESET_COUNTER_BTOR2).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let p = bb
            .predicate_bdd(&PredicateExpr::eq("cnt", 0))
            .expect("predicate cnt==0");
        // cnt==0 holds at the initial state, so AF(cnt==0) holds ⇒ no stall witness.
        assert!(bb.exact_stall_lasso(&file, &p).is_none());
    }

    /// D1.8b — the verdict entry point also yields the stall lasso for a Violated
    /// bare `AF p`, and `None` when it holds.
    #[test]
    fn d1_8b_verdict_with_witness_af_violated() {
        let af = crate::mu_calculus::parser::parse("mu X. ((cnt == 3) or [] X)").expect("parse");
        let (v, w) =
            exact_symbolic_verdict_with_witness(RESET_COUNTER_BTOR2, &af).expect("verdict");
        assert_eq!(v, ExactVerdict::Violated);
        let lasso = w.expect("Violated AF ⇒ a stall lasso witness");
        assert!(!lasso.cycle.is_empty());
        for st in lasso.prefix.iter().chain(lasso.cycle.iter()) {
            assert_ne!(st.get("cnt"), Some(&3), "every stall state is ¬(cnt==3)");
        }

        // `AF (cnt==0)` holds at reset (cnt is already 0) ⇒ no witness.
        let holds = crate::mu_calculus::parser::parse("mu X. ((cnt == 0) or [] X)").expect("parse");
        let (v2, w2) =
            exact_symbolic_verdict_with_witness(RESET_COUNTER_BTOR2, &holds).expect("verdict");
        assert_eq!(v2, ExactVerdict::Holds);
        assert!(w2.is_none());
    }

    // D1.8b-2 — a counter where the stall is REACHABLE but not initial: from cnt=0 the
    // next state is forced to 1 (so `AF (cnt==1)` HOLDS at reset), but `cnt=2`/`cnt=3`
    // can hold under `!go` forever (a `¬(cnt==1)` stall), reachable via 0→1→2. So
    // `AG AF (cnt==1)` is Violated by a reachable-not-initial stall.
    const REACH_STALL_BTOR2: &str = r#"
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 input 2 go
5 zero 1
6 one 1
7 eq 2 3 5
8 add 1 3 6
9 ite 1 4 8 3
10 ite 1 7 6 9
11 next 1 3 10
12 init 1 3 5
"#;

    /// D1.8b-2 — `exact_reachable_stall_lasso` finds a stall reachable from reset that
    /// `exact_stall_lasso` (stall-at-reset only) misses. The lasso's cycle is `¬p`; the
    /// prefix is the reset→stall reach path (and MAY pass through a `p` state — the
    /// point of `AG AF p` is that `p` holds only finitely).
    #[test]
    fn d1_8b2_reachable_stall_lasso() {
        let file = parser::parse(REACH_STALL_BTOR2).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let p = bb
            .predicate_bdd(&PredicateExpr::eq("cnt", 1))
            .expect("predicate cnt==1");

        // Stall-at-reset finds nothing: from cnt=0 every path reaches cnt=1.
        assert!(
            bb.exact_stall_lasso(&file, &p).is_none(),
            "AF(cnt==1) holds at reset — no stall AT reset"
        );

        // Reachable-stall finds the AG-AF witness.
        let lasso = bb
            .exact_reachable_stall_lasso(&file, &p)
            .expect("a ¬(cnt==1) stall is reachable from reset");
        assert!(
            !lasso.cycle.is_empty(),
            "the reachable stall must close a cycle"
        );
        assert_eq!(
            lasso.prefix.first().and_then(|st| st.get("cnt")),
            Some(&0),
            "the lasso starts at the reset state"
        );
        // The CYCLE avoids p forever (only the prefix may touch p finitely).
        for st in &lasso.cycle {
            assert_ne!(st.get("cnt"), Some(&1), "the stall cycle is ¬(cnt==1)");
        }
        // Self-validate every edge (incl. the closing edge) via the concrete simulator.
        let step_exists = |from: &BTreeMap<String, u128>, to: &BTreeMap<String, u128>| -> bool {
            (0..=1u128).any(|go| {
                let mut asg: HashMap<String, u128> =
                    from.iter().map(|(k, v)| (k.clone(), *v)).collect();
                asg.insert("go".to_string(), go);
                bb.eval_step(&asg).get("cnt").copied().unwrap_or(0) == *to.get("cnt").unwrap()
            })
        };
        let mut path: Vec<&BTreeMap<String, u128>> =
            lasso.prefix.iter().chain(lasso.cycle.iter()).collect();
        path.push(&lasso.cycle[0]);
        for w in path.windows(2) {
            assert!(
                step_exists(w[0], w[1]),
                "lasso edge {:?} -> {:?} must be a real transition",
                w[0],
                w[1]
            );
        }
    }

    /// D1.8b-2 — the verdict entry point surfaces the reachable-stall witness for a
    /// Violated `AG AF p`.
    #[test]
    fn d1_8b2_verdict_with_witness_ag_af_violated() {
        let ag_af =
            crate::mu_calculus::parser::parse("nu Y. ((mu X. ((cnt == 1) or [] X)) and [] Y)")
                .expect("parse");
        let (v, w) =
            exact_symbolic_verdict_with_witness(REACH_STALL_BTOR2, &ag_af).expect("verdict");
        assert_eq!(v, ExactVerdict::Violated, "AG AF(cnt==1) is Violated");
        let lasso = w.expect("Violated AG AF ⇒ a reachable-stall witness");
        assert!(!lasso.cycle.is_empty());
        for st in &lasso.cycle {
            assert_ne!(st.get("cnt"), Some(&1));
        }
    }

    /// D1.8c — the generated GR(1) response formula `GF a → GF b` gives the correct
    /// **definite** verdict over the exact engine on hand-verified models. Validates
    /// the soundness-critical alternating-fixpoint structure of `gr1_response_formula`
    /// against known GR(1) truth.
    #[test]
    fn d1_8c_gr1_response_formula_verdicts() {
        let formula = crate::mu_calculus::parser::parse(&gr1_response_formula("s == 0", "s == 1"))
            .expect("gr1 formula parses");

        // TOGGLE: forced s0⇄s1. Every path hits s==0 and s==1 infinitely, so
        // `GF(s==0) → GF(s==1)` HOLDS (no ¬(s==1) cycle exists).
        const TOGGLE: &str = r#"
1 sort bitvec 1
2 state 1 s
3 not 1 2
4 next 1 2 3
5 zero 1
6 init 1 2 5
"#;
        assert_eq!(
            exact_symbolic_verdict(TOGGLE, &formula).expect("verdict"),
            ExactVerdict::Holds,
            "forced toggle: GF(s==0) → GF(s==1) holds"
        );

        // STAYLOOP: s0 (s==0, ¬(s==1)) self-loops under `stay`, else advances to s1.
        // `stay=1` forever keeps s==0 (assume recurs) and never reaches s==1 — a
        // reachable assume-recurring guarantee-avoiding cycle ⇒ VIOLATED.
        const STAYLOOP: &str = r#"
1 sort bitvec 1
2 state 1 s
3 input 1 stay
4 zero 1
5 one 1
6 eq 1 2 4
7 ite 1 3 4 5
8 ite 1 6 7 4
9 next 1 2 8
10 init 1 2 4
"#;
        assert_eq!(
            exact_symbolic_verdict(STAYLOOP, &formula).expect("verdict"),
            ExactVerdict::Violated,
            "stay-loop: an assume-recurring, guarantee-avoiding cycle is reachable"
        );

        // Vacuous: an unsatisfiable assume (`s == 3` over a 1-bit register) makes
        // `GF assume` false, so `GF assume → GF guarantee` holds trivially — no bad
        // cycle can recur through an unreachable assume.
        let vacuous = crate::mu_calculus::parser::parse(&gr1_response_formula("s == 3", "s == 1"))
            .expect("parse");
        assert_eq!(
            exact_symbolic_verdict(STAYLOOP, &vacuous).expect("verdict"),
            ExactVerdict::Holds,
            "unsatisfiable assume ⇒ GF assume → GF guarantee holds vacuously"
        );
    }

    /// D1.4 — `resolve_predicate_expr_registers` rewrites every register name in a
    /// `PredicateExpr` tree through the resolver (idempotent on names it doesn't
    /// remap).
    #[test]
    fn d1_4_resolve_predicate_expr_registers_rewrites() {
        let remap = |n: &str| -> String {
            if n == "cnt_q" {
                "cnt_d".to_string()
            } else {
                n.to_string()
            }
        };
        let expr = PredicateExpr::And(
            Box::new(PredicateExpr::eq("cnt_q", 0)),
            Box::new(PredicateExpr::CmpReg {
                lhs: "cnt_q".to_string(),
                op: CmpOp::Eq,
                rhs: "acc".to_string(),
            }),
        );
        let out = resolve_predicate_expr_registers(&expr, &remap);
        match out {
            PredicateExpr::And(a, b) => {
                assert!(matches!(*a, PredicateExpr::Cmp { register, .. } if register == "cnt_d"));
                assert!(
                    matches!(*b, PredicateExpr::CmpReg { lhs, rhs, .. } if lhs == "cnt_d" && rhs == "acc")
                );
            }
            _ => panic!("expected And"),
        }
    }

    /// D1 HEADLINE (docker-gated) — exact ROBDD μ-calculus MC *decides* on real
    /// OpenTitan `uart_tx` an `AF`-liveness that predicate abstraction returns ⊥
    /// for: `AG AF (bit_cnt_q == 0)` — "a transmission in progress always completes"
    /// — is **Violated** (definite), and the engine returns a concrete stall lasso.
    /// The counter does NOT always drain: `bit_cnt_q` loads on a write, and a
    /// persistently-asserted `wr` (or a stalled `tick_baud_x16`) holds it non-zero
    /// forever — a real liveness failure. The value of the exact engine is that it
    /// decides this **definitely with a counterexample** where the predicate-cube
    /// path answers ⊥ (an `AF` needs a ranking predicate abstraction cannot
    /// synthesize).
    ///
    /// REGRESSION for the frozen-register fix (2026-07-05): this asserted `Holds`
    /// until the `next_funcs` keying bug was fixed. `bit_cnt_q`'s name survives only
    /// on a `uext` alias of an unnamed state (post flatten/async2sync/dffunmap); the
    /// bug keyed `next_funcs` off the raw state symbol, so the lookup missed and the
    /// register was silently FROZEN at its 0 init — making `AG AF` a *vacuous*
    /// `Holds`. With the register actually transitioning, the true verdict is
    /// `Violated`. Companion verdicts remain definite + sound: `AG EF`
    /// (recoverability — idle always reachable, since reset drains the counter) and
    /// `EF (bit_cnt==0)` are `Holds`; `AG (bit_cnt < 12)` (a real bounded-counter
    /// safety invariant) is `Holds`.
    /// `#[ignore]`; the SV is read at run time so `make ci` only compiles it.
    #[test]
    #[ignore = "needs sv2v + yosys — run in the mununu-sva docker image"]
    fn e2e_d1_uart_tx_exact_liveness_verdict() {
        use crate::adapter::yosys::{YosysOptions, sv_to_btor2_with_blackboxes};

        let sv = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/verify/m1_opentitan_uart_tx/source/uart_tx.sv"
        ))
        .expect("read uart_tx.sv");
        let yopts = YosysOptions {
            top: Some("uart_tx".to_string()),
            use_sv2v: true,
            ..Default::default()
        };
        let (btor2, _bb) = sv_to_btor2_with_blackboxes(&sv, &yopts).expect("extract uart_tx");
        let verdict = |f: &str| -> ExactVerdict {
            let ff = crate::mu_calculus::parser::parse(f).expect("formula parses");
            exact_symbolic_verdict(&btor2, &ff).expect("definite verdict, binds")
        };

        // THE HEADLINE: `AG AF (bit_cnt_q == 0)` — "a transmission always completes"
        // — is decided **Violated** (definite liveness failure) on real OpenTitan
        // RTL: a persistently-asserted `wr` (or a stalled tick) holds the counter
        // non-zero forever. The predicate-abstraction path answers this ⊥ (no
        // ranking); exact ROBDD MC decides it definitely, with a stall lasso. This
        // is the frozen-register-fix regression: a false `Holds` here means the
        // `next_funcs` alias-keying bug regressed and `bit_cnt_q` is frozen again.
        assert_eq!(
            verdict("nu X. ((mu Y. ((bit_cnt_q == 0) or [] Y)) and [] X)"),
            ExactVerdict::Violated,
            "AG AF (bit_cnt_q==0) is Violated on uart_tx (a stalled/persistently-written \
             counter never drains) — NOT the vacuous Holds of a frozen register"
        );
        // Companion sanity (all definite + sound): `AG EF` (recoverability — idle is
        // always reachable because reset drains the counter) and `EF (bit_cnt==0)`
        // hold; `AG (bit_cnt < 12)` is a real bounded-counter safety invariant.
        assert_eq!(
            verdict("nu Y. ((mu X. ((bit_cnt_q == 0) or <> X)) and [] Y)"),
            ExactVerdict::Holds,
        );
        assert_eq!(
            verdict("mu Y. ((bit_cnt_q == 0) or <> Y)"),
            ExactVerdict::Holds,
        );
        assert_eq!(
            verdict("nu X. ((bit_cnt_q < 12) and [] X)"),
            ExactVerdict::Holds,
            "the transmit bit counter is bounded (never ≥ 12)"
        );
    }

    /// Evaluate `AG AF (cnt==0)` over the gated counter and print the per-cube
    /// verdict. `c1 = {cnt≠0}` has may-edges to both `c0` and `c1` but (per the
    /// brute-force-verified relation) NO must-edge, so `AF (cnt==0)` at `c1` must
    /// be ⊥ (`must=false`, `may=true`), hence `AG AF` is ⊥ — NOT a definite
    /// VIOLATED. If this prints `False` at a feasible cube, the evaluation is the
    /// unsoundness source; if `⊥`, the verify-auto VIOLATED comes from elsewhere
    /// (pinning / shadow / compound / final-verdict projection).
    #[test]
    fn rf5_soundness_gated_counter_ag_af_is_bottom_not_violated() {
        let predicates = [PredicateExpr::eq("cnt", 0)];
        let file = parser::parse(GATED_COUNTER_BTOR2).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let rel = bb
            .abstract_relation(&predicates, Some(MustSemantics::ForallExists))
            .expect("rel");
        let names = ["done".to_string()];
        let formula = crate::mu_calculus::parser::parse("nu X. ((mu Y. (done or [] Y)) and [] X)")
            .expect("formula");
        let v = rel.evaluate(&formula, &names).expect("eval");
        for c in rel.feasible_cubes() {
            let verdict = rel.verdict_at(&v, c);
            eprintln!("AG AF (cnt==0) @ cube {c}: {verdict:?}");
            // No feasible cube may be definite-False: the concrete design can
            // always eventually reach cnt==0 (or stay), so a definite VIOLATED
            // would be unsound. ⊥ or True are acceptable.
            assert_ne!(
                verdict,
                crate::mu_calculus::trit::Trit::False,
                "cube {c}: AG AF (cnt==0) is definite-False — unsound over the gated counter"
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
            let mut set_vars: Vec<&str> = Vec::new();
            for (i, nm) in names.iter().enumerate() {
                let held = (ci >> i) & 1 == 1;
                let v = if held {
                    Tristate::KleeneT
                } else {
                    Tristate::KleeneF
                };
                b.with_3valued_predicate(ids[j], *nm, v);
                if held {
                    set_vars.push(*nm);
                }
            }
            // R-F5.5c — also populate the 2-valued state-variable bitset. The cube
            // AP labels are definite (KleeneT/KleeneF), so this is consistent with
            // the 3-valued map above and a no-op for the unguarded fragment. It is
            // required for guarded modalities: `evaluate_tri`'s `GuardPartitions`
            // reads `state_variable_bitset` (not `state_3valued_predicates`) when
            // matching `req_cur`/`forb_cur`/`req_next`/`forb_next`.
            b.with_variables_for_state(ids[j], set_vars);
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

    /// R-F5.5c — modalities guarded by `req_cur`/`forb_cur`/`req_next`/`forb_next`
    /// state predicates agree cell-for-cell with `evaluate_tri` on the explicit
    /// cube-KMTS (the guard filters modal steps by source/target cube exactly as
    /// the explicit `guard_matches`). Saturating counter, `p = (cnt == 0)`,
    /// `q = (cnt ≥ 2)`.
    #[test]
    fn rf5_5c_symbolic_eval_matches_tri_guarded_saturating_counter() {
        let predicates = [
            PredicateExpr::eq("cnt", 0),
            PredicateExpr::Cmp {
                register: "cnt".to_string(),
                op: CmpOp::Ge,
                value: 2,
            },
        ];
        let formulas = [
            // next-state guards
            "[ req_next = {q} ] p",
            "< req_next = {q} > p",
            "[ forb_next = {q} ] p",
            "< forb_next = {p} > q",
            // current-state guards
            "[ req_cur = {p} ] q",
            "< req_cur = {q} > p",
            // combined next guards (require q, forbid p in the target)
            "[ req_next = {q}, forb_next = {p} ] p",
            // guarded modality inside a fixpoint
            "nu X. p and [ req_next = {q} ] X",
            "mu X. q or < req_next = {p} > X",
            // nested νμ with a guarded outer box
            "nu Y. (mu X. (q or <> X)) and [ req_cur = {p} ] Y",
            // unguarded still works alongside guarded
            "nu X. p and [] X",
        ];
        assert_symbolic_eval_matches_tri(
            SATURATING_COUNTER_BTOR2,
            &predicates,
            &["p", "q"],
            &[("cnt", 2)],
            &formulas,
        );
    }

    /// R-F5.5c — the same guarded fragment over a KMTS carrying `MayOnly` edges
    /// (two registers, relational predicate), so guarded verdicts include
    /// non-trivial `⊥`s that must still match `evaluate_tri`.
    #[test]
    fn rf5_5c_symbolic_eval_matches_tri_guarded_two_registers() {
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
            "[ req_next = {q} ] p",
            "< req_next = {r} > p",
            "[ forb_next = {p} ] q",
            "[ req_cur = {r} ] p",
            "nu X. p and [ req_next = {q} ] X",
            "mu X. r or < req_next = {p} > X",
            "nu Y. (mu X. (p or <> X)) and [ req_cur = {r} ] Y",
        ];
        assert_symbolic_eval_matches_tri(
            TWO_REGISTER_BTOR2,
            &predicates,
            &["p", "q", "r"],
            &[("cnt", 2), ("acc", 2)],
            &formulas,
        );
    }

    /// R-F5.5c — label / controllability / step-bounded guards are out-of-fragment
    /// over a predicate cube (see `cube_modality_soundness_warnings`) and are an
    /// honest error, never a wrong/vacuous verdict.
    #[test]
    fn rf5_5c_label_control_step_guards_are_rejected() {
        let predicates = [PredicateExpr::eq("cnt", 0)];
        let file = parser::parse(SATURATING_COUNTER_BTOR2).expect("parse");
        let bb = BddBitBlaster::build(&file).expect("build");
        let rel = bb
            .abstract_relation(&predicates, Some(MustSemantics::ForallExists))
            .expect("relation");
        let names = ["p".to_string()];

        for (formula_str, why) in [
            ("[ ctrl = controllable ] p", "controllability guard"),
            ("< ( steps <= 2 ) > p", "step-bounded modality"),
            ("[ labels = {step} ] p", "label guard"),
        ] {
            let formula = crate::mu_calculus::parser::parse(formula_str).expect("formula parses");
            assert!(
                rel.evaluate(&formula, &names).is_err(),
                "{why} must be an honest error over a cube, not a verdict: `{formula_str}`"
            );
        }
    }
}
