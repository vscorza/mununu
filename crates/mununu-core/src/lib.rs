#![allow(clippy::pedantic, clippy::nursery)]

//! Mununu core library — formal verification for reactive systems
//! modeled as Compositional Labeled Transition Systems (CLTS).

pub mod abstraction;
pub mod adapter;
pub mod clts;
pub mod codesign;
pub mod composition;
pub mod context;
pub mod context_dsl;
pub mod contract;
pub mod controllability;
pub mod corpus;
pub mod examples;
pub mod guard;
pub mod iter;
pub mod llvm_ir;
pub mod ltl;
pub mod mu_calculus;
pub mod mununu_annotations;
pub mod persistence;
pub mod verify;

#[cfg(any(test, feature = "test_support"))]
pub mod test_support;

#[cfg(any(test, feature = "test_support"))]
pub mod bench_support;

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
