//! Register-based Kripke structure construction for SystemVerilog.
//!
//! Builds an explicit-state Kripke structure from register/signal valuations,
//! going beyond typedef-enum FSM extraction. Each distinct valuation of the
//! active registers is a state; transitions are computed by evaluating the
//! combinational and sequential logic for each (state, input) pair.
//!
//! # Abstraction
//!
//! Wide registers must be abstracted via `// @mununu domain` annotations.
//! Cone-of-influence reduction automatically excludes registers not relevant
//! to checked properties.

use super::ast::*;
use crate::adapter::domain::{AbstractState, AbstractValue, AbstractionType, FieldDomain};
use crate::adapter::ir::*;
use crate::adapter::state_enum;
use crate::adapter::{AdapterError, AdapterErrorKind, AdapterWarning, WarningKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Reserved valuation key marking the out-of-bounds (OOB) sink state.
///
/// The marker survives the CTXDSL identifier sanitizer unchanged (no `$` or
/// other special characters to be rewritten), so the key in the side-channel
/// `state_valuations` map matches the actual CTXDSL state name. Collision with
/// a user-declared SystemVerilog identifier is implausibly unlikely given the
/// double-underscore + `mununu` namespace.
///
/// The mu-calculus evaluator detects this marker and masks every carrying
/// state out of every formula's satisfying set (`mu_calculus::evaluator::oob_bits`),
/// giving the OOB sink "bottom" semantics in the BitVec evaluator and
/// `Unknown` in the trit evaluator. Reference: Bruns–Godefroid CONCUR 2000
/// (generalized model checking, safety projection).
pub const OOB_STATE_KEY: &str = "__mununu_oob__";

/// Diagnostic flag returned by `clamp_to_domain` to signal that a value would
/// have escaped the abstracted domain. The clamped value is still returned so
/// downstream code can continue, but the caller (typically `compute_next_state`)
/// collects these flags and routes the affected transition to the OOB sink.
#[derive(Debug, Clone)]
pub enum OverflowInfo {
    InDomain,
    CounterAbove {
        register: String,
        value: i64,
        bound: i64,
    },
    EnumIndexOutOfRange {
        register: String,
        index: i64,
        variant_count: usize,
    },
}

impl OverflowInfo {
    fn is_overflow(&self) -> bool {
        !matches!(self, OverflowInfo::InDomain)
    }

    fn register(&self) -> Option<&str> {
        match self {
            OverflowInfo::InDomain => None,
            OverflowInfo::CounterAbove { register, .. } => Some(register),
            OverflowInfo::EnumIndexOutOfRange { register, .. } => Some(register),
        }
    }

    fn message(&self) -> String {
        match self {
            OverflowInfo::InDomain => String::new(),
            OverflowInfo::CounterAbove {
                register,
                value,
                bound,
            } => format!(
                "register '{}' would take value {} (bound = {}); transition routed to OOB sink. \
                 Widen the abstraction domain in the .mununu.json sidecar to recover precision.",
                register, value, bound
            ),
            OverflowInfo::EnumIndexOutOfRange {
                register,
                index,
                variant_count,
            } => format!(
                "register '{}' would index variant {} but only {} variants are declared; \
                 transition routed to OOB sink.",
                register, index, variant_count
            ),
        }
    }
}

/// Construct the unique OOB sentinel `AbstractState` shared across the automaton.
fn make_oob_abstract_state() -> AbstractState {
    let mut s = BTreeMap::new();
    s.insert(OOB_STATE_KEY.to_string(), AbstractValue::Bool(true));
    s
}

/// Information about a register extracted from the module.
#[derive(Debug, Clone)]
pub struct RegisterInfo {
    pub name: String,
    pub width: usize,
    pub domain: FieldDomain,
    pub kind: SignalKind,
    /// Concrete value → variant name mapping (from `enum {IDLE=0, START=3, OTHER}`).
    pub value_map: Vec<(String, i64)>,
    /// Whether this signal is combinational (value computed from `assign` each cycle).
    pub combinational: bool,
}

/// Build a Kripke structure from the module's registers and logic.
/// Uses inline `@mununu` annotations for configuration.
///
/// Returns `(AutomatonSpec, Vec<PropertySpec>, state_count)`.
pub fn build_kripke(
    module: &Module,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<(AutomatonSpec, Vec<PropertySpec>, usize), AdapterError> {
    let config = super::annotation::merge_config(None, module);
    build_kripke_with_config(module, &config, warnings)
}

/// Build a Kripke structure using a `MergedConfig` (from sidecar or inline).
pub fn build_kripke_with_config(
    module: &Module,
    config: &super::annotation::MergedConfig,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<(AutomatonSpec, Vec<PropertySpec>, usize), AdapterError> {
    // Step 1: Build registers from config
    let mut registers = build_registers_from_config(module, config, warnings);

    // Step 1.5: Scan for significant constants and suggest value maps
    let significant_constants = scan_significant_constants(module);
    for reg in &registers {
        if reg.domain.abstraction == AbstractionType::Ignored
            && reg.width > 4
            && let Some(constants) = significant_constants.get(&reg.name)
            && !constants.is_empty()
        {
            let const_list: Vec<String> = constants.iter().map(|c| c.to_string()).collect();
            let suggested_variants: Vec<String> = constants
                .iter()
                .map(|c| format!("VAL_{}={}", c, c))
                .chain(std::iter::once("OTHER".to_string()))
                .collect();
            warnings.push(AdapterWarning {
                kind: WarningKind::NeutralControllability,
                message: format!(
                    "Register '{}' ({}-bit, ignored) uses significant constants: [{}]. \
                     Suggested: // @mununu domain {}: enum {{{}}}",
                    reg.name,
                    reg.width,
                    const_list.join(", "),
                    reg.name,
                    suggested_variants.join(", ")
                ),
                location: None,
            });
        }
    }

    // Step 2: Cone-of-influence reduction (inline path only)
    // When a sidecar is used, the user explicitly controls preservation via
    // `preserve: true/false` — COI auto-exclusion is not applied.
    if !config.from_sidecar {
        let property_signals = collect_property_signals_from_config(config);
        if !property_signals.is_empty() {
            let deps = build_dependency_graph(module);
            let relevant = compute_cone_of_influence(&property_signals, &deps);
            for reg in &mut registers {
                let is_relevant = relevant.contains(&reg.name)
                    || property_signals
                        .iter()
                        .any(|tok| tok.starts_with(&reg.name));
                if reg.domain.abstraction != AbstractionType::Ignored && !is_relevant {
                    warnings.push(AdapterWarning {
                        kind: WarningKind::ApproximateTranslation,
                        message: format!(
                            "Register '{}' excluded by cone-of-influence (not relevant to properties)",
                            reg.name
                        ),
                        location: None,
                    });
                    reg.domain.abstraction = AbstractionType::Ignored;
                }
            }
        }
    }

    // Step 3: Check state space size
    let active_registers: Vec<&RegisterInfo> = registers
        .iter()
        .filter(|r| r.domain.abstraction != AbstractionType::Ignored)
        .collect();

    let total_states: usize = if active_registers.is_empty() {
        1
    } else {
        active_registers
            .iter()
            .map(|r| r.domain.cardinality())
            .product()
    };

    if total_states > (1 << 18) {
        return Err(AdapterError {
            kind: AdapterErrorKind::StateSpaceOverflow,
            message: format!(
                "Kripke state space has {} states (from {} registers), exceeding the 2^18 limit. \
                 Use // @mununu domain <reg>: ignored or preserve: false in .mununu.json.",
                total_states,
                active_registers.len()
            ),
            location: None,
        });
    }
    if total_states > (1 << 12) {
        warnings.push(AdapterWarning {
            kind: WarningKind::LargeStateSpace,
            message: format!(
                "Kripke state space has {} states — synthesis may be slow",
                total_states
            ),
            location: None,
        });
    }

    // Step 4: Build input domains from config
    let input_domains: Vec<FieldDomain> = build_input_domains_from_config(module, config);

    // Step 5: Enumerate all register states and input combinations
    let reg_fields: Vec<&FieldDomain> = registers
        .iter()
        .filter(|r| r.domain.abstraction != AbstractionType::Ignored)
        .map(|r| &r.domain)
        .collect();

    let all_reg_states = state_enum::enumerate_cross_product(&reg_fields);
    let all_input_combos =
        state_enum::enumerate_cross_product(&input_domains.iter().collect::<Vec<_>>());

    // Step 6: Determine initial state from reset values
    let initial_state = extract_initial_state(module, &registers);

    // Step 7: Build transitions by evaluating logic
    let comb_assigns = collect_comb_assigns(module);
    let seq_assigns = collect_seq_assigns(module);

    let mut transitions = Vec::new();
    let mut state_names: HashMap<AbstractState, String> = HashMap::new();

    for reg_state in &all_reg_states {
        let src_name = state_enum::make_state_name(reg_state);
        state_names.insert(reg_state.clone(), src_name.clone());
    }

    let variant_to_numeric = build_variant_to_numeric(&registers, config, module);

    // Inject parameter constants into the valuation context
    // Parameters: merge module defaults with config overrides
    let mut param_values: BTreeMap<String, AbstractValue> = module
        .parameters
        .iter()
        .map(|p| (p.name.clone(), AbstractValue::Counter(p.default_value)))
        .collect();
    for (name, val) in &config.parameters {
        param_values.insert(name.clone(), AbstractValue::Counter(*val));
    }

    // Build label name overrides for connected inputs (multi-module shared labels)
    let label_overrides: HashMap<String, String> = config
        .input_domains
        .iter()
        .filter_map(|(name, cfg)| cfg.label_name.as_ref().map(|ln| (name.clone(), ln.clone())))
        .collect();

    let mut oob_inserted = false;
    let oob_state = make_oob_abstract_state();
    let oob_name = OOB_STATE_KEY.to_string();
    // Per-register cap on BoundOverflow warnings to prevent log spam from a single
    // register overflowing in many input combinations.
    let mut warning_count_per_register: HashMap<String, usize> = HashMap::new();
    const MAX_WARNINGS_PER_REGISTER: usize = 10;

    for reg_state in &all_reg_states {
        let src_name: String = state_names[reg_state].clone();
        for input_combo in &all_input_combos {
            // Merge register state, input values, and parameters
            let mut full_val: BTreeMap<String, AbstractValue> = reg_state.clone();
            full_val.extend(input_combo.clone());
            full_val.extend(param_values.clone());

            // For value-mapped registers, also inject the numeric equivalent
            // so RTL expressions like `cmd_reg == 3` work correctly
            for (reg_name, vmap) in &variant_to_numeric {
                if let Some(AbstractValue::Variant(variant)) = full_val.get(reg_name)
                    && let Some(&num_val) = vmap.get(variant.as_str())
                {
                    full_val.insert(reg_name.clone(), AbstractValue::Counter(num_val));
                }
            }

            // Evaluate combinational logic
            eval_comb_assigns(&comb_assigns, &mut full_val, &registers);

            // Compute next register state and any overflow flags from clamping
            let (next_state, overflows) = compute_next_state(
                &seq_assigns,
                &comb_assigns,
                reg_state,
                &full_val,
                &registers,
            );

            // SOUNDNESS: over-approx — if any register's next-state escapes the
            // abstracted domain (overflow at clamp_to_domain) OR the resulting
            // composite state isn't in the enumerated cross-product (e.g.,
            // BitSlice writes that bypass clamping, or Counter values outside
            // an enum's value_map), the transition is routed to a designated
            // OOB sink rather than dropped silently. The mu-calculus evaluator
            // masks the OOB state out of every formula's satisfying set, so
            // any source state with a transition here falsifies safety formulas
            // (`[a]Z` requires OOB ∈ Z; OOB ∉ Z always). Reference:
            // Bruns–Godefroid CONCUR 2000 (generalized model checking, safety
            // projection of partial-state semantics).
            let oob_detected = !overflows.is_empty() || !state_names.contains_key(&next_state);

            if oob_detected {
                // Lazy creation: insert the OOB sentinel and one self-loop per
                // input combination on first detection, idempotent thereafter.
                if !oob_inserted {
                    state_names.insert(oob_state.clone(), oob_name.clone());
                    for ic in &all_input_combos {
                        let labels = make_input_labels(ic, &label_overrides);
                        transitions.push(TransitionSpec {
                            source: oob_name.clone(),
                            target: oob_name.clone(),
                            labels,
                        });
                    }
                    oob_inserted = true;
                }

                let labels = make_input_labels(input_combo, &label_overrides);
                transitions.push(TransitionSpec {
                    source: src_name.clone(),
                    target: oob_name.clone(),
                    labels,
                });

                for ovf in overflows {
                    if let Some(reg_name) = ovf.register() {
                        let count = warning_count_per_register
                            .entry(reg_name.to_string())
                            .or_insert(0);
                        if *count < MAX_WARNINGS_PER_REGISTER {
                            warnings.push(AdapterWarning {
                                kind: WarningKind::BoundOverflow,
                                message: ovf.message(),
                                location: None,
                            });
                            *count += 1;
                        }
                    }
                }
            } else if let Some(tgt_name) = state_names.get(&next_state) {
                let labels = make_input_labels(input_combo, &label_overrides);
                transitions.push(TransitionSpec {
                    source: src_name.clone(),
                    target: tgt_name.clone(),
                    labels,
                });
            }
        }
    }

    // If OOB was inserted, append it to the state list so prune_unreachable_states
    // emits a `StateSpec` for it (with the `$oob$` marker valuation).
    let mut all_reg_states_with_oob = all_reg_states;
    if oob_inserted {
        all_reg_states_with_oob.push(oob_state);
    }

    // Steps 8-9: Prune unreachable states and build state specs
    let (states, transitions) = prune_unreachable_states(
        &initial_state,
        &all_reg_states_with_oob,
        &state_names,
        transitions,
        &registers,
        &param_values,
        &comb_assigns,
    );

    // Step 10: Build automaton with controllability classification
    let automaton = build_automaton_spec(module, config, states, transitions);

    // Step 11: Build properties from config
    let properties = build_property_specs(config)?;

    let state_count = automaton.states.len();
    Ok((automaton, properties, state_count))
}

// ---------------------------------------------------------------------------
// State pruning and automaton assembly
// ---------------------------------------------------------------------------

/// Prune unreachable states via BFS and build enriched `StateSpec` entries.
///
/// Returns the filtered (states, transitions) pair.
fn prune_unreachable_states(
    initial_state: &AbstractState,
    all_reg_states: &[AbstractState],
    state_names: &HashMap<AbstractState, String>,
    transitions: Vec<TransitionSpec>,
    registers: &[RegisterInfo],
    param_values: &BTreeMap<String, AbstractValue>,
    comb_assigns: &[CombAssign],
) -> (Vec<StateSpec>, Vec<TransitionSpec>) {
    let initial_name = state_names.get(initial_state).cloned().unwrap_or_else(|| {
        all_reg_states
            .first()
            .map(|s| state_names[s].clone())
            .unwrap_or_else(|| "s0".to_string())
    });

    let edges: Vec<(&str, &str)> = transitions
        .iter()
        .map(|t| (t.source.as_str(), t.target.as_str()))
        .collect();
    let reachable = state_enum::bfs_reachable(&initial_name, &edges);

    // Identify combinational registers for valuation enrichment
    let comb_register_names: Vec<&str> = registers
        .iter()
        .filter(|r| r.combinational && r.domain.abstraction != AbstractionType::Ignored)
        .map(|r| r.name.as_str())
        .collect();

    let states: Vec<StateSpec> = all_reg_states
        .iter()
        .filter_map(|s| {
            let name = &state_names[s];
            if !reachable.contains(name) {
                return None;
            }
            let valuations = build_state_valuations(
                s,
                &comb_register_names,
                param_values,
                comb_assigns,
                registers,
            );
            Some(StateSpec {
                name: name.clone(),
                is_initial: *name == initial_name,
                valuations: Some(valuations),
            })
        })
        .collect();

    let transitions: Vec<TransitionSpec> = transitions
        .into_iter()
        .filter(|t| reachable.contains(&t.source) && reachable.contains(&t.target))
        .collect();

    (states, transitions)
}

/// Build structured valuations for a single Kripke state.
///
/// Starts from the register values in `reg_state`, then evaluates combinational
/// assignments so that predicates can reference combinational outputs such as
/// `full_T` (derived from `assign full = (count >= 2)`).
///
/// Special case: the OOB sink state carries only the `$oob$ → "true"` marker.
/// Downstream code in the mu-calculus evaluator detects this marker and masks
/// the state out of every formula's satisfying set (bottom semantics).
fn build_state_valuations(
    reg_state: &AbstractState,
    comb_register_names: &[&str],
    param_values: &BTreeMap<String, AbstractValue>,
    comb_assigns: &[CombAssign],
    registers: &[RegisterInfo],
) -> BTreeMap<String, String> {
    if reg_state.contains_key(OOB_STATE_KEY) {
        let mut v = BTreeMap::new();
        v.insert(OOB_STATE_KEY.to_string(), "true".to_string());
        return v;
    }

    let mut valuations: BTreeMap<String, String> = reg_state
        .iter()
        .map(|(k, v)| (k.clone(), v.display_short()))
        .collect();

    // Compute and include combinational signal values in valuations.
    // This enables state predicates to reference combinational outputs
    // (e.g., `full_T` for `assign full = (count >= 2)`).
    if !comb_register_names.is_empty() {
        let mut full_val: BTreeMap<String, AbstractValue> = reg_state.clone();
        full_val.extend(param_values.clone());
        eval_comb_assigns(comb_assigns, &mut full_val, registers);
        for comb_name in comb_register_names {
            if let Some(val) = full_val.get(*comb_name) {
                valuations.insert(comb_name.to_string(), val.display_short());
            }
        }
    }

    valuations
}

/// Build the `AutomatonSpec` with controllability classification.
fn build_automaton_spec(
    module: &Module,
    config: &super::annotation::MergedConfig,
    states: Vec<StateSpec>,
    transitions: Vec<TransitionSpec>,
) -> AutomatonSpec {
    let controllable_set: HashSet<&str> = config.controllable.iter().map(|s| s.as_str()).collect();

    let all_labels: HashSet<String> = transitions
        .iter()
        .flat_map(|t| t.labels.iter().cloned())
        .collect();
    let controllable_labels: Vec<String> = all_labels
        .iter()
        .filter(|l| controllable_set.iter().any(|s| l.starts_with(s)))
        .cloned()
        .collect();

    AutomatonSpec {
        name: module.name.clone(),
        states,
        transitions,
        controllable_labels,
        internal_labels: vec![],
    }
}

/// Build property specs from merged config, resolving template refs.
///
/// Returns `Err(AdapterError)` if any property is malformed (missing both
/// `formula` and `template_ref`, or referencing an unknown template). This is a
/// fail-loud behavior: previously such properties were silently dropped, leading
/// to false-positive "satisfied" verdicts where violations were never checked.
fn build_property_specs(
    config: &super::annotation::MergedConfig,
) -> Result<Vec<PropertySpec>, AdapterError> {
    let registry = crate::adapter::templates::TemplateRegistry::builtin();
    let mut out = Vec::with_capacity(config.properties.len());
    for p in &config.properties {
        let role = match p.role.as_str() {
            "assumption" => PropertyRole::Assumption,
            "standalone" => PropertyRole::Standalone,
            _ => PropertyRole::Guarantee,
        };
        // Raw formula takes precedence over template_ref
        let formula_str = if let Some(f) = &p.formula {
            f.clone()
        } else if let Some(tref) = &p.template_ref {
            match registry.instantiate(tref) {
                Ok(inst) => inst.formula,
                Err(e) => {
                    return Err(AdapterError {
                        kind: AdapterErrorKind::ParseError,
                        message: format!(
                            "property '{}' references unknown template '{}': {}. \
                             Add the template to the registry or replace `template_ref` with a raw `formula`.",
                            p.id, tref.template, e
                        ),
                        location: None,
                    });
                }
            }
        } else {
            return Err(AdapterError {
                kind: AdapterErrorKind::ParseError,
                message: format!(
                    "property '{}' declares neither `formula` nor `template_ref` — \
                     cannot translate. Add one of the two fields.",
                    p.id
                ),
                location: None,
            });
        };
        out.push(PropertySpec {
            name: p.id.clone(),
            kind: PropertyKind::Safety,
            formula: PropertyFormula::MuCalculus(formula_str),
            role,
            over: None,
            description: None,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Value-map helpers
// ---------------------------------------------------------------------------

/// Build a reverse value map (variant name → numeric value) for all registers
/// and annotated input ports. Used to resolve RTL comparisons like `cmd == 3`
/// when the register is abstracted as an enum.
fn build_variant_to_numeric(
    registers: &[RegisterInfo],
    config: &super::annotation::MergedConfig,
    module: &Module,
) -> HashMap<String, HashMap<String, i64>> {
    let mut result: HashMap<String, HashMap<String, i64>> = registers
        .iter()
        .filter(|r| !r.value_map.is_empty())
        .map(|r| {
            let map: HashMap<String, i64> = r.value_map.iter().cloned().collect();
            (r.name.clone(), map)
        })
        .collect();

    if config.from_sidecar {
        // Sidecar path: use value maps from config only
        for (name, inp_config) in &config.input_domains {
            if !inp_config.value_map.is_empty() {
                let map: HashMap<String, i64> = inp_config.value_map.iter().cloned().collect();
                result.insert(name.clone(), map);
            }
        }
        for (name, sig_config) in &config.signal_domains {
            if !sig_config.value_map.is_empty() && !result.contains_key(name) {
                let map: HashMap<String, i64> = sig_config.value_map.iter().cloned().collect();
                result.insert(name.clone(), map);
            }
        }
    } else {
        // Inline path: use value maps from module annotations
        for ann in &module.domain_annotations {
            if let DomainAnnotationKind::Enum { value_map, .. } = &ann.domain_kind
                && !value_map.is_empty()
                && !result.contains_key(&ann.register_name)
            {
                let map: HashMap<String, i64> = value_map.iter().cloned().collect();
                result.insert(ann.register_name.clone(), map);
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Register extraction
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Config-driven register and input building
// ---------------------------------------------------------------------------

/// Build registers from a MergedConfig (sidecar or inline-derived).
fn build_registers_from_config(
    module: &Module,
    config: &super::annotation::MergedConfig,
    warnings: &mut Vec<AdapterWarning>,
) -> Vec<RegisterInfo> {
    if !config.from_sidecar {
        // Inline annotations — use the original extract_registers which
        // auto-detects signals and applies inline @mununu overrides.
        let mut registers = extract_registers(module, warnings);

        // Inline path also gets auto-detected combinational outputs (from
        // `merge_from_inline`'s output-port scan): add them as Kripke
        // registers if they're declared `combinational` in the merged
        // config. Without this, an `output logic ack` driven from
        // `always_comb` is invisible to the state space and its comb logic
        // is silently discarded.
        for port in &module.ports {
            if port.direction == PortDirection::Output
                && let Some(sig_config) = config.signal_domains.get(&port.name)
                && sig_config.combinational
                && !registers.iter().any(|r| r.name == port.name)
            {
                registers.push(RegisterInfo {
                    name: port.name.clone(),
                    width: port.width,
                    domain: sig_config.domain.clone(),
                    kind: SignalKind::Output,
                    value_map: sig_config.value_map.clone(),
                    combinational: true,
                });
            }
        }

        return registers;
    }

    let mut registers = Vec::new();
    for decl in &module.declarations {
        let (name, width) = match decl {
            Declaration::Enum {
                var_name: Some(var),
                variants,
                ..
            } => (
                var.clone(),
                variants.len().next_power_of_two().trailing_zeros() as usize,
            ),
            Declaration::Logic { name, width } => {
                if module.ports.iter().any(|p| p.name == *name) {
                    continue;
                }
                (name.clone(), *width)
            }
            _ => continue,
        };

        if let Some(sig_config) = config.signal_domains.get(&name) {
            registers.push(RegisterInfo {
                name: name.clone(),
                width,
                domain: sig_config.domain.clone(),
                kind: if config.controllable.contains(&name) {
                    SignalKind::Output
                } else {
                    SignalKind::Internal
                },
                value_map: sig_config.value_map.clone(),
                combinational: sig_config.combinational,
            });
        } else {
            // Signal not in config — default to Ignored
            registers.push(RegisterInfo {
                name: name.clone(),
                width,
                domain: FieldDomain {
                    name: name.clone(),
                    abstraction: AbstractionType::Ignored,
                    bound: None,
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                kind: SignalKind::Internal,
                value_map: vec![],
                combinational: false,
            });
        }
    }

    // Also include output ports that are declared as combinational signals in the sidecar.
    // These are signals like `assign full = (count >= 2)` that are computed from registers
    // but need to be tracked in state valuations for predicate resolution and composition.
    for port in &module.ports {
        if port.direction == PortDirection::Output
            && let Some(sig_config) = config.signal_domains.get(&port.name)
            && sig_config.combinational
            && !registers.iter().any(|r| r.name == port.name)
        {
            registers.push(RegisterInfo {
                name: port.name.clone(),
                width: port.width,
                domain: sig_config.domain.clone(),
                kind: SignalKind::Output,
                value_map: sig_config.value_map.clone(),
                combinational: true,
            });
        }
    }

    registers
}

/// Build input domains from config.
fn build_input_domains_from_config(
    module: &Module,
    config: &super::annotation::MergedConfig,
) -> Vec<FieldDomain> {
    if !config.from_sidecar {
        // Inline path: use the original auto-detection logic.
        // Include 1-bit inputs as Boolean, multi-bit inputs only if annotated.
        let annotated_inputs: HashSet<&str> = module
            .domain_annotations
            .iter()
            .map(|a| a.register_name.as_str())
            .collect();
        let domain_map: HashMap<&str, &DomainAnnotationKind> = module
            .domain_annotations
            .iter()
            .map(|a| (a.register_name.as_str(), &a.domain_kind))
            .collect();

        return module
            .ports
            .iter()
            .filter(|p| {
                p.direction == PortDirection::Input
                    && p.name != "clk"
                    && p.name != "rst"
                    && p.name != "rst_n"
                    && (p.width == 1 || annotated_inputs.contains(p.name.as_str()))
            })
            .map(|p| {
                if let Some(ann) = domain_map.get(p.name.as_str()) {
                    annotation_to_domain(&p.name, ann)
                } else {
                    FieldDomain {
                        name: p.name.clone(),
                        abstraction: AbstractionType::Boolean,
                        bound: None,
                        lower_bound: None,
                        variants: None,
                        initial: AbstractValue::Bool(false),
                    }
                }
            })
            .collect();
    }

    // Sidecar path: include only inputs listed in config
    let mut domains = Vec::new();
    for port in &module.ports {
        if port.direction != PortDirection::Input
            || port.name == "clk"
            || port.name == "rst"
            || port.name == "rst_n"
        {
            continue;
        }
        if let Some(inp_config) = config.input_domains.get(&port.name)
            && inp_config.preserve
        {
            domains.push(inp_config.domain.clone());
        }
    }
    domains
}

/// Collect property signals from config properties (for COI).
fn collect_property_signals_from_config(
    config: &super::annotation::MergedConfig,
) -> HashSet<String> {
    let mut signals = HashSet::new();
    for prop in &config.properties {
        if let Some(formula) = &prop.formula {
            collect_identifiers_from_formula(formula, &mut signals);
        }
    }
    signals
}

/// Extract registers from inline @mununu annotations (original path).
fn extract_registers(module: &Module, warnings: &mut Vec<AdapterWarning>) -> Vec<RegisterInfo> {
    let mut registers = Vec::new();

    // Build annotation lookup
    let domain_map: HashMap<&str, &DomainAnnotationKind> = module
        .domain_annotations
        .iter()
        .map(|a| (a.register_name.as_str(), &a.domain_kind))
        .collect();

    // Determine controllability overrides
    let controllable_set: HashSet<&str> = module
        .controllable_signals
        .iter()
        .map(|s| s.as_str())
        .collect();
    let input_override_set: HashSet<&str> =
        module.input_signals.iter().map(|s| s.as_str()).collect();

    for decl in &module.declarations {
        match decl {
            Declaration::Enum {
                variants,
                var_name: Some(var),
                ..
            } => {
                let kind = classify_signal(var, &controllable_set, &input_override_set, module);
                registers.push(RegisterInfo {
                    name: var.clone(),
                    width: variants.len().next_power_of_two().trailing_zeros() as usize,
                    domain: FieldDomain {
                        name: var.clone(),
                        abstraction: AbstractionType::EnumValues,
                        bound: None,
                        lower_bound: None,
                        variants: Some(variants.clone()),
                        initial: AbstractValue::Variant(
                            variants.first().cloned().unwrap_or_default(),
                        ),
                    },
                    kind,
                    value_map: vec![],
                    combinational: false,
                });
            }
            Declaration::Logic { name, width } => {
                // Skip if this is a port (ports are input signals, not registers)
                if module.ports.iter().any(|p| p.name == *name) {
                    continue;
                }

                let kind = classify_signal(name, &controllable_set, &input_override_set, module);

                let domain = if let Some(ann) = domain_map.get(name.as_str()) {
                    annotation_to_domain(name, ann)
                } else if *width == 1 {
                    FieldDomain {
                        name: name.clone(),
                        abstraction: AbstractionType::Boolean,
                        bound: None,
                        lower_bound: None,
                        variants: None,
                        initial: AbstractValue::Bool(false),
                    }
                } else if *width <= 4 {
                    warnings.push(AdapterWarning {
                        kind: WarningKind::NeutralControllability,
                        message: format!(
                            "Register '{}' ({}-bit) auto-abstracted to bounded_counter 0..{}. \
                             Use // @mununu domain {}: <kind> for explicit control.",
                            name,
                            width,
                            (1i64 << width) - 1,
                            name
                        ),
                        location: None,
                    });
                    FieldDomain {
                        name: name.clone(),
                        abstraction: AbstractionType::BoundedCounter,
                        bound: Some((1i64 << width) - 1),
                        lower_bound: None,
                        variants: None,
                        initial: AbstractValue::Counter(0),
                    }
                } else {
                    warnings.push(AdapterWarning {
                        kind: WarningKind::UnsupportedConstruct,
                        message: format!(
                            "Register '{}' ({}-bit) ignored — too wide for explicit enumeration. \
                             Use // @mununu domain {}: bounded_counter 0..N to abstract.",
                            name, width, name
                        ),
                        location: None,
                    });
                    FieldDomain {
                        name: name.clone(),
                        abstraction: AbstractionType::Ignored,
                        bound: None,
                        lower_bound: None,
                        variants: None,
                        initial: AbstractValue::Counter(0),
                    }
                };

                // Extract value_map from annotation if present
                let value_map = domain_map
                    .get(name.as_str())
                    .and_then(|ann| match ann {
                        DomainAnnotationKind::Enum { value_map, .. } => Some(value_map.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();

                registers.push(RegisterInfo {
                    name: name.clone(),
                    width: *width,
                    domain,
                    kind,
                    value_map,
                    combinational: false,
                });
            }
            _ => {}
        }
    }

    registers
}

fn annotation_to_domain(name: &str, ann: &DomainAnnotationKind) -> FieldDomain {
    match ann {
        DomainAnnotationKind::Boolean => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::Boolean,
            bound: None,
            lower_bound: None,
            variants: None,
            initial: AbstractValue::Bool(false),
        },
        DomainAnnotationKind::BoundedCounter { lower, upper } => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::BoundedCounter,
            bound: Some(*upper),
            lower_bound: None,
            variants: None,
            initial: AbstractValue::Counter(*lower),
        },
        DomainAnnotationKind::Enum { variants, .. } => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::EnumValues,
            bound: None,
            lower_bound: None,
            variants: Some(variants.clone()),
            initial: AbstractValue::Variant(variants.first().cloned().unwrap_or_default()),
        },
        DomainAnnotationKind::Ignored => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::Ignored,
            bound: None,
            lower_bound: None,
            variants: None,
            initial: AbstractValue::Counter(0),
        },
    }
}

/// Classify a SystemVerilog signal as Input / Output / Internal /
/// Neutral. This is the SV-side mapping of the shared port-direction
/// rule from `crate::controllability` (Document A §4); the custom-SV
/// pipeline historically classified into `SignalKind` rather than the
/// CLTS `LabelControllability`, so this thin wrapper preserves the
/// established surface while documenting the connection.
///
/// Override lists are still honoured as escape hatches per
/// Document A §4.ii.
fn classify_signal(
    name: &str,
    controllable: &HashSet<&str>,
    input_override: &HashSet<&str>,
    module: &Module,
) -> SignalKind {
    if controllable.contains(name) {
        return SignalKind::Output;
    }
    if input_override.contains(name) {
        return SignalKind::Input;
    }
    // Port direction at the module boundary — the canonical input to the
    // shared `controllability::classify_from_direction` rule.
    for port in &module.ports {
        if port.name == name {
            return match port.direction {
                PortDirection::Input => SignalKind::Input,
                PortDirection::Output => SignalKind::Output,
                PortDirection::Inout => SignalKind::Neutral,
            };
        }
    }
    SignalKind::Internal
}

// ---------------------------------------------------------------------------
// Cone-of-influence reduction
// ---------------------------------------------------------------------------

/// Extract identifiers from a formula string (heuristic: alphanumeric tokens).
fn collect_identifiers_from_formula(formula: &str, out: &mut HashSet<String>) {
    // Extract identifiers: sequences of [a-zA-Z_][a-zA-Z0-9_]*
    // Skip LTL keywords: G, F, X, U, W, R, nu, mu, true, false
    let keywords: HashSet<&str> = [
        "G", "F", "X", "U", "W", "R", "nu", "mu", "true", "false", "and", "or", "not",
    ]
    .into();

    let mut chars = formula.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_alphabetic() || ch == '_' {
            let mut ident = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() || c == '_' {
                    ident.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if !keywords.contains(ident.as_str()) {
                out.insert(ident);
            }
        } else {
            chars.next();
        }
    }
}

/// Build a dependency graph: for each signal, which other signals does it depend on?
pub fn build_dependency_graph(module: &Module) -> HashMap<String, HashSet<String>> {
    let mut deps: HashMap<String, HashSet<String>> = HashMap::new();

    // From always_ff blocks: register next-state depends on signals in expressions
    for block in &module.always_blocks {
        match block {
            AlwaysBlock::AlwaysFF { body, .. } => {
                collect_statement_deps(body, &mut deps);
            }
            AlwaysBlock::AlwaysComb { body } => {
                collect_statement_deps(body, &mut deps);
            }
        }
    }

    // From continuous assigns
    for assign in &module.assigns {
        let mut expr_deps = HashSet::new();
        collect_expr_idents(&assign.value, &mut expr_deps);
        deps.entry(assign.target.name().to_string())
            .or_default()
            .extend(expr_deps);
    }

    deps
}

fn collect_statement_deps(stmt: &Statement, deps: &mut HashMap<String, HashSet<String>>) {
    match stmt {
        Statement::NonblockingAssign { target, value }
        | Statement::BlockingAssign { target, value } => {
            let mut expr_deps = HashSet::new();
            collect_expr_idents(value, &mut expr_deps);
            deps.entry(target.name().to_string())
                .or_default()
                .extend(expr_deps);
        }
        Statement::If {
            cond,
            then_branch,
            else_branch,
        } => {
            // Condition affects all assignments in branches
            let mut cond_deps = HashSet::new();
            collect_expr_idents(cond, &mut cond_deps);
            // We can't easily know which targets are in branches without walking,
            // so just collect deps from branches
            collect_statement_deps(then_branch, deps);
            if let Some(else_br) = else_branch {
                collect_statement_deps(else_br, deps);
            }
            // Add condition deps to all targets found in branches
            let targets: Vec<String> = collect_assignment_targets(then_branch);
            for target in targets {
                deps.entry(target).or_default().extend(cond_deps.clone());
            }
        }
        Statement::Case {
            selector,
            branches,
            default,
            ..
        } => {
            for branch in branches {
                collect_statement_deps(&branch.body, deps);
                // Selector is a dependency for all targets in branches
                let targets = collect_assignment_targets(&branch.body);
                for target in targets {
                    deps.entry(target).or_default().insert(selector.clone());
                }
            }
            if let Some(d) = default {
                collect_statement_deps(d, deps);
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                collect_statement_deps(s, deps);
            }
        }
    }
}

fn collect_assignment_targets(stmt: &Statement) -> Vec<String> {
    let mut targets = Vec::new();
    match stmt {
        Statement::NonblockingAssign { target, .. } | Statement::BlockingAssign { target, .. } => {
            targets.push(target.name().to_string());
        }
        Statement::If {
            then_branch,
            else_branch,
            ..
        } => {
            targets.extend(collect_assignment_targets(then_branch));
            if let Some(e) = else_branch {
                targets.extend(collect_assignment_targets(e));
            }
        }
        Statement::Case {
            branches, default, ..
        } => {
            for b in branches {
                targets.extend(collect_assignment_targets(&b.body));
            }
            if let Some(d) = default {
                targets.extend(collect_assignment_targets(d));
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                targets.extend(collect_assignment_targets(s));
            }
        }
    }
    targets
}

fn collect_expr_idents(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name) => {
            out.insert(name.clone());
        }
        Expr::Number(_) | Expr::Bool(_) => {}
        Expr::Not(inner) => collect_expr_idents(inner, out),
        Expr::BinOp { left, right, .. } => {
            collect_expr_idents(left, out);
            collect_expr_idents(right, out);
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_expr_idents(cond, out);
            collect_expr_idents(then_expr, out);
            collect_expr_idents(else_expr, out);
        }
        Expr::BitSelect { base, index } => {
            collect_expr_idents(base, out);
            collect_expr_idents(index, out);
        }
        Expr::BitSlice { base, msb, lsb } => {
            collect_expr_idents(base, out);
            collect_expr_idents(msb, out);
            collect_expr_idents(lsb, out);
        }
        Expr::Concat(parts) => {
            for p in parts {
                collect_expr_idents(p, out);
            }
        }
    }
}

/// Compute cone-of-influence: BFS backward from property signals through dependency graph.
fn compute_cone_of_influence(
    seeds: &HashSet<String>,
    deps: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    // Build reverse dependency map: if A depends on B, then B influences A
    // We want: starting from property signals, what registers are needed?
    // But we need the forward closure: property signals + anything they depend on

    let mut relevant = HashSet::new();
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();

    while let Some(signal) = queue.pop_front() {
        if relevant.insert(signal.clone()) {
            // This signal depends on other signals
            if let Some(signal_deps) = deps.get(&signal) {
                for dep in signal_deps {
                    if !relevant.contains(dep) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
    }

    relevant
}

// ---------------------------------------------------------------------------
// Combinational logic evaluation
// ---------------------------------------------------------------------------

/// A combinational assignment: target = expr, with the conjunction of `if` /
/// `case` guards under which the assignment fires. An empty guard list means
/// unconditional — the SV "top-of-`always_comb`" default-assignment idiom.
///
/// SOUNDNESS: collected in source order so that `eval_comb_assigns` applies
/// last-write-wins, matching IEEE 1800 §10.4 procedural-block semantics.
/// Top-of-block defaults appear first with empty guards; case-arm overrides
/// appear later with non-empty guards. When the guards hold, the override
/// wins; when they do not, the earlier default survives. This is what the
/// Caliptra-RTL maintainer relies on in chipsalliance/caliptra-rtl#150.
#[derive(Debug, Clone)]
struct CombAssign {
    target: AssignTarget,
    value: Expr,
    /// Guard conjunction (reused from the always_ff path).
    guards: Vec<SeqGuard>,
}

/// Collect all combinational assignments (assign + always_comb).
fn collect_comb_assigns(module: &Module) -> Vec<CombAssign> {
    let mut assigns = Vec::new();

    for a in &module.assigns {
        assigns.push(CombAssign {
            target: a.target.clone(),
            value: a.value.clone(),
            guards: Vec::new(),
        });
    }

    for block in &module.always_blocks {
        if let AlwaysBlock::AlwaysComb { body } = block {
            collect_comb_from_statement(body, &[], &mut assigns);
        }
    }

    assigns
}

fn collect_comb_from_statement(
    stmt: &Statement,
    guards: &[SeqGuard],
    assigns: &mut Vec<CombAssign>,
) {
    match stmt {
        Statement::BlockingAssign { target, value } => {
            assigns.push(CombAssign {
                target: target.clone(),
                value: value.clone(),
                guards: guards.to_vec(),
            });
        }
        Statement::Block(stmts) => {
            for s in stmts {
                collect_comb_from_statement(s, guards, assigns);
            }
        }
        Statement::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let mut then_guards = guards.to_vec();
            then_guards.push(SeqGuard::If(cond.clone()));
            collect_comb_from_statement(then_branch, &then_guards, assigns);

            if let Some(else_br) = else_branch {
                let mut else_guards = guards.to_vec();
                else_guards.push(SeqGuard::IfNot(cond.clone()));
                collect_comb_from_statement(else_br, &else_guards, assigns);
            }
        }
        Statement::Case {
            selector,
            branches,
            default,
        } => {
            for branch in branches {
                let mut case_guards = guards.to_vec();
                case_guards.push(SeqGuard::CaseEq(selector.clone(), branch.label.clone()));
                collect_comb_from_statement(&branch.body, &case_guards, assigns);
            }
            // SOUNDNESS: a missing `default:` arm is NOT modeled as register-
            // hold here — that is the always_ff path's role. Inside always_comb,
            // when no arm matches and there is no `default:`, the SV-defined
            // behavior is "the top-of-block default assignments stand". Those
            // defaults were already collected by the recursion above, so we
            // simply emit nothing for the no-match path. If a `default:` arm
            // IS present, we collect it under the negated disjunction of the
            // arm guards — the same pattern used by collect_seq_from_statement
            // (line 1703).
            if let Some(d) = default {
                let mut default_guards = guards.to_vec();
                for branch in branches {
                    default_guards.push(SeqGuard::CaseNeq(selector.clone(), branch.label.clone()));
                }
                collect_comb_from_statement(d, &default_guards, assigns);
            }
            // SOUNDNESS: `case` vs `casez` vs `casex` are all matched as exact
            // equality on the selector by guards_satisfied (line ~1809). For
            // binary-literal labels (no `?`/`Z`/`X` wildcards in the label
            // itself) this is sound. For wildcard labels we under-approximate
            // — wildcard arms that should match multiple encodings are matched
            // only on the literal pattern. This is a known limitation; flag it
            // in the adapter's warnings if any case label contains wildcard
            // characters in future work.
        }
        // SOUNDNESS: unhandled statement kinds (e.g., for-loops, function
        // calls) are dropped. This is unsound for comb blocks that use them
        // — the SV adapter parser does not currently emit such statements,
        // so the path is unreachable in practice, but if a new Statement
        // variant is added the compiler's exhaustiveness check will not
        // fire here. Keep this comment in sync with ast::Statement.
        _ => {}
    }
}

/// Evaluate combinational assignments, updating the valuation map.
///
/// SOUNDNESS: applies assignments in source order, last-write-wins, filtering
/// each by its `guards`. This matches IEEE 1800 §10.4 always_comb semantics:
/// top-of-block defaults (empty guards) execute unconditionally, then case-arm
/// overrides execute when their guards hold. When `guards_satisfied` admits an
/// arm whose selector cannot be evaluated (None), we conservatively let the
/// arm fire — over-approx, sound for safety. If `eval_expr` on the RHS returns
/// None we fall back to `Bool(false)` for backward compatibility with the
/// previous behavior; this is conservative for output assertions ("not
/// asserted") but UNSOUND for any signal whose default value is non-zero. The
/// fix is to either default the signal at the top of the block (the Caliptra
/// idiom) or to track signal-specific reset values — preferred direction.
fn eval_comb_assigns(
    assigns: &[CombAssign],
    values: &mut BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) {
    for assign in assigns {
        if !guards_satisfied(&assign.guards, values, registers) {
            continue;
        }
        let result =
            eval_expr(&assign.value, values, registers).unwrap_or(AbstractValue::Bool(false));
        apply_assign_target(&assign.target, result, values);
    }
}

/// Apply a value to an assign target (simple name or bit-slice of a register).
fn apply_assign_target(
    target: &AssignTarget,
    value: AbstractValue,
    values: &mut BTreeMap<String, AbstractValue>,
) {
    match target {
        AssignTarget::Simple(name) => {
            values.insert(name.clone(), value);
        }
        AssignTarget::BitSlice { base, msb, lsb } => {
            let old_val = values
                .get(base)
                .and_then(|v| match v {
                    AbstractValue::Counter(n) => Some(*n),
                    _ => None,
                })
                .unwrap_or(0);
            let new_bits = match &value {
                AbstractValue::Counter(n) => *n,
                AbstractValue::Bool(b) => *b as i64,
                _ => 0,
            };
            let result = write_bit_slice(old_val, new_bits, *msb, *lsb);
            values.insert(base.clone(), AbstractValue::Counter(result));
        }
    }
}

/// Write `new_val` into bits [msb:lsb] of `old_val`, preserving other bits.
fn write_bit_slice(old_val: i64, new_val: i64, msb: usize, lsb: usize) -> i64 {
    let width = msb - lsb + 1;
    let mask = ((1i64 << width) - 1) << lsb;
    (old_val & !mask) | ((new_val << lsb) & mask)
}

/// Evaluate an expression against the current valuation.
/// Public wrapper for expression evaluation (used by multi-module output annotation).
pub fn eval_expr_pub(
    expr: &Expr,
    values: &BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) -> Option<AbstractValue> {
    eval_expr(expr, values, registers)
}

fn eval_expr(
    expr: &Expr,
    values: &BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) -> Option<AbstractValue> {
    match expr {
        Expr::Ident(name) => {
            // First check the valuation map
            if let Some(v) = values.get(name) {
                return Some(v.clone());
            }
            // If not found, it might be an enum variant name (e.g., IDLE, WRITING)
            // Return it as a Variant — clamp_to_domain will validate it
            Some(AbstractValue::Variant(name.clone()))
        }
        Expr::Number(n) => Some(AbstractValue::Counter(*n)),
        Expr::Bool(b) => Some(AbstractValue::Bool(*b)),
        Expr::Not(inner) => {
            let v = eval_expr(inner, values, registers)?;
            match v {
                AbstractValue::Bool(b) => Some(AbstractValue::Bool(!b)),
                AbstractValue::Counter(n) => Some(AbstractValue::Bool(n == 0)),
                _ => None,
            }
        }
        Expr::BinOp { op, left, right } => {
            let lv = eval_expr(left, values, registers)?;
            let rv = eval_expr(right, values, registers)?;
            eval_binop(*op, &lv, &rv, registers)
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            let cv = eval_expr(cond, values, registers)?;
            if is_truthy(&cv) {
                eval_expr(then_expr, values, registers)
            } else {
                eval_expr(else_expr, values, registers)
            }
        }
        // SOUNDNESS: over-approx via guards_satisfied admit-on-None for operand
        // shapes the precision recovery doesn't reach (e.g., abstract
        // index/base in BitSelect). PRECISION (Phase 7): Variant operands are
        // resolved through value_map lookup before falling back to None.
        Expr::BitSelect { base, index } => {
            let bv = resolve_to_counter(eval_expr(base, values, registers)?, registers);
            let iv = resolve_to_counter(eval_expr(index, values, registers)?, registers);
            match (&bv, &iv) {
                (AbstractValue::Counter(base_val), AbstractValue::Counter(idx)) => {
                    let bit = (base_val >> idx) & 1;
                    Some(AbstractValue::Bool(bit != 0))
                }
                _ => None, // abstract operands → cannot compute
            }
        }
        Expr::BitSlice { base, msb, lsb } => {
            let bv = resolve_to_counter(eval_expr(base, values, registers)?, registers);
            let mv = resolve_to_counter(eval_expr(msb, values, registers)?, registers);
            let lv = resolve_to_counter(eval_expr(lsb, values, registers)?, registers);
            match (&bv, &mv, &lv) {
                (
                    AbstractValue::Counter(base_val),
                    AbstractValue::Counter(msb_val),
                    AbstractValue::Counter(lsb_val),
                ) => {
                    let width = msb_val - lsb_val + 1;
                    let mask = (1i64 << width) - 1;
                    let result = (base_val >> lsb_val) & mask;
                    Some(AbstractValue::Counter(result))
                }
                _ => None, // abstract operands → cannot compute
            }
        }
        Expr::Concat(parts) => {
            // Concatenation of counter values — shift and combine
            let mut result: i64 = 0;
            for part in parts {
                let v = resolve_to_counter(eval_expr(part, values, registers)?, registers);
                match v {
                    AbstractValue::Counter(n) => {
                        // Rough: just shift left by 1 bit per part (imprecise for multi-bit)
                        result = (result << 1) | (n & 1);
                    }
                    AbstractValue::Bool(b) => {
                        result = (result << 1) | (b as i64);
                    }
                    // SOUNDNESS: Variant without a unique value_map mapping —
                    // over-approx via guards_satisfied admit-on-None.
                    _ => return None,
                }
            }
            Some(AbstractValue::Counter(result))
        }
    }
}

/// Convert a `Variant` to `Counter` via cross-register `value_map` lookup if
/// possible (Phase 7 precision recovery). Other shapes pass through unchanged.
fn resolve_to_counter(v: AbstractValue, registers: &[RegisterInfo]) -> AbstractValue {
    if let AbstractValue::Variant(name) = &v
        && let Some(num) = lookup_variant_value(name, registers)
    {
        return AbstractValue::Counter(num);
    }
    v
}

fn eval_binop(
    op: BinOp,
    lv: &AbstractValue,
    rv: &AbstractValue,
    registers: &[RegisterInfo],
) -> Option<AbstractValue> {
    match op {
        BinOp::Eq => {
            // PRECISION (Phase 7): cross-resolve mixed Variant/Counter via value_map
            // before falling through to plain equality. This makes
            // `cmd_reg == 3` true when cmd_reg is the Variant "START" with
            // value_map {START: 3}, even if the L208-216 preprocessing didn't apply.
            if let Some((l, r)) = to_i64_pair(lv, rv, registers) {
                return Some(AbstractValue::Bool(l == r));
            }
            Some(AbstractValue::Bool(lv == rv))
        }
        BinOp::Ne => {
            if let Some((l, r)) = to_i64_pair(lv, rv, registers) {
                return Some(AbstractValue::Bool(l != r));
            }
            Some(AbstractValue::Bool(lv != rv))
        }
        BinOp::And => Some(AbstractValue::Bool(is_truthy(lv) && is_truthy(rv))),
        BinOp::Or => Some(AbstractValue::Bool(is_truthy(lv) || is_truthy(rv))),
        BinOp::Lt => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Bool(l < r))
        }
        BinOp::Le => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Bool(l <= r))
        }
        BinOp::Gt => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Bool(l > r))
        }
        BinOp::Ge => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Bool(l >= r))
        }
        BinOp::Add => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l + r))
        }
        BinOp::Sub => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l - r))
        }
        BinOp::Mul => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l * r))
        }
        BinOp::Div => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            if r == 0 {
                None
            } else {
                Some(AbstractValue::Counter(l / r))
            }
        }
        BinOp::Mod => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            if r == 0 {
                None
            } else {
                Some(AbstractValue::Counter(l % r))
            }
        }
        BinOp::Shl => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l << r.min(63)))
        }
        BinOp::Shr => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l >> r.min(63)))
        }
        BinOp::BitOr => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l | r))
        }
        BinOp::BitXor => {
            // SOUNDNESS: like BitOr/BitAnd, the result is not masked to the
            // declared source width — caller must keep operands within the
            // bounded_counter range. The OOB-sink mechanic catches escapees.
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l ^ r))
        }
        BinOp::BitAnd => {
            let (l, r) = to_i64_pair(lv, rv, registers)?;
            Some(AbstractValue::Counter(l & r))
        }
    }
}

