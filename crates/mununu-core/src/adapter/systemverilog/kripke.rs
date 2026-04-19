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
use crate::adapter::{AdapterError, AdapterErrorKind, AdapterWarning, WarningKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Information about a register extracted from the module.
#[derive(Debug, Clone)]
pub struct RegisterInfo {
    pub name: String,
    pub width: usize,
    pub domain: FieldDomain,
    pub kind: SignalKind,
    /// Concrete value → variant name mapping (from `enum {IDLE=0, START=3, OTHER}`).
    /// Used to translate between numeric values and enum variants during evaluation.
    pub value_map: Vec<(String, i64)>,
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

    let all_reg_states = enumerate_cross_product(&reg_fields);
    let all_input_combos = enumerate_cross_product(&input_domains.iter().collect::<Vec<_>>());

    // Step 6: Determine initial state from reset values
    let initial_state = extract_initial_state(module, &registers);

    // Step 7: Build transitions by evaluating logic
    let comb_assigns = collect_comb_assigns(module);
    let seq_assigns = collect_seq_assigns(module);

    let mut transitions = Vec::new();
    let mut state_names: HashMap<AbstractState, String> = HashMap::new();

    for reg_state in &all_reg_states {
        let src_name = make_state_name(reg_state);
        state_names.insert(reg_state.clone(), src_name.clone());
    }

    // Build reverse value map: for registers AND annotated input ports with value_map
    let mut variant_to_numeric: HashMap<String, HashMap<String, i64>> = registers
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
                variant_to_numeric.insert(name.clone(), map);
            }
        }
        for (name, sig_config) in &config.signal_domains {
            if !sig_config.value_map.is_empty() && !variant_to_numeric.contains_key(name) {
                let map: HashMap<String, i64> = sig_config.value_map.iter().cloned().collect();
                variant_to_numeric.insert(name.clone(), map);
            }
        }
    } else {
        // Inline path: use value maps from module annotations
        for ann in &module.domain_annotations {
            if let DomainAnnotationKind::Enum { value_map, .. } = &ann.domain_kind
                && !value_map.is_empty()
                && !variant_to_numeric.contains_key(&ann.register_name)
            {
                let map: HashMap<String, i64> = value_map.iter().cloned().collect();
                variant_to_numeric.insert(ann.register_name.clone(), map);
            }
        }
    }

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

    for reg_state in &all_reg_states {
        let src_name = &state_names[reg_state];
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
            eval_comb_assigns(&comb_assigns, &mut full_val);

            // Compute next register state
            let next_state = compute_next_state(&seq_assigns, reg_state, &full_val, &registers);

            // Find or create the target state name
            if let Some(tgt_name) = state_names.get(&next_state) {
                let labels = make_input_labels(input_combo);
                transitions.push(TransitionSpec {
                    source: src_name.clone(),
                    target: tgt_name.clone(),
                    labels,
                });
            }
            // If target is outside domain (e.g., counter overflow), transition is dropped
        }
    }

    // Step 8: Prune unreachable states
    let initial_name = state_names.get(&initial_state).cloned().unwrap_or_else(|| {
        // If initial state isn't in the enumeration, use first state
        all_reg_states
            .first()
            .map(|s| state_names[s].clone())
            .unwrap_or_else(|| "s0".to_string())
    });

    let reachable = bfs_reachable(&initial_name, &transitions);

    let states: Vec<StateSpec> = all_reg_states
        .iter()
        .filter_map(|s| {
            let name = &state_names[s];
            if reachable.contains(name) {
                Some(StateSpec {
                    name: name.clone(),
                    is_initial: *name == initial_name,
                })
            } else {
                None
            }
        })
        .collect();

    let transitions: Vec<TransitionSpec> = transitions
        .into_iter()
        .filter(|t| reachable.contains(&t.source) && reachable.contains(&t.target))
        .collect();

    // Step 9: Classify labels by controllability
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

    let automaton = AutomatonSpec {
        name: module.name.clone(),
        states,
        transitions,
        controllable_labels,
        internal_labels: vec![],
    };

    // Step 10: Build properties from config
    let properties: Vec<PropertySpec> = config
        .properties
        .iter()
        .map(|p| {
            let role = match p.role.as_str() {
                "assumption" => PropertyRole::Assumption,
                "standalone" => PropertyRole::Standalone,
                _ => PropertyRole::Guarantee,
            };
            PropertySpec {
                name: p.id.clone(),
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus(p.formula.clone()),
                role,
                over: None,
            }
        })
        .collect();

    let state_count = automaton.states.len();
    Ok((automaton, properties, state_count))
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
        return extract_registers(module, warnings);
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
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                kind: SignalKind::Internal,
                value_map: vec![],
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
        collect_identifiers_from_formula(&prop.formula, &mut signals);
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
                        variants: Some(variants.clone()),
                        initial: AbstractValue::Variant(
                            variants.first().cloned().unwrap_or_default(),
                        ),
                    },
                    kind,
                    value_map: vec![],
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
            variants: None,
            initial: AbstractValue::Bool(false),
        },
        DomainAnnotationKind::BoundedCounter { lower, upper } => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::BoundedCounter,
            bound: Some(*upper),
            variants: None,
            initial: AbstractValue::Counter(*lower),
        },
        DomainAnnotationKind::Enum { variants, .. } => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::EnumValues,
            bound: None,
            variants: Some(variants.clone()),
            initial: AbstractValue::Variant(variants.first().cloned().unwrap_or_default()),
        },
        DomainAnnotationKind::Ignored => FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::Ignored,
            bound: None,
            variants: None,
            initial: AbstractValue::Counter(0),
        },
    }
}

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
    // Check port direction
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
        deps.entry(assign.target.clone())
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
            deps.entry(target.clone()).or_default().extend(expr_deps);
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
            targets.push(target.clone());
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

