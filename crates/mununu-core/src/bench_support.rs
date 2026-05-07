//! Shared bench harness — fixture cache and provenance recorder.
//!
//! Re-exported under `feature = "test_support"` so the four `*_only.rs`
//! benches can `use mununu_core::bench_support as common;`. (Cargo
//! auto-detects every `benches/*.rs` as its own bench target — keeping
//! shared helpers as a library module avoids that.)
//!
//! Per the iteration policy in `notebook/REFINEMENT.md`, additive
//! changes here (new fixture, new cache strategy) do NOT bump the
//! manifest schema version. Breaking changes (renaming a fixture,
//! changing a deterministic seed) DO require superseding the affected
//! EXP archives.
//!
//! The fixture cache lives under `target/test-fixtures/` (gitignored via
//! `target/`). Each fixture is keyed by name and rebuilt on first access;
//! subsequent runs deserialize from disk via `mununu_core::persistence`,
//! so composition / minimization / mu-calculus benches don't pay the
//! construction cost.
//!
//! Determinism contract: every fixture function takes a `u64` seed (or a
//! single canonical seed for parameter-free templates) and uses
//! `mununu_core::test_support::*` generators which are pinned to
//! `rand_chacha::ChaCha20Rng`. Same seed + same code → byte-identical
//! `Clts` across machines.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use crate::persistence::{load_clts_from_path, save_clts_to_path};
use crate::test_support::{self, RandomClts};

pub type DefaultClts = Clts<DefaultStateIdx, DefaultLabelIdx>;

/// Where cached fixtures live. Relative to the workspace root because
/// Criterion runs benches with cwd = workspace root by default.
pub fn fixtures_dir() -> PathBuf {
    let cargo_target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    PathBuf::from(cargo_target).join("test-fixtures")
}

/// Load `name` from the fixture cache, or build it via `builder` and
/// persist for next time. The on-disk format is mununu's own binary
/// snapshot — versioned independently of this file.
///
/// **NOT thread-safe.** Criterion runs sequentially within a process,
/// which is sufficient. If parallel bench runs are introduced, add a
/// per-fixture file lock here.
pub fn load_or_build<F>(name: &str, builder: F) -> DefaultClts
where
    F: FnOnce() -> DefaultClts,
{
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("create fixtures dir {}: {e}", dir.display()));
    let path = dir.join(format!("{name}.clts.bin"));
    if path.exists() {
        match load_clts_from_path(&path) {
            Ok(clts) => return clts,
            Err(e) => {
                eprintln!(
                    "==> fixture {} unreadable ({e}); rebuilding",
                    path.display()
                );
            }
        }
    }
    let clts = builder();
    if let Err(e) = save_clts_to_path(&clts, &path) {
        eprintln!(
            "==> warn: could not persist fixture {}: {e}",
            path.display()
        );
    }
    clts
}

/// Force a fresh build, ignoring any cached copy. Useful for
/// builder-only benches where the construction itself is the
/// measurement target.
pub fn rebuild<F>(name: &str, builder: F) -> DefaultClts
where
    F: FnOnce() -> DefaultClts,
{
    let dir = fixtures_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.clts.bin"));
    let _ = std::fs::remove_file(&path);
    let clts = builder();
    if let Err(e) = save_clts_to_path(&clts, &path) {
        eprintln!(
            "==> warn: could not persist fixture {}: {e}",
            path.display()
        );
    }
    clts
}

/// Canonical fixture set. Each entry is named so EXP archives can refer
/// to them by string. Add more here as benches need them; do not change
/// existing entries without a superseding EXP (per REFINEMENT.md).
pub mod fixtures {
    use super::*;

    pub fn chain_1k() -> DefaultClts {
        load_or_build("chain_1k", || test_support::chain(1_000, 4))
    }

    pub fn chain_10k() -> DefaultClts {
        load_or_build("chain_10k", || test_support::chain(10_000, 4))
    }

    pub fn ring_1k() -> DefaultClts {
        load_or_build("ring_1k", || test_support::ring(1_000, 4))
    }

    pub fn grid_32x32() -> DefaultClts {
        load_or_build("grid_32x32", || test_support::grid(32, 32))
    }

    pub fn grid_64x64() -> DefaultClts {
        load_or_build("grid_64x64", || test_support::grid(64, 64))
    }

    pub fn random_512_d20() -> DefaultClts {
        load_or_build("random_512_d20_seed0xC0FFEE", || {
            RandomClts::new(0xC0FFEE)
                .with_states(512)
                .with_density(0.20)
                .with_alphabet(4)
                .build()
        })
    }
}

/// Brief sanity print, called from each bench's `main()` so the bench
/// log shows what fixtures are in play. Cheap; runs once per process.
pub fn announce(label: &str) {
    eprintln!("==> bench harness: {label}");
    eprintln!("==> fixtures dir: {}", fixtures_dir().display());
}

/// Helper kept for the iteration policy: dropping `_unused` signals to
/// the compiler that these helpers are intentionally library-shaped
/// (the `*_only.rs` benches each pull a subset).
#[doc(hidden)]
pub fn _unused() -> &'static Path {
    Path::new(".")
}