fn is_truthy(v: &AbstractValue) -> bool {
    match v {
        AbstractValue::Bool(b) => *b,
        AbstractValue::Counter(n) => *n != 0,
        AbstractValue::Present(p) => *p,
        AbstractValue::Variant(_) => true,
    }
}

/// Search every register's `value_map` for `variant`, returning the numeric
/// value if exactly one mapping is found.
///
/// Phase 7 precision recovery: when a `Variant` flows into `to_i64_pair`,
/// `BitSelect`, or `Concat` and there is a unique register whose value_map
/// names the variant, the numeric value can be substituted, recovering the
/// concrete comparison instead of falling back to None (which would force B5
/// to admit a phantom transition). Returns None if the variant is not in any
/// register's value_map, or if multiple registers map it to different values
/// (ambiguous — fall back to over-approx).
fn lookup_variant_value(variant: &str, registers: &[RegisterInfo]) -> Option<i64> {
    let mut found: Option<i64> = None;
    for reg in registers {
        for (name, val) in &reg.value_map {
            if name == variant {
                match found {
                    Some(prev) if prev != *val => return None,
                    Some(_) => {}
                    None => found = Some(*val),
                }
            }
        }
    }
    found
}

fn to_i64_pair(
    lv: &AbstractValue,
    rv: &AbstractValue,
    registers: &[RegisterInfo],
) -> Option<(i64, i64)> {
    let l = match lv {
        AbstractValue::Counter(n) => *n,
        AbstractValue::Bool(b) => *b as i64,
        // PRECISION (Phase 7): try the cross-register value_map lookup before
        // falling back. This recovers concrete comparisons for Variants whose
        // value_map preprocessing didn't apply (e.g., variants computed by
        // combinational logic after L208-216 ran).
        AbstractValue::Variant(name) => lookup_variant_value(name, registers)?,
        // SOUNDNESS: over-approx via guards_satisfied admit-on-None. Present
        // values have no numeric coercion; the guard at L1556 admits the
        // transition unconditionally, preserving safety.
        _ => return None,
    };
    let r = match rv {
        AbstractValue::Counter(n) => *n,
        AbstractValue::Bool(b) => *b as i64,
        AbstractValue::Variant(name) => lookup_variant_value(name, registers)?,
        _ => return None,
    };
    Some((l, r))
}