/// A combinational assignment: target = expr.
#[derive(Debug, Clone)]
struct CombAssign {
    target: String,
    value: Expr,
}

/// Collect all combinational assignments (assign + always_comb).
fn collect_comb_assigns(module: &Module) -> Vec<CombAssign> {
    let mut assigns = Vec::new();

    for a in &module.assigns {
        assigns.push(CombAssign {
            target: a.target.clone(),
            value: a.value.clone(),
        });
    }

    for block in &module.always_blocks {
        if let AlwaysBlock::AlwaysComb { body } = block {
            collect_comb_from_statement(body, &mut assigns);
        }
    }

    assigns
}

fn collect_comb_from_statement(stmt: &Statement, assigns: &mut Vec<CombAssign>) {
    match stmt {
        Statement::BlockingAssign { target, value } => {
            assigns.push(CombAssign {
                target: target.clone(),
                value: value.clone(),
            });
        }
        Statement::Block(stmts) => {
            for s in stmts {
                collect_comb_from_statement(s, assigns);
            }
        }
        // For if/case in always_comb, we'd need conditional evaluation.
        // For now, skip complex control flow in comb blocks.
        _ => {}
    }
}

/// Evaluate combinational assignments, updating the valuation map.
fn eval_comb_assigns(assigns: &[CombAssign], values: &mut BTreeMap<String, AbstractValue>) {
    // Simple single-pass evaluation (assumes no dependency cycles)
    for assign in assigns {
        if let Some(result) = eval_expr(&assign.value, values) {
            values.insert(assign.target.clone(), result);
        }
    }
}

/// Evaluate an expression against the current valuation.
fn eval_expr(expr: &Expr, values: &BTreeMap<String, AbstractValue>) -> Option<AbstractValue> {
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
            let v = eval_expr(inner, values)?;
            match v {
                AbstractValue::Bool(b) => Some(AbstractValue::Bool(!b)),
                AbstractValue::Counter(n) => Some(AbstractValue::Bool(n == 0)),
                _ => None,
            }
        }
        Expr::BinOp { op, left, right } => {
            let lv = eval_expr(left, values)?;
            let rv = eval_expr(right, values)?;
            eval_binop(*op, &lv, &rv)
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            let cv = eval_expr(cond, values)?;
            if is_truthy(&cv) {
                eval_expr(then_expr, values)
            } else {
                eval_expr(else_expr, values)
            }
        }
        // BitSelect, BitSlice, Concat — return None (havoc) for abstract values
        Expr::BitSelect { base, index } => {
            let bv = eval_expr(base, values)?;
            let iv = eval_expr(index, values)?;
            match (&bv, &iv) {
                (AbstractValue::Counter(base_val), AbstractValue::Counter(idx)) => {
                    let bit = (base_val >> idx) & 1;
                    Some(AbstractValue::Bool(bit != 0))
                }
                _ => None,
            }
        }
        Expr::BitSlice { base, msb, lsb } => {
            let bv = eval_expr(base, values)?;
            let mv = eval_expr(msb, values)?;
            let lv = eval_expr(lsb, values)?;
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
                _ => None,
            }
        }
        Expr::Concat(parts) => {
            // Concatenation of counter values — shift and combine
            let mut result: i64 = 0;
            for part in parts {
                let v = eval_expr(part, values)?;
                match v {
                    AbstractValue::Counter(n) => {
                        // Rough: just shift left by 1 bit per part (imprecise for multi-bit)
                        result = (result << 1) | (n & 1);
                    }
                    AbstractValue::Bool(b) => {
                        result = (result << 1) | (b as i64);
                    }
                    _ => return None,
                }
            }
            Some(AbstractValue::Counter(result))
        }
    }
}

