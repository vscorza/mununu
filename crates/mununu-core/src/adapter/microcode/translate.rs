//! Microcode AST → `AdapterIR` translation.
//!
//! Each microcode step becomes one CLTS state. Each `step.next`
//! becomes one transition; the step's `ops` produce the transition's
//! labels (one canonical rendezvous label per op, per the table in
//! the plan's Part 5.5).
//!
//! ## Soundness
//!
//! Per `docs/abstraction.md`'s microprogram entry: register values
//! and memory contents are *not* tracked here. The microcode adapter
//! emits transitions tagged with side-effect labels; the actual
//! values flow through the surrounding cache / memory automata. This
//! is the canonical abstraction recipe — sound for safety properties
//! that reference only step states and label firings, sound for
//! reachability over those, optimistic for fine-grained value
//! properties (which the discipline already declared out of scope
//! for v1).

use crate::adapter::ir::{AdapterIR, AutomatonSpec, Metadata, StateSpec, TransitionSpec};
use crate::adapter::microcode::ast::{MemRegion, Microcode, Op};
use crate::adapter::{AdapterError, AdapterErrorKind, AdapterWarning, SourceFormat, WarningKind};

/// Translate a parsed [`Microcode`] document into the shared `AdapterIR`.
pub fn to_ir(
    program: Microcode,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<AdapterIR, AdapterError> {
    let automaton_name = sanitise_ident(&program.name);
    if automaton_name.is_empty() {
        return Err(AdapterError {
            kind: AdapterErrorKind::IrConsistencyError,
            message: "microcode document has no `name`".to_string(),
            location: None,
        });
    }
    if program.steps.is_empty() {
        return Err(AdapterError {
            kind: AdapterErrorKind::IrConsistencyError,
            message: format!("microcode `{automaton_name}` has no `steps` — nothing to translate."),
            location: None,
        });
    }

    // ---------- per-step states -----------------------------------
    let mut states: Vec<StateSpec> = Vec::with_capacity(program.steps.len());
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (idx, step) in program.steps.iter().enumerate() {
        let state_name = sanitise_ident(&step.id);
        if state_name.is_empty() {
            return Err(AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                message: format!("step #{idx} has an empty `id`."),
                location: None,
            });
        }
        if !seen_ids.insert(state_name.clone()) {
            return Err(AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                message: format!(
                    "duplicate step id `{state_name}` in microcode `{automaton_name}`."
                ),
                location: None,
            });
        }
        states.push(StateSpec {
            name: state_name,
            is_initial: idx == 0,
            valuations: None,
            three_valued: None,
        });
    }

    // ---------- per-step transitions ------------------------------
    let mut transitions: Vec<TransitionSpec> = Vec::new();
    let mut controllable_labels: Vec<String> = Vec::new();
    let mut internal_labels: Vec<String> = Vec::new();

    for step in &program.steps {
        let source = sanitise_ident(&step.id);
        let Some(next_id) = step.next.as_ref() else {
            // Terminal step — no outgoing transition.
            if !step.ops.is_empty() {
                warnings.push(AdapterWarning {
                    kind: WarningKind::ApproximateTranslation,
                    message: format!(
                        "step `{source}` is terminal (no `next`) but declares {} op(s); they're dropped.",
                        step.ops.len()
                    ),
                    location: None,
                });
            }
            continue;
        };
        let target = sanitise_ident(next_id);
        if !seen_ids.contains(&target) {
            return Err(AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                message: format!(
                    "step `{source}` declares `next = {next_id}` but no step with that id exists."
                ),
                location: None,
            });
        }

        // A step with no ops becomes a no-op transition tagged with
        // `tick_<source>` — that gives the composition something to
        // synchronise on if needed.
        let labels = if step.ops.is_empty() {
            vec![format!("tick_{source}")]
        } else {
            step.ops
                .iter()
                .map(|op| label_for_op(op, &program.mem))
                .collect::<Result<Vec<_>, _>>()?
        };

        // Classify each label's controllability per the discipline:
        // every microcode-emitted label is controllable by default
        // (the microprogram is the controller). Shared rendezvous
        // labels stay controllable; private register / memory labels
        // become internal.
        for l in &labels {
            if label_is_internal(l) {
                if !internal_labels.contains(l) {
                    internal_labels.push(l.clone());
                }
            } else if !controllable_labels.contains(l) {
                controllable_labels.push(l.clone());
            }
        }

        transitions.push(TransitionSpec {
            source,
            target,
            labels,
            modality: crate::context_dsl::ast::TransitionModalitySpec::Sharp,

            additional_targets: Vec::new(),
        });
    }

    // ---------- extra_controllable (labels declared but not fired) ---
    // The microcode source owns these labels even without firing
    // them. See Microcode::extra_controllable doc-comment for the
    // motivating multi-source / cache-snoop scenario.
    for l in &program.extra_controllable {
        if !controllable_labels.contains(l) {
            controllable_labels.push(l.clone());
        }
    }

    // ---------- __mununu overrides --------------------------------
    if let Some(ann) = program.mununu.as_ref() {
        for l in &ann.internal {
            if !internal_labels.contains(l) {
                internal_labels.push(l.clone());
            }
            controllable_labels.retain(|c| c != l);
        }
        for l in &ann.controllable {
            if !controllable_labels.contains(l) {
                controllable_labels.push(l.clone());
            }
            internal_labels.retain(|c| c != l);
        }
        // `uncontrollable` labels are simply dropped from both sets
        // (CLTS default is uncontrollable).
        for l in &ann.uncontrollable {
            controllable_labels.retain(|c| c != l);
            internal_labels.retain(|c| c != l);
        }
    }

    let total_states = states.len();
    let total_ops: usize = program.steps.iter().map(|s| s.ops.len()).sum();

    Ok(AdapterIR {
        metadata: Metadata {
            title: automaton_name.clone(),
            source_format: SourceFormat::XState, // shared variant until SourceFormat::Microcode lands
            description: program.description.clone().or_else(|| {
                Some(format!(
                    "Translated from microcode `{}` ({} steps, {} ops)",
                    automaton_name, total_states, total_ops,
                ))
            }),
            game_semantics: None,
            known_status: None,
        },
        signals: Vec::new(),
        automata: vec![AutomatonSpec {
            name: automaton_name,
            states,
            transitions,
            controllable_labels,
            internal_labels,
        }],
        compositions: Vec::new(),
        properties: Vec::new(),
        controller: None,
    })
}