// ---------------------------------------------------------------------------
// Sequential logic (next-state computation)
// ---------------------------------------------------------------------------

/// A sequential assignment from always_ff: target <= expr (nonblocking).
#[derive(Debug, Clone)]
struct SeqAssign {
    target: AssignTarget,
    value: Expr,
    /// Guard conditions (from enclosing if/case)
    guards: Vec<SeqGuard>,
}

#[derive(Debug, Clone)]
enum SeqGuard {
    If(Expr),
    IfNot(Expr),
    CaseEq(String, String),  // (selector, label) — selector == label
    CaseNeq(String, String), // (selector, label) — selector != label (for default)
}

/// Collect sequential assignments from always_ff blocks (non-reset branch).
fn collect_seq_assigns(module: &Module) -> Vec<SeqAssign> {
    let mut assigns = Vec::new();

    for block in &module.always_blocks {
        if let AlwaysBlock::AlwaysFF { body, .. } = block {
            // Find the else branch (non-reset) of the if (rst) pattern
            let active_body = find_non_reset_body(body);
            collect_seq_from_statement(active_body, &[], &mut assigns);
        }
    }

    assigns
}

fn find_non_reset_body(stmt: &Statement) -> &Statement {
    match stmt {
        Statement::If {
            else_branch: Some(else_br),
            ..
        } => else_br,
        Statement::Block(stmts) => {
            for s in stmts {
                if let Statement::If {
                    else_branch: Some(else_br),
                    ..
                } = s
                {
                    return else_br;
                }
            }
            stmt
        }
        _ => stmt,
    }
}

