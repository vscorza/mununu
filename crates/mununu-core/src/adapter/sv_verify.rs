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

use crate::adapter::btor2::kmts_lift::PredicateSpec;
use crate::adapter::btor2::parser;
use crate::adapter::btor2::predicate_expr::parse_predicate_expr;
use crate::adapter::liveness_rescue::{
    Atom, LivenessVerdict, parse_response_atom, parse_response_pairs,
    response_liveness_rescue_atoms, response_liveness_rescue_conjunction,
};
use crate::adapter::reach_portfolio::{ReachOutcome, decide_reach_portfolio_parallel};
use crate::adapter::recoverability::verify_recoverability_with_predicates;
use crate::adapter::yosys::{SvFrontend, YosysOptions, sv_to_btor2};
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
    /// Extra on-disk include-search directories (`-I<dir>`), so
    /// `` `include "frag.vh" `` resolves against the original source tree
    /// without the fragment being read as a standalone compilation unit.
    /// Feeds [`YosysOptions::extra_include_dirs`]; empty by default.
    pub include_dirs: Vec<std::path::PathBuf>,
    /// RTL front-end selection. `Slang` forces the yosys-slang plugin, which
    /// lifts modern-SV constructs `read_verilog`/sv2v reject. Feeds
    /// [`YosysOptions::frontend`]; `Auto` by default.
    pub frontend: SvFrontend,
}

