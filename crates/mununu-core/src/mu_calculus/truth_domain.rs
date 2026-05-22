//! R.1 — `TruthDomain` trait and `BoolDomain` instantiation.
//!
//! Per the KMTS architecture
//! (`docs/design/native-sv-abstraction.md` §6.4,
//! `docs/design/kmts-theory.md` §4.1), 3-valued model checking
//! involves **two distinct lattice structures over the same
//! underlying set**:
//!
//! - **Truth lattice** — used for formula semantics
//!   (`∧`, `∨`, `¬`). In `BoolDomain` this is `false < true`. In
//!   `KleeneDomain` (R.3) this is `false < true` with `KleeneBot`
//!   *incomparable* to both — Kleene's strong 3-valued connectives
//!   from CONCUR 2000.
//! - **Information lattice** — used for fixpoint convergence. In
//!   `BoolDomain` this *coincides* with the truth lattice. In
//!   `KleeneDomain` `KleeneBot < KleeneF` and `KleeneBot < KleeneT`,
//!   with `KleeneF` and `KleeneT` incomparable. Convergence means
//!   "becoming more defined."
//!
//! The two-lattice dichotomy is the most common implementation
//! pitfall in 3-valued model checking (Bruns–Godefroid CONCUR 2000
//! §4 has the canonical treatment). Surfacing both in the trait
//! upfront — even though `BoolDomain` aliases them — prevents R.3
//! from needing to churn the API once `KleeneDomain` arrives.
//!
//! R.1 ships only [`BoolDomain`] (the 2-valued specialisation that
//! preserves today's evaluator semantics). R.3 ships `KleeneDomain`
//! against the same trait. R.5 wires CEGAR refinement against the
//! 3-valued verdict.

use crate::clts::{IdStorage, StateId, Tristate};

/// R.1 — The two-lattice abstraction over 3-valued model-checking
/// truth values. Instantiated by [`BoolDomain`] (legacy 2-valued,
/// shipping in R.1) and `KleeneDomain` (3-valued, R.3 deliverable).
///
/// Implementors choose:
///
/// - `Element` — the carrier set (a [`bool`] for `BoolDomain`; a
///   [`Tristate`] for `KleeneDomain`).
/// - Truth-order operations (`truth_*`) — semantics of formula
///   connectives (`∧`, `∨`, `¬`).
/// - Information-order operations (`info_*`) — convergence
///   discipline for least / greatest fixpoint iteration.
/// - Modal operators (`box_modality`, `diamond_modality`) — how
///   `[a]φ` and `⟨a⟩φ` read the may / must transition split.
///
/// **Per-call inputs to modal operators.** The evaluator passes
/// the truth values of φ at each successor:
///
/// - `may` — values at every may-successor of `s` on label `a`.
/// - `must` — values at every must-successor of `s` on label `a`
///   (must ⊆ may by KMTS invariant; for Sharp-only CLTSes, must = may).
///
/// `BoolDomain` ignores the may / must split (it treats every
/// transition as Sharp) so the two slices are identical.
/// `KleeneDomain` (R.3) reads both, applying the §6.2 asymmetric
/// semantics (`[a]φ = T iff all may-successors are T; F iff some
/// must-successor is F; else ⊥`).
pub trait TruthDomain {
    type Element: Clone + std::fmt::Debug + Eq;

    // ---- Truth-order operations (formula semantics) ----

    /// `false` / `KleeneF` — the truth-order least element.
    fn truth_bot(&self) -> Self::Element;
    /// `true` / `KleeneT` — the truth-order top element.
    fn truth_top(&self) -> Self::Element;
    /// `∨` — Kleene-3-valued disjunction (`T ∨ ⊥ = T`, `F ∨ ⊥ = ⊥`).
    fn truth_join(&self, a: &Self::Element, b: &Self::Element) -> Self::Element;
    /// `∧` — Kleene-3-valued conjunction (`F ∧ ⊥ = F`, `T ∧ ⊥ = ⊥`).
    fn truth_meet(&self, a: &Self::Element, b: &Self::Element) -> Self::Element;
    /// `¬` — Kleene negation (`¬⊥ = ⊥`).
    fn truth_negate(&self, a: &Self::Element) -> Self::Element;

    // ---- Information-order operations (fixpoint convergence) ----

    /// `false` / `KleeneBot` — the information-order least element.
    /// **Coincides with [`Self::truth_bot`] in [`BoolDomain`]** —
    /// `false` is both the truth-order bottom and the information-order
    /// bottom in 2-valued land. **Differs in `KleeneDomain`** where
    /// the information-order bottom is `KleeneBot` (the most-uncertain
    /// element).
    fn info_bot(&self) -> Self::Element;
    /// Information-order join — combines two values toward
    /// more-definedness. In `BoolDomain` this coincides with
    /// `truth_join`. In `KleeneDomain`: `KleeneBot ⊔_i T = T`,
    /// `KleeneBot ⊔_i F = F`; `T ⊔_i F` is structurally inconsistent
    /// and indicates a fixpoint-iteration bug (the formula's semantics
    /// can never produce two definite-and-disagreeing values for the
    /// same state).
    fn info_join(&self, a: &Self::Element, b: &Self::Element) -> Self::Element;
    /// Information-order ≤. `info_leq(a, b) = true` iff `b` is at
    /// least as defined as `a`. Used as the convergence predicate
    /// in Kleene iteration (`f^n(x) ⊑_i f^(n+1)(x)` is the
    /// invariant maintained by monotone fixpoint computation).
    fn info_leq(&self, a: &Self::Element, b: &Self::Element) -> bool;

