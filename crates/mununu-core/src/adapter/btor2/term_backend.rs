//! §Phase 10 Option-4 step 1a (2026-06-12) — the `BvTermBackend`
//! seam: one generic BTOR2-walk driver, parameterised over the
//! value type the walk produces.
//!
//! # Why this exists
//!
//! mununu interprets the same BTOR2 operator set in two places that
//! grew independently and have drifted in capability (see
//! [`docs/design/expression-interpretation-unification.md`]):
//!
//! - the **concrete** evaluator ([`super::bit_blast::eval_op`] over
//!   `BvValue`) — powers explicit-state enumeration + the R.2.5
//!   predicate-cube may-edge sampling; cannot evaluate `Op::Read` /
//!   `Op::Write`.
//! - the **SMT** encoder ([`crate::adapter::sidecar::predicate_image::btor2_encode::encode_op`]
//!   over `z3::ast::BV`) — powers must-edge queries; speaks Z3 array
//!   theory.
//!
//! Both walk the *same* AST with the *same* operator set, differing
//! only in the value type and the per-op primitive. This module
//! defines the **single seam** that unifies them: a `BvTermBackend`
//! trait whose associated `Value` is the only thing that varies, and
//! a generic [`walk_design`] driver that visits the BTOR2 node DAG
//! once and dispatches each node to the backend.
//!
//! # Staging (the Option-4 track)
//!
//! - **step 1a (this module):** the trait + driver + the
//!   [`super::bit_blast::ConcreteBackend`] implementation
//!   (delegating to the existing `eval_op`, so the arithmetic is
//!   bit-identical). Proven equivalent to the existing
//!   `evaluate_pure` by a parallel-path test. The production
//!   `evaluate_pure` is UNCHANGED — the driver path is parallel +
//!   test-pinned, not yet a cutover.
//! - step 1b: retire `evaluate_pure`'s bespoke loop by pointing it
//!   at `walk_design::<ConcreteBackend>` (the existing bit-blast
//!   suite is the gate).
//! - step 1c: the `Z3Backend` (porting `encode_op` + the shipped
//!   array encoder) implements the same trait — the must-edge path
//!   becomes "run the Z3 backend."
//! - step 1d+: real UF in the Z3 backend; the ibex-regfile
//!   read-after-write milestone.
//!
//! Mirrors the [`crate::mu_calculus::truth_domain::TruthDomain`]
//! tagless-final precedent the verdict evaluator already uses.

use crate::adapter::btor2::ast::{Btor2File, ConstValue, Nid, Node, Op, Operand};
use crate::adapter::btor2::parser::bv_width;

/// A backend that interprets BTOR2 operators into some value type.
///
/// The associated `Value` is the only thing that varies between the
/// concrete (`BvValue`) and SMT (`z3` term) interpretations. The
/// backend owns its own environment (the `Nid → Value` binding
/// store) so the generic [`walk_design`] driver stays
/// value-type-agnostic.
pub trait BvTermBackend {
    /// The value the walk produces per node.
    type Value: Clone;
    /// Backend-specific evaluation error.
    type Error;

    /// Evaluate a `Node::Const` to a value of the given bit-width.
    fn eval_const(&mut self, value: &ConstValue, width: u32) -> Result<Self::Value, Self::Error>;

    /// Evaluate a `Node::Op` to a value. This is the seam that
    /// `eval_op` (concrete) and `encode_op` (SMT) each provide.
    /// Operand values are read from the backend's env via its own
    /// resolution (the backend holds the env).
    fn eval_op(
        &mut self,
        op: Op,
        immediates: &[u32],
        args: &[Operand],
        width: u32,
    ) -> Result<Self::Value, Self::Error>;

    /// Bind `nid → value` in the backend's env.
    fn bind(&mut self, nid: Nid, value: Self::Value);

    /// Whether `Node::Init` lines should copy their value into the
    /// state cell (true only when computing the initial-state
    /// assignment).
    fn honor_init(&self) -> bool;

    /// Read an operand's bound value (handling BTOR2 negative-NID
    /// negation), used for the `Node::Init` copy step.
    fn read_operand(&self, op: Operand) -> Option<Self::Value>;

    /// UF-substitution hook: when `nid` is a UF-wrapped operator,
    /// return the substitute value (the walk binds it and skips
    /// `eval_op`). Returns `None` for non-wrapped nodes, in which
    /// case the walk evaluates the operator normally.
    fn uf_substitute(&mut self, nid: Nid, width: u32) -> Option<Self::Value>;
}

/// Structural error from the generic walk, or a wrapped backend
/// error.
#[derive(Debug)]
pub enum WalkError<E> {
    /// A `Node::Const` or `Node::Op` referenced a non-bitvec sort.
    NonBitvecSort(Nid),
    /// A `Node::Init` value was not yet evaluated when honoured.
    Unevaluated(Nid),
    /// The backend's own evaluation failed.
    Backend(E),
}

/// §Phase 10 Option-4 step 1a — the single generic BTOR2-walk
/// driver. Visits the node DAG in declaration order, binding each
/// `Const` / `Op` node's value into the backend's env. Mirrors the
/// node iteration in [`super::bit_blast`]'s `evaluate_pure_with_uf_rep`
/// — but parameterised over the backend so concrete + SMT share one
/// loop.
///
/// Inputs / states are assumed pre-bound by the caller (the
/// concrete path's `make_*_env`; the SMT path's pass-1 variable
/// declaration). `Sort` + side-effect nodes (`Next` / `Bad` / …)
/// add nothing to the env.
pub fn walk_design<B: BvTermBackend>(
    file: &Btor2File,
    backend: &mut B,
) -> Result<(), WalkError<B::Error>> {
    for line in &file.lines {
        match &line.node {
            // Inputs / states pre-bound; sort lines carry no value.
            Node::Sort { .. } | Node::Input { .. } | Node::State { .. } => {}
            Node::Const { sort, value } => {
                let width = bv_width(file, *sort).ok_or(WalkError::NonBitvecSort(line.nid))?;
                let v = backend
                    .eval_const(value, width)
                    .map_err(WalkError::Backend)?;
                backend.bind(line.nid, v);
            }
            Node::Init { state, value, .. } => {
                if backend.honor_init() {
                    let v = backend
                        .read_operand(*value)
                        .ok_or(WalkError::Unevaluated(line.nid))?;
                    backend.bind(*state, v);
                }
            }
            Node::Op { sort, op, args, .. } => {
                let width = bv_width(file, *sort).ok_or(WalkError::NonBitvecSort(line.nid))?;
                // UF substitution: bind the representative + skip the
                // operator evaluation when the node is UF-wrapped.
                if let Some(sub) = backend.uf_substitute(line.nid, width) {
                    backend.bind(line.nid, sub);
                    continue;
                }
                let v = backend
                    .eval_op(*op, &line.immediates, args, width)
                    .map_err(WalkError::Backend)?;
                backend.bind(line.nid, v);
            }
            // Side-effect declarations don't add to env.
            Node::Next { .. }
            | Node::Bad { .. }
            | Node::Constraint { .. }
            | Node::Fair { .. }
            | Node::Output { .. }
            | Node::Justice { .. } => {}
        }
    }
    Ok(())
}
