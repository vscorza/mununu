//! P2 — recoverability `AG EF good` ("from every reachable state, can the design
//! still get back to a good state?"), the branching property SVA cannot express.
//!
//! # The property
//!
//! Recoverability is the CTL formula `AG EF good` — from every reachable state
//! (`AG`) there **exists** a path back to `good` (`EF`). In the modal-μ calculus it
//! is an alternating fixpoint (a greatest fixpoint wrapping a least fixpoint):
//!
//! ```text
//!   nu Y. ((mu X. (good || <> X)) && [] Y)
//! ```
//!
//! The `<>` (some-successor) inside the `[]` (all-successors) is the branching
//! content — it quantifies existentially over futures *inside* a universal envelope,
//! which is exactly what a linear formalism (LTL / SVA) cannot state. See
//! [`docs/design/recoverability-vs-sva.md`](../../../docs/design/recoverability-vs-sva.md).
//!
//! # How it is decided
//!
//! This module offers the ergonomic entry point: name the `good` atom, and it builds
//! the `AG EF good` formula and decides it with the **exact 3-valued symbolic
//! engine** ([`crate::adapter::btor2::symbolic_bitblast::exact_symbolic_verdict`]) —
//! sound at every alternation depth (Bruns–Godefroid), definite within the engine's
//! 40-bit cone cap. Over the cap it abstains (`Unknown`).
//!
//! For designs wider than the exact cap, the **predicate-cube + `smt-hyper-must`**
//! path (`mununu btor2 cegar --formula … --must-edge-inference smt-hyper-must`)
//! decides the same formula via abstraction — the path the V.7-c OpenTitan `csrng`
//! recoverability showcase uses. This ergonomic command does not replace that path;
//! it makes the small/medium case a one-liner and is the surface peer of the safety
//! (`btor2 verify`) and response-liveness (`btor2 verify-liveness`) commands.

use crate::adapter::btor2::predicate_expr::parse_predicate_expr;
use crate::adapter::btor2::symbolic_bitblast::exact_symbolic_verdict;
use crate::mu_calculus::parser as mu_parser;
use crate::verdict::PropertyVerdict;

/// Decide recoverability `AG EF (good)` of `btor2_content`, where `good` is a single
/// register-comparison atom string (`"state_q == 3"`).
///
/// Returns the canonical [`PropertyVerdict`]: `Holds` (every reachable state can
/// reach `good`), `Violated` (a reachable trap cannot), or `Unknown` (the design is
/// over the exact engine's cap or uses an unsupported construct — try the cube +
/// `smt-hyper-must` path for scale). Errors only when `good` is not a parseable atom.
pub fn verify_recoverability(btor2_content: &str, good: &str) -> Result<PropertyVerdict, String> {
    // Validate the atom up front for a target-specific error message (the µ-parser
    // would otherwise report it as a formula-syntax error).
    parse_predicate_expr(good).map_err(|e| {
        format!("recoverability target `{good}` is not a register-comparison atom (`REG op VALUE`): {e:?}")
    })?;

    // AG EF good = nu Y. ((mu X. (good || <> X)) && [] Y).
    let formula_str = format!("nu Y. ((mu X. (({good}) || <> X)) && [] Y)");
    let formula = mu_parser::parse(&formula_str)
        .map_err(|e| format!("building the AG EF formula for `{good}`: {e:?}"))?;

    Ok(match exact_symbolic_verdict(btor2_content, &formula) {
        Ok(v) => PropertyVerdict::from(v),
        // Over the 40-bit cone cap or an unsupported construct: a sound abstention.
        Err(_) => PropertyVerdict::Unknown,
    })
}

/// The `AG EF good` formula string this command decides, for provenance / echoing
/// on a surface (`AG EF (<good>)`).
pub fn recoverability_property_str(good: &str) -> String {
    format!("AG EF ({good})")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 3-state responder: st 0=idle, 1=req, 2=grant; idle -go-> req; req -> grant;
    // grant -> idle. Every reachable state can reach idle ⇒ AG EF (st==0) HOLDS.
    const RESPONDER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 eq 2 3 4
10 eq 2 3 7
11 ite 1 6 7 4
12 ite 1 10 8 4
13 ite 1 9 11 12
14 next 1 3 13
";

    // 4-state staller: st 0=idle, 1=req, 3=stuck (absorbing); 2=grant unreachable.
    // idle -go-> req; req -> stuck; stuck -> stuck. The reachable `stuck` cannot get
    // back to idle ⇒ AG EF (st==0) VIOLATED.
    const STALLER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 constd 1 3
10 eq 2 3 4
11 eq 2 3 7
12 ite 1 6 7 4
13 ite 1 11 9 3
14 ite 1 10 12 13
15 next 1 3 14
";

    #[test]
    fn recoverable_design_holds() {
        assert_eq!(
            verify_recoverability(RESPONDER, "st == 0").expect("decides"),
            PropertyVerdict::Holds
        );
    }

    #[test]
    fn design_with_absorbing_trap_is_violated() {
        assert_eq!(
            verify_recoverability(STALLER, "st == 0").expect("decides"),
            PropertyVerdict::Violated
        );
    }

    #[test]
    fn malformed_target_errors() {
        assert!(verify_recoverability(RESPONDER, "not an atom !!").is_err());
    }

    #[test]
    fn property_string_echoes_the_target() {
        assert_eq!(recoverability_property_str("st == 0"), "AG EF (st == 0)");
    }
}