    // ---- Modal operators ----

    /// `[a]φ` — universal modality. `T` iff every may-successor has
    /// `T`; `F` iff some must-successor has `F`; else `⊥` (in
    /// `KleeneDomain`) or trivially `T`/`F` (in `BoolDomain`,
    /// where the may/must distinction collapses).
    ///
    /// **Sharp / single-target must.** Each entry of `must` is the φ
    /// value at one must-successor's single target. For R.1 / R.3
    /// (standard KMTS, no `MustHyperOnly`) this is the only modal
    /// path the evaluator takes.
    fn box_modality(&self, may: &[Self::Element], must: &[Self::Element]) -> Self::Element;
    /// `⟨a⟩φ` — existential modality. `T` iff some must-successor
    /// has `T`; `F` iff every may-successor has `F`; else `⊥` /
    /// trivial.
    fn diamond_modality(&self, may: &[Self::Element], must: &[Self::Element]) -> Self::Element;

    /// R.4.5 — `[a]φ` over a GKMTS that may carry `MustHyperOnly`
    /// transitions (per Shoham–Grumberg LMCS 2007 §3;
    /// `docs/design/native-sv-abstraction.md` §6.11;
    /// `docs/design/kmts-theory.md` §3.5).
    ///
    /// `must_edges` is **outer-per-must-edge, inner-per-hyper-target**:
    /// `must_edges[i]` is the slice of φ values at the targets of the
    /// i-th must-edge. For a single-target (Sharp / single-must) edge,
    /// `must_edges[i].len() == 1`; for a hyper-must edge the inner
    /// slice has one entry per hyper-target.
    ///
    /// **Hyper-must semantics for the box false-check** (the load-bearing
    /// rule): a hyper-must edge `s →ᴴ T` contributes a false witness
    /// to `[a]φ` iff **every** `t ∈ T` has `φ(t) = false`. Because the
    /// refinement realizes the must-edge by hitting any one `t`, a
    /// single non-false target is enough to escape the false-witness.
    ///
    /// **Per-edge reduction is truth-order JOIN (∨), not meet.** In
    /// 2-valued logic, `∨` over targets is `false` iff *all* targets
    /// are `false` — which is exactly the condition for the hyper-must
    /// edge to witness `[a]φ = false`. In Kleene 3-valued logic the
    /// same identity holds: `false ∨ false = false`, `false ∨ t = ⊥`
    /// for `t ∈ {⊥, true}`. So the per-edge join lifts the
    /// "all targets false" check into a single value that the flat
    /// [`Self::box_modality`] can consume on its `must` slice
    /// (which it checks for `false` membership).
    ///
    /// (The asymmetry vs [`Self::diamond_modality_hyper`] — which
    /// uses meet — is real: diamond's true-witness requires *every*
    /// target to be true, hence `∧`; box's false-witness requires
    /// *every* target to be false, hence `∨` over the polarity-flipped
    /// values — equivalently, just `∨` over the original values since
    /// `false ∨ false = false`.)
    ///
    /// **Default implementation** delegates to [`Self::box_modality`]
    /// after the per-edge join reduction. Empty hyper-target sets are
    /// skipped (an unrealizable must-edge cannot contribute a witness).
    /// Both `BoolDomain` and `KleeneDomain` inherit this default —
    /// the semantics are correct under either truth domain's `truth_join`.
    fn box_modality_hyper(
        &self,
        may: &[Self::Element],
        must_edges: &[&[Self::Element]],
    ) -> Self::Element {
        let must_per_edge: Vec<Self::Element> = must_edges
            .iter()
            .filter_map(|edge_targets| {
                let mut iter = edge_targets.iter();
                let first = iter.next()?.clone();
                Some(iter.fold(first, |acc, t| self.truth_join(&acc, t)))
            })
            .collect();
        self.box_modality(may, &must_per_edge)
    }

    /// R.4.5 — `⟨a⟩φ` over a GKMTS.
    ///
    /// **Hyper-must semantics for the diamond true-witness:** a
    /// hyper-must edge `s →ᴴ T` contributes a true witness to `⟨a⟩φ`
    /// iff **every** `t ∈ T` has `φ(t) = true`. Per-edge reduction is
    /// truth-order MEET (∧) — the conjunction is `true` iff all targets
    /// are `true`, which is the condition for the must-witness to
    /// guarantee φ regardless of which `t` the refinement picks.
    ///
    /// **Default implementation** delegates to [`Self::diamond_modality`]
    /// after the per-edge meet reduction. Empty hyper-target sets are
    /// skipped. Both `BoolDomain` and `KleeneDomain` inherit this
    /// default.
    fn diamond_modality_hyper(
        &self,
        may: &[Self::Element],
        must_edges: &[&[Self::Element]],
    ) -> Self::Element {
        let must_per_edge: Vec<Self::Element> = must_edges
            .iter()
            .filter_map(|edge_targets| {
                let mut iter = edge_targets.iter();
                let first = iter.next()?.clone();
                Some(iter.fold(first, |acc, t| self.truth_meet(&acc, t)))
            })
            .collect();
        self.diamond_modality(may, &must_per_edge)
    }

