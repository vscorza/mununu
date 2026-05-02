//! CIRCT MLIR extraction — builds reactive transition systems from CIRCT output.
//!
//! Parses `hw`/`comb`/`seq` dialect MLIR and constructs explicit-state Kripke
//! structures using register cross-product enumeration. Uses the shared
//! [`state_enum`](mununu_core::adapter::state_enum) infrastructure for state
//! enumeration and BFS reachability pruning.
//!
//! # Pipeline
//!
//! ```text
//! circt-verilog design.sv | mununu-extract circt --output spec.espec.json
//! ```

use mununu_core::adapter::domain::{AbstractState, AbstractValue, FieldDomain};
use mununu_core::adapter::extraction::ast::{
    AutomatonDef, ExtractionSpec, ModelConfig, PropertyDef, SourceRef, StateDef,
    StateDefStructured, TransitionDef,
};
use mununu_core::adapter::state_enum;
use regex::Regex;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// MLIR AST types
// ---------------------------------------------------------------------------

/// A parsed CIRCT MLIR module.
#[derive(Debug)]
pub struct MlirModule {
    pub name: Option<String>,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub ops: HashMap<String, Op>,
    pub registers: Vec<Register>,
}

/// A module port.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Port {
    pub name: String,
    pub typ: String,
}

/// An SSA operation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Op {
    Constant {
        value: String,
        typ: String,
    },
    Register {
        name: String,
    },
    Icmp {
        op: String,
        lhs: String,
        rhs: String,
    },
    Mux {
        sel: String,
        true_val: String,
        false_val: String,
    },
    And {
        operands: Vec<String>,
    },
    Or {
        operands: Vec<String>,
    },
    Xor {
        operands: Vec<String>,
    },
}

/// A sequential register (state element).
#[derive(Debug, Clone)]
pub struct Register {
    pub name: String,
    pub next: String,
    pub reset_value: String,
    pub typ: String,
}

// ---------------------------------------------------------------------------
// MLIR parser
// ---------------------------------------------------------------------------

/// Parse CIRCT MLIR text into a simplified SSA representation.
pub fn parse_mlir(mlir_text: &str) -> MlirModule {
    let mut module = MlirModule {
        name: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        ops: HashMap::new(),
        registers: Vec::new(),
    };

    // Precompile regex patterns
    let re_module = Regex::new(r"hw\.module @(\w+)\((.+?)\)\s*\{").unwrap();
    let re_port = Regex::new(r"(in|out)\s+%(\w+)\s*:\s*(\w+)").unwrap();
    let re_constant = Regex::new(r"%(\S+)\s*=\s*hw\.constant\s+(.+?)(?:\s*:\s*(.+))?$").unwrap();
    let re_firreg = Regex::new(
        r"%(\w+)\s*=\s*seq\.firreg\s+%(\w+)\s+clock\s+%(\w+).*reset\s+\w+\s+%(\w+),\s*%(\S+)\s*:\s*(.+)",
    ).unwrap();
    let re_icmp = Regex::new(r"%(\w+)\s*=\s*comb\.icmp\s+(\w+)\s+%(\w+),\s*%(\S+)\s*:").unwrap();
    let re_mux =
        Regex::new(r"%(\w+)\s*=\s*comb\.mux\s+(?:bin\s+)?%(\w+),\s*%(\S+),\s*%(\S+)\s*:").unwrap();

    for line in mlir_text.lines() {
        let line = line.trim();

        // Module declaration
        if let Some(caps) = re_module.captures(line) {
            module.name = Some(caps[1].to_string());
            let ports_str = &caps[2];
            for port_caps in re_port.captures_iter(ports_str) {
                let port = Port {
                    name: port_caps[2].to_string(),
                    typ: port_caps[3].to_string(),
                };
                if &port_caps[1] == "in" {
                    module.inputs.push(port);
                } else {
                    module.outputs.push(port);
                }
            }
        }

        // Constants
        if let Some(caps) = re_constant.captures(line) {
            let name = caps[1].to_string();
            let value = caps[2].trim().to_string();
            let typ = caps
                .get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| {
                    if value == "true" || value == "false" {
                        "i1".to_string()
                    } else {
                        "unknown".to_string()
                    }
                });
            module.ops.insert(name, Op::Constant { value, typ });
        }

        // Registers
        if let Some(caps) = re_firreg.captures(line) {
            let reg = Register {
                name: caps[1].to_string(),
                next: caps[2].to_string(),
                reset_value: caps[5].to_string(),
                typ: caps[6].trim().to_string(),
            };
            let reg_name = reg.name.clone();
            module.registers.push(reg);
            module
                .ops
                .insert(reg_name.clone(), Op::Register { name: reg_name });
        }

        // Comparisons
        if let Some(caps) = re_icmp.captures(line) {
            module.ops.insert(
                caps[1].to_string(),
                Op::Icmp {
                    op: caps[2].to_string(),
                    lhs: caps[3].to_string(),
                    rhs: caps[4].to_string(),
                },
            );
        }

        // Mux
        if let Some(caps) = re_mux.captures(line) {
            module.ops.insert(
                caps[1].to_string(),
                Op::Mux {
                    sel: caps[2].to_string(),
                    true_val: caps[3].to_string(),
                    false_val: caps[4].to_string(),
                },
            );
        }

        // Bitwise ops (and, or, xor)
        for op_name in &["and", "or", "xor"] {
            let pattern = format!(r"%(\w+)\s*=\s*comb\.{}\s+(.+?)\s*:", op_name);
            if let Ok(re) = Regex::new(&pattern)
                && let Some(caps) = re.captures(line)
            {
                let operands: Vec<String> = caps[2]
                    .split(',')
                    .map(|o| o.trim().trim_start_matches('%').to_string())
                    .collect();
                let op = match *op_name {
                    "and" => Op::And { operands },
                    "or" => Op::Or { operands },
                    "xor" => Op::Xor { operands },
                    _ => unreachable!(),
                };
                module.ops.insert(caps[1].to_string(), op);
                break;
            }
        }
    }

    module
}

