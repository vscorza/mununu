//! BTOR2 → Z3 transition-relation encoder (Phase A.4 step 4.2).
//!
//! Walks the BTOR2 NID DAG and produces a single Z3 boolean
//! `T(s, s')` representing the transition relation. State / input
//! NIDs become Z3 `BV` variables over `s` (current step) or
//! `s_next` (next step); `next` lines become equalities tying the
//! two together; the `Btor2SmtView` returned by [`encode_design`]
//! exposes these to the all-SMT enumerator in [`super::all_smt`].
//!
//! Z3 must be called inside a [`z3::with_z3_config`] scope — the
//! crate ships its context as a thread-local. Callers (the all-SMT
//! enumerator, the recall harness) own that scope and drop the
//! `Btor2SmtView` before exiting it.
//!
//! # SOUNDNESS
//!
//! The encoder is **exact** for every operator it supports. Operators
//! outside [`crate::adapter::btor2::ast::Op::is_blastable`] cause the
//! encoder to refuse the design (hard error, surfaced as
//! [`EncodeError::UnsupportedOperator`]) rather than emit an
//! over-approximation. Array sorts (`read`/`write`) are rejected in
//! `Theory::BvOnly`; step 4.5 will handle them under `Theory::BvUfArray`.

use std::collections::HashMap;

use crate::adapter::btor2::ast::{Btor2File, ConstValue, Nid, Node, Op, Operand, Sort};
use crate::adapter::btor2::parser::bv_width;