    // ---- Bridges to mununu's CLTS layer ----
    //
    // These are *trait* methods (need `&self` for dispatch), not free
    // constructors, so the `from_*` naming convention does not apply
    // (clippy's `wrong_self_convention` lint flags it). The `lift_*`
    // name also reads more clearly: we're *lifting* a CLTS-layer
    // value into the evaluator's truth domain.

    /// Lift a 2-valued state-AP labelling into this domain. Called
    /// when the evaluator reads a Sharp-only CLTS's
    /// `state_variables` bitset — for `BoolDomain` this is identity;
    /// for `KleeneDomain` it maps `true → KleeneT`, `false → KleeneF`
    /// (never `KleeneBot`, because the labelling is two-valued).
    fn lift_bool(&self, b: bool) -> Self::Element;

    /// Lift a 3-valued state-AP labelling into this domain. Called
    /// when the evaluator reads a KMTS-aware CLTS's
    /// `state_3valued_predicates` map. For `BoolDomain`: any
    /// `KleeneBot` value is treated as `false` (conservative for
    /// safety — the abstraction is too coarse to confirm the
    /// predicate). For `KleeneDomain` (R.3): identity.
    fn lift_tristate(&self, t: Tristate) -> Self::Element;

    /// Whether this domain projection of a CLTS's predicate at a
    /// state is the 3-valued "Unknown" / `KleeneBot`. Always
    /// `false` for `BoolDomain`. Used by callers that want to
    /// distinguish "definitely refined" from "abstractly
    /// underdetermined" in their result reporting.
    fn is_unknown(&self, _state: &StateId<impl IdStorage>, _value: &Self::Element) -> bool {
        false
    }
}

/// R.1 — Legacy 2-valued instantiation of [`TruthDomain`]. Preserves
/// today's evaluator semantics exactly: every CLTS transition is
/// treated as Sharp, every state-AP labelling is two-valued
/// (`true`/`false`), every modal operator collapses to the standard
/// Boolean-CTL/μ-calculus rule.
///
/// **The default `TruthDomain` for every existing adapter and
/// every call site that has not been migrated to KMTS yet.** R.3
/// adds `KleeneDomain` alongside `BoolDomain`; callers choose at
/// evaluator invocation time.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoolDomain;

impl TruthDomain for BoolDomain {
    type Element = bool;

    fn truth_bot(&self) -> bool {
        false
    }
    fn truth_top(&self) -> bool {
        true
    }
    fn truth_join(&self, a: &bool, b: &bool) -> bool {
        *a || *b
    }
    fn truth_meet(&self, a: &bool, b: &bool) -> bool {
        *a && *b
    }
    fn truth_negate(&self, a: &bool) -> bool {
        !*a
    }

    fn info_bot(&self) -> bool {
        // In `BoolDomain` the information lattice coincides with the
        // truth lattice — the information-order bottom IS `false`.
        // `KleeneDomain` (R.3) returns `KleeneBot` here instead.
        false
    }
    fn info_join(&self, a: &bool, b: &bool) -> bool {
        // Information join in `BoolDomain` is just truth join because
        // the two lattices coincide. The fixpoint engine iterates
        // until `info_leq` reports a fixpoint, which in 2-valued land
        // is the same as truth-equality.
        *a || *b
    }
    fn info_leq(&self, a: &bool, b: &bool) -> bool {
        // false ⊑_i true (becoming more defined); equal values are ⊑.
        // In 2-valued semantics this matches `*a <= *b`.
        !*a || *b
    }

    fn box_modality(&self, may: &[bool], _must: &[bool]) -> bool {
        // `[a]φ = T iff every may-successor has T`. In Sharp-only
        // CLTSes may = must, so the must slice is redundant; `BoolDomain`
        // accepts it for trait compatibility and ignores it.
        // Convention: empty successor list ⇒ `true` (vacuous universal,
        // matching the existing evaluator's behaviour for terminal states).
        may.iter().all(|v| *v)
    }
    fn diamond_modality(&self, may: &[bool], _must: &[bool]) -> bool {
        // `⟨a⟩φ = T iff some must-successor has T`. Same Sharp-only
        // collapse: may = must, so we read may. Convention: empty
        // successor list ⇒ `false` (vacuous existential).
        may.iter().any(|v| *v)
    }

