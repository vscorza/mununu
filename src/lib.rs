#![allow(clippy::pedantic, clippy::nursery)]

//! Mununu — thin re-export wrapper.
//!
//! The actual code lives in `mununu-core`. This crate re-exports everything
//! so that `main.rs`, tests, and benchmarks can use `mununu::` paths
//! until they are migrated to reference `mununu_core::` directly.

pub use mununu_core::*;