fn collect_seq_from_statement(stmt: &Statement, guards: &[SeqGuard], assigns: &mut Vec<SeqAssign>) {
    match stmt {
        Statement::NonblockingAssign { target, value } => {
            assigns.push(SeqAssign {
                target: target.clone(),
                value: value.clone(),
                guards: guards.to_vec(),
            });
        }
        Statement::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let mut then_guards = guards.to_vec();
            then_guards.push(SeqGuard::If(cond.clone()));
            collect_seq_from_statement(then_branch, &then_guards, assigns);

            if let Some(else_br) = else_branch {
                let mut else_guards = guards.to_vec();
                else_guards.push(SeqGuard::IfNot(cond.clone()));
                collect_seq_from_statement(else_br, &else_guards, assigns);
            }
        }
        Statement::Case {
            selector,
            branches,
            default,
            ..
        } => {
            for branch in branches {
                let mut case_guards = guards.to_vec();
                case_guards.push(SeqGuard::CaseEq(selector.clone(), branch.label.clone()));
                collect_seq_from_statement(&branch.body, &case_guards, assigns);
            }
            if let Some(d) = default {
                // Default case: guard that selector is NOT any explicit label
                let mut default_guards = guards.to_vec();
                for branch in branches {
                    default_guards.push(SeqGuard::CaseNeq(selector.clone(), branch.label.clone()));
                }
                collect_seq_from_statement(d, &default_guards, assigns);
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                collect_seq_from_statement(s, guards, assigns);
            }
        }
        _ => {}
    }
}

