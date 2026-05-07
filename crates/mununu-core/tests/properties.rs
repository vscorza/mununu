//! Aggregator for the property-based test suite.
//!
//! Property tests live in `tests/properties/<subsystem>.rs`. This file
//! pulls them all in via a single `mod properties;` declaration so
//! `cargo test --test properties` runs the whole suite.
//!
//! Default `PROPTEST_CASES=64` (sub-second per property, pre-commit
//! friendly). Override with the env var for deeper runs:
//!
//!   PROPTEST_CASES=4096 cargo test --test properties --features test_support
//!
//! All proptests are gated by `feature = "test_support"` because they
//! depend on the deterministic CLTS generators in
//! `mununu_core::test_support`.

#![cfg(feature = "test_support")]

#[path = "properties/clts.rs"]
mod clts;
#[path = "properties/composition.rs"]
mod composition;
#[path = "properties/minimization.rs"]
mod minimization;
#[path = "properties/mu_calculus.rs"]
mod mu_calculus;