/// Build the canonical rendezvous label for one op. Returns
/// `wr_mem_<region>` / `rd_mem_<region>` / `fence_<order>` /
/// `irq_ack_<source>` for shared memory, fences, and interrupts; and
/// `wr_reg_<reg>` / `wr_priv_<region>` / `rd_priv_<region>` for
/// internal-only register and private-memory effects.
fn label_for_op(
    op: &Op,
    mem: &std::collections::BTreeMap<String, MemRegion>,
) -> Result<String, AdapterError> {
    match op {
        Op::WriteReg { reg, .. } => Ok(format!("wr_reg_{}", sanitise_ident(reg))),
        Op::WriteMem { region, tag, .. } => {
            let shared = mem
                .get(region)
                .map(|r| r.kind.eq_ignore_ascii_case("shared"))
                .unwrap_or(true);
            Ok(if shared {
                let mut name = format!("wr_mem_{}", sanitise_ident(region));
                if let Some(t) = tag {
                    name.push('_');
                    name.push_str(&sanitise_ident(t));
                }
                name
            } else {
                format!("wr_priv_{}", sanitise_ident(region))
            })
        }
        Op::ReadMem { region, tag, .. } => {
            let shared = mem
                .get(region)
                .map(|r| r.kind.eq_ignore_ascii_case("shared"))
                .unwrap_or(true);
            Ok(if shared {
                let mut name = format!("rd_mem_{}", sanitise_ident(region));
                if let Some(t) = tag {
                    name.push('_');
                    name.push_str(&sanitise_ident(t));
                }
                name
            } else {
                format!("rd_priv_{}", sanitise_ident(region))
            })
        }
        Op::Fence { order } => Ok(format!("fence_{}", sanitise_ident(order))),
        Op::IrqAck { source } => Ok(format!("irq_ack_{}", sanitise_ident(source))),
    }
}

/// Labels that classify as "internal" by default. Today: the
/// register-file labels (`wr_reg_*`), the private-region labels
/// (`wr_priv_*` / `rd_priv_*`), and the no-op `tick_*` labels. Shared
/// rendezvous labels (`wr_mem_*`, `rd_mem_*`, `fence_*`, `irq_ack_*`)
/// are controllable.
fn label_is_internal(label: &str) -> bool {
    label.starts_with("wr_reg_")
        || label.starts_with("wr_priv_")
        || label.starts_with("rd_priv_")
        || label.starts_with("tick_")
}

/// Sanitise a microcode identifier into a CTXDSL identifier.
pub(crate) fn sanitise_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for c in s.chars() {
        let ok = if first {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        };
        out.push(if ok { c } else { '_' });
        first = false;
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}