/// Error variants raised by [`encode_design`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    UnsupportedOperator {
        nid: i64,
        op_name: &'static str,
    },
    ArraySortUnsupportedInBvOnly {
        nid: i64,
    },
    /// A `next` line references an operand whose width does not match
    /// its state cell. The BTOR2 parser usually catches this; surfaced
    /// here only as a defence-in-depth.
    WidthMismatch {
        nid: i64,
        state_width: u32,
        operand_width: u32,
    },
    /// Bit-vector width too small for the integer constant. Should
    /// not happen on well-formed BTOR2; surfaced as a hard error
    /// rather than silently truncating.
    ConstantOutOfRange {
        nid: i64,
    },
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::UnsupportedOperator { nid, op_name } => write!(
                f,
                "BTOR2 NID {nid}: operator '{op_name}' not supported by the predicate-image encoder."
            ),
            EncodeError::ArraySortUnsupportedInBvOnly { nid } => write!(
                f,
                "BTOR2 NID {nid}: array sort under Theory::BvOnly. Use Theory::BvUfArray (step 4.5)."
            ),
            EncodeError::WidthMismatch {
                nid,
                state_width,
                operand_width,
            } => write!(
                f,
                "BTOR2 NID {nid}: width mismatch — state cell is {state_width} bits, operand is {operand_width} bits."
            ),
            EncodeError::ConstantOutOfRange { nid } => write!(
                f,
                "BTOR2 NID {nid}: constant value does not fit in declared sort width."
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

/// A signal exposed by the encoded design — state cell or input.
/// Carries the human-readable symbol (when the BTOR2 file declared
/// one) and the bit-width.
#[derive(Debug, Clone)]
pub struct EncodedSignal {
    pub nid: Nid,
    pub width: u32,
    pub symbol: Option<String>,
    pub kind: SignalKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    State,
    Input,
}

/// Output of [`encode_design`]. Holds the Z3 bit-vector handles for
/// every state cell + input under both current-step (`s`) and
/// next-step (`s_next`) projections, plus the conjoined transition
/// relation.
///
/// Must be dropped before exiting the surrounding
/// [`z3::with_z3_config`] scope.
pub struct Btor2SmtView {
    /// Z3 BV handle for the current-step value of each state cell.
    /// Indexed by BTOR2 NID.
    pub state_curr: HashMap<Nid, z3::ast::BV>,
    /// Z3 BV handle for the next-step value of each state cell.
    pub state_next: HashMap<Nid, z3::ast::BV>,
    /// Z3 BV handle for each input (constant across the step).
    pub inputs: HashMap<Nid, z3::ast::BV>,
    /// §Phase 10 stage 3.c.1 — Z3 Array handle for the current-step
    /// value of each array-sorted (memory) state cell. Empty under
    /// `Theory::BvOnly`; populated under `Theory::BvUfArray`. The
    /// array's domain sort is the BTOR2 index width, the range sort
    /// is the element width. Memory cells only flow into the BV
    /// world through `Op::Read` (→ `select`); their own next-state
    /// is an extensional array equality in the transition relation.
    pub state_curr_arr: HashMap<Nid, z3::ast::Array>,
    /// §Phase 10 stage 3.c.1 — Z3 Array handle for the next-step
    /// value of each array-sorted state cell. Mirror of
    /// `state_curr_arr` over `s_next`.
    pub state_next_arr: HashMap<Nid, z3::ast::Array>,
    /// Per-signal metadata for callers that need to round-trip
    /// names / widths back to the sidecar.
    pub signals: Vec<EncodedSignal>,
    /// The conjoined transition relation `T(s, s') = ⋀ s_next == next(...)`.
    pub transition: z3::ast::Bool,
}

impl Btor2SmtView {
    /// Look up the next-step BV for a state-cell NID.
    pub fn next_state(&self, nid: Nid) -> Option<&z3::ast::BV> {
        self.state_next.get(&nid)
    }

    /// Look up the current-step BV for a state-cell NID.
    pub fn curr_state(&self, nid: Nid) -> Option<&z3::ast::BV> {
        self.state_curr.get(&nid)
    }

    /// Bit-width of a signal by NID.
    pub fn width_of(&self, nid: Nid) -> Option<u32> {
        self.signals.iter().find(|s| s.nid == nid).map(|s| s.width)
    }
}

/// Encode a BTOR2 file into a Z3 transition relation under
/// `Theory::BvOnly` (pure bit-vectors; array sorts rejected).
///
/// Must be called inside a [`z3::with_z3_config`] scope. Returns
/// [`EncodeError::UnsupportedOperator`] on the first non-blastable
/// op encountered; the caller has no way to recover other than
/// rejecting the design or switching to an external symbolic engine.
///
/// §Phase 10 stage 3.c.1 — this is now a thin delegator to
/// [`encode_design_with_theory`] with [`Theory::BvOnly`]. Every
/// existing caller (must-edge queries, all-SMT enumeration) keeps
/// the identical behaviour: array-sorted designs still error with
/// [`EncodeError::ArraySortUnsupportedInBvOnly`]. Memory-bearing
/// designs opt into the array encoding via
/// `encode_design_with_theory(file, Theory::BvUfArray)`.
pub fn encode_design(file: &Btor2File) -> Result<Btor2SmtView, EncodeError> {
    encode_design_with_theory(file, super::theory::Theory::BvOnly)
}

/// §Phase 10 stage 3.c.1 (2026-06-12) — encode a BTOR2 file into a
/// Z3 transition relation under the chosen [`Theory`].
///
/// - [`Theory::BvOnly`]: pure QF_BV. Array-sorted state cells +
///   `Op::Read`/`Op::Write` error out (the pre-stage-3.c.1
///   behaviour, preserved bit-for-bit).
/// - [`Theory::BvUfArray`]: QF_AUFBV. Array-sorted state cells are
///   declared as Z3 `Array` consts (domain = index width, range =
///   element width); `Op::Read` encodes as `select` (→ BV);
///   array-sorted `next` lines encode as extensional array
///   equalities in the transition relation. The functional-
///   consistency axioms (`read(write(a,i,v),j) = i==j ? v :
///   read(a,j)`) are emitted automatically by Z3's Array theory —
///   no manual axiom encoding.
///
/// **Soundness.** The array encoding is *exact* for the memory
/// semantics BTOR2 expresses — it does not over-approximate. (The
/// §Phase 10 havoc-may / array-must composition handles the
/// may-side over-approximation upstream in the predicate-cube lift;
/// this encoder is the precise must-side oracle.)
pub fn encode_design_with_theory(
    file: &Btor2File,
    theory: super::theory::Theory,
) -> Result<Btor2SmtView, EncodeError> {
    let arrays_ok = theory.supports_indirect_references();

    // Z3 0.20's handles carry a lifetime tied to the global context
    // the `with_z3_config` scope installed; the crate's API returns
    // `'static`-flavoured handles.
    let mut state_curr: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut state_next: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut state_curr_arr: HashMap<Nid, z3::ast::Array> = HashMap::new();
    let mut state_next_arr: HashMap<Nid, z3::ast::Array> = HashMap::new();
    let mut inputs: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut signals = Vec::new();

    let symbols = crate::adapter::btor2::parser::collect_symbols(file);

    // Pass 1 — declare handles for every state cell and input.
    for line in &file.lines {
        match &line.node {
            Node::State { sort, .. } => {
                // Distinguish bit-vector state cells from array
                // (memory) state cells. `bv_width` returns None for
                // array sorts.
                if let Some(width) = bv_width(file, *sort) {
                    let sym = symbols.get(&line.nid).cloned();
                    let label = sym.clone().unwrap_or_else(|| format!("st{}", line.nid));
                    let curr = z3::ast::BV::new_const(format!("s_{label}").as_str(), width);
                    let next = z3::ast::BV::new_const(format!("s_next_{label}").as_str(), width);
                    state_curr.insert(line.nid, curr);
                    state_next.insert(line.nid, next);
                    signals.push(EncodedSignal {
                        nid: line.nid,
                        width,
                        symbol: sym,
                        kind: SignalKind::State,
                    });
                } else if arrays_ok {
                    // Array-sorted memory cell — declare Z3 Array
                    // consts for the current + next step.
                    let (idx_w, elem_w) = array_widths(file, *sort)
                        .ok_or(EncodeError::ArraySortUnsupportedInBvOnly { nid: line.nid })?;
                    let sym = symbols.get(&line.nid).cloned();
                    let label = sym.clone().unwrap_or_else(|| format!("mem{}", line.nid));
                    let dom = z3::Sort::bitvector(idx_w);
                    let rng = z3::Sort::bitvector(elem_w);
                    let curr = z3::ast::Array::new_const(format!("a_{label}").as_str(), &dom, &rng);
                    let next =
                        z3::ast::Array::new_const(format!("a_next_{label}").as_str(), &dom, &rng);
                    state_curr_arr.insert(line.nid, curr);
                    state_next_arr.insert(line.nid, next);
                    // Memory cells are tracked in the array maps, not
                    // the BV `signals` vector (which feeds bit-vector
                    // cube enumeration). The predicate-cube lift's
                    // cube predicates reference BV registers only.
                } else {
                    return Err(EncodeError::ArraySortUnsupportedInBvOnly { nid: line.nid });
                }
            }
            Node::Input { sort, .. } => {
                let width = bv_width(file, *sort)
                    .ok_or(EncodeError::ArraySortUnsupportedInBvOnly { nid: line.nid })?;
                let sym = symbols.get(&line.nid).cloned();
                let label = sym.clone().unwrap_or_else(|| format!("in{}", line.nid));
                let v = z3::ast::BV::new_const(format!("in_{label}").as_str(), width);
                inputs.insert(line.nid, v);
                signals.push(EncodedSignal {
                    nid: line.nid,
                    width,
                    symbol: sym,
                    kind: SignalKind::Input,
                });
            }
            _ => {}
        }
    }

    // Pass 2 — walk every `next` line and conjoin the appropriate
    // equality into the transition relation. BV state cells get a
    // BV equality `s_next == eval(value)`; array state cells get an
    // extensional array equality `a_next == eval_array(value)`.
    let mut conjuncts: Vec<z3::ast::Bool> = Vec::new();
    let mut cache: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut array_cache: HashMap<Nid, z3::ast::Array> = HashMap::new();
    let array_curr = if arrays_ok {
        Some(&state_curr_arr)
    } else {
        None
    };

    for line in &file.lines {
        if let Node::Next { state, value, .. } = &line.node {
            // Array-sorted next: extensional array equality.
            if let Some(arr_next) = state_next_arr.get(state) {
                let value_arr = eval_array_operand(
                    *value,
                    file,
                    &state_curr,
                    &inputs,
                    &state_curr_arr,
                    &mut cache,
                    &mut array_cache,
                )?;
                conjuncts.push(arr_next.eq(&value_arr));
                continue;
            }
            // BV-sorted next: bit-vector equality.
            let value_bv =
                eval_operand(*value, file, &state_curr, &inputs, array_curr, &mut cache)?;
            let next_bv = state_next
                .get(state)
                .ok_or(EncodeError::WidthMismatch {
                    nid: line.nid,
                    state_width: 0,
                    operand_width: value_bv.get_size(),
                })?
                .clone();
            if next_bv.get_size() != value_bv.get_size() {
                return Err(EncodeError::WidthMismatch {
                    nid: line.nid,
                    state_width: next_bv.get_size(),
                    operand_width: value_bv.get_size(),
                });
            }
            conjuncts.push(next_bv.eq(&value_bv));
        }
    }

    let transition = if conjuncts.is_empty() {
        z3::ast::Bool::from_bool(true)
    } else {
        let refs: Vec<&z3::ast::Bool> = conjuncts.iter().collect();
        z3::ast::Bool::and(&refs)
    };

    Ok(Btor2SmtView {
        state_curr,
        state_next,
        state_curr_arr,
        state_next_arr,
        inputs,
        signals,
        transition,
    })
}

/// §Phase 10 stage 3.c.1 — resolve the index + element bit-widths of
/// an array sort. BTOR2 `Sort::Array { index, element }` references
/// two other sort lines (each a bitvec sort); this resolves both.
fn array_widths(file: &Btor2File, sort_nid: Nid) -> Option<(u32, u32)> {
    let line = file.lookup(sort_nid)?;
    let Node::Sort {
        sort: Sort::Array { index, element },
    } = &line.node
    else {
        return None;
    };
    let idx_w = bv_width(file, *index)?;
    let elem_w = bv_width(file, *element)?;
    Some((idx_w, elem_w))
}

/// §Phase 10 stage 3.c.1 — resolve an array-sorted operand to a Z3
/// `Array` term. Array values occupy a sub-DAG that exits to the BV
/// world only through `Op::Read` (→ `select`). The producers of
/// array values are:
/// - `Node::State` (array sort) → the declared current-step Array.
/// - `Op::Write(arr, idx, val)` → `arr.store(idx, val)`.
/// - `Op::Ite(cond, then_arr, else_arr)` → conditional array select.
///
/// Caches by NID so DAG sharing in the BTOR2 file maps to Z3
/// expression sharing. Index/value sub-terms are evaluated through
/// the BV path (`eval_operand`).
#[allow(clippy::too_many_arguments)]
fn eval_array_operand(
    operand: Operand,
    file: &Btor2File,
    state_curr: &HashMap<Nid, z3::ast::BV>,
    inputs: &HashMap<Nid, z3::ast::BV>,
    state_curr_arr: &HashMap<Nid, z3::ast::Array>,
    cache: &mut HashMap<Nid, z3::ast::BV>,
    array_cache: &mut HashMap<Nid, z3::ast::Array>,
) -> Result<z3::ast::Array, EncodeError> {
    // Array operands are never negated (BTOR2 only negates bit-vec
    // signals); `nid()` strips any sign defensively.
    let nid = operand.nid();

    if let Some(cached) = array_cache.get(&nid) {
        return Ok(cached.clone());
    }

    let line = file.lookup(nid).ok_or(EncodeError::UnsupportedOperator {
        nid,
        op_name: "<missing-array-nid>",
    })?;

    let arr = match &line.node {
        Node::State { .. } => {
            state_curr_arr
                .get(&nid)
                .cloned()
                .ok_or(EncodeError::UnsupportedOperator {
                    nid,
                    op_name: "array-state",
                })?
        }
        Node::Op {
            op: Op::Write,
            args,
            ..
        } => {
            let base = eval_array_operand(
                args[0],
                file,
                state_curr,
                inputs,
                state_curr_arr,
                cache,
                array_cache,
            )?;
            let idx = eval_operand(
                args[1],
                file,
                state_curr,
                inputs,
                Some(state_curr_arr),
                cache,
            )?;
            let val = eval_operand(
                args[2],
                file,
                state_curr,
                inputs,
                Some(state_curr_arr),
                cache,
            )?;
            base.store(&idx, &val)
        }
        Node::Op {
            op: Op::Ite, args, ..
        } => {
            let cond = eval_operand(
                args[0],
                file,
                state_curr,
                inputs,
                Some(state_curr_arr),
                cache,
            )?;
            let then_arr = eval_array_operand(
                args[1],
                file,
                state_curr,
                inputs,
                state_curr_arr,
                cache,
                array_cache,
            )?;
            let else_arr = eval_array_operand(
                args[2],
                file,
                state_curr,
                inputs,
                state_curr_arr,
                cache,
                array_cache,
            )?;
            let zero = z3::ast::BV::from_u64(0, cond.get_size());
            let cond_bool = cond.eq(&zero).not();
            cond_bool.ite(&then_arr, &else_arr)
        }
        _ => {
            return Err(EncodeError::UnsupportedOperator {
                nid,
                op_name: "non-array-producing node in array position",
            });
        }
    };

    array_cache.insert(nid, arr.clone());
    Ok(arr)
}

/// Recursively evaluate an operand to a Z3 BV. Caches sub-expressions
/// keyed by NID so the DAG sharing in the BTOR2 file translates
/// directly into Z3 expression sharing.
fn eval_operand(
    operand: Operand,
    file: &Btor2File,
    state_curr: &HashMap<Nid, z3::ast::BV>,
    inputs: &HashMap<Nid, z3::ast::BV>,
    array_curr: Option<&HashMap<Nid, z3::ast::Array>>,
    cache: &mut HashMap<Nid, z3::ast::BV>,
) -> Result<z3::ast::BV, EncodeError> {
    let nid = operand.nid();
    let negated = operand.is_negated();

    if let Some(cached) = cache.get(&nid) {
        let v = cached.clone();
        return Ok(if negated { v.bvnot() } else { v });
    }

    let line = file.lookup(nid).ok_or(EncodeError::UnsupportedOperator {
        nid,
        op_name: "<missing-nid>",
    })?;

    let v = match &line.node {
        Node::State { .. } => {
            state_curr
                .get(&nid)
                .cloned()
                .ok_or(EncodeError::UnsupportedOperator {
                    nid,
                    op_name: "state",
                })?
        }
        Node::Input { .. } => {
            inputs
                .get(&nid)
                .cloned()
                .ok_or(EncodeError::UnsupportedOperator {
                    nid,
                    op_name: "input",
                })?
        }
        Node::Const { sort, value } => encode_const(line.nid, *sort, value, file)?,
        Node::Op { sort, op, args, .. } => {
            let width = bv_width(file, *sort)
                .ok_or(EncodeError::ArraySortUnsupportedInBvOnly { nid: line.nid })?;
            encode_op(
                line.nid,
                *op,
                args,
                &line.immediates,
                width,
                file,
                state_curr,
                inputs,
                array_curr,
                cache,
            )?
        }
        Node::Sort { .. }
        | Node::Init { .. }
        | Node::Next { .. }
        | Node::Bad { .. }
        | Node::Constraint { .. }
        | Node::Fair { .. }
        | Node::Output { .. }
        | Node::Justice { .. } => {
            return Err(EncodeError::UnsupportedOperator {
                nid,
                op_name: "non-value-node",
            });
        }
    };

    cache.insert(nid, v.clone());
    Ok(if negated { v.bvnot() } else { v })
}

fn encode_const(
    nid: Nid,
    sort_nid: Nid,
    value: &ConstValue,
    file: &Btor2File,
) -> Result<z3::ast::BV, EncodeError> {
    let width =
        bv_width(file, sort_nid).ok_or(EncodeError::ArraySortUnsupportedInBvOnly { nid })?;
    let bits: u64 = match value {
        ConstValue::Zero => 0,
        ConstValue::One => 1,
        ConstValue::Ones => {
            if width >= 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            }
        }
        ConstValue::Bin(s) => {
            u64::from_str_radix(s, 2).map_err(|_| EncodeError::ConstantOutOfRange { nid })?
        }
        ConstValue::Dec(d) => {
            if *d < 0 || *d > (i128::from(u64::MAX)) {
                return Err(EncodeError::ConstantOutOfRange { nid });
            }
            *d as u64
        }
        ConstValue::Hex(s) => {
            u64::from_str_radix(s, 16).map_err(|_| EncodeError::ConstantOutOfRange { nid })?
        }
    };
    Ok(z3::ast::BV::from_u64(bits, width))
}