/// Compute the next register state from the current full valuation.
///
/// Returns `(next_state, overflows)`. A non-empty `overflows` vec means at
/// least one register would have escaped its abstracted domain at this step;
/// the caller routes the transition to the OOB sink.
///
/// `reg_state` is the pre-injection register valuation (Variants for enum
/// registers). `full_val` is the post-injection valuation used for expression
/// evaluation (Variants replaced by Counters via value_map).
fn compute_next_state(
    seq_assigns: &[SeqAssign],
    comb_assigns: &[CombAssign],
    reg_state: &AbstractState,
    full_val: &BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) -> (AbstractState, Vec<OverflowInfo>) {
    let mut next: AbstractState = reg_state.clone();
    let mut overflows: Vec<OverflowInfo> = Vec::new();

    let ignored: HashSet<&str> = registers
        .iter()
        .filter(|r| r.domain.abstraction == AbstractionType::Ignored)
        .map(|r| r.name.as_str())
        .collect();
    let comb_signals: HashSet<&str> = registers
        .iter()
        .filter(|r| r.combinational)
        .map(|r| r.name.as_str())
        .collect();

    for assign in seq_assigns {
        let target_name = assign.target.name();
        if ignored.contains(target_name) || comb_signals.contains(target_name) {
            continue;
        }
        if guards_satisfied(&assign.guards, full_val, registers)
            && let Some(result) = eval_expr(&assign.value, full_val, registers)
        {
            match &assign.target {
                AssignTarget::Simple(name) => {
                    let (clamped, info) = clamp_to_domain(name, &result, registers);
                    if info.is_overflow() {
                        overflows.push(info);
                    }
                    next.insert(name.clone(), clamped);
                }
                AssignTarget::BitSlice { .. } => {
                    apply_assign_target(&assign.target, result, &mut next);
                }
            }
        }
    }

    if !comb_signals.is_empty() {
        let mut next_val = full_val.clone();
        for (k, v) in &next {
            next_val.insert(k.clone(), v.clone());
        }
        eval_comb_assigns(comb_assigns, &mut next_val, registers);

        for reg in registers {
            if reg.combinational && reg.domain.abstraction != AbstractionType::Ignored {
                if let Some(val) = next_val.get(&reg.name) {
                    let (clamped, info) = clamp_to_domain(&reg.name, val, registers);
                    if info.is_overflow() {
                        overflows.push(info);
                    }
                    next.insert(reg.name.clone(), clamped);
                } else {
                    // Comb evaluation returned None (e.g., comparison with unknown addr).
                    // Default to initial value (false for booleans).
                    next.insert(reg.name.clone(), reg.domain.initial.clone());
                }
            }
        }
    }

    (next, overflows)
}

