//! Three-valued (Kleene) sets for sound model checking over partial state spaces.
//!
//! A [`TritSet`] tracks, per state, whether a formula is definitely True,
//! definitely False, or Unknown. It is the bitset analogue of the three-valued
//! μ-calculus semantics from Bruns–Godefroid CONCUR 2000 (generalized model
//! checking) and Huth–Jagadeesan–Schmidt ESOP 2001 (modal transition systems).
//!
//! # Encoding
//!
//! ```text
//!   state_value  must_true  may_true
//!     True          1          1
//!     Unknown       0          1
//!     False         0          0
//!   (invalid)       1          0   <-- enforced via must_true ⊆ may_true
//! ```
//!
//! The invariant `must_true ⊆ may_true` is preserved by every operation in
//! this module. A state is in `must_true` only if it is also in `may_true`;
//! a state is "definitely False" iff it is in neither.
//!
//! # Verdicts
//!
//! At the API boundary, [`TritSet::verdict_at`] projects a state's trit value
//! to one of [`Trit::True`], [`Trit::False`], [`Trit::Unknown`]. Callers that
//! want backward-compatible boolean verdicts can use [`TritSet::must_true`]
//! (conservative for safety: state is reported as satisfying only when
//! definitely satisfying) or [`TritSet::may_true`] (conservative for liveness).
//!
//! # OOB integration
//!
//! Out-of-bounds (OOB) sink states (per [`adapter::systemverilog::kripke::OOB_STATE_KEY`]
//! convention) are represented as Unknown for every atomic predicate:
//! `must_true` cleared, `may_true` set. Source states with transitions to OOB
//! propagate this Unknown through modalities, distinguishing "we couldn't
//! verify" from "we found a counterexample" — the practical user benefit of
//! Phase 8 over the OOB-as-bottom approximation in Phases 1–3.

use std::ops::{BitAndAssign, BitOrAssign, Not};

use bitvec::prelude::*;

/// Per-state trit verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trit {
    /// Property is definitely true at this state.
    True,
    /// Property is definitely false at this state.
    False,
    /// Property's value cannot be determined from the abstract model.
    Unknown,
}

impl From<crate::clts::Tristate> for Trit {
    /// R.1 — bridge between the CLTS-layer 3-valued state-AP
    /// labelling ([`crate::clts::Tristate`], variants prefixed
    /// `Kleene*`) and the evaluator-layer 3-valued formula verdict
    /// ([`Trit`], variants matching the literature's
    /// `True`/`False`/`Unknown`). Both enums encode the same
    /// algebraic domain; the parallel naming reflects the layer
    /// split (data model vs evaluator output) introduced when the
    /// KMTS state-labelling field landed in `clts/mod.rs` (the
    /// `mu_calculus` module already had `Trit` for verdict output).
    /// Conversion is lossless and total.
    fn from(t: crate::clts::Tristate) -> Self {
        match t {
            crate::clts::Tristate::KleeneT => Trit::True,
            crate::clts::Tristate::KleeneF => Trit::False,
            crate::clts::Tristate::KleeneBot => Trit::Unknown,
        }
    }
}

impl From<Trit> for crate::clts::Tristate {
    fn from(t: Trit) -> Self {
        match t {
            Trit::True => crate::clts::Tristate::KleeneT,
            Trit::False => crate::clts::Tristate::KleeneF,
            Trit::Unknown => crate::clts::Tristate::KleeneBot,
        }
    }
}

/// A three-valued set: pair of bitsets `(must_true, may_true)` with invariant
/// `must_true ⊆ may_true`.
#[derive(Debug, Clone)]
pub struct TritSet {
    pub(crate) must_true: BitVec<usize, Lsb0>,
    pub(crate) may_true: BitVec<usize, Lsb0>,
}

impl TritSet {
    /// Construct a TritSet where every state has the given trit value, except
    /// states marked in `oob_bits` are forced to `Unknown` (must=false, may=true).
    ///
    /// This is the workhorse constructor for atomic predicates: the caller
    /// supplies the BitVec of states where the predicate definitely holds
    /// (already OOB-masked, see Phase 3 in `state_matching::create_bitset_for_pattern`),
    /// and `oob_bits` separately marks the OOB sink. The result has
    /// must = bits (OOB cleared) and may = bits ∪ oob_bits.
    pub fn from_predicate(bits: BitVec<usize, Lsb0>, oob_bits: &BitVec<usize, Lsb0>) -> Self {
        let mut must = bits.clone();
        // Defensive: ensure OOB is cleared from must.
        let mut not_oob = oob_bits.clone();
        for i in 0..not_oob.len() {
            let v = not_oob[i];
            not_oob.set(i, !v);
        }
        must.bitand_assign(not_oob.as_bitslice());

        let mut may = bits;
        may.bitor_assign(oob_bits.as_bitslice());

        Self {
            must_true: must,
            may_true: may,
        }
    }

