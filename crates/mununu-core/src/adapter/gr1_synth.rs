//! GR(1) controller synthesis driven from an adapter [`AdapterIR`].
//!
//! Bridges the structured spec produced by LTL-bearing adapters (TLSF today) to
//! the sound GR(1) synthesizer in [`crate::mu_calculus::gr1_build`]. Inputs and
//! outputs come from the signal kinds; assumptions and guarantees from the
//! LTL-formula properties' assume/guarantee roles.

use crate::adapter::ir::{AdapterIR, PropertyFormula, PropertyRole, SignalKind};
use crate::mu_calculus::gr1_build::{Gr1Synthesis, synthesise_gr1};

/// Synthesize a sound GR(1) controller from an adapter IR.
///
/// - inputs  = `SignalKind::Input` signals
/// - outputs = `SignalKind::Output` signals
/// - assumptions = LTL properties with `PropertyRole::Assumption`
/// - guarantees  = LTL properties with `PropertyRole::Guarantee` or `Invariant`
///   (an INVARIANT is a system safety guarantee)
///
/// Non-LTL properties and `Standalone` properties are ignored. Returns an error
/// if the spec falls outside the supported GR(1) fragment (see
/// [`crate::mu_calculus::gr1_build::Gr1Spec::classify`]).
pub fn synthesise_gr1_from_ir(ir: &AdapterIR, module: &str) -> Result<Gr1Synthesis, String> {
    let inputs: Vec<String> = ir
        .signals
        .iter()
        .filter(|s| matches!(s.kind, SignalKind::Input))
        .map(|s| s.name.clone())
        .collect();
    let outputs: Vec<String> = ir
        .signals
        .iter()
        .filter(|s| matches!(s.kind, SignalKind::Output))
        .map(|s| s.name.clone())
        .collect();

    let mut assumptions = Vec::new();
    let mut guarantees = Vec::new();
    for p in &ir.properties {
        if let PropertyFormula::Ltl(f) = &p.formula {
            match p.role {
                PropertyRole::Assumption => assumptions.push(f.clone()),
                PropertyRole::Guarantee | PropertyRole::Invariant => guarantees.push(f.clone()),
                PropertyRole::Standalone => {}
            }
        }
    }

    if outputs.is_empty() {
        return Err("GR(1) synthesis needs at least one output signal".to_string());
    }
    synthesise_gr1(&assumptions, &guarantees, &inputs, &outputs, module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::AdapterOptions;

    #[test]
    fn synthesise_gr1_from_request_grant_tlsf_ir() {
        let src = r#"
INFO { TITLE: "rg"; DESCRIPTION: "req/grant"; SEMANTICS: Mealy; TARGET: Mealy; }
MAIN {
  INPUTS { req; }
  OUTPUTS { grant; }
  ASSUMPTIONS { G F req; }
  GUARANTEES { G (req -> F grant); G (grant -> X !grant); }
}
"#;
        let ir = crate::adapter::tlsf::translate_to_ir(src, &AdapterOptions::default())
            .expect("TLSF parses to IR");
        let r = synthesise_gr1_from_ir(&ir, "rg_ctrl").expect("classifies + synthesizes");
        assert!(r.realizable, "request_grant is realizable via the IR path");
        let sv = r.controller_sv.expect("controller emitted");
        assert!(sv.contains("module rg_ctrl"));
        assert!(sv.contains("input logic req"));
        assert!(sv.contains("output logic grant"));
    }
}