    fn lift_bool(&self, b: bool) -> bool {
        b
    }
    fn lift_tristate(&self, t: Tristate) -> bool {
        // Conservative coercion: `KleeneBot` becomes `false` because
        // `BoolDomain` cannot represent the abstraction's uncertainty.
        // This preserves safety-property soundness (the abstract model
        // reports "predicate does not hold" wherever the lifter could
        // not confirm it), at the cost of liveness-property precision.
        // `KleeneDomain` (R.3) preserves `KleeneBot` natively.
        matches!(t, Tristate::KleeneT)
    }
}

/// R.3 — 3-valued (Kleene) instantiation of [`TruthDomain`]. The
/// formula evaluator uses this domain to produce verdicts in
/// `{ KleeneT, KleeneF, KleeneBot }` over KMTS-aware CLTSes that
/// carry [`crate::clts::TransitionModality::MayOnly`] transitions
/// and/or `KleeneBot` state-predicate labellings.
///
/// **Soundness contract** (Bruns–Godefroid CONCUR 2000, preservation
/// theorem; restated in `docs/design/native-sv-abstraction.md` §6.2):
/// for any property φ and abstract KMTS `M_α` of concrete `M`,
///
/// - `KleeneDomain ⊨ φ @ s = KleeneT` ⇒ `M ⊨ φ @ s = true`
/// - `KleeneDomain ⊨ φ @ s = KleeneF` ⇒ `M ⊨ φ @ s = false`
/// - `KleeneDomain ⊨ φ @ s = KleeneBot` ⇒ `M ⊨ φ @ s` is
///   either `true` or `false`; the abstraction is too coarse to decide
///   and CEGAR (R.5) refines.
///
/// **Sharp-only collapse.** When every transition is
/// [`crate::clts::TransitionModality::Sharp`] and every state-AP is
/// in `{ KleeneT, KleeneF }` (the data shape every legacy adapter
/// produces post-R.1), `KleeneDomain` verdicts contain no `KleeneBot`
/// and project to exactly the same Booleans `BoolDomain` would emit.
/// The verdict-baseline regression test (R.3 done-criterion) enforces
/// this property over the existing test suite.
#[derive(Debug, Clone, Copy, Default)]
pub struct KleeneDomain;

impl TruthDomain for KleeneDomain {
    type Element = Tristate;

    fn truth_bot(&self) -> Tristate {
        Tristate::KleeneF
    }
    fn truth_top(&self) -> Tristate {
        Tristate::KleeneT
    }

    fn truth_join(&self, a: &Tristate, b: &Tristate) -> Tristate {
        // Kleene strong disjunction:
        //   T ∨ x = T, F ∨ x = x, ⊥ ∨ ⊥ = ⊥, ⊥ ∨ F = ⊥.
        // The "definitely true wins" rule keeps `T` absorbing; the
        // "definitely false is the identity" rule keeps `F` neutral.
        match (a, b) {
            (Tristate::KleeneT, _) | (_, Tristate::KleeneT) => Tristate::KleeneT,
            (Tristate::KleeneBot, _) | (_, Tristate::KleeneBot) => Tristate::KleeneBot,
            _ => Tristate::KleeneF,
        }
    }

    fn truth_meet(&self, a: &Tristate, b: &Tristate) -> Tristate {
        // Kleene strong conjunction (dual of join):
        //   F ∧ x = F, T ∧ x = x, ⊥ ∧ ⊥ = ⊥, ⊥ ∧ T = ⊥.
        match (a, b) {
            (Tristate::KleeneF, _) | (_, Tristate::KleeneF) => Tristate::KleeneF,
            (Tristate::KleeneBot, _) | (_, Tristate::KleeneBot) => Tristate::KleeneBot,
            _ => Tristate::KleeneT,
        }
    }

    fn truth_negate(&self, a: &Tristate) -> Tristate {
        // Kleene negation: T ↔ F, ⊥ fixed (we have no evidence either way,
        // so the negation also has no evidence).
        match a {
            Tristate::KleeneT => Tristate::KleeneF,
            Tristate::KleeneF => Tristate::KleeneT,
            Tristate::KleeneBot => Tristate::KleeneBot,
        }
    }

    fn info_bot(&self) -> Tristate {
        // The information-order least element is `KleeneBot` — "least
        // defined." Fixpoint iteration starts here for least fixpoints
        // (`Mu`) and ascends toward more-defined values.
        Tristate::KleeneBot
    }

    fn info_join(&self, a: &Tristate, b: &Tristate) -> Tristate {
        // Information join: combine two values toward more-definedness.
        //   ⊥ ⊔_i x = x  (the more-defined value wins)
        //   T ⊔_i T = T, F ⊔_i F = F  (agreement)
        //   T ⊔_i F  is structurally inconsistent — two definite-and-
        //   disagreeing values for the same fixpoint cell indicates a
        //   bug in the modal-operator code. We debug_assert and fall
        //   back to KleeneBot (the safe "we don't know" value), which
        //   preserves liveness soundness if the assertion is compiled
        //   out in release.
        match (a, b) {
            (Tristate::KleeneBot, x) | (x, Tristate::KleeneBot) => *x,
            (Tristate::KleeneT, Tristate::KleeneT) => Tristate::KleeneT,
            (Tristate::KleeneF, Tristate::KleeneF) => Tristate::KleeneF,
            _ => {
                debug_assert!(
                    false,
                    "info_join of two definite-and-disagreeing values \
                     ({a:?} ⊔_i {b:?}) — fixpoint iteration is producing \
                     inconsistent updates, likely a modal-operator bug"
                );
                Tristate::KleeneBot
            }
        }
    }

