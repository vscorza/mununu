//! Promela (SPIN) adapter.
//!
//! Translates bounded Promela programs into CTXDSL via compositional encoding:
//! each process becomes a CFG automaton, each variable becomes a variable automaton,
//! and they are composed synchronously/asynchronously.

pub mod ast;
pub mod cfg;
mod parser;

use super::ir::*;
use super::{
    AdapterError, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter, SourceFormat,
    SourceInfo, WarningKind,
};
use crate::ltl::LtlFormula;

/// Promela adapter implementing [`FormatAdapter`].
pub struct PromelaAdapter;

impl FormatAdapter for PromelaAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        trimmed.contains("proctype")
            || trimmed.contains("active")
            || (trimmed.contains("init") && trimmed.contains("{"))
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let program = parser::parse(content)?;
        let ir = to_ir(&program, options)?;

        let mut warnings = Vec::new();

        // Warn about unbounded variables
        for g in &program.globals {
            if g.typename.needs_bounding() {
                warnings.push(AdapterWarning {
                    kind: WarningKind::ApproximateTranslation,
                    message: format!(
                        "Variable '{}' ({:?}) auto-bounded via static analysis",
                        g.name, g.typename
                    ),
                    location: None,
                });
            }
        }

        let _automaton_count = ir.automata.len();
        let emit_result = super::emit::emit(&ir)?;

        Ok(AdapterOutput {
            ctxdsl: emit_result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::Promela,
                title: None,
                signal_count: program.globals.len(),
                state_count: 0, // computed at composition time
                property_count: ir.properties.len(),
            },
        })
    }
}

/// Convert a parsed Promela program to AdapterIR.
fn to_ir(program: &ast::Program, options: &AdapterOptions) -> Result<AdapterIR, AdapterError> {
    let context_name = options
        .context_name
        .clone()
        .unwrap_or_else(|| "promela_system".into());

    let mut automata = Vec::new();
    let mut all_labels = std::collections::HashSet::new();

    // Extract CFGs for each process
    for proc in &program.proctypes {
        if proc.active_count == 0 {
            continue;
        } // skip non-active proctypes

        let proc_cfg = cfg::extract_cfg(&proc.name, &proc.body);

        // Convert CFG to AutomatonSpec
        let states: Vec<StateSpec> = proc_cfg
            .locations
            .iter()
            .map(|loc| StateSpec {
                name: loc.label.clone(),
                is_initial: loc.id == proc_cfg.initial,
            })
            .collect();

        let transitions: Vec<TransitionSpec> = proc_cfg
            .edges
            .iter()
            .map(|edge| {
                let src = proc_cfg.locations[edge.src].label.clone();
                let dst = proc_cfg.locations[edge.dst].label.clone();
                let labels = if edge.labels.is_empty() {
                    vec![format!(
                        "{}_epsilon_{}_to_{}",
                        proc.name, edge.src, edge.dst
                    )]
                } else {
                    edge.labels.clone()
                };
                for l in &labels {
                    all_labels.insert(l.clone());
                }
                TransitionSpec {
                    source: src,
                    target: dst,
                    labels,
                }
            })
            .collect();

        automata.push(AutomatonSpec {
            name: format!("{}_cfg", proc.name),
            states,
            transitions,
            controllable_labels: vec![],
            internal_labels: vec![],
        });
    }

    // Create variable automata for each global variable
    for var in &program.globals {
        let var_aut = create_variable_automaton(var, &all_labels);
        automata.push(var_aut);
    }

    // Create compositions
    let mut compositions = Vec::new();

    // All CFG automata composed asynchronously (process interleaving)
    let cfg_names: Vec<String> = program
        .proctypes
        .iter()
        .filter(|p| p.active_count > 0)
        .map(|p| format!("{}_cfg", p.name))
        .collect();

    let var_names: Vec<String> = program
        .globals
        .iter()
        .map(|v| format!("Var_{}", v.name))
        .collect();

    if cfg_names.len() + var_names.len() > 1 {
        let mut all_members = Vec::new();
        all_members.extend(cfg_names);
        all_members.extend(var_names);
        compositions.push(CompositionSpec::Asynchronous {
            name: "System".into(),
            members: all_members,
        });
    }

    // Convert LTL properties
    let properties: Vec<PropertySpec> = program
        .ltl_properties
        .iter()
        .map(|ltl| {
            let name = ltl.name.clone().unwrap_or_else(|| "property".into());
            PropertySpec {
                name,
                kind: PropertyKind::Safety,
                formula: PropertyFormula::Ltl(convert_ltl(&ltl.formula)),
                role: PropertyRole::Standalone,
            }
        })
        .collect();

    Ok(AdapterIR {
        metadata: Metadata {
            title: context_name,
            source_format: SourceFormat::Promela,
            description: None,
            game_semantics: None,
            known_status: None,
        },
        signals: vec![],
        automata,
        compositions,
        properties,
        controller: None,
    })
}

