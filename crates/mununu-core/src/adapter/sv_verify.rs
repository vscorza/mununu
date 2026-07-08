//! SV-direct property verbs — lift a SystemVerilog module to BTOR2 (sv2v + Yosys)
//! and decide a property in **one** call.
//!
//! An external tool (e.g. an agent writing RTL) can hand raw SV plus a target atom
//! and get a verdict, with no manual `sv emit-btor2-per-module` → `btor2 verify-*`
//! round-trip. Each function is a thin bridge: [`sv_to_btor2`] then the matching
//! BTOR2 verb core ([`decide_reach_portfolio_parallel`],
//! [`response_liveness_rescue_atoms`], [`verify_recoverability`]). The SV → BTOR2
//! lift requires sv2v + Yosys on the host (see [`docs/verifying-rtl.md`]); a missing
//! tool returns a structured error.
//!
//! Atoms are validated **before** the (toolchain-gated) lift, so a malformed
//! request fails fast and identically to the BTOR2-direct verbs.
//!
//! **Reset**: these verbs lift the design with the reset as a *free input* (no
//! reset-gating). Sound, and the honest default — recoverability in particular is
//! reset-dependent (see `docs/design/recoverability-vs-sva.md` §3.2). Reset-gating
//! (as `sv verify-auto` offers) is a future option.

use crate::adapter::btor2::parser;
use crate::adapter::btor2::predicate_expr::parse_predicate_expr;
use crate::adapter::liveness_rescue::{
    Atom, LivenessVerdict, parse_response_atom, response_liveness_rescue_atoms,
};
use crate::adapter::reach_portfolio::{ReachOutcome, decide_reach_portfolio_parallel};
use crate::adapter::recoverability::verify_recoverability;
use crate::adapter::yosys::{YosysOptions, sv_to_btor2};
use crate::verdict::PropertyVerdict;

/// The SV → BTOR2 lift inputs shared by every SV-direct verb.
pub struct SvLift {
    /// Primary SystemVerilog source.
    pub source: String,
    /// Additional SV sources (packages / includes) as `(name, content)`.
    pub additional_sources: Vec<(String, String)>,
    /// Top module (Yosys auto-detects when `None`).
    pub top: Option<String>,
    /// Run sv2v before Yosys (modern SV module-header imports, etc.).
    pub use_sv2v: bool,
}

impl SvLift {
    /// Lift to a single flattened BTOR2 string (sv2v optional + Yosys).
    fn lift(&self) -> Result<String, String> {
        let yopts = YosysOptions {
            top: self.top.clone(),
            additional_sources: self.additional_sources.clone(),
            use_sv2v: self.use_sv2v,
            ..Default::default()
        };
        sv_to_btor2(&self.source, &yopts)
            .map_err(|e| format!("SV → BTOR2 (sv2v + Yosys): {}", e.message))
    }
}

/// `sv verify` — lift SV and decide `bad`-reachability of its assertions with the
/// multi-engine safety portfolio.
pub fn sv_verify_safety(lift: &SvLift) -> Result<ReachOutcome, String> {
    let btor2 = lift.lift()?;
    let file =
        parser::parse(&btor2).map_err(|e| format!("parsing the lifted BTOR2: {}", e.message))?;
    Ok(decide_reach_portfolio_parallel(&file))
}

/// `sv verify-liveness` — lift SV and decide `AG(request → AF grant)` via the l2s
/// reduction + the portfolio. The atoms are parsed first (a malformed atom errors
/// before the lift).
pub fn sv_verify_liveness(
    lift: &SvLift,
    request: &str,
    grant: &str,
) -> Result<(LivenessVerdict, ReachOutcome), String> {
    let ante: Atom = parse_response_atom(request)?;
    let cons: Atom = parse_response_atom(grant)?;
    let btor2 = lift.lift()?;
    response_liveness_rescue_atoms(&btor2, &ante, &cons, false).ok_or_else(|| {
        "could not build the liveness monitor — an atom likely binds no signal in the design"
            .to_string()
    })
}

/// `sv verify-recoverability` — lift SV and decide `AG EF target`. The target atom is
/// validated before the lift.
pub fn sv_verify_recoverability(lift: &SvLift, target: &str) -> Result<PropertyVerdict, String> {
    parse_predicate_expr(target).map_err(|e| {
        format!("recoverability target `{target}` is not a register-comparison atom: {e:?}")
    })?;
    let btor2 = lift.lift()?;
    verify_recoverability(&btor2, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lift_of(source: &str) -> SvLift {
        SvLift {
            source: source.to_string(),
            additional_sources: Vec::new(),
            top: None,
            use_sv2v: false,
        }
    }

    // The atom guards fire BEFORE the toolchain-gated lift, so they are testable in
    // make-ci (no sv2v / Yosys needed).
    #[test]
    fn liveness_rejects_malformed_atom_before_lift() {
        let e = sv_verify_liveness(&lift_of("module m; endmodule"), "x == y", "st == 1");
        assert!(
            e.is_err(),
            "a relational request atom must be rejected pre-lift"
        );
    }

    #[test]
    fn recoverability_rejects_malformed_target_before_lift() {
        let e = sv_verify_recoverability(&lift_of("module m; endmodule"), "not an atom !!");
        assert!(e.is_err(), "a malformed target must be rejected pre-lift");
    }

    // The full lift → verdict path needs sv2v + Yosys; covered by the e2e suite in
    // the mununu-sva image, not make-ci.
}