// SOUNDNESS: over-approx — when eval_expr returns None (abstract operands), the
// guard is treated as possibly-true and the transition is admitted. This adds
// phantom transitions but never removes real ones. Sound for safety (the
// abstract model contains every real behavior, so any safety violation found is
// also a real violation, modulo the abstraction). UNSOUND for liveness (admitted
// phantoms can produce spurious progress witnesses that do not exist in the
// concrete system). Reference: Huth–Jagadeesan–Schmidt ESOP 2001 (modal
// transition systems), Bruns–Godefroid CONCUR 2000 (generalized model checking).
fn guards_satisfied(
    guards: &[SeqGuard],
    values: &BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) -> bool {
    guards.iter().all(|g| match g {
        SeqGuard::If(expr) => match eval_expr(expr, values, registers) {
            Some(v) => is_truthy(&v),
            None => true,
        },
        SeqGuard::IfNot(expr) => match eval_expr(expr, values, registers) {
            Some(v) => !is_truthy(&v),
            None => true,
        },
        SeqGuard::CaseEq(selector, label) => {
            values.get(selector).is_some_and(|v| match v {
                AbstractValue::Variant(s) => {
                    // Direct name match
                    if s == label {
                        return true;
                    }
                    // Value-map match: label is numeric, check if variant maps to it
                    if let Ok(n) = label.parse::<i64>()
                        && let Some(reg) = registers.iter().find(|r| r.name == *selector)
                    {
                        return reg
                            .value_map
                            .iter()
                            .any(|(name, val)| name == s && *val == n);
                    }
                    false
                }
                AbstractValue::Counter(n) => {
                    // Try numeric label first
                    if let Ok(l) = label.parse::<i64>() {
                        return *n == l;
                    }
                    // Label is an enum name — resolve via value_map
                    if let Some(reg) = registers.iter().find(|r| r.name == *selector)
                        && let Some((_, val)) = reg.value_map.iter().find(|(name, _)| name == label)
                    {
                        return *n == *val;
                    }
                    false
                }
                AbstractValue::Bool(b) => (*b && label == "1") || (!b && label == "0"),
                _ => false,
            })
        }
        SeqGuard::CaseNeq(selector, label) => {
            // Negated case match — for default branch
            values.get(selector).is_none_or(|v| match v {
                AbstractValue::Variant(s) => {
                    if s == label {
                        return false;
                    }
                    if let Ok(n) = label.parse::<i64>()
                        && let Some(reg) = registers.iter().find(|r| r.name == *selector)
                    {
                        return !reg
                            .value_map
                            .iter()
                            .any(|(name, val)| name == s && *val == n);
                    }
                    true
                }
                AbstractValue::Counter(n) => {
                    if let Ok(l) = label.parse::<i64>() {
                        return *n != l;
                    }
                    if let Some(reg) = registers.iter().find(|r| r.name == *selector)
                        && let Some((_, val)) = reg.value_map.iter().find(|(name, _)| name == label)
                    {
                        return *n != *val;
                    }
                    true
                }
                AbstractValue::Bool(b) => !(*b && label == "1") && (*b || label != "0"),
                _ => true,
            })
        }
    })
}