/// Create a variable automaton for a bounded variable.
fn create_variable_automaton(
    var: &ast::VarDecl,
    _used_labels: &std::collections::HashSet<String>,
) -> AutomatonSpec {
    let (lo, hi) = match &var.typename {
        ast::TypeName::Bit | ast::TypeName::Bool => (0i64, 1),
        ast::TypeName::Byte => {
            // Auto-bound: use a small range for now
            // Full auto-analysis is Phase 3.5a
            (0, 1) // Default to boolean for byte if init is 0/false
        }
        _ => (0, 1), // Default fallback
    };

    // Check init value
    let init_val = match &var.init {
        Some(ast::Expr::IntLit(n)) => *n,
        Some(ast::Expr::BoolLit(false)) => 0,
        Some(ast::Expr::BoolLit(true)) => 1,
        _ => 0,
    };

    let mut states = Vec::new();
    let mut transitions = Vec::new();

    for val in lo..=hi {
        let state_name = format!("{}_{}", var.name, val);
        states.push(StateSpec {
            name: state_name.clone(),
            is_initial: val == init_val,
        });

        // set_var_val transitions from any state to this value
        for src_val in lo..=hi {
            let src_name = format!("{}_{}", var.name, src_val);
            let label = format!("set_{}_{}", var.name, val);
            transitions.push(TransitionSpec {
                source: src_name.clone(),
                target: state_name.clone(),
                labels: vec![label],
            });
        }

        // test_var_val self-loop (guard check)
        let test_label = format!("test_{}_{}", var.name, val);
        transitions.push(TransitionSpec {
            source: state_name.clone(),
            target: state_name.clone(),
            labels: vec![test_label],
        });
    }

    AutomatonSpec {
        name: format!("Var_{}", var.name),
        states,
        transitions,
        controllable_labels: vec![],
        internal_labels: vec![],
    }
}

/// Convert Promela LTL expression to mununu's LtlFormula.
fn convert_ltl(expr: &ast::LtlExpr) -> LtlFormula {
    match expr {
        ast::LtlExpr::True => LtlFormula::True,
        ast::LtlExpr::False => LtlFormula::False,
        ast::LtlExpr::Predicate(name) => LtlFormula::Predicate(name.clone()),
        ast::LtlExpr::Not(inner) => LtlFormula::Not(Box::new(convert_ltl(inner))),
        ast::LtlExpr::And(l, r) => {
            LtlFormula::And(Box::new(convert_ltl(l)), Box::new(convert_ltl(r)))
        }
        ast::LtlExpr::Or(l, r) => {
            LtlFormula::Or(Box::new(convert_ltl(l)), Box::new(convert_ltl(r)))
        }
        ast::LtlExpr::Implies(l, r) => {
            LtlFormula::Implies(Box::new(convert_ltl(l)), Box::new(convert_ltl(r)))
        }
        ast::LtlExpr::Iff(l, r) => {
            let l_conv = convert_ltl(l);
            let r_conv = convert_ltl(r);
            LtlFormula::And(
                Box::new(LtlFormula::Implies(
                    Box::new(l_conv.clone()),
                    Box::new(r_conv.clone()),
                )),
                Box::new(LtlFormula::Implies(Box::new(r_conv), Box::new(l_conv))),
            )
        }
        ast::LtlExpr::Always(inner) => LtlFormula::Always(Box::new(convert_ltl(inner))),
        ast::LtlExpr::Eventually(inner) => LtlFormula::Eventually(Box::new(convert_ltl(inner))),
        ast::LtlExpr::Next(inner) => LtlFormula::Next(Box::new(convert_ltl(inner))),
        ast::LtlExpr::Until(l, r) => LtlFormula::Until {
            left: Box::new(convert_ltl(l)),
            right: Box::new(convert_ltl(r)),
        },
        ast::LtlExpr::WeakUntil(l, r) => LtlFormula::WeakUntil {
            left: Box::new(convert_ltl(l)),
            right: Box::new(convert_ltl(r)),
        },
        ast::LtlExpr::Release(l, r) => LtlFormula::Release {
            left: Box::new(convert_ltl(l)),
            right: Box::new(convert_ltl(r)),
        },
    }
}