    /// `true` everywhere, with OOB held as Unknown.
    ///
    /// Used for `Node::True` and as the start of greatest fixpoints (`Nu`).
    pub fn all_true(state_count: usize, oob_bits: &BitVec<usize, Lsb0>) -> Self {
        let mut must = BitVec::repeat(true, state_count);
        let mut not_oob = oob_bits.clone();
        for i in 0..not_oob.len() {
            let v = not_oob[i];
            not_oob.set(i, !v);
        }
        must.bitand_assign(not_oob.as_bitslice());
        let may = BitVec::repeat(true, state_count);
        Self {
            must_true: must,
            may_true: may,
        }
    }

    /// `false` everywhere.
    ///
    /// Used for `Node::False` and as the start of least fixpoints (`Mu`).
    pub fn all_false(state_count: usize) -> Self {
        Self {
            must_true: BitVec::repeat(false, state_count),
            may_true: BitVec::repeat(false, state_count),
        }
    }

    /// Number of states in this trit set.
    pub fn len(&self) -> usize {
        self.must_true.len()
    }

    /// `true` iff there are no states.
    pub fn is_empty(&self) -> bool {
        self.must_true.is_empty()
    }

    /// Project the trit value at a single state.
    pub fn verdict_at(&self, state: usize) -> Trit {
        match (self.must_true.get(state), self.may_true.get(state)) {
            (Some(m), _) if *m => Trit::True,
            (_, Some(p)) if *p => Trit::Unknown,
            _ => Trit::False,
        }
    }

    /// View of the must-true bitset (states definitely satisfying the formula).
    pub fn must_true(&self) -> &BitVec<usize, Lsb0> {
        &self.must_true
    }

    /// View of the may-true bitset (states possibly satisfying the formula).
    pub fn may_true(&self) -> &BitVec<usize, Lsb0> {
        &self.may_true
    }

    /// Three-valued And. `must` is intersection; `may` is intersection.
    pub fn and(mut self, other: &Self) -> Self {
        self.must_true.bitand_assign(other.must_true.as_bitslice());
        self.may_true.bitand_assign(other.may_true.as_bitslice());
        self
    }

    /// Three-valued Or. `must` is union; `may` is union.
    pub fn or(mut self, other: &Self) -> Self {
        self.must_true.bitor_assign(other.must_true.as_bitslice());
        self.may_true.bitor_assign(other.may_true.as_bitslice());
        self
    }

    /// Equality on (must, may). Used for fixpoint convergence detection.
    pub fn eq_set(&self, other: &Self) -> bool {
        self.must_true == other.must_true && self.may_true == other.may_true
    }

    /// Construct a TritSet directly from `(must, may)` bitsets.
    ///
    /// The caller is responsible for the `must ⊆ may` invariant. This is
    /// intended for the parallel-evaluation strategy in the trit evaluator,
    /// where modal operators decompose into two independent BitVec passes.
    pub fn from_parts(must: BitVec<usize, Lsb0>, may: BitVec<usize, Lsb0>) -> Self {
        debug_assert_eq!(
            must.len(),
            may.len(),
            "must and may bitsets must have equal length"
        );
        Self {
            must_true: must,
            may_true: may,
        }
    }
}

fn bitvec_complement(b: &BitVec<usize, Lsb0>) -> BitVec<usize, Lsb0> {
    let mut out = BitVec::repeat(false, b.len());
    for (mut o, v) in out.iter_mut().zip(b.iter()) {
        o.set(!*v);
    }
    out
}

/// Three-valued Not. Swaps polarity of (must, may) and complements:
/// `must(¬X) = ¬may(X)`, `may(¬X) = ¬must(X)`. This swap is what makes the
/// Kleene semantics nontrivial — a separate parallel evaluation (one for must,
/// one for may) cannot capture it without explicit polarity tracking.
impl Not for TritSet {
    type Output = TritSet;

