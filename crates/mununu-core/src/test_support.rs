//! Deterministic CLTS generators for tests, proptests, stress tests, and
//! the bench fixture cache.
//!
//! Gated by `feature = "test_support"` to keep release builds free of test
//! machinery. Re-exported under the same gate from `lib.rs`.
//!
//! The contract:
//! - Every generator takes a `u64` seed and is reproducible across runs and
//!   platforms via `rand_chacha::ChaCha20Rng`. Pinning the RNG matters for
//!   the reproducibility contract — `rand::thread_rng()` is forbidden here.
//! - Every generator is total: it builds successfully or panics with a
//!   diagnostic.
//! - State indexing follows the convention `s{N}` (e.g., `s0`, `s1`, ...).
//!   Initial state is `s0` unless documented otherwise.
//! - Templates that take `density` accept values in `[0.0, 1.0]`; out-of-range
//!   values clamp.
//!
//! Refinement workflow: see `notebook/REFINEMENT.md`. New templates are
//! additive (no scaffold version bump). **Changing the deterministic seed
//! semantics of an existing generator is a breaking change** — bump a
//! fixture-format version and supersede affected EXP archives.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

/// Default labels used by the templates. Kept short so label-set hashing
/// dominates over string content cost.
const DEFAULT_LABELS: &[&str] = &["a", "b", "c", "d", "e", "f", "g", "h"];

/// Convenience alias — the templates always materialize `Clts` with the
/// default state and label index types.
pub type DefaultClts = Clts<DefaultStateIdx, DefaultLabelIdx>;

/// Build a chain CLTS of `n` states: `s0 → s1 → ... → s(n-1)`.
///
/// One label per edge, drawn round-robin from the first `alphabet` entries
/// of `DEFAULT_LABELS`. `s0` is initial.
///
/// # Panics
/// If `n == 0` or `alphabet == 0`.
pub fn chain(n: usize, alphabet: usize) -> DefaultClts {
    assert!(n > 0, "chain length must be > 0");
    assert!(alphabet > 0, "alphabet size must be > 0");
    let alphabet = alphabet.min(DEFAULT_LABELS.len());

    let mut builder = Clts::builder();
    builder.reserve_states(n).reserve_transitions(n);

    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = builder
            .state_with_name(format!("s{i}"))
            .expect("state index in range");
        if i == 0 {
            builder.initial_state_id(id);
        }
        ids.push(id);
    }

    for i in 0..n - 1 {
        let label = builder
            .labels()
            .intern(std::iter::once(DEFAULT_LABELS[i % alphabet]))
            .expect("label intern succeeds");
        builder.transition_ids(ids[i], &[label], ids[i + 1]);
    }

    builder.build().expect("chain builds")
}

/// Build a ring CLTS of `n` states: a chain with an additional edge from
/// `s(n-1)` back to `s0`. `s0` is initial.
///
/// # Panics
/// If `n == 0` or `alphabet == 0`.
pub fn ring(n: usize, alphabet: usize) -> DefaultClts {
    assert!(n > 0, "ring length must be > 0");
    assert!(alphabet > 0, "alphabet size must be > 0");
    let alphabet = alphabet.min(DEFAULT_LABELS.len());

    let mut builder = Clts::builder();
    builder.reserve_states(n).reserve_transitions(n);

    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = builder
            .state_with_name(format!("s{i}"))
            .expect("state index in range");
        if i == 0 {
            builder.initial_state_id(id);
        }
        ids.push(id);
    }

    for i in 0..n {
        let label = builder
            .labels()
            .intern(std::iter::once(DEFAULT_LABELS[i % alphabet]))
            .expect("label intern succeeds");
        let next = (i + 1) % n;
        builder.transition_ids(ids[i], &[label], ids[next]);
    }

    builder.build().expect("ring builds")
}