// ---------------------------------------------------------------------------
// SSA evaluator
// ---------------------------------------------------------------------------

/// Evaluate an SSA value given register/input assignments.
fn evaluate(
    ops: &HashMap<String, Op>,
    name: &str,
    env: &HashMap<String, i64>,
    cache: &mut HashMap<String, i64>,
    depth: u32,
) -> i64 {
    if depth > 100 {
        return 0;
    }

    if let Some(&val) = cache.get(name) {
        return val;
    }

    if let Some(&val) = env.get(name) {
        cache.insert(name.to_string(), val);
        return val;
    }

    let Some(op) = ops.get(name) else {
        cache.insert(name.to_string(), 0);
        return 0;
    };

    let result = match op.clone() {
        Op::Constant { value, .. } => match value.as_str() {
            "true" => 1,
            "false" => 0,
            _ => value.parse::<i64>().unwrap_or(0),
        },
        Op::Register { name: reg_name } => *env.get(&reg_name).unwrap_or(&0),
        Op::Icmp { op, lhs, rhs } => {
            let l = evaluate(ops, &lhs, env, cache, depth + 1);
            let r = evaluate(ops, &rhs, env, cache, depth + 1);
            let cmp = match op.as_str() {
                "eq" | "ceq" => l == r,
                "ne" | "cne" => l != r,
                "slt" => l < r,
                "sgt" => l > r,
                "sle" => l <= r,
                "sge" => l >= r,
                "ult" => (l as u64) < (r as u64),
                _ => false,
            };
            i64::from(cmp)
        }
        Op::Mux {
            sel,
            true_val,
            false_val,
        } => {
            let s = evaluate(ops, &sel, env, cache, depth + 1);
            if s != 0 {
                evaluate(ops, &true_val, env, cache, depth + 1)
            } else {
                evaluate(ops, &false_val, env, cache, depth + 1)
            }
        }
        Op::And { operands } => {
            let vals: Vec<i64> = operands
                .iter()
                .map(|o| evaluate(ops, o, env, cache, depth + 1))
                .collect();
            vals.into_iter().reduce(|a, b| a & b).unwrap_or(0)
        }
        Op::Or { operands } => {
            let vals: Vec<i64> = operands
                .iter()
                .map(|o| evaluate(ops, o, env, cache, depth + 1))
                .collect();
            vals.into_iter().reduce(|a, b| a | b).unwrap_or(0)
        }
        Op::Xor { operands } => {
            let vals: Vec<i64> = operands
                .iter()
                .map(|o| evaluate(ops, o, env, cache, depth + 1))
                .collect();
            vals.into_iter().reduce(|a, b| a ^ b).unwrap_or(0)
        }
    };

    cache.insert(name.to_string(), result);
    result
}

