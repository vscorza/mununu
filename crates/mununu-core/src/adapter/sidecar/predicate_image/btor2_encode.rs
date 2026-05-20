//! BTOR2 → SMT transition-relation encoder (Phase A.4 step 4.2).
//!
//! Walks the BTOR2 NID DAG and emits a single Z3 boolean formula
//! `T(s, s')` representing the transition relation: for each
//! state-cell NID `n` with a `next` line of value-operand `v`, emit
//! `s'_n == eval(v, s)`. State / input NIDs become Z3 `BV` variables
//! over `s` or `s'` as appropriate; constants and operators round-
//! trip through Z3's bit-vector ops.
//!
//! **Step 4.1 status: skeleton only.** The full encoder lands in step
//! 4.2 with end-to-end tests against `safety_demo.btor` and two SV
//! fixtures.
//!
//! # SOUNDNESS
//!
//! The encoder must be **exact** for every operator it supports —
//! over-approximating an operator would let the predicate-image
//! enumeration surface edges that don't exist concretely, which is
//! sound for safety but loses precision. Operators outside the Phase 1
//! supported set
//! ([`crate::adapter::btor2::ast::Op::is_blastable`]) cause the
//! encoder to refuse the design (hard error, surfaced as an
//! `EncodeError::UnsupportedOperator`).

use crate::adapter::btor2::ast::Btor2File;

/// Error variants raised by [`encode_transition_relation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// An operator outside the Phase 1 supported set was encountered.
    /// The encoder refuses to over-approximate; the user must run an
    /// external symbolic engine (per the documented Phase 3 hand-off)
    /// for designs that include this operator.
    UnsupportedOperator { nid: i64, op_name: &'static str },
    /// The BTOR2 file references an array sort (read / write). Phase
    /// A.4 BvOnly path rejects these; arrays land in step 4.5 with
    /// `Theory::BvUfArray`.
    ArraySortUnsupportedInBvOnly { nid: i64 },
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::UnsupportedOperator { nid, op_name } => {
                write!(
                    f,
                    "BTOR2 NID {nid}: operator '{op_name}' not supported by the Phase A.4 \
                     transition-relation encoder. Hand the design to an external symbolic \
                     engine, or wait for the operator to be added in a follow-up phase."
                )
            }
            EncodeError::ArraySortUnsupportedInBvOnly { nid } => {
                write!(
                    f,
                    "BTOR2 NID {nid} references an array sort; Theory::BvOnly rejects \
                     arrays. Use Theory::BvUfArray (Phase A.4 step 4.5) for designs with \
                     array sorts."
                )
            }
        }
    }
}

impl std::error::Error for EncodeError {}

/// Placeholder for the BTOR2 transition-relation encoder.
///
/// Step 4.2 will implement this: walk every `Next { state, value }`
/// line, emit `s'_state == eval(value, s, inputs)`, conjoin into a
/// single Z3 boolean.
pub fn encode_transition_relation(_file: &Btor2File) -> Result<(), EncodeError> {
    // Step 4.1 skeleton: signature only. Returning `Ok(())` keeps
    // downstream callers compilable; step 4.2 changes the return
    // type to carry the encoded `z3::ast::Bool<'ctx>` (or an opaque
    // handle into the `PredicateImage` solver state).
    Ok(())
}
