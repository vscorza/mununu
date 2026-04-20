//! TLSF (Temporal Logic Synthesis Format) adapter.
//!
//! Translates TLSF specifications into CTXDSL via the shared IR.
//! Supports non-parametric TLSF v1.1 (INFO + MAIN with INPUTS/OUTPUTS/
//! ASSUMPTIONS/INVARIANTS/GUARANTEES).

mod parser;

use super::ir::*;
use super::{
    AdapterError, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter, SourceFormat,
    SourceInfo, WarningKind,
};

/// TLSF adapter implementing [`FormatAdapter`].
pub struct TlsfAdapter;

impl FormatAdapter for TlsfAdapter {
    fn detect(content: &str) -> bool {
        // TLSF files start with INFO { or have MAIN { with INPUTS/OUTPUTS
        let trimmed = content.trim_start();
        trimmed.starts_with("INFO") || (trimmed.contains("MAIN") && trimmed.contains("INPUTS"))
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        // Parse TLSF
        let spec = parser::parse(content)?;

        // Convert to IR
        let ir = to_ir(&spec, options)?;

        // Emit CTXDSL with compound atomic labels and game-aware formulas
        let emit_result = super::emit::emit(&ir)?;
        let state_count = emit_result.state_count;

        let mut warnings = Vec::new();
        if state_count > 10_000 {
            warnings.push(AdapterWarning {
                kind: WarningKind::LargeStateSpace,
                message: format!(
                    "Signal-state encoding produces {} states from {} signals",
                    state_count,
                    ir.signals.len()
                ),
                location: None,
            });
        }

        Ok(AdapterOutput {
            ctxdsl: emit_result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::Tlsf,
                title: Some(ir.metadata.title.clone()),
                signal_count: ir.signals.len(),
                state_count,
                property_count: ir.properties.len(),
            },
            state_valuations: Default::default(),
        })
    }
}

/// Convert a parsed TLSF spec to AdapterIR.
fn to_ir(spec: &parser::TlsfSpec, options: &AdapterOptions) -> Result<AdapterIR, AdapterError> {
    let context_name = options
        .context_name
        .clone()
        .unwrap_or_else(|| spec.title.clone().unwrap_or_else(|| "tlsf_spec".into()));

    let mut signals = Vec::new();

    // Input signals
    for name in &spec.inputs {
        signals.push(Signal {
            name: name.clone(),
            kind: SignalKind::Input,
            domain: SignalDomain::Boolean,
            role: SignalRole::StateAndLabel,
        });
    }

    // Output signals
    for name in &spec.outputs {
        signals.push(Signal {
            name: name.clone(),
            kind: SignalKind::Output,
            domain: SignalDomain::Boolean,
            role: SignalRole::StateAndLabel,
        });
    }

    // Properties
    let mut properties = Vec::new();

    for (i, assume) in spec.assumptions.iter().enumerate() {
        properties.push(PropertySpec {
            name: format!("assume_{}", i + 1),
            kind: PropertyKind::Liveness,
            formula: PropertyFormula::Ltl(assume.clone()),
            role: PropertyRole::Assumption,
            over: None,
        });
    }

    for (i, inv) in spec.invariants.iter().enumerate() {
        properties.push(PropertySpec {
            name: format!("inv_{}", i + 1),
            kind: PropertyKind::Safety,
            formula: PropertyFormula::Ltl(inv.clone()),
            role: PropertyRole::Invariant,
            over: None,
        });
    }

    for (i, guar) in spec.guarantees.iter().enumerate() {
        properties.push(PropertySpec {
            name: format!("guarantee_{}", i + 1),
            kind: PropertyKind::Liveness,
            formula: PropertyFormula::Ltl(guar.clone()),
            role: PropertyRole::Guarantee,
            over: None,
        });
    }

    // Controller
    let controller = Some(ControllerSpec {
        name: "synth".into(),
        source_automaton: "Signals".into(),
        formula_name: "syntcomp_prop".into(),
    });

    Ok(AdapterIR {
        metadata: Metadata {
            title: context_name,
            source_format: SourceFormat::Tlsf,
            description: spec.description.clone(),
            game_semantics: spec.semantics,
            known_status: spec.known_status,
        },
        signals,
        automata: vec![],
        compositions: vec![],
        properties,
        controller,
    })
}