fn eval_binop(op: BinOp, lv: &AbstractValue, rv: &AbstractValue) -> Option<AbstractValue> {
    match op {
        BinOp::Eq => Some(AbstractValue::Bool(lv == rv)),
        BinOp::Ne => Some(AbstractValue::Bool(lv != rv)),
        BinOp::And => Some(AbstractValue::Bool(is_truthy(lv) && is_truthy(rv))),
        BinOp::Or => Some(AbstractValue::Bool(is_truthy(lv) || is_truthy(rv))),
        BinOp::Lt => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Bool(l < r))
        }
        BinOp::Le => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Bool(l <= r))
        }
        BinOp::Gt => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Bool(l > r))
        }
        BinOp::Ge => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Bool(l >= r))
        }
        BinOp::Add => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Counter(l + r))
        }
        BinOp::Sub => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Counter(l - r))
        }
        BinOp::Mul => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Counter(l * r))
        }
        BinOp::Div => {
            let (l, r) = to_i64_pair(lv, rv)?;
            if r == 0 {
                None
            } else {
                Some(AbstractValue::Counter(l / r))
            }
        }
        BinOp::Mod => {
            let (l, r) = to_i64_pair(lv, rv)?;
            if r == 0 {
                None
            } else {
                Some(AbstractValue::Counter(l % r))
            }
        }
        BinOp::Shl => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Counter(l << r.min(63)))
        }
        BinOp::Shr => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Counter(l >> r.min(63)))
        }
        BinOp::BitOr => {
            let (l, r) = to_i64_pair(lv, rv)?;
            Some(AbstractValue::Counter(l | r))
        }
        BinOp::BitAnd => {
            let (l, r) = to_i64_pair(lv, rv)?;
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

fn to_i64_pair(lv: &AbstractValue, rv: &AbstractValue) -> Option<(i64, i64)> {
    let l = match lv {
        AbstractValue::Counter(n) => *n,
        AbstractValue::Bool(b) => *b as i64,
        _ => return None,
    };
    let r = match rv {
        AbstractValue::Counter(n) => *n,
        AbstractValue::Bool(b) => *b as i64,
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
    target: String,
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
/// `reg_state` is the pre-injection register valuation (Variants for enum
/// registers). `full_val` is the post-injection valuation used for expression
/// evaluation (Variants replaced by Counters via value_map).
fn compute_next_state(
    seq_assigns: &[SeqAssign],
    reg_state: &AbstractState,
    full_val: &BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) -> AbstractState {
    // Start from the original register state (pre-injection Variants)
    let mut next: AbstractState = reg_state.clone();

    // Apply sequential assignments whose guards are satisfied.
    // Skip assignments to Ignored registers — they are not part of the
    // state space and inserting them would pollute the next-state map,
    // causing lookups against state_names to fail silently.
    let ignored: HashSet<&str> = registers
        .iter()
        .filter(|r| r.domain.abstraction == AbstractionType::Ignored)
        .map(|r| r.name.as_str())
        .collect();

    for assign in seq_assigns {
        if ignored.contains(assign.target.as_str()) {
            continue;
        }
        if guards_satisfied(&assign.guards, full_val, registers)
            && let Some(result) = eval_expr(&assign.value, full_val)
        {
            // Clamp to domain bounds
            let clamped = clamp_to_domain(&assign.target, &result, registers);
            next.insert(assign.target.clone(), clamped);
        }
    }

    next
}

fn guards_satisfied(
    guards: &[SeqGuard],
    values: &BTreeMap<String, AbstractValue>,
    registers: &[RegisterInfo],
) -> bool {
    guards.iter().all(|g| match g {
        SeqGuard::If(expr) => eval_expr(expr, values).is_some_and(|v| is_truthy(&v)),
        SeqGuard::IfNot(expr) => eval_expr(expr, values).is_some_and(|v| !is_truthy(&v)),
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

fn clamp_to_domain(
    target: &str,
    value: &AbstractValue,
    registers: &[RegisterInfo],
) -> AbstractValue {
    if let Some(reg) = registers.iter().find(|r| r.name == target) {
        match (&reg.domain.abstraction, value) {
            (AbstractionType::BoundedCounter, AbstractValue::Counter(n)) => {
                let bound = reg.domain.bound.unwrap_or(3);
                AbstractValue::Counter((*n).max(0).min(bound))
            }
            (AbstractionType::Boolean, AbstractValue::Counter(n)) => AbstractValue::Bool(*n != 0),
            (AbstractionType::Boolean, AbstractValue::Bool(_)) => value.clone(),
            // Variant assignment: validate the name is a known variant
            (AbstractionType::EnumValues, AbstractValue::Variant(name)) => {
                if let Some(variants) = &reg.domain.variants {
                    if variants.contains(name) {
                        return value.clone();
                    }
                    // Unknown variant name — use catch-all (last variant)
                    // This handles typos and the eval_expr Ident fallback
                    if let Some(last) = variants.last() {
                        return AbstractValue::Variant(last.clone());
                    }
                }
                value.clone()
            }
            // If assigning a counter to an enum, use value_map if available
            (AbstractionType::EnumValues, AbstractValue::Counter(n)) => {
                // Try value_map first (e.g., enum {IDLE=0, START=3, OTHER})
                if !reg.value_map.is_empty() {
                    if let Some((variant_name, _)) = reg.value_map.iter().find(|(_, val)| val == n)
                    {
                        return AbstractValue::Variant(variant_name.clone());
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
                            return AbstractValue::Variant(catchall.clone());
                        }
                    }
                    value.clone()
                } else if let Some(variants) = &reg.domain.variants {
                    // No value_map — map by index
                    let idx = *n as usize;
                    if idx < variants.len() {
                        AbstractValue::Variant(variants[idx].clone())
                    } else {
                        value.clone()
                    }
                } else {
                    value.clone()
                }
            }
            _ => value.clone(),
        }
    } else {
        value.clone()
    }
}

// ---------------------------------------------------------------------------
// State enumeration and naming
// ---------------------------------------------------------------------------

fn enumerate_cross_product(fields: &[&FieldDomain]) -> Vec<AbstractState> {
    let active_fields: Vec<&&FieldDomain> = fields
        .iter()
        .filter(|f| f.abstraction != AbstractionType::Ignored)
        .collect();

    if active_fields.is_empty() {
        return vec![BTreeMap::new()];
    }

    let mut states = vec![BTreeMap::new()];
    for field in &active_fields {
        let values = field.values();
        let mut new_states = Vec::with_capacity(states.len() * values.len());
        for state in &states {
            for value in &values {
                let mut new_state = state.clone();
                new_state.insert(field.name.clone(), value.clone());
                new_states.push(new_state);
            }
        }
        states = new_states;
    }
    states
}

fn make_state_name(state: &AbstractState) -> String {
    if state.is_empty() {
        return "s0".to_string();
    }
    state
        .iter()
        .map(|(k, v)| format!("{}_{}", k, v.display_short()))
        .collect::<Vec<_>>()
        .join("_")
}

/// Build individual labels for each input signal in a combination.
///
/// Each input signal gets its own label (e.g., `rd_en_T`, `wr_en_F`),
/// and transitions carry all of them as a multi-label set. This produces
/// CTXDSL like `transition A -> B on label rd_en_T, label wr_en_F;`
/// which is more readable than a single concatenated label.
fn make_input_labels(input_combo: &AbstractState) -> Vec<String> {
    if input_combo.is_empty() {
        return vec!["tick".to_string()];
    }
    input_combo
        .iter()
        .map(|(k, v)| format!("{}_{}", k, v.display_short()))
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
                let bound = reg.domain.bound.unwrap_or(3);
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

fn bfs_reachable(initial: &str, transitions: &[TransitionSpec]) -> HashSet<String> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in transitions {
        adj.entry(t.source.as_str())
            .or_default()
            .push(t.target.as_str());
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(initial.to_string());
    queue.push_back(initial.to_string());

    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = adj.get(current.as_str()) {
            for &next in neighbors {
                if visited.insert(next.to_string()) {
                    queue.push_back(next.to_string());
                }
            }
        }
    }

    visited
}

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
        let result = eval_expr(&expr, &values);
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
        let result = eval_expr(&expr, &values);
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
        let result = eval_expr(&expr, &values);
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

        // 2-bit counter auto-abstracted to bounded_counter 0..3 → 4 values
        assert!(automaton.states.len() <= 4);
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