    fn not(self) -> Self::Output {
        let TritSet {
            must_true,
            may_true,
        } = self;
        let new_must = bitvec_complement(&may_true);
        let new_may = bitvec_complement(&must_true);
        TritSet {
            must_true: new_must,
            may_true: new_may,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bv(bits: &[bool]) -> BitVec<usize, Lsb0> {
        let mut b = BitVec::repeat(false, bits.len());
        for (i, v) in bits.iter().enumerate() {
            b.set(i, *v);
        }
        b
    }

    #[test]
    fn from_predicate_oob_becomes_unknown() {
        // 3 states: state 0 satisfies P, state 1 does not, state 2 is OOB.
        let bits = bv(&[true, false, false]);
        let oob = bv(&[false, false, true]);
        let trit = TritSet::from_predicate(bits, &oob);

        assert_eq!(trit.verdict_at(0), Trit::True);
        assert_eq!(trit.verdict_at(1), Trit::False);
        assert_eq!(trit.verdict_at(2), Trit::Unknown);
    }

    #[test]
    fn all_true_holds_oob_as_unknown() {
        let oob = bv(&[false, false, true]);
        let trit = TritSet::all_true(3, &oob);

        assert_eq!(trit.verdict_at(0), Trit::True);
        assert_eq!(trit.verdict_at(1), Trit::True);
        assert_eq!(trit.verdict_at(2), Trit::Unknown);
    }

    #[test]
    fn all_false_is_uniform() {
        let trit = TritSet::all_false(3);
        for i in 0..3 {
            assert_eq!(trit.verdict_at(i), Trit::False);
        }
    }

    #[test]
    fn not_swaps_polarity_kleene() {
        // P_at = (T, F, U)  →  ¬P_at = (F, T, U)
        let bits = bv(&[true, false, false]);
        let oob = bv(&[false, false, true]);
        let p = TritSet::from_predicate(bits, &oob);
        let np = p.not();

        assert_eq!(np.verdict_at(0), Trit::False);
        assert_eq!(np.verdict_at(1), Trit::True);
        assert_eq!(np.verdict_at(2), Trit::Unknown);
    }

    #[test]
    fn and_kleene_truth_table() {
        // P at 4 states: T, T, U, F
        // Q at 4 states: T, F, U, U
        // P ∧ Q:        T, F, U, F
        let oob = bv(&[false, false, true, false]);
        let p = TritSet::from_predicate(bv(&[true, true, false, false]), &oob);
        // Q's may_true at index 3 must include it for "U" — so we treat 3 as oob too?
        // Better: build Q manually.
        let q = TritSet {
            must_true: bv(&[true, false, false, false]),
            may_true: bv(&[true, false, true, true]),
        };

        let r = p.and(&q);
        assert_eq!(r.verdict_at(0), Trit::True);
        assert_eq!(r.verdict_at(1), Trit::False);
        assert_eq!(r.verdict_at(2), Trit::Unknown);
        assert_eq!(r.verdict_at(3), Trit::False);
    }

    #[test]
    fn or_kleene_truth_table() {
        // P at 4 states: F, F, U, T
        // Q at 4 states: F, U, U, F
        // P ∨ Q:        F, U, U, T
        let p = TritSet {
            must_true: bv(&[false, false, false, true]),
            may_true: bv(&[false, false, true, true]),
        };
        let q = TritSet {
            must_true: bv(&[false, false, false, false]),
            may_true: bv(&[false, true, true, false]),
        };

        let r = p.or(&q);
        assert_eq!(r.verdict_at(0), Trit::False);
        assert_eq!(r.verdict_at(1), Trit::Unknown);
        assert_eq!(r.verdict_at(2), Trit::Unknown);
        assert_eq!(r.verdict_at(3), Trit::True);
    }

    #[test]
    fn double_negation_round_trips() {
        let oob = bv(&[false, false, true, false]);
        let p = TritSet::from_predicate(bv(&[true, false, false, true]), &oob);
        let p_clone = p.clone();
        let pp = p.not().not();
        assert!(pp.eq_set(&p_clone));
    }

    #[test]
    fn must_subset_may_invariant_after_ops() {
        let oob = bv(&[false, false, true]);
        let p = TritSet::from_predicate(bv(&[true, false, false]), &oob);
        let q = TritSet::from_predicate(bv(&[false, true, false]), &oob);

        for r in [
            p.clone().and(&q),
            p.clone().or(&q),
            p.clone().not(),
            q.clone().not(),
        ] {
            for i in 0..r.len() {
                assert!(
                    !*r.must_true.get(i).unwrap() || *r.may_true.get(i).unwrap(),
                    "must ⊆ may invariant violated at state {i}"
                );
            }
        }
    }
}
