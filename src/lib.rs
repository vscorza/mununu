#![allow(clippy::pedantic, clippy::nursery)]

//! Mununu core library — formal verification for reactive systems
//! modeled as Compositional Labeled Transition Systems (CLTS).

pub mod abstraction;
pub mod clts;
pub mod composition;
pub mod context;
pub mod context_dsl;
pub mod examples;
pub mod guard;
pub mod iter;
pub mod ltl;
pub mod mu_calculus;
pub mod persistence;

#[cfg(feature = "api")]
pub mod api;

/// Temporary placeholder to ensure the crate builds.
pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes() {
        init();
    }
}