/// Build a grid CLTS of `width × height` states: each non-boundary cell has
/// edges to its right neighbor (label `a`) and bottom neighbor (label `b`).
/// Top-left cell `(0,0)` is the initial state.
///
/// # Panics
/// If `width == 0` or `height == 0`.
pub fn grid(width: usize, height: usize) -> DefaultClts {
    assert!(width > 0 && height > 0, "grid dimensions must be > 0");

    let mut builder = Clts::builder();
    builder
        .reserve_states(width * height)
        .reserve_transitions(2 * width * height);

    let coord = |x: usize, y: usize| -> usize { y * width + x };
    let mut ids = vec![None; width * height];
    for y in 0..height {
        for x in 0..width {
            let id = builder
                .state_with_name(format!("g{x}_{y}"))
                .expect("state index in range");
            if x == 0 && y == 0 {
                builder.initial_state_id(id);
            }
            ids[coord(x, y)] = Some(id);
        }
    }
    let ids: Vec<_> = ids.into_iter().map(|o| o.expect("filled")).collect();

    let label_right = builder
        .labels()
        .intern(std::iter::once("a"))
        .expect("label intern succeeds");
    let label_down = builder
        .labels()
        .intern(std::iter::once("b"))
        .expect("label intern succeeds");

    for y in 0..height {
        for x in 0..width {
            let from = ids[coord(x, y)];
            if x + 1 < width {
                builder.transition_ids(from, &[label_right], ids[coord(x + 1, y)]);
            }
            if y + 1 < height {
                builder.transition_ids(from, &[label_down], ids[coord(x, y + 1)]);
            }
        }
    }

    builder.build().expect("grid builds")
}

/// Parameters for the random CLTS generator. Defaults are tuned to produce
/// a small, varied test fixture. Override per call site.
///
/// `density` is the probability per (source, target) ordered pair that an
/// edge exists. `density = 0.0` yields a CLTS with no edges; `density = 1.0`
/// yields a complete digraph (with self-loops).
#[derive(Debug, Clone)]
pub struct RandomClts {
    pub seed: u64,
    pub states: usize,
    pub density: f64,
    pub alphabet: usize,
    /// Maximum labels per edge (drawn from the alphabet uniformly without replacement).
    pub max_labels_per_edge: usize,
    /// Number of leading labels in the alphabet to mark as uncontrollable.
    /// Defaults to 0 (all labels controllable). Set to ≥ 1 when the
    /// generated CLTS will be composed with another that shares the
    /// alphabet — mununu's composition checker rejects shared
    /// CONTROLLABLE actions but allows shared UNCONTROLLABLE ones.
    pub uncontrollable_prefix: usize,
}

impl Default for RandomClts {
    fn default() -> Self {
        Self {
            seed: 0,
            states: 16,
            density: 0.2,
            alphabet: 4,
            max_labels_per_edge: 1,
            uncontrollable_prefix: 0,
        }
    }
}

