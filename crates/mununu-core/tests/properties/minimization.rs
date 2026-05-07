//! Property tests for `minimize_bisimulation` and adjacent invariants.
//!
//! - `idempotence`: `minimize(minimize(c)) == minimize(c)` (state count
//!   stable after a second pass; the second pass returns `None`).
//! - `monotone`: `|states_after| ≤ |states_before|`.
//!
//! Followups (not landed this sitting):
//! - `diff_naive_vs_production` against an in-test naive bisim oracle
//!   (E5 in the plan; required ahead of EXP-0009 Paige-Tarjan).
//! - `bisimilarity_preserved`: trace-equivalent up to length k.

use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu_core::composition::minimize::minimize_bisimulation;
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

    /// Minimization is idempotent: minimizing a minimized CLTS is a no-op.
    /// Concretely: the second call returns `None` (no further reduction).
    /// This property is critical to verify before swapping K-S for
    /// Paige-Tarjan in EXP-0009.
    #[test]
    fn idempotence(
        seed in any::<u64>(),
        states in 4usize..=24,
        density_pct in 5u32..=40,
    ) {
        let density = density_pct as f64 / 100.0;
        let original = RandomClts::new(seed)
            .with_states(states)
            .with_density(density)
            .with_alphabet(3)
            .build();

        let first = minimize_bisimulation(&original, None).expect("first minimize");
        let Some((minimized, _)) = first else {
            // Already minimal — second-pass invariant trivially holds.
            return Ok(());
        };

        let second = minimize_bisimulation(&minimized, None).expect("second minimize");
        prop_assert!(
            second.is_none(),
            "minimize(minimize(c)) should return None (already minimal); got Some with {} states from input {} states",
            second.as_ref().map(|(c, _): &(Clts<DefaultStateIdx, DefaultLabelIdx>, _)| c.state_count()).unwrap_or(0),
            minimized.state_count()
        );
    }

    /// Minimization is monotone in state count: `|after| ≤ |before|`. A
    /// regression that introduces or duplicates states would surface
    /// immediately.
    #[test]
    fn state_count_monotone(
        seed in any::<u64>(),
        states in 4usize..=24,
        density_pct in 5u32..=40,
    ) {
        let density = density_pct as f64 / 100.0;
        let original = RandomClts::new(seed)
            .with_states(states)
            .with_density(density)
            .with_alphabet(3)
            .build();

        let result = minimize_bisimulation(&original, None).expect("minimize");
        if let Some((minimized, _)) = result {
            prop_assert!(
                minimized.state_count() <= original.state_count(),
                "minimization grew state count: {} → {}",
                original.state_count(),
                minimized.state_count()
            );
        }
    }
}
