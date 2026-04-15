//! Mununu extraction frontend — AST-based model extraction from source code.
//!
//! Reads a `.extract.json` config and source file, parses the AST via
//! tree-sitter, derives automata from field domains and method behaviors,
//! and outputs a `.espec.json` extraction spec.

// WIP: extraction pipeline not fully wired yet — suppress warnings until Phase 2
#![allow(
    dead_code,
    unused_variables,
    clippy::collapsible_if,
    clippy::println_empty_string,
    clippy::ptr_arg
)]

pub mod extractor;
pub mod parser;

fn main() {
    // TODO: implement CLI in Phase C
    eprintln!("mununu-extract: extraction frontend");
    eprintln!(
        "Usage: mununu-extract config.extract.json --source path/to/source.ts --output spec.espec.json"
    );
    eprintln!("");
    eprintln!("Available domain profiles:");
    for name in mununu_core::adapter::extraction::ast_extract::domain::available_profiles() {
        eprintln!("  - {name}");
    }
    std::process::exit(1);
}
