//! AST-based extraction — derives automata from source code + config.
//!
//! This module sits BEFORE the existing extraction adapter in the pipeline:
//!
//! ```text
//! Source + Config (.extract.json)
//!     → ast_extract (this module)       [AST parsing, state derivation]
//!     → .espec.json                     [existing format, now machine-generated]
//!     → ExtractionAdapter               [existing: .espec.json → AdapterIR → CTXDSL]
//! ```
//!
//! The agent writes the config (which classes, which fields, what abstraction).
//! This tool reads the AST and derives the automaton topology (states, transitions).

pub mod call_summary;
pub mod config;
pub mod domain;
pub mod state_space;

// AST parsing (tree-sitter) is behind a feature flag.
// Phase B will add: pub mod parser; pub mod extractor;