#[allow(clippy::too_many_arguments)]
fn encode_op(
    nid: Nid,
    op: Op,
    args: &[Operand],
    immediates: &[u32],
    width: u32,
    file: &Btor2File,
    state_curr: &HashMap<Nid, z3::ast::BV>,
    inputs: &HashMap<Nid, z3::ast::BV>,
    array_curr: Option<&HashMap<Nid, z3::ast::Array>>,
    cache: &mut HashMap<Nid, z3::ast::BV>,
) -> Result<z3::ast::BV, EncodeError> {
    // §Phase 10 stage 3.c.1 — `Op::Read` is the one operator that
    // joins the array sub-DAG to the BV world (`select` yields the
    // element BV). It must be handled BEFORE the `get` closure below
    // borrows `cache`, because it needs both the BV cache (for the
    // index sub-term) and the array evaluator (for the array
    // sub-term). Under `Theory::BvOnly` (`array_curr == None`) it
    // falls through to the array-rejection error, preserving the
    // pre-stage-3.c.1 behaviour.
    if op == Op::Read {
        let array_curr = array_curr.ok_or(EncodeError::ArraySortUnsupportedInBvOnly { nid })?;
        let mut array_cache: HashMap<Nid, z3::ast::Array> = HashMap::new();
        let arr = eval_array_operand(
            args[0],
            file,
            state_curr,
            inputs,
            array_curr,
            cache,
            &mut array_cache,
        )?;
        let idx = eval_operand(args[1], file, state_curr, inputs, Some(array_curr), cache)?;
        let elem = arr
            .select(&idx)
            .as_bv()
            .ok_or(EncodeError::UnsupportedOperator {
                nid,
                op_name: "read (select did not yield a bit-vector element)",
            })?;
        return Ok(elem);
    }

    // Helpers to materialise operands inline.
    let mut get = |i: usize| -> Result<z3::ast::BV, EncodeError> {
        eval_operand(args[i], file, state_curr, inputs, array_curr, cache)
    };
    let bool_to_bv1 = |b: z3::ast::Bool| {
        let one = z3::ast::BV::from_u64(1, 1);
        let zero = z3::ast::BV::from_u64(0, 1);
        b.ite(&one, &zero)
    };

    match op {
        // Bitwise / boolean
        Op::Not => Ok(get(0)?.bvnot()),
        Op::And => Ok(get(0)?.bvand(&get(1)?)),
        Op::Or => Ok(get(0)?.bvor(&get(1)?)),
        Op::Xor => Ok(get(0)?.bvxor(&get(1)?)),
        Op::Nand => Ok(get(0)?.bvand(&get(1)?).bvnot()),
        Op::Nor => Ok(get(0)?.bvor(&get(1)?).bvnot()),
        Op::Xnor => Ok(get(0)?.bvxor(&get(1)?).bvnot()),
        Op::Iff => Ok(bool_to_bv1(get(0)?.eq(&get(1)?))),
        Op::Implies => {
            // a → b  ≡  ¬a ∨ b ; over BV(1).
            let a = get(0)?;
            let b = get(1)?;
            Ok(a.bvnot().bvor(&b))
        }
        // Comparison (always emits BV(1))
        Op::Eq => Ok(bool_to_bv1(get(0)?.eq(&get(1)?))),
        Op::Neq => Ok(bool_to_bv1(get(0)?.eq(&get(1)?).not())),
        Op::Ult => Ok(bool_to_bv1(get(0)?.bvult(&get(1)?))),
        Op::Ulte => Ok(bool_to_bv1(get(0)?.bvule(&get(1)?))),
        Op::Ugt => Ok(bool_to_bv1(get(0)?.bvugt(&get(1)?))),
        Op::Ugte => Ok(bool_to_bv1(get(0)?.bvuge(&get(1)?))),
        Op::Slt => Ok(bool_to_bv1(get(0)?.bvslt(&get(1)?))),
        Op::Slte => Ok(bool_to_bv1(get(0)?.bvsle(&get(1)?))),
        Op::Sgt => Ok(bool_to_bv1(get(0)?.bvsgt(&get(1)?))),
        Op::Sgte => Ok(bool_to_bv1(get(0)?.bvsge(&get(1)?))),
        // Arithmetic
        Op::Add => Ok(get(0)?.bvadd(&get(1)?)),
        Op::Sub => Ok(get(0)?.bvsub(&get(1)?)),
        Op::Mul => Ok(get(0)?.bvmul(&get(1)?)),
        Op::Neg => Ok(get(0)?.bvneg()),
        Op::Inc => {
            let a = get(0)?;
            let one = z3::ast::BV::from_u64(1, a.get_size());
            Ok(a.bvadd(&one))
        }
        Op::Dec => {
            let a = get(0)?;
            let one = z3::ast::BV::from_u64(1, a.get_size());
            Ok(a.bvsub(&one))
        }
        // Shifts
        Op::Sll => Ok(get(0)?.bvshl(&get(1)?)),
        Op::Srl => Ok(get(0)?.bvlshr(&get(1)?)),
        Op::Sra => Ok(get(0)?.bvashr(&get(1)?)),
        Op::Rol => Ok(get(0)?.bvrotl(&get(1)?)),
        Op::Ror => Ok(get(0)?.bvrotr(&get(1)?)),
        // Concat / slice / extension
        Op::Concat => Ok(get(0)?.concat(&get(1)?)),
        Op::Slice => {
            let inner = get(0)?;
            let upper = *immediates.first().ok_or(EncodeError::UnsupportedOperator {
                nid,
                op_name: "slice (missing immediate)",
            })?;
            let lower = *immediates.get(1).ok_or(EncodeError::UnsupportedOperator {
                nid,
                op_name: "slice (missing immediate)",
            })?;
            Ok(inner.extract(upper, lower))
        }
        Op::Uext => {
            let inner = get(0)?;
            let pad = width.saturating_sub(inner.get_size());
            Ok(inner.zero_ext(pad))
        }
        Op::Sext => {
            let inner = get(0)?;
            let pad = width.saturating_sub(inner.get_size());
            Ok(inner.sign_ext(pad))
        }
        // Reductions
        Op::Redand => {
            let inner = get(0)?;
            let ones = z3::ast::BV::from_i64(-1, inner.get_size());
            Ok(bool_to_bv1(inner.eq(&ones)))
        }
        Op::Redor => {
            let inner = get(0)?;
            let zero = z3::ast::BV::from_u64(0, inner.get_size());
            Ok(bool_to_bv1(inner.eq(&zero).not()))
        }
        Op::Redxor => {
            // Parity bit. For an n-bit value v, redxor = v[0] xor v[1] xor ... xor v[n-1].
            let inner = get(0)?;
            let n = inner.get_size();
            let mut acc = inner.extract(0, 0);
            for i in 1..n {
                let bit = inner.extract(i, i);
                acc = acc.bvxor(&bit);
            }
            Ok(acc)
        }
        // ite (3-operand)
        Op::Ite => {
            let cond = get(0)?;
            let then_bv = get(1)?;
            let else_bv = get(2)?;
            let zero = z3::ast::BV::from_u64(0, cond.get_size());
            let cond_bool = cond.eq(&zero).not();
            Ok(cond_bool.ite(&then_bv, &else_bv))
        }
        // Unsupported in Phase A.4 BvOnly — array ops + divisions +
        // overflow detectors. Match the bit-blaster's behaviour:
        // refuse with a clear error.
        Op::Sdiv | Op::Udiv | Op::Smod | Op::Srem | Op::Urem => {
            Err(EncodeError::UnsupportedOperator {
                nid,
                op_name: "division/modulo (sdiv/udiv/smod/srem/urem)",
            })
        }
        Op::Saddo | Op::Ssubo | Op::Smulo | Op::Uaddo | Op::Usubo | Op::Umulo | Op::Sdivo => {
            Err(EncodeError::UnsupportedOperator {
                nid,
                op_name: "overflow detector",
            })
        }
        Op::Read | Op::Write => Err(EncodeError::ArraySortUnsupportedInBvOnly { nid }),
    }
}

