//! Syntactic seed extraction for predicate-image discovery.
//!
//! Phase A.4 step 4.3 will populate this module with the AVR /
//! Goel–Sakallah syntax-guided seed pattern (sub-expressions, case
//! labels, `== const` RHS values). Step 4.1 ships the type shape
//! only.
//!
//! The seeds feed [`super::all_smt`] as the initial predicate set; the
//! all-SMT enumeration then walks the transition relation to discover
//! any additional reachable predicate witnesses.

use super::Predicate;

/// Seed source — informational only. Helps users see why a discovered
/// value showed up (case label vs. comparison RHS vs. SMT-derived).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeedSource {
    /// Constant in a `case <signal>: arm` label.
    CaseLabel,
    /// Right-hand side of a `signal == k` comparison in an
    /// `always_ff` / `always_comb` guard.
    EqualityComparison,
    /// `bad` / `constraint` / `justice` / `fair` operand constant.
    BadConstraint,
    /// Operand constant of a relational op (`<`, `<=`, `>`, `>=`,
    /// `!=`) — included so user-written formula atoms get matched.
    RelationalComparison,
}

/// Output of the syntactic-seed walker — a predicate + its source
/// rationale. Round-tripped into [`super::Predicate`] for the all-SMT
/// engine.
#[derive(Debug, Clone)]
pub struct SeedPredicate {
    pub predicate: Predicate,
    pub source: SeedSource,
}

/// Placeholder for the BTOR2 syntactic seed walker. Step 4.3 will
/// implement this: walk every `bad` / `constraint` / `justice` /
/// `fair` operand DAG collecting `eq` / `ult` / `slt` / `ulte` /
/// `slte` / `ugt` / `sgt` / `ugte` / `sgte` operands that bottom-out
/// at a `const`.
pub fn collect_btor2_seeds(_file: &crate::adapter::btor2::ast::Btor2File) -> Vec<SeedPredicate> {
    Vec::new()
}

/// Placeholder for the SV syntactic seed walker. Step 4.3 will
/// implement this: harvest case-label constants and `signal == k`
/// RHS literals from `always_ff` / `always_comb` blocks. The existing
/// [`crate::adapter::systemverilog::kripke::scan_significant_constants`]
/// is the closest current code; this module supersedes it once step
/// 4.3 lands.
pub fn collect_sv_seeds(
    _module: &crate::adapter::systemverilog::ast::Module,
) -> Vec<SeedPredicate> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_source_variants_are_distinct() {
        let xs = [
            SeedSource::CaseLabel,
            SeedSource::EqualityComparison,
            SeedSource::BadConstraint,
            SeedSource::RelationalComparison,
        ];
        let unique: std::collections::HashSet<_> = xs.iter().collect();
        assert_eq!(unique.len(), xs.len());
    }
}