/// Clamp an abstract value to fit the register's declared domain, returning the
/// clamped value plus an `OverflowInfo` flag describing whether the input value
/// would have escaped the abstracted domain.
///
/// SOUNDNESS: when the input value escapes the domain, the clamped result is
/// still in-domain and downstream code can use it; the OverflowInfo flag tells
/// the caller (typically `compute_next_state`) to also emit an OOB transition
/// so the abstract model conservatively records that "anything could happen"
/// from this point. Without the OOB transition, clamping would be a silent
/// under-approximation: the abstract model would freeze at the bound while the
/// concrete system continues. Reference: Bruns–Godefroid CONCUR 2000 (sound
/// fix via partial-state OOB sink).
///
/// In-domain cases:
/// - Boolean coercion (Counter → Bool, Bool → Bool): no overflow concept.
/// - Enum variant assignment with a known variant: in-domain.
/// - Enum variant assignment with an unknown variant: collapsed to catch-all
///   (over-approx, sound for safety) — still in-domain.
/// - Enum from Counter via value_map (mapped or fallback to catch-all): in-domain.
fn clamp_to_domain(
    target: &str,
    value: &AbstractValue,
    registers: &[RegisterInfo],
) -> (AbstractValue, OverflowInfo) {
    if let Some(reg) = registers.iter().find(|r| r.name == target) {
        match (&reg.domain.abstraction, value) {
            (AbstractionType::BoundedCounter, AbstractValue::Counter(n)) => {
                let bound = reg
                    .domain
                    .bound
                    .unwrap_or(crate::adapter::domain::DEFAULT_COUNTER_BOUND);
                let clamped = (*n).max(0).min(bound);
                let info = if *n > bound {
                    OverflowInfo::CounterAbove {
                        register: reg.name.clone(),
                        value: *n,
                        bound,
                    }
                } else {
                    OverflowInfo::InDomain
                };
                (AbstractValue::Counter(clamped), info)
            }
            (AbstractionType::Boolean, AbstractValue::Counter(n)) => {
                (AbstractValue::Bool(*n != 0), OverflowInfo::InDomain)
            }
            (AbstractionType::Boolean, AbstractValue::Bool(_)) => {
                (value.clone(), OverflowInfo::InDomain)
            }
            (AbstractionType::EnumValues, AbstractValue::Variant(name)) => {
                if let Some(variants) = &reg.domain.variants {
                    if variants.contains(name) {
                        return (value.clone(), OverflowInfo::InDomain);
                    }
                    // SOUNDNESS: over-approx — unknown variant collapsed to catch-all.
                    // Sound for safety (extra variant behaviors are conservative).
                    if let Some(last) = variants.last() {
                        return (AbstractValue::Variant(last.clone()), OverflowInfo::InDomain);
                    }
                }
                (value.clone(), OverflowInfo::InDomain)
            }
            (AbstractionType::EnumValues, AbstractValue::Counter(n)) => {
                if !reg.value_map.is_empty() {
                    if let Some((variant_name, _)) = reg.value_map.iter().find(|(_, val)| val == n)
                    {
                        return (
                            AbstractValue::Variant(variant_name.clone()),
                            OverflowInfo::InDomain,
                        );
                    }
                    // Not in map → catch-all (last variant without a mapping)
                    if let Some(variants) = &reg.domain.variants {
                        let mapped_names: HashSet<&str> = reg
                            .value_map
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect();
                        if let Some(catchall) =
                            variants.iter().find(|v| !mapped_names.contains(v.as_str()))
                        {
                            return (
                                AbstractValue::Variant(catchall.clone()),
                                OverflowInfo::InDomain,
                            );
                        }
                    }
                    (value.clone(), OverflowInfo::InDomain)
                } else if let Some(variants) = &reg.domain.variants {
                    let idx = *n as usize;
                    if idx < variants.len() {
                        (
                            AbstractValue::Variant(variants[idx].clone()),
                            OverflowInfo::InDomain,
                        )
                    } else {
                        // SOUNDNESS (formerly unsound silent fall-through): index
                        // outside enum domain now flags overflow, routing the
                        // transition to OOB. Sound for safety.
                        (
                            value.clone(),
                            OverflowInfo::EnumIndexOutOfRange {
                                register: reg.name.clone(),
                                index: *n,
                                variant_count: variants.len(),
                            },
                        )
                    }
                } else {
                    (value.clone(), OverflowInfo::InDomain)
                }
            }
            _ => (value.clone(), OverflowInfo::InDomain),
        }
    } else {
        (value.clone(), OverflowInfo::InDomain)
    }
}

/// Build individual labels for each input signal in a combination.
///
/// Each input signal gets its own label (e.g., `rd_en_T`, `wr_en_F`),
/// and transitions carry all of them as a multi-label set. This produces
/// CTXDSL like `transition A -> B on label rd_en_T, label wr_en_F;`
/// which is more readable than a single concatenated label.
fn make_input_labels(
    input_combo: &AbstractState,
    label_overrides: &HashMap<String, String>,
) -> Vec<String> {
    if input_combo.is_empty() {
        return vec!["tick".to_string()];
    }
    input_combo
        .iter()
        .map(|(k, v)| {
            let label_prefix = label_overrides.get(k).unwrap_or(k);
            format!("{}_{}", label_prefix, v.display_short())
        })
        .collect()
}

/// Extract initial state from reset branch.
fn extract_initial_state(module: &Module, registers: &[RegisterInfo]) -> AbstractState {
    let mut initial: AbstractState = registers
        .iter()
        .filter(|r| r.domain.abstraction != AbstractionType::Ignored)
        .map(|r| (r.name.clone(), r.domain.initial.clone()))
        .collect();

    // Override with reset values from always_ff
    for block in &module.always_blocks {
        if let AlwaysBlock::AlwaysFF {
            reset: Some(reset), ..
        } = block
        {
            for (target, value_str) in &reset.assignments {
                if let Some(reg) = registers.iter().find(|r| r.name == *target) {
                    if reg.domain.abstraction == AbstractionType::Ignored {
                        continue;
                    }
                    let value = parse_reset_value(value_str, reg);
                    initial.insert(target.clone(), value);
                }
            }
        }
    }

    initial
}

fn parse_reset_value(value_str: &str, reg: &RegisterInfo) -> AbstractValue {
    // Try as enum variant first
    if let Some(variants) = &reg.domain.variants
        && variants.contains(&value_str.to_string())
    {
        return AbstractValue::Variant(value_str.to_string());
    }
    // Try as number
    if let Ok(n) = value_str.parse::<i64>() {
        match reg.domain.abstraction {
            AbstractionType::Boolean => return AbstractValue::Bool(n != 0),
            AbstractionType::BoundedCounter => {
                let bound = reg
                    .domain
                    .bound
                    .unwrap_or(crate::adapter::domain::DEFAULT_COUNTER_BOUND);
                return AbstractValue::Counter(n.max(0).min(bound));
            }
            _ => return AbstractValue::Counter(n),
        }
    }
    // '0 or '1 patterns
    if value_str.starts_with('\'') {
        return match reg.domain.abstraction {
            AbstractionType::Boolean => AbstractValue::Bool(false),
            _ => AbstractValue::Counter(0),
        };
    }
    // Default to domain initial
    reg.domain.initial.clone()
}

// ---------------------------------------------------------------------------
// Significant constant scanning
// ---------------------------------------------------------------------------

/// Scan the module for significant constants used with each register/signal.
///
/// Looks for patterns like `reg == CONST`, `reg != CONST`, `reg < CONST`,
/// `case(reg) CONST: ...` and collects the constant set per register.
pub fn scan_significant_constants(module: &Module) -> HashMap<String, Vec<i64>> {
    let mut constants: HashMap<String, HashSet<i64>> = HashMap::new();

    // Scan always_ff and always_comb bodies
    for block in &module.always_blocks {
        match block {
            AlwaysBlock::AlwaysFF { body, .. } => {
                scan_stmt_constants(body, &mut constants);
            }
            AlwaysBlock::AlwaysComb { body } => {
                scan_stmt_constants(body, &mut constants);
            }
        }
    }

    // Scan continuous assigns
    for assign in &module.assigns {
        scan_expr_constants(&assign.value, &mut constants);
    }

    // Convert to sorted vecs
    constants
        .into_iter()
        .map(|(k, v)| {
            let mut sorted: Vec<i64> = v.into_iter().collect();
            sorted.sort();
            (k, sorted)
        })
        .collect()
}

