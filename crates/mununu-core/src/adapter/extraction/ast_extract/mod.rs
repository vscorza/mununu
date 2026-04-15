//! AST-based extraction — shared types for extraction configuration,
//! domain profiles, call summaries, and state space derivation.
//!
//! These types are shared between `mununu-core` and `mununu-extract`.
//! The tree-sitter dependent code (parser, extractor) lives in
//! `mununu-extract` to keep tree-sitter out of the core binary.

pub mod call_summary;
pub mod config;
pub mod domain;
pub mod state_space;