// Suppress the unused-arg warning emitted by `Sort` checks in the
// non-array paths; the encoder doesn't need the sort node itself
// since the parser already validated widths.
#[allow(dead_code)]
fn _sort_check(_s: &Sort) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser::parse;

    #[test]
    fn encode_safety_demo_btor() {
        let src = include_str!("../../../../../../examples/btor2/safety_demo.btor");
        let file = parse(src).expect("parse safety_demo");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design(&file).expect("encode safety_demo");
            // safety_demo has 3 state cells (cnt + 2 anonymous);
            // 2 inputs (rst, clk).
            assert!(
                view.signals
                    .iter()
                    .any(|s| s.symbol.as_deref() == Some("cnt"))
            );
            assert!(
                view.signals
                    .iter()
                    .any(|s| s.symbol.as_deref() == Some("rst"))
            );
            assert!(
                view.signals
                    .iter()
                    .any(|s| s.symbol.as_deref() == Some("clk"))
            );
        });
    }

    #[test]
    fn encode_cap_overflow_btor() {
        let src = include_str!(
            "../../../../../../examples/verify/bench_predicate_image_a4/adversarial/cap_overflow.btor"
        );
        let file = parse(src).expect("parse cap_overflow");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design(&file).expect("encode cap_overflow");
            assert_eq!(
                view.signals
                    .iter()
                    .filter(|s| s.kind == SignalKind::State)
                    .count(),
                1
            );
            // `cnt` should be 8 bits.
            let cnt = view
                .signals
                .iter()
                .find(|s| s.symbol.as_deref() == Some("cnt"))
                .expect("cnt signal present");
            assert_eq!(cnt.width, 8);
        });
    }

    #[test]
    fn encode_sparse_predicates_btor() {
        let src = include_str!(
            "../../../../../../examples/verify/bench_predicate_image_a4/adversarial/sparse_predicates.btor"
        );
        let file = parse(src).expect("parse sparse_predicates");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design(&file).expect("encode sparse_predicates");
            let s = view
                .signals
                .iter()
                .find(|sig| sig.symbol.as_deref() == Some("s"))
                .expect("s signal present");
            assert_eq!(s.width, 3);
            assert_eq!(s.kind, SignalKind::State);
        });
    }

    // ─────────────────────────────────────────────────────────────
    // §Phase 10 stage 3.c.1 — Z3 Array theory (QF_AUFBV) encoding
    // ─────────────────────────────────────────────────────────────

    use super::super::theory::Theory;

    /// A tiny memory design: a single array-sorted state cell `mem`
    /// (5-bit address, 8-bit data), one ite-on-array next (no-op
    /// write), one read. Mirrors `bit_blast.rs`'s
    /// PHASE10_FIXTURE_WITH_MEMORY.
    const MEM_BTOR_READ_ONLY: &str = r#"