impl SvLift {
    /// Lift to a single flattened BTOR2 string (sv2v optional + Yosys).
    fn lift(&self) -> Result<String, String> {
        let yopts = YosysOptions {
            top: self.top.clone(),
            additional_sources: self.additional_sources.clone(),
            use_sv2v: self.use_sv2v,
            extra_include_dirs: self.include_dirs.clone(),
            frontend: self.frontend,
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

/// `sv verify-liveness-all` — lift SV and decide the **conjunction** of
/// response-liveness properties `⋀ᵢ AG(aᵢ → AF bᵢ)` via the l2s reduction + the
/// portfolio (one lift, one `bad`-reachability query per response). Each `responses`
/// entry is a `"ANTE => CONS"` pair; every pair is parsed first, so a malformed
/// response errors before the (toolchain-gated) lift — matching [`sv_verify_liveness`].
///
/// Returns the combined verdict + the per-response [`ReachOutcome`] (same order as
/// `responses`), via [`response_liveness_rescue_conjunction`].
pub fn sv_verify_liveness_all(
    lift: &SvLift,
    responses: &[String],
) -> Result<(LivenessVerdict, Vec<ReachOutcome>), String> {
    let pairs = parse_response_pairs(responses)?;
    let btor2 = lift.lift()?;
    response_liveness_rescue_conjunction(&btor2, &pairs, false).ok_or_else(|| {
        "could not build a liveness monitor — an atom likely binds no signal in the design, \
         or no responses were given"
            .to_string()
    })
}

/// `sv verify-recoverability` — lift SV and decide `AG EF target`. The target atom is
/// validated before the lift.
pub fn sv_verify_recoverability(lift: &SvLift, target: &str) -> Result<PropertyVerdict, String> {
    sv_verify_recoverability_with_predicates(lift, target, &[])
}

/// [`sv_verify_recoverability`] with optional extra abstraction predicates for the
/// cube-path escalation (P2 Slice 1). The target atom is validated before the
/// (toolchain-gated) lift; the extras only refine the cube if the exact engine abstains.
pub fn sv_verify_recoverability_with_predicates(
    lift: &SvLift,
    target: &str,
    extra_predicates: &[PredicateSpec],
) -> Result<PropertyVerdict, String> {
    parse_predicate_expr(target).map_err(|e| {
        format!("recoverability target `{target}` is not a register-comparison atom: {e:?}")
    })?;
    let btor2 = lift.lift()?;
    verify_recoverability_with_predicates(&btor2, target, extra_predicates)
}

/// `sv verify-recoverability --refine` — lift SV and decide `AG EF target`, returning the canonical
/// verdict PLUS a structured [`VerdictRefinement`] (the `Vacuous`/`bot_diagnosis` today; the config
/// partition and discovered assumptions in later phases). The refinement is diagnostic-only — it never
/// changes the canonical verdict. Sidecar-free (operates on the same lift as the plain verb).
pub fn sv_verify_recoverability_refined(
    lift: &SvLift,
    target: &str,
    extra_predicates: &[PredicateSpec],
) -> Result<(PropertyVerdict, crate::verdict::VerdictRefinement), String> {
    parse_predicate_expr(target).map_err(|e| {
        format!("recoverability target `{target}` is not a register-comparison atom: {e:?}")
    })?;
    let btor2 = lift.lift()?;
    Ok(
        crate::adapter::recoverability::verify_recoverability_refined(
            &btor2,
            target,
            extra_predicates,
        ),
    )
}

/// `sv check-fsm` — lift SV and auto-scan every FSM-like state register for a reachable
/// illegal encoding (no user input). The SV-direct peer of `btor2 check-fsm`.
pub fn sv_check_fsm(
    lift: &SvLift,
    max_width: u32,
) -> Result<Vec<crate::adapter::fsm_scan::FsmFinding>, String> {
    let btor2 = lift.lift()?;
    crate::adapter::fsm_scan::fsm_encoding_scan(&btor2, max_width)
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
            include_dirs: Vec::new(),
            frontend: SvFrontend::Auto,
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

    // P2 industrial anchor — recoverability on REAL OpenTitan RTL. From every reachable
    // state the AES cipher-control FSM can return to CIPHER_CTRL_IDLE (= 6'b001001 = 9):
    // `AG EF (aes_cipher_ctrl_cs == 9)`, the branching νμ property SVA cannot express,
    // decides HOLDS end-to-end through the SV lift + the recoverability verb. NOTE: the
    // vendored design is the EXTRACTED control FSM (18 state bits — within the exact
    // engine's ~40-bit cap, so the exact engine decides it); the OVER-CAP cube +
    // smt-hyper-must scale path is proven separately by the wide-fixture differential
    // tests in `crate::adapter::recoverability` (an 80-bit design where the exact engine
    // abstains and the cube path decides both polarities).
    #[test]
    #[ignore = "requires sv2v + Yosys + z3 (mununu-sva docker image); run with --ignored"]
    fn e2e_opentitan_aes_cipher_ctrl_recoverability_holds() {
        use std::path::PathBuf;
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/verify/dc_opentitan_aes_cipher_control_fsm/source");
        let read = |n: &str| {
            std::fs::read_to_string(dir.join(n)).unwrap_or_else(|e| panic!("read {n}: {e}"))
        };
        let lift = SvLift {
            source: read("aes_cipher_control_fsm.sv"),
            additional_sources: vec![
                ("aes_pkg.sv".into(), read("aes_pkg.sv")),
                ("aes_reg_pkg.sv".into(), read("aes_reg_pkg.sv")),
                ("prim_util_pkg.sv".into(), read("prim_util_pkg.sv")),
                ("prim_assert.sv".into(), read("prim_assert.sv")),
            ],
            top: Some("aes_cipher_control_fsm".into()),
            use_sv2v: true,
            include_dirs: Vec::new(),
            frontend: SvFrontend::Auto,
        };
        let verdict = sv_verify_recoverability(&lift, "aes_cipher_ctrl_cs == 9")
            .expect("recoverability decides on the AES cipher-control FSM");
        assert_eq!(
            verdict,
            PropertyVerdict::Holds,
            "AG EF idle must HOLD on the AES cipher-control FSM (every reachable state can \
             return to CIPHER_CTRL_IDLE); got {verdict:?}"
        );
    }

    // The full lift → verdict path needs sv2v + Yosys; covered by the e2e suite in
    // the mununu-sva image, not make-ci.
}
