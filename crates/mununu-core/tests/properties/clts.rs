//! Property tests for CLTS construction and persistence.
//!
//! - `roundtrip_persistence`: save → load → equal-by-observation.
//!   Catches regressions in `persistence/serialize.rs` driven by
//!   builder changes (e.g., field reordering, CSR migration).

use mununu_core::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, StateId};
use mununu_core::persistence::{load_clts_from_path, save_clts_to_path};
use mununu_core::test_support::RandomClts;

use proptest::prelude::*;
use tempfile::tempdir;

/// Compare two CLTS instances by their externally-observable properties:
/// state count, initial state set (by index), per-state outgoing transition
/// targets (sorted set), and total edge count. We don't compare label-store
/// internals because the in-memory representation can carry implementation-
/// dependent ordering that survives or doesn't survive a round-trip; what
/// matters is that the *graph* round-trips faithfully.
fn observable_eq(
    a: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    b: &Clts<DefaultStateIdx, DefaultLabelIdx>,
) -> bool {
    if a.state_count() != b.state_count() {
        return false;
    }
    let initials_a: std::collections::BTreeSet<u32> = a
        .initial_states()
        .iter()
        .map(|sid| sid.index() as u32)
        .collect();
    let initials_b: std::collections::BTreeSet<u32> = b
        .initial_states()
        .iter()
        .map(|sid| sid.index() as u32)
        .collect();
    if initials_a != initials_b {
        return false;
    }
    for s in 0..a.state_count() {
        let sa = StateId::from_index(s).unwrap();
        let sb = StateId::from_index(s).unwrap();
        let mut targets_a: Vec<u32> = a
            .outgoing(sa)
            .iter()
            .map(|t| t.target().index() as u32)
            .collect();
        let mut targets_b: Vec<u32> = b
            .outgoing(sb)
            .iter()
            .map(|t| t.target().index() as u32)
            .collect();
        targets_a.sort_unstable();
        targets_b.sort_unstable();
        if targets_a != targets_b {
            return false;
        }
    }
    true
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Relatively low default; raise via PROPTEST_CASES=4096 in nightly.
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64),
        ..ProptestConfig::default()
    })]

    /// Round-trip property: a CLTS produced by `RandomClts` must serialize
    /// and deserialize without observable change.
    #[test]
    fn roundtrip_persistence(
        seed in any::<u64>(),
        states in 4usize..=64,
        density_pct in 0u32..=50,
    ) {
        let density = density_pct as f64 / 100.0;
        let original = RandomClts::new(seed)
            .with_states(states)
            .with_density(density)
            .with_alphabet(3)
            .build();

        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("roundtrip.clts.bin");
        save_clts_to_path(&original, &path).expect("save");
        let restored = load_clts_from_path(&path).expect("load");

        prop_assert!(
            observable_eq(&original, &restored),
            "save/load did not preserve observable structure (seed={}, states={}, density_pct={})",
            seed, states, density_pct
        );
    }
}