    fn info_leq(&self, a: &Tristate, b: &Tristate) -> bool {
        // `a ⊑_i b` iff `b` is at least as defined as `a`.
        //   ⊥ ⊑_i ⊥, ⊥ ⊑_i T, ⊥ ⊑_i F   (⊥ is below everything)
        //   T ⊑_i T, F ⊑_i F             (reflexive on definite values)
        //   T ⊑_i F is false; F ⊑_i T is false (definite values are
        //   incomparable to each other).
        match (a, b) {
            (Tristate::KleeneBot, _) => true,
            (x, y) if x == y => true,
            _ => false,
        }
    }

    fn box_modality(&self, may: &[Tristate], must: &[Tristate]) -> Tristate {
        // `[a]φ` per `docs/design/native-sv-abstraction.md` §6.2 +
        // §6.11 (R.4.5 hyper-must extension):
        //   - `KleeneT` iff every may-successor is T AND no must-edge
        //     contributes a non-definite (KleeneBot) per-edge value.
        //     The second clause matters only for hyper-must inputs
        //     (R.4.5 +) where the per-edge JOIN over hyper-targets
        //     can be KleeneBot; for Sharp-only inputs the must slice
        //     contains only definite values, so the second clause is
        //     vacuously true and the semantics collapses to the
        //     §6.2 Sharp form.
        //   - `KleeneF` iff some must-edge is definitely F (all its
        //     hyper-targets false, equivalently per-edge JOIN is F).
        //   - `KleeneBot` otherwise — covers both "some may-successor
        //     undefined" and "some must-edge undefined" cases.
        //
        // The asymmetry: every may (over-approx) must be T to
        // conclude T (sound); a single must witness suffices for F.
        //
        // Convention: empty may-list ⇒ vacuous T (no successors means
        // no way to falsify the universal). Matches BoolDomain.
        if must.iter().any(|v| matches!(v, Tristate::KleeneF)) {
            Tristate::KleeneF
        } else if may.iter().all(|v| matches!(v, Tristate::KleeneT))
            && must.iter().all(|v| matches!(v, Tristate::KleeneT))
        {
            Tristate::KleeneT
        } else {
            Tristate::KleeneBot
        }
    }

    fn diamond_modality(&self, may: &[Tristate], must: &[Tristate]) -> Tristate {
        // `⟨a⟩φ` per §6.2 + §6.11 (dual of box_modality):
        //   - `KleeneT` iff some must-edge is definitely T (all its
        //     hyper-targets true, equivalently per-edge MEET is T).
        //   - `KleeneF` iff every may-successor is F AND no must-edge
        //     contributes a non-definite (KleeneBot) per-edge value.
        //   - `KleeneBot` otherwise.
        //
        // Convention: empty may-list ⇒ vacuous F. Matches BoolDomain.
        if must.iter().any(|v| matches!(v, Tristate::KleeneT)) {
            Tristate::KleeneT
        } else if may.iter().all(|v| matches!(v, Tristate::KleeneF))
            && must.iter().all(|v| matches!(v, Tristate::KleeneF))
        {
            Tristate::KleeneF
        } else {
            Tristate::KleeneBot
        }
    }

    fn lift_bool(&self, b: bool) -> Tristate {
        // Sharp-only adapters' 2-valued bitsets lift losslessly into
        // {KleeneT, KleeneF}; KleeneBot is never produced here.
        Tristate::from_bool(b)
    }

    fn lift_tristate(&self, t: Tristate) -> Tristate {
        t
    }