impl RandomClts {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }

    pub fn with_states(mut self, states: usize) -> Self {
        self.states = states;
        self
    }

    pub fn with_density(mut self, density: f64) -> Self {
        self.density = density.clamp(0.0, 1.0);
        self
    }

    pub fn with_alphabet(mut self, alphabet: usize) -> Self {
        self.alphabet = alphabet.max(1).min(DEFAULT_LABELS.len());
        self
    }

    pub fn with_max_labels_per_edge(mut self, max: usize) -> Self {
        self.max_labels_per_edge = max.max(1).min(DEFAULT_LABELS.len());
        self
    }

    /// Mark the first `n` alphabet entries as uncontrollable. Useful when
    /// the resulting CLTS will be composed with another that shares the
    /// alphabet. Saturates at the alphabet size.
    pub fn with_uncontrollable_prefix(mut self, n: usize) -> Self {
        self.uncontrollable_prefix = n;
        self
    }

    /// Materialize the random CLTS. Reproducible across runs and platforms
    /// by virtue of `ChaCha20Rng` and the integer seed.
    pub fn build(self) -> DefaultClts {
        assert!(self.states > 0, "RandomClts.states must be > 0");

        let mut rng = ChaCha20Rng::seed_from_u64(self.seed);
        let alphabet = self.alphabet.min(DEFAULT_LABELS.len()).max(1);
        let max_labels = self.max_labels_per_edge.clamp(1, alphabet);

        let mut builder = Clts::builder();
        let expected_edges = (self.states.pow(2)) as f64 * self.density;
        builder
            .reserve_states(self.states)
            .reserve_transitions(expected_edges as usize + self.states);

        let mut ids = Vec::with_capacity(self.states);
        for i in 0..self.states {
            let id = builder
                .state_with_name(format!("r{i}"))
                .expect("state index in range");
            if i == 0 {
                builder.initial_state_id(id);
            }
            ids.push(id);
        }

        // Pre-intern every singleton-label so we don't re-hash for every edge.
        // Multi-label edges intern their joint set on demand below.
        let single_label_ids: Vec<_> = (0..alphabet)
            .map(|i| {
                builder
                    .labels()
                    .intern(std::iter::once(DEFAULT_LABELS[i]))
                    .expect("label intern succeeds")
            })
            .collect();

        // Mark the leading `uncontrollable_prefix` labels uncontrollable.
        let uncontrollable_n = self.uncontrollable_prefix.min(alphabet);
        for &lid in single_label_ids.iter().take(uncontrollable_n) {
            builder.set_label_controllability(lid, LabelControllability::Uncontrollable);
        }

        for from_idx in 0..self.states {
            for to_idx in 0..self.states {
                if !rng.gen_bool(self.density) {
                    continue;
                }
                let n_labels = if max_labels == 1 {
                    1
                } else {
                    rng.gen_range(1..=max_labels)
                };
                if n_labels == 1 {
                    let label = single_label_ids[rng.gen_range(0..alphabet)];
                    builder.transition_ids(ids[from_idx], &[label], ids[to_idx]);
                } else {
                    let mut chosen: Vec<&str> = Vec::with_capacity(n_labels);
                    let mut indices: Vec<usize> = (0..alphabet).collect();
                    for _ in 0..n_labels {
                        let pick = rng.gen_range(0..indices.len());
                        let idx = indices.swap_remove(pick);
                        chosen.push(DEFAULT_LABELS[idx]);
                    }
                    let label = builder
                        .labels()
                        .intern(chosen.iter().copied())
                        .expect("label intern succeeds");
                    builder.transition_ids(ids[from_idx], &[label], ids[to_idx]);
                }
            }
        }

        builder.build().expect("random CLTS builds")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_has_n_states_and_n_minus_one_edges() {
        let c = chain(8, 2);
        assert_eq!(c.state_count(), 8);
        let edge_count: usize = (0..c.state_count())
            .map(|s| {
                c.outgoing(crate::clts::StateId::from_index(s).unwrap())
                    .len()
            })
            .sum();
        assert_eq!(edge_count, 7);
    }

    #[test]
    fn ring_has_n_states_and_n_edges() {
        let c = ring(5, 1);
        assert_eq!(c.state_count(), 5);
        let edge_count: usize = (0..c.state_count())
            .map(|s| {
                c.outgoing(crate::clts::StateId::from_index(s).unwrap())
                    .len()
            })
            .sum();
        assert_eq!(edge_count, 5);
    }

    #[test]
    fn grid_has_width_times_height_states() {
        let c = grid(3, 4);
        assert_eq!(c.state_count(), 12);
    }

    #[test]
    fn random_is_deterministic_across_runs() {
        let a = RandomClts::new(42)
            .with_states(16)
            .with_density(0.3)
            .build();
        let b = RandomClts::new(42)
            .with_states(16)
            .with_density(0.3)
            .build();
        assert_eq!(a.state_count(), b.state_count());
        for s in 0..a.state_count() {
            let s_id = crate::clts::StateId::from_index(s).unwrap();
            assert_eq!(a.outgoing(s_id).len(), b.outgoing(s_id).len());
        }
    }

    #[test]
    fn random_seed_changes_output() {
        let a = RandomClts::new(1).with_states(16).with_density(0.3).build();
        let b = RandomClts::new(2).with_states(16).with_density(0.3).build();
        let total_a: usize = (0..a.state_count())
            .map(|s| {
                a.outgoing(crate::clts::StateId::from_index(s).unwrap())
                    .len()
            })
            .sum();
        let total_b: usize = (0..b.state_count())
            .map(|s| {
                b.outgoing(crate::clts::StateId::from_index(s).unwrap())
                    .len()
            })
            .sum();
        // It's possible (but extremely unlikely with seed=1 vs seed=2) for the
        // counts to match by coincidence. ChaCha20 makes that effectively zero.
        assert_ne!(total_a, total_b, "different seeds should differ");
    }
}
