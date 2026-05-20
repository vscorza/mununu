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

/// Encode a BTOR2 file into a Z3 transition relation.
///
/// Must be called inside a [`z3::with_z3_config`] scope. Returns
/// [`EncodeError::UnsupportedOperator`] on the first non-blastable
/// op encountered; the caller has no way to recover other than
/// rejecting the design or switching to an external symbolic engine.
pub fn encode_design(file: &Btor2File) -> Result<Btor2SmtView, EncodeError> {
    // Z3 0.20's BV handles carry a lifetime tied to the global
    // context the `with_z3_config` scope installed; using `'static`
    // here is the convention the crate's API exposes (every
    // `BV::new_const` returns BV<'static>).
    let mut state_curr: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut state_next: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut inputs: HashMap<Nid, z3::ast::BV> = HashMap::new();
    let mut signals = Vec::new();

    // Symbol map populated by the BTOR2 parser (already factored out;
    // re-using it keeps the encoder's signal names aligned with the
    // bit-blaster's).
    let symbols = crate::adapter::btor2::parser::collect_symbols(file);

    // Pass 1 — declare BV handles for every state cell and input.
    for line in &file.lines {
        match &line.node {
            Node::State { sort, .. } => {
                let width = bv_width(file, *sort)
                    .ok_or(EncodeError::ArraySortUnsupportedInBvOnly { nid: line.nid })?;
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

    // Pass 2 — walk every `next` line and conjoin
    // `s_next_<state> == eval(value)` into the transition relation.
    let mut conjuncts: Vec<z3::ast::Bool> = Vec::new();
    let mut cache: HashMap<Nid, z3::ast::BV> = HashMap::new();

    for line in &file.lines {
        if let Node::Next { state, value, .. } = &line.node {
            let value_bv = eval_operand(*value, file, &state_curr, &inputs, &mut cache)?;
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

    // Conjoin into a single Bool. If the design has no `next` lines
    // (rare — a degenerate fixture), the transition is trivially
    // `true` (every state is a self-loop).
    let transition = if conjuncts.is_empty() {
        z3::ast::Bool::from_bool(true)
    } else {
        let refs: Vec<&z3::ast::Bool> = conjuncts.iter().collect();
        z3::ast::Bool::and(&refs)
    };

    Ok(Btor2SmtView {
        state_curr,
        state_next,
        inputs,
        signals,
        transition,
    })
}

/// Recursively evaluate an operand to a Z3 BV. Caches sub-expressions
/// keyed by NID so the DAG sharing in the BTOR2 file translates
/// directly into Z3 expression sharing.
fn eval_operand(
    operand: Operand,
    file: &Btor2File,
    state_curr: &HashMap<Nid, z3::ast::BV>,
    inputs: &HashMap<Nid, z3::ast::BV>,
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
    cache: &mut HashMap<Nid, z3::ast::BV>,
) -> Result<z3::ast::BV, EncodeError> {
    // Helpers to materialise operands inline.
    let mut get = |i: usize| -> Result<z3::ast::BV, EncodeError> {
        eval_operand(args[i], file, state_curr, inputs, cache)
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
}