    fn is_unknown(&self, _state: &StateId<impl IdStorage>, value: &Tristate) -> bool {
        matches!(value, Tristate::KleeneBot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_domain_truth_lattice_is_boolean() {
        let d = BoolDomain;
        assert!(!d.truth_bot());
        assert!(d.truth_top());
        assert!(d.truth_join(&true, &false));
        assert!(!d.truth_meet(&true, &false));
        assert!(!d.truth_negate(&true));
        assert!(d.truth_negate(&false));
    }

    #[test]
    fn bool_domain_info_lattice_coincides_with_truth() {
        let d = BoolDomain;
        // info_bot == truth_bot in BoolDomain.
        assert_eq!(d.info_bot(), d.truth_bot());
        // info_join == truth_join (both are ∨ in 2-valued land).
        for (a, b) in [(true, true), (true, false), (false, true), (false, false)] {
            assert_eq!(d.info_join(&a, &b), d.truth_join(&a, &b));
        }
        // info_leq: false ⊑ false, false ⊑ true, true ⊑ true, NOT true ⊑ false.
        assert!(d.info_leq(&false, &false));
        assert!(d.info_leq(&false, &true));
        assert!(d.info_leq(&true, &true));
        assert!(!d.info_leq(&true, &false));
    }

    #[test]
    fn bool_domain_box_modality_collapses_must_to_may() {
        let d = BoolDomain;
        // `[a]φ`: every may-successor must hold.
        assert!(d.box_modality(&[true, true], &[true, true]));
        assert!(!d.box_modality(&[true, false], &[true, false]));
        // Empty successor list → vacuous true.
        assert!(d.box_modality(&[], &[]));
        // Must slice is ignored (Sharp-only collapse).
        assert!(d.box_modality(&[true], &[false]));
    }

    #[test]
    fn bool_domain_diamond_modality_reads_may() {
        let d = BoolDomain;
        // `⟨a⟩φ`: some must-successor must hold (collapses to may here).
        assert!(d.diamond_modality(&[false, true], &[false, true]));
        assert!(!d.diamond_modality(&[false, false], &[false, false]));
        // Empty list → vacuous false.
        assert!(!d.diamond_modality(&[], &[]));
        // Must slice is ignored.
        assert!(d.diamond_modality(&[true], &[false]));
    }

    #[test]
    fn bool_domain_lift_tristate_is_conservative() {
        let d = BoolDomain;
        assert!(d.lift_tristate(Tristate::KleeneT));
        assert!(!d.lift_tristate(Tristate::KleeneF));
        // KleeneBot conservatively becomes false in BoolDomain.
        assert!(!d.lift_tristate(Tristate::KleeneBot));
    }

    #[test]
    fn bool_domain_lift_bool_is_identity() {
        let d = BoolDomain;
        assert!(d.lift_bool(true));
        assert!(!d.lift_bool(false));
    }

    // ---- R.3 KleeneDomain tests ----

    #[test]
    fn kleene_truth_join_is_strong_disjunction() {
        let d = KleeneDomain;
        use Tristate::*;
        // T absorbs everything
        assert_eq!(d.truth_join(&KleeneT, &KleeneT), KleeneT);
        assert_eq!(d.truth_join(&KleeneT, &KleeneF), KleeneT);
        assert_eq!(d.truth_join(&KleeneT, &KleeneBot), KleeneT);
        assert_eq!(d.truth_join(&KleeneBot, &KleeneT), KleeneT);
        // F is identity for join when the other arg is not T
        assert_eq!(d.truth_join(&KleeneF, &KleeneF), KleeneF);
        // ⊥ ∨ F = ⊥ (we might still see a true witness later)
        assert_eq!(d.truth_join(&KleeneBot, &KleeneF), KleeneBot);
        assert_eq!(d.truth_join(&KleeneF, &KleeneBot), KleeneBot);
        assert_eq!(d.truth_join(&KleeneBot, &KleeneBot), KleeneBot);
    }

    #[test]
    fn kleene_truth_meet_is_strong_conjunction() {
        let d = KleeneDomain;
        use Tristate::*;
        // F absorbs everything
        assert_eq!(d.truth_meet(&KleeneF, &KleeneT), KleeneF);
        assert_eq!(d.truth_meet(&KleeneF, &KleeneBot), KleeneF);
        assert_eq!(d.truth_meet(&KleeneBot, &KleeneF), KleeneF);
        // T is identity for meet
        assert_eq!(d.truth_meet(&KleeneT, &KleeneT), KleeneT);
        // ⊥ ∧ T = ⊥ (we might still see a false witness later)
        assert_eq!(d.truth_meet(&KleeneT, &KleeneBot), KleeneBot);
        assert_eq!(d.truth_meet(&KleeneBot, &KleeneT), KleeneBot);
        assert_eq!(d.truth_meet(&KleeneBot, &KleeneBot), KleeneBot);
    }

    #[test]
    fn kleene_negate_preserves_bot() {
        let d = KleeneDomain;
        use Tristate::*;
        assert_eq!(d.truth_negate(&KleeneT), KleeneF);
        assert_eq!(d.truth_negate(&KleeneF), KleeneT);
        assert_eq!(d.truth_negate(&KleeneBot), KleeneBot);
        // Double-negation round-trips.
        for v in [KleeneT, KleeneF, KleeneBot] {
            assert_eq!(d.truth_negate(&d.truth_negate(&v)), v);
        }
    }

    #[test]
    fn kleene_info_lattice_has_bot_below_definite_values() {
        let d = KleeneDomain;
        use Tristate::*;
        assert_eq!(d.info_bot(), KleeneBot);
        // ⊥ ⊑ everything
        assert!(d.info_leq(&KleeneBot, &KleeneBot));
        assert!(d.info_leq(&KleeneBot, &KleeneT));
        assert!(d.info_leq(&KleeneBot, &KleeneF));
        // Reflexive on definite values
        assert!(d.info_leq(&KleeneT, &KleeneT));
        assert!(d.info_leq(&KleeneF, &KleeneF));
        // Definite values are incomparable to each other in info-order
        assert!(!d.info_leq(&KleeneT, &KleeneF));
        assert!(!d.info_leq(&KleeneF, &KleeneT));
        // Definite values cannot be approximated by ⊥ (would lose information)
        assert!(!d.info_leq(&KleeneT, &KleeneBot));
        assert!(!d.info_leq(&KleeneF, &KleeneBot));
    }

    #[test]
    fn kleene_info_join_promotes_definedness() {
        let d = KleeneDomain;
        use Tristate::*;
        // ⊥ ⊔_i x = x
        assert_eq!(d.info_join(&KleeneBot, &KleeneT), KleeneT);
        assert_eq!(d.info_join(&KleeneT, &KleeneBot), KleeneT);
        assert_eq!(d.info_join(&KleeneBot, &KleeneF), KleeneF);
        assert_eq!(d.info_join(&KleeneF, &KleeneBot), KleeneF);
        assert_eq!(d.info_join(&KleeneBot, &KleeneBot), KleeneBot);
        // Agreement on definite values
        assert_eq!(d.info_join(&KleeneT, &KleeneT), KleeneT);
        assert_eq!(d.info_join(&KleeneF, &KleeneF), KleeneF);
    }

    #[test]
    fn kleene_box_modality_asymmetric_per_spec_6_2() {
        let d = KleeneDomain;
        use Tristate::*;
        // KleeneT iff every may-successor is T
        assert_eq!(d.box_modality(&[KleeneT, KleeneT], &[KleeneT]), KleeneT);
        // KleeneF iff some must-successor is F
        assert_eq!(d.box_modality(&[KleeneT, KleeneF], &[KleeneF]), KleeneF);
        // Mixed: a may-successor is ⊥, no must-successor is F → ⊥
        assert_eq!(d.box_modality(&[KleeneT, KleeneBot], &[KleeneT]), KleeneBot);
        // Empty successor lists ⇒ vacuous T (matches BoolDomain convention)
        assert_eq!(d.box_modality(&[], &[]), KleeneT);
        // A must-F outweighs all-may-T (Sharp invariant violated here in
        // input — non-Sharp KMTSes where some-must but not all-may could
        // appear; the F verdict is correct because the must-edge is a
        // concrete counterexample regardless of may saturation).
        assert_eq!(d.box_modality(&[KleeneT, KleeneT], &[KleeneF]), KleeneF);
    }

    #[test]
    fn kleene_diamond_modality_asymmetric_per_spec_6_2() {
        let d = KleeneDomain;
        use Tristate::*;
        // KleeneT iff some must-successor is T
        assert_eq!(d.diamond_modality(&[KleeneT], &[KleeneT]), KleeneT);
        // KleeneF iff every may-successor is F
        assert_eq!(d.diamond_modality(&[KleeneF, KleeneF], &[]), KleeneF);
        // Mixed: a may-successor is ⊥, no must-T → ⊥
        assert_eq!(
            d.diamond_modality(&[KleeneF, KleeneBot], &[KleeneF]),
            KleeneBot
        );
        // Empty lists ⇒ vacuous F
        assert_eq!(d.diamond_modality(&[], &[]), KleeneF);
        // A must-T witness overrides a may-F majority.
        assert_eq!(d.diamond_modality(&[KleeneF, KleeneT], &[KleeneT]), KleeneT);
    }

    #[test]
    fn kleene_lift_round_trips() {
        let d = KleeneDomain;
        use Tristate::*;
        assert_eq!(d.lift_bool(true), KleeneT);
        assert_eq!(d.lift_bool(false), KleeneF);
        for v in [KleeneT, KleeneF, KleeneBot] {
            assert_eq!(d.lift_tristate(v), v);
        }
    }

    #[test]
    fn kleene_is_unknown_only_for_bot() {
        let d = KleeneDomain;
        use crate::clts::StateId;
        use Tristate::*;
        let id: StateId<u32> = StateId::from_index(0).expect("index 0 fits u32");
        assert!(d.is_unknown(&id, &KleeneBot));
        assert!(!d.is_unknown(&id, &KleeneT));
        assert!(!d.is_unknown(&id, &KleeneF));
    }

    // ---- R.4.5 — hyper-must modal-operator tests ----
    //
    // Per `docs/design/native-sv-abstraction.md` §6.11 / Shoham–
    // Grumberg LMCS 2007 §3, a `MustHyperOnly` edge `s →ᴴ T`
    // contributes:
    //   - to `[a]φ = F` iff EVERY t ∈ T has φ(t) = F (refinement can
    //     pick any t; if any one is non-F then a refinement exists
    //     where the must-edge does not witness false)
    //   - to `⟨a⟩φ = T` iff EVERY t ∈ T has φ(t) = T (refinement can
    //     pick any t; must-edge witnesses true only if every choice
    //     yields true)
    //
    // The per-edge reduction in the default trait impls is JOIN for
    // box (so the "all targets F" check becomes "per-edge value is F"
    // for the flat box_modality) and MEET for diamond (symmetric).

    #[test]
    fn bool_domain_hyper_modality_is_sharp_only_per_documentation() {
        // BoolDomain documents that it treats every transition as Sharp
        // (the may/must distinction collapses). When the trait default
        // `box_modality_hyper` delegates to BoolDomain's flat
        // `box_modality`, the `must` slice is IGNORED — so BoolDomain
        // cannot distinguish hyper-must false witnesses from a Sharp
        // must over a single target.
        //
        // This test pins that documented limitation rather than
        // pretending BoolDomain handles hyper-must: any hyper-must
        // input through BoolDomain's hyper-modality methods reduces
        // to its flat counterpart on the may slice alone.
        let d = BoolDomain;
        let must_edges: &[&[bool]] = &[&[false, false]];
        // may all true → flat box returns true; the hyper-must false
        // witness is dropped (BoolDomain's documented Sharp-only collapse).
        assert!(d.box_modality_hyper(&[true, true], must_edges));
        // diamond inherits the same collapse — may all false, must
        // slice ignored → returns false.
        let must_t: &[&[bool]] = &[&[true, true]];
        assert!(!d.diamond_modality_hyper(&[false], must_t));
        // Callers that need real hyper-must semantics on 2-valued
        // inputs must use KleeneDomain with `lift_bool` (which then
        // applies the §6.2 + §6.11 semantics correctly).
    }

    #[test]
    fn kleene_box_hyper_all_targets_false_yields_false() {
        let d = KleeneDomain;
        use Tristate::*;
        // hyper-must edge with both targets F → contributes false witness
        let must_edges: &[&[Tristate]] = &[&[KleeneF, KleeneF]];
        assert_eq!(
            d.box_modality_hyper(&[KleeneT, KleeneT], must_edges),
            KleeneF
        );
    }

    #[test]
    fn kleene_box_hyper_one_true_target_blocks_false() {
        let d = KleeneDomain;
        use Tristate::*;
        // hyper-must edge with one F and one T target → join is T → no false witness from this edge
        let must_edges: &[&[Tristate]] = &[&[KleeneF, KleeneT]];
        // may all T → box returns T (no false witness anywhere)
        assert_eq!(
            d.box_modality_hyper(&[KleeneT, KleeneT], must_edges),
            KleeneT
        );
    }

    #[test]
    fn kleene_box_hyper_one_bot_target_keeps_bot() {
        let d = KleeneDomain;
        use Tristate::*;
        // hyper-must with one F and one ⊥ → join is ⊥ (not a definite false witness)
        let must_edges: &[&[Tristate]] = &[&[KleeneF, KleeneBot]];
        // may all T → no definite false-witness, but ⊥ means we cannot conclude T either
        assert_eq!(
            d.box_modality_hyper(&[KleeneT, KleeneT], must_edges),
            KleeneBot
        );
    }

    #[test]
    fn kleene_diamond_hyper_all_targets_true_yields_true() {
        let d = KleeneDomain;
        use Tristate::*;
        // hyper-must with both targets T → must-witness is solid → diamond T
        let must_edges: &[&[Tristate]] = &[&[KleeneT, KleeneT]];
        assert_eq!(d.diamond_modality_hyper(&[KleeneF], must_edges), KleeneT);
    }

    #[test]
    fn kleene_diamond_hyper_one_false_target_blocks_true() {
        let d = KleeneDomain;
        use Tristate::*;
        // hyper-must with one T and one F → meet is F → no true witness from this edge
        // and may all F → diamond returns F (every may-successor false)
        let must_edges: &[&[Tristate]] = &[&[KleeneT, KleeneF]];
        assert_eq!(d.diamond_modality_hyper(&[KleeneF], must_edges), KleeneF);
    }

    #[test]
    fn hyper_modality_empty_target_skips_edge() {
        // Per the trait docs, an empty hyper-target set is skipped
        // (an unrealizable must-edge cannot contribute a witness).
        let d = KleeneDomain;
        use Tristate::*;
        let must_edges: &[&[Tristate]] = &[&[], &[KleeneT, KleeneT]];
        // First edge skipped; second edge witnesses true.
        assert_eq!(d.diamond_modality_hyper(&[KleeneF], must_edges), KleeneT);
    }

    #[test]
    fn hyper_modality_single_target_matches_flat() {
        // R.4.5 invariant: a hyper-must with one target must produce
        // the same result as the flat box_modality / diamond_modality
        // with that single target — Sharp is the singleton-target
        // degeneracy of MustHyperOnly.
        let d = KleeneDomain;
        use Tristate::*;
        let must_flat = vec![KleeneF];
        let must_hyper: &[&[Tristate]] = &[&[KleeneF]];
        assert_eq!(
            d.box_modality(&[KleeneT], &must_flat),
            d.box_modality_hyper(&[KleeneT], must_hyper),
        );
        let must_flat_t = vec![KleeneT];
        let must_hyper_t: &[&[Tristate]] = &[&[KleeneT]];
        assert_eq!(
            d.diamond_modality(&[KleeneF], &must_flat_t),
            d.diamond_modality_hyper(&[KleeneF], must_hyper_t),
        );
    }
}