// ---------------------------------------------------------------------------
// Reactive system extraction
// ---------------------------------------------------------------------------

/// Extracted reactive transition system.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ReactiveSystem {
    pub module_name: String,
    pub states: Vec<(String, bool)>, // (name, is_initial)
    pub transitions: Vec<(String, String, String)>, // (from, to, label)
    pub input_names: Vec<String>,
    pub register_names: Vec<String>,
    pub total_enumerated: usize,
    pub reachable: usize,
}

/// Extract a reactive transition system from a parsed MLIR module.
///
/// Uses shared [`state_enum`] infrastructure for cross-product enumeration
/// and BFS reachability pruning.
pub fn extract_reactive_system(module: &MlirModule) -> Result<ReactiveSystem, String> {
    let module_name = module.name.clone().unwrap_or_else(|| "Unknown".to_string());

    if module.registers.is_empty() {
        return Err("No state registers found".to_string());
    }

    // Build register domains using shared FieldDomain
    let mut reg_domains: Vec<FieldDomain> = Vec::new();
    let mut reg_reset_values: Vec<i64> = Vec::new();

    for reg in &module.registers {
        let width = parse_width(&reg.typ);
        let reset_int = resolve_reset_value(&module.ops, &reg.reset_value);

        // For small registers, enumerate all values; abstract large ones to boolean
        let (lo, hi) = if width <= 4 {
            let n_values = 1i64 << width;
            (-(n_values / 2), n_values / 2 - 1)
        } else {
            (0, 1) // abstract large registers to boolean
        };

        reg_domains.push(FieldDomain::with_range(reg.name.clone(), lo, hi, reset_int));
        reg_reset_values.push(reset_int);
    }

    // Identify non-clock/reset inputs
    let inputs: Vec<&Port> = module
        .inputs
        .iter()
        .filter(|p| p.name != "clk" && p.name != "rst")
        .collect();
    let input_names: Vec<String> = inputs.iter().map(|p| p.name.clone()).collect();

    // Build input domains (all i1 → {0, 1})
    let input_domains: Vec<FieldDomain> = inputs
        .iter()
        .map(|p| FieldDomain::with_range(p.name.clone(), 0, 1, 0))
        .collect();

    // Enumerate register state space using shared infrastructure
    let reg_refs: Vec<&FieldDomain> = reg_domains.iter().collect();
    let all_reg_states = state_enum::enumerate_cross_product(&reg_refs);
    let total_enumerated = all_reg_states.len();

    // Enumerate input combinations
    let input_refs: Vec<&FieldDomain> = input_domains.iter().collect();
    let all_input_combos = state_enum::enumerate_cross_product(&input_refs);

    // Build state names and transitions
    let mut transitions: Vec<(String, String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();

    let initial_state = state_enum::initial_state_from_fields(&reg_domains);
    let initial_name = state_enum::make_state_name(&initial_state);

    for reg_state in &all_reg_states {
        let src_name = state_enum::make_state_name(reg_state);

        for input_combo in &all_input_combos {
            // Build SSA environment: registers + inputs
            let mut env: HashMap<String, i64> = HashMap::new();
            for (key, val) in reg_state.iter().chain(input_combo.iter()) {
                if let AbstractValue::Counter(n) = val {
                    env.insert(key.clone(), *n);
                }
            }

            // Evaluate next-state for each register
            let mut cache: HashMap<String, i64> = HashMap::new();
            let mut next_state: AbstractState = std::collections::BTreeMap::new();

            for (i, reg) in module.registers.iter().enumerate() {
                let nv = evaluate(&module.ops, &reg.next, &env, &mut cache, 0);
                // Clamp to valid range
                let domain = &reg_domains[i];
                let lo = domain.lower_bound.unwrap_or(0);
                let hi = domain.bound.unwrap_or(1);
                let width = parse_width(&reg.typ);
                let clamped = if nv < lo || nv > hi {
                    let n_values = 1i64 << width;
                    ((nv + n_values / 2).rem_euclid(n_values)) - n_values / 2
                } else {
                    nv
                };
                next_state.insert(reg.name.clone(), AbstractValue::Counter(clamped));
            }

            let dst_name = state_enum::make_state_name(&next_state);

            // Create label from input combination
            let label = if input_names.is_empty() {
                "tick".to_string()
            } else {
                let parts: Vec<String> = input_names
                    .iter()
                    .map(|name| {
                        let val = input_combo
                            .get(name)
                            .and_then(|v| match v {
                                AbstractValue::Counter(n) => Some(n),
                                _ => None,
                            })
                            .unwrap_or(&0);
                        format!("{}_{}", name, val)
                    })
                    .collect();
                format!("ev_{}", parts.join("_"))
            };

            let key = (src_name.clone(), dst_name.clone(), label.clone());
            if seen.insert(key) {
                transitions.push((src_name.clone(), dst_name, label));
            }
        }
    }

    // BFS reachability pruning using shared infrastructure
    let edges: Vec<(&str, &str)> = transitions
        .iter()
        .map(|(s, t, _)| (s.as_str(), t.as_str()))
        .collect();
    let reachable = state_enum::bfs_reachable(&initial_name, &edges);

    let states: Vec<(String, bool)> = all_reg_states
        .iter()
        .filter_map(|s| {
            let name = state_enum::make_state_name(s);
            if reachable.contains(&name) {
                Some((name.clone(), name == initial_name))
            } else {
                None
            }
        })
        .collect();

    let transitions: Vec<(String, String, String)> = transitions
        .into_iter()
        .filter(|(s, t, _)| reachable.contains(s) && reachable.contains(t))
        .collect();

    let reachable_count = states.len();
    let register_names: Vec<String> = module.registers.iter().map(|r| r.name.clone()).collect();

    Ok(ReactiveSystem {
        module_name,
        states,
        transitions,
        input_names,
        register_names,
        total_enumerated,
        reachable: reachable_count,
    })
}

/// Build an `.espec.json` ExtractionSpec from an extracted reactive system.
pub fn build_espec(system: &ReactiveSystem) -> ExtractionSpec {
    let automaton_id = system.module_name.clone();
    let context_name = system.module_name.to_lowercase();

    let states: Vec<StateDef> = system
        .states
        .iter()
        .map(|(name, is_initial)| {
            StateDef::Structured(StateDefStructured {
                name: name.clone(),
                initial: *is_initial,
            })
        })
        .collect();

    let mut all_labels: Vec<String> = system
        .transitions
        .iter()
        .map(|(_, _, l)| l.clone())
        .collect();
    all_labels.sort();
    all_labels.dedup();
    all_labels.push("noop".to_string());

    let mut transitions: Vec<TransitionDef> = system
        .transitions
        .iter()
        .map(|(from, to, label)| TransitionDef {
            from: from.clone(),
            to: to.clone(),
            label: label.clone(),
            mode: "both".to_string(),
            derived_from: None,
            comment: None,
        })
        .collect();

    // Add noop self-loops
    for (name, _) in &system.states {
        transitions.push(TransitionDef {
            from: name.clone(),
            to: name.clone(),
            label: "noop".to_string(),
            mode: "both".to_string(),
            derived_from: None,
            comment: None,
        });
    }

    ExtractionSpec {
        schema: Some("extraction_spec_v1".to_string()),
        source: SourceRef {
            repo: None,
            commit: None,
            file: None,
            class: None,
            cve: None,
            ghsa: None,
            issue: Some(format!(
                "Reactive system extracted from CIRCT MLIR for module {}. \
                 Registers: {:?}. States: {}/{} reachable.",
                system.module_name,
                system.register_names,
                system.reachable,
                system.total_enumerated
            )),
            fix_pr: None,
            fix_commit: None,
        },
        state_fields: vec![],
        methods: vec![],
        bugs: vec![],
        model_config: ModelConfig {
            context_name,
            controllable_labels: vec![],
            uncontrollable_labels: all_labels,
            automata: vec![AutomatonDef {
                id: automaton_id.clone(),
                states,
                controllable_labels: vec![],
                transitions,
                fields: vec![],
                note: Some(format!(
                    "Reactive system from CIRCT. Registers: {:?}. {}/{} states reachable.",
                    system.register_names, system.reachable, system.total_enumerated
                )),
                role: None,
            }],
            composition: None,
            properties: vec![PropertyDef {
                id: "safety".to_string(),
                description: Some("Trivial safety — all reachable states satisfy".to_string()),
                formula: Some("nu X. ([] X)".to_string()),
                formula_template: None,
                template_ref: None,
                over: Some(automaton_id),
                holds_in_fixed: None,
                holds_in_vulnerable: None,
            }],
            controllers: vec![],
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse bit width from MLIR type string (e.g., "i2" → 2).
fn parse_width(typ: &str) -> u32 {
    let re = Regex::new(r"i(\d+)").unwrap();
    re.captures(typ)
        .and_then(|caps| caps[1].parse().ok())
        .unwrap_or(1)
}

/// Resolve a register reset value to an integer.
fn resolve_reset_value(ops: &HashMap<String, Op>, name: &str) -> i64 {
    match ops.get(name) {
        Some(Op::Constant { value, .. }) => match value.as_str() {
            "false" => 0,
            "true" => 1,
            _ => value.parse().unwrap_or(0),
        },
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_MLIR: &str = r#"
hw.module @counter(in %clk : !seq.clock, in %rst : i1, in %en : i1) {
    %c0_i2 = hw.constant 0 : i2
    %c1_i2 = hw.constant 1 : i2
    %true = hw.constant true
    %0 = comb.icmp eq %count, %c1_i2 : i2
    %1 = comb.mux bin %en, %next_val, %count : i2
    %next_val = comb.mux bin %0, %c0_i2, %inc : i2
    %inc = comb.add %count, %c1_i2 : i2
    %count = seq.firreg %1 clock %clk reset async %rst, %c0_i2 : i2
}
"#;

    #[test]
    fn parse_simple_module() {
        let module = parse_mlir(SIMPLE_MLIR);
        assert_eq!(module.name.as_deref(), Some("counter"));
        assert_eq!(module.registers.len(), 1);
        assert_eq!(module.registers[0].name, "count");
        assert!(module.inputs.iter().any(|p| p.name == "en"));
    }

    #[test]
    fn parse_constant_values() {
        let module = parse_mlir(SIMPLE_MLIR);
        match module.ops.get("c0_i2") {
            Some(Op::Constant { value, .. }) => assert_eq!(value, "0"),
            _ => panic!("Expected constant op"),
        }
        match module.ops.get("true") {
            Some(Op::Constant { value, .. }) => assert_eq!(value, "true"),
            _ => panic!("Expected constant op"),
        }
    }

    #[test]
    fn evaluate_constant() {
        let module = parse_mlir(SIMPLE_MLIR);
        let env = HashMap::new();
        let mut cache = HashMap::new();
        assert_eq!(evaluate(&module.ops, "c0_i2", &env, &mut cache, 0), 0);
        assert_eq!(evaluate(&module.ops, "true", &env, &mut cache, 0), 1);
    }

    // A simpler MLIR for testing the full pipeline
    const TOGGLE_MLIR: &str = r#"
hw.module @toggle(in %clk : !seq.clock, in %rst : i1, in %flip : i1) {
    %false = hw.constant false
    %true = hw.constant true
    %next = comb.mux bin %flip, %inv, %state : i1
    %inv = comb.xor %state, %true : i1
    %state = seq.firreg %next clock %clk reset async %rst, %false : i1
}
"#;

    #[test]
    fn extract_toggle_system() {
        let module = parse_mlir(TOGGLE_MLIR);
        let system = extract_reactive_system(&module).unwrap();

        assert_eq!(system.module_name, "toggle");
        assert_eq!(system.register_names, vec!["state"]);
        assert!(system.reachable <= system.total_enumerated);
        // Toggle has 2 register states (i1: 0, 1 mapped to -1, 0 or 0, 1)
        assert!(system.reachable >= 1);
        assert!(!system.transitions.is_empty());
    }

    #[test]
    fn build_toggle_espec() {
        let module = parse_mlir(TOGGLE_MLIR);
        let system = extract_reactive_system(&module).unwrap();
        let spec = build_espec(&system);

        assert_eq!(spec.schema.as_deref(), Some("extraction_spec_v1"));
        assert_eq!(spec.model_config.automata.len(), 1);
        assert_eq!(spec.model_config.properties.len(), 1);
        assert_eq!(spec.model_config.properties[0].id, "safety");
    }
}
