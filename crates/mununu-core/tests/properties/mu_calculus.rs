//! Property tests for the mu-calculus evaluator.
//!
//! - `iteration_ranks_deterministic`: solving the same fixpoint twice on
//!   identical inputs produces byte-identical iteration-rank signatures.
//!   Pins down the EXP-0002 SoA migration: any future change to
//!   `IterationRanks` that introduces nondeterminism (e.g., a parallel
//!   reduction without ordering, or a HashMap upstream that leaks
//!   iteration-order) will surface here.
//!
//! Followups (not landed this sitting):
//! - LTL→μ semantic equivalence on small Kripke.
//! - Duality `¬μX.f(X) ≡ νX.¬f(¬X)` via NNF normalization.

use bitvec::prelude::*;
use mununu_core::mu_calculus::{Environment, EvaluationOptions, evaluate_with_witnesses, parser};
use mununu_core::test_support::RandomClts;

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32),
        ..ProptestConfig::default()
    })]

    /// Two runs of the same fixpoint solve must produce byte-identical
    /// signatures for every state.
    ///
    /// Method: solve a depth-2 alternating fixpoint
    /// `nu X. ((mu Y. (target or <> Y)) and [] X)` (recurrence)
    /// twice on the same random CLTS, then compare
    /// `WitnessMap::signature()` for every state against the formula's
    /// fixpoint nesting order.
    #[test]
    fn iteration_ranks_deterministic(
        seed in any::<u64>(),
        states in 4usize..=24,
        density_pct in 5u32..=40,
    ) {
        let density = density_pct as f64 / 100.0;
        let clts = RandomClts::new(seed)
            .with_states(states)
            .with_density(density)
            .with_alphabet(3)
            .build();

        let formula = parser::parse("nu X. ((mu Y. (target or <> Y)) and [] X)")
            .expect("formula parses");

        // Build a deterministic predicate: target = states whose index ≡ 0 (mod 3).
        let mut target = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        for i in 0..clts.state_count() {
            if i % 3 == 0 {
                target.set(i, true);
            }
        }
        let env = Environment::new(clts.state_count()).with_predicate("target", target);

        let opts = EvaluationOptions::default();
        let (set_a, wm_a) = evaluate_with_witnesses(&formula, &clts, &env, &opts).expect("eval a");
        let (set_b, wm_b) = evaluate_with_witnesses(&formula, &clts, &env, &opts).expect("eval b");

        // Result bitsets must match — a sanity prerequisite.
        prop_assert_eq!(&set_a, &set_b, "fixpoint result not deterministic");

        let nesting = formula.fixpoint_nesting_order();
        for s in 0..clts.state_count() {
            let sig_a = wm_a.signature(s, &nesting);
            let sig_b = wm_b.signature(s, &nesting);
            prop_assert_eq!(
                &sig_a,
                &sig_b,
                "state {} signature differs between runs",
                s
            );
        }
    }
}