1 sort bitvec 1
2 sort bitvec 5
3 sort bitvec 8
4 sort array 2 3
5 state 4 mem
6 input 1 we
7 input 2 addr
8 input 3 wdata
9 ite 4 6 5 5
10 next 4 5 9
11 read 3 5 7
12 zero 3
13 eq 1 11 12
14 bad 13
"#;

    /// A memory design that performs an actual `write` (store) then
    /// reads back the same address into an observed BV register. The
    /// array functional-consistency axiom forces
    /// `read(store(mem, a, v), a) == v`.
    const MEM_BTOR_WRITE_THEN_READ: &str = r#"
1 sort bitvec 1
2 sort bitvec 5
3 sort bitvec 8
4 sort array 2 3
5 state 4 mem
6 state 3 observed
7 input 2 a
8 input 3 v
9 write 4 5 7 8
10 next 4 5 9
11 read 3 9 7
12 next 3 6 11
"#;

    #[test]
    fn bvonly_rejects_array_design() {
        // Regression guard: the BvOnly path (and the `encode_design`
        // delegator) must STILL reject array-sorted designs, exactly
        // as before stage 3.c.1.
        let file = parse(MEM_BTOR_READ_ONLY).expect("parse mem design");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let result = encode_design(&file);
            assert!(
                matches!(
                    result.err(),
                    Some(EncodeError::ArraySortUnsupportedInBvOnly { .. })
                ),
                "BvOnly must reject array-sorted state cells with ArraySortUnsupportedInBvOnly"
            );
        });
    }

    #[test]
    fn bvufarray_encodes_array_design() {
        // Under BvUfArray the same design encodes successfully.
        let file = parse(MEM_BTOR_READ_ONLY).expect("parse mem design");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design_with_theory(&file, Theory::BvUfArray)
                .expect("BvUfArray must encode the array design");
            // The memory cell is tracked in the array maps, not the
            // BV `signals` vector.
            assert_eq!(view.state_curr_arr.len(), 1, "one array state cell");
            assert_eq!(view.state_next_arr.len(), 1);
            // The BV input registers are still present.
            assert!(view.inputs.values().count() >= 3, "we + addr + wdata");
        });
    }

    #[test]
    fn bvufarray_read_after_write_is_forced_by_array_axiom() {
        // The transition relation must FORCE
        // `observed_next == v` because
        // `read(store(mem, a, v), a) == v` by Z3's extensional
        // array axiom. We prove this by asserting the transition
        // AND `observed_next != v` and checking UNSAT.
        let file = parse(MEM_BTOR_WRITE_THEN_READ).expect("parse write-then-read");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design_with_theory(&file, Theory::BvUfArray)
                .expect("encode write-then-read");

            // Locate the `observed` state cell's next BV (NID 6) and
            // the `v` input BV (NID 8).
            let observed_next = view
                .state_next
                .get(&6)
                .expect("observed next BV present")
                .clone();
            let v_input = view.inputs.get(&8).expect("v input BV present").clone();

            let solver = z3::Solver::new();
            solver.assert(&view.transition);
            // Negation of the array-consistency consequence.
            solver.assert(observed_next.eq(&v_input).not());
            assert_eq!(
                solver.check(),
                z3::SatResult::Unsat,
                "read(store(mem,a,v),a) != v must be UNSAT under array theory"
            );
        });
    }

    #[test]
    fn bvufarray_read_after_write_equality_is_sat() {
        // Dual of the above: the transition AND
        // `observed_next == v` must be SAT (the axiom is satisfiable,
        // not vacuous).
        let file = parse(MEM_BTOR_WRITE_THEN_READ).expect("parse write-then-read");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = encode_design_with_theory(&file, Theory::BvUfArray)
                .expect("encode write-then-read");
            let observed_next = view.state_next.get(&6).expect("observed next").clone();
            let v_input = view.inputs.get(&8).expect("v input").clone();

            let solver = z3::Solver::new();
            solver.assert(&view.transition);
            solver.assert(observed_next.eq(&v_input));
            assert_eq!(
                solver.check(),
                z3::SatResult::Sat,
                "read(store(mem,a,v),a) == v must be SAT (axiom not vacuous)"
            );
        });
    }

    #[test]
    fn array_widths_resolves_index_and_element() {
        // The array_widths helper must resolve both sort widths from
        // a `sort array <idx> <elem>` declaration.
        let file = parse(MEM_BTOR_READ_ONLY).expect("parse");
        // Sort NID 4 is `sort array 2 3` → index sort 2 (5-bit),
        // element sort 3 (8-bit).
        let (idx_w, elem_w) = array_widths(&file, 4).expect("array_widths resolves NID 4");
        assert_eq!(idx_w, 5);
        assert_eq!(elem_w, 8);
        // A bitvec sort NID returns None.
        assert!(array_widths(&file, 3).is_none());
    }
}