fn scan_stmt_constants(stmt: &Statement, constants: &mut HashMap<String, HashSet<i64>>) {
    match stmt {
        Statement::NonblockingAssign { value, .. } | Statement::BlockingAssign { value, .. } => {
            scan_expr_constants(value, constants);
        }
        Statement::If {
            cond,
            then_branch,
            else_branch,
        } => {
            scan_expr_constants(cond, constants);
            scan_stmt_constants(then_branch, constants);
            if let Some(e) = else_branch {
                scan_stmt_constants(e, constants);
            }
        }
        Statement::Case {
            selector,
            branches,
            default,
        } => {
            // Case labels are significant constants for the selector
            for branch in branches {
                if let Ok(n) = branch.label.parse::<i64>() {
                    constants.entry(selector.clone()).or_default().insert(n);
                }
            }
            for branch in branches {
                scan_stmt_constants(&branch.body, constants);
            }
            if let Some(d) = default {
                scan_stmt_constants(d, constants);
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                scan_stmt_constants(s, constants);
            }
        }
    }
}

fn scan_expr_constants(expr: &Expr, constants: &mut HashMap<String, HashSet<i64>>) {
    match expr {
        // Pattern: ident == number or ident != number, etc.
        Expr::BinOp {
            op: BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
            left,
            right,
        } => {
            match (left.as_ref(), right.as_ref()) {
                (Expr::Ident(name), Expr::Number(n)) => {
                    constants.entry(name.clone()).or_default().insert(*n);
                }
                (Expr::Number(n), Expr::Ident(name)) => {
                    constants.entry(name.clone()).or_default().insert(*n);
                }
                _ => {}
            }
            scan_expr_constants(left, constants);
            scan_expr_constants(right, constants);
        }
        Expr::BinOp { left, right, .. } => {
            scan_expr_constants(left, constants);
            scan_expr_constants(right, constants);
        }
        Expr::Not(inner) => scan_expr_constants(inner, constants),
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            scan_expr_constants(cond, constants);
            scan_expr_constants(then_expr, constants);
            scan_expr_constants(else_expr, constants);
        }
        Expr::BitSelect { base, index } => {
            scan_expr_constants(base, constants);
            scan_expr_constants(index, constants);
        }
        Expr::BitSlice { base, msb, lsb } => {
            scan_expr_constants(base, constants);
            scan_expr_constants(msb, constants);
            scan_expr_constants(lsb, constants);
        }
        Expr::Concat(parts) => {
            for p in parts {
                scan_expr_constants(p, constants);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// BFS reachability
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::systemverilog::parser;

    #[test]
    fn extract_registers_from_enum() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                typedef enum logic [1:0] {IDLE, WAIT, DONE} state_t;
                state_t state;
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let regs = extract_registers(&module, &mut warnings);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "state");
        assert_eq!(regs[0].domain.abstraction, AbstractionType::EnumValues);
        assert_eq!(regs[0].domain.cardinality(), 3);
    }

    #[test]
    fn extract_registers_with_annotation() {
        let module = parser::parse(
            r#"// @mununu domain counter: bounded_counter 0..3
            module test(input logic clk, input logic rst);
                logic [7:0] counter;
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let regs = extract_registers(&module, &mut warnings);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].name, "counter");
        assert_eq!(regs[0].domain.abstraction, AbstractionType::BoundedCounter);
        assert_eq!(regs[0].domain.bound, Some(3));
        assert_eq!(regs[0].domain.cardinality(), 4);
    }

    #[test]
    fn extract_registers_wide_ignored() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [31:0] data;
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let regs = extract_registers(&module, &mut warnings);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].domain.abstraction, AbstractionType::Ignored);
        assert!(warnings.iter().any(|w| w.message.contains("ignored")));
    }

    #[test]
    fn extract_registers_auto_boolean() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic valid;
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let regs = extract_registers(&module, &mut warnings);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].domain.abstraction, AbstractionType::Boolean);
    }

    #[test]
    fn eval_expr_arithmetic() {
        let mut values = BTreeMap::new();
        values.insert("a".to_string(), AbstractValue::Counter(3));
        values.insert("b".to_string(), AbstractValue::Counter(2));

        let expr = Expr::BinOp {
            op: BinOp::Add,
            left: Box::new(Expr::Ident("a".to_string())),
            right: Box::new(Expr::Ident("b".to_string())),
        };
        let result = eval_expr(&expr, &values, &[]);
        assert_eq!(result, Some(AbstractValue::Counter(5)));
    }

    #[test]
    fn eval_expr_ternary() {
        let mut values = BTreeMap::new();
        values.insert("flag".to_string(), AbstractValue::Bool(true));

        let expr = Expr::Ternary {
            cond: Box::new(Expr::Ident("flag".to_string())),
            then_expr: Box::new(Expr::Number(42)),
            else_expr: Box::new(Expr::Number(0)),
        };
        let result = eval_expr(&expr, &values, &[]);
        assert_eq!(result, Some(AbstractValue::Counter(42)));
    }

    #[test]
    fn eval_expr_comparison() {
        let mut values = BTreeMap::new();
        values.insert("x".to_string(), AbstractValue::Counter(3));

        let expr = Expr::BinOp {
            op: BinOp::Lt,
            left: Box::new(Expr::Ident("x".to_string())),
            right: Box::new(Expr::Number(5)),
        };
        let result = eval_expr(&expr, &values, &[]);
        assert_eq!(result, Some(AbstractValue::Bool(true)));
    }

    #[test]
    fn cone_of_influence_reduces_state_space() {
        // Property references "state" but not "data"
        let mut seeds = HashSet::new();
        seeds.insert("state".to_string());

        let mut deps = HashMap::new();
        deps.insert("state".to_string(), {
            let mut s = HashSet::new();
            s.insert("req".to_string());
            s
        });
        deps.insert("data".to_string(), {
            let mut s = HashSet::new();
            s.insert("payload".to_string());
            s
        });

        let relevant = compute_cone_of_influence(&seeds, &deps);
        assert!(relevant.contains("state"));
        assert!(relevant.contains("req"));
        assert!(!relevant.contains("data"));
        assert!(!relevant.contains("payload"));
    }

    #[test]
    fn build_kripke_two_bit_counter() {
        let module = parser::parse(
            r#"// @mununu ltl bounded: nu X. ([] X)
            // @mununu mode kripke
            module counter(input logic clk, input logic rst, input logic en);
                logic [1:0] count;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) count <= 0;
                    else if (en) count <= count + 1;
                end
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let (automaton, properties, _state_count) = build_kripke(&module, &mut warnings).unwrap();

        // 2-bit counter auto-abstracted to bounded_counter 0..3 → 4 values,
        // plus 1 OOB sink state for the count=3 + en overflow path.
        // (The unguarded `count <= count + 1` produces 4 → out-of-domain, now
        // routed to the OOB sink instead of silently dropped.)
        assert!(automaton.states.len() <= 5);
        assert!(!automaton.transitions.is_empty());
        assert_eq!(properties.len(), 1);

        // Initial state should exist
        assert!(automaton.states.iter().any(|s| s.is_initial));
    }

    // ---------------------------------------------------------------
    // Phase 7: Constant discovery and value maps
    // ---------------------------------------------------------------

    #[test]
    fn scan_constants_from_comparisons() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [7:0] cmd;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) cmd <= 0;
                    else if (cmd == 3) cmd <= 0;
                    else if (cmd == 255) cmd <= 3;
                end
            endmodule"#,
        )
        .unwrap();

        let constants = scan_significant_constants(&module);
        let cmd_consts = constants.get("cmd").expect("cmd should have constants");
        assert!(cmd_consts.contains(&3));
        assert!(cmd_consts.contains(&255));
    }

    #[test]
    fn scan_constants_from_case() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                typedef enum logic [1:0] {IDLE, WAIT, DONE} state_t;
                state_t state;
                logic [7:0] mode;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) begin state <= IDLE; mode <= 0; end
                    else case (state)
                        IDLE: if (mode == 5) state <= WAIT;
                        WAIT: state <= DONE;
                        DONE: state <= IDLE;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();

        let constants = scan_significant_constants(&module);
        // mode has constant 5 from the comparison
        let mode_consts = constants.get("mode").expect("mode should have constants");
        assert!(mode_consts.contains(&5));
    }

    #[test]
    fn constant_suggestion_warning_for_wide_ignored_register() {
        let module = parser::parse(
            r#"// @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            module test(input logic clk, input logic rst);
                logic [31:0] cmd;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) cmd <= 0;
                    else if (cmd == 42) cmd <= 0;
                end
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let _result = build_kripke(&module, &mut warnings);
        // Should have a suggestion warning for cmd with constant 42
        let suggestion = warnings
            .iter()
            .find(|w| w.message.contains("significant constants") && w.message.contains("42"));
        assert!(
            suggestion.is_some(),
            "Should suggest value map for cmd with constant 42"
        );
    }

    #[test]
    fn parse_enum_with_value_map() {
        let module = parser::parse(
            r#"// @mununu domain cmd: enum {IDLE=0, START=3, STOP=255, OTHER}
            module test(input logic clk);
                logic [7:0] cmd;
            endmodule"#,
        )
        .unwrap();

        assert_eq!(module.domain_annotations.len(), 1);
        let ann = &module.domain_annotations[0];
        assert_eq!(ann.register_name, "cmd");
        match &ann.domain_kind {
            DomainAnnotationKind::Enum {
                variants,
                value_map,
            } => {
                assert_eq!(variants, &["IDLE", "START", "STOP", "OTHER"]);
                assert_eq!(value_map.len(), 3);
                assert!(value_map.contains(&("IDLE".to_string(), 0)));
                assert!(value_map.contains(&("START".to_string(), 3)));
                assert!(value_map.contains(&("STOP".to_string(), 255)));
            }
            _ => panic!("expected Enum with value_map"),
        }
    }

    #[test]
    fn kripke_with_value_mapped_enum() {
        let module = parser::parse(
            r#"// @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            // @mununu domain cmd: enum {IDLE=0, START=3, STOP=255, OTHER}
            module test(input logic clk, input logic rst, input logic go);
                logic [7:0] cmd;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) cmd <= 0;
                    else if (go) cmd <= 3;
                end
            endmodule"#,
        )
        .unwrap();

        let mut warnings = Vec::new();
        let (automaton, _, _) = build_kripke(&module, &mut warnings).unwrap();

        // Should have 4 variants but reachability prunes to those reachable from IDLE
        // IDLE (cmd=0) → go → START (cmd=3), and IDLE → !go → IDLE
        assert!(automaton.states.len() <= 4);
        assert!(automaton.states.len() >= 2); // at least IDLE and START reachable
        assert!(automaton.states.iter().any(|s| s.is_initial));
    }
}
