//! LLVM IR extraction — builds `.espec.json` from LLVM IR via GEP analysis.
//!
//! Traces GEP (GetElementPtr) → load/store chains to extract guards and
//! effects on struct fields. Uses the shared [`state_enum`] infrastructure
//! for boolean cross-product enumeration and BFS reachability pruning.
//!
//! # Pipeline
//!
//! ```text
//! rustc --emit=llvm-ir source.rs | mununu-extract llvm --output spec.espec.json
//! ```

use mununu_core::adapter::domain::{AbstractValue, FieldDomain};
use mununu_core::adapter::extraction::ast::{
    AutomatonDef, ExtractionSpec, ModelConfig, PropertyDef, SourceRef, StateDef,
    StateDefStructured, TransitionDef,
};
use mununu_core::adapter::state_enum;
use regex::Regex;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// LLVM IR AST types
// ---------------------------------------------------------------------------

/// A parsed LLVM IR module.
#[derive(Debug)]
pub struct LlvmModule {
    pub source_filename: Option<String>,
    pub struct_types: HashMap<String, Vec<String>>,
    pub functions: Vec<LlvmFunction>,
}

/// A parsed function with GEP-level SSA info.
#[derive(Debug)]
#[allow(dead_code)]
pub struct LlvmFunction {
    pub mangled: String,
    pub demangled: String,
    pub has_self: bool,
    pub basic_blocks: HashMap<String, BasicBlock>,
    pub gep_map: HashMap<String, usize>, // %ssa_name → field_offset
}

/// A basic block with instructions and terminator.
#[derive(Debug)]
pub struct BasicBlock {
    pub instructions: Vec<Instruction>,
    pub terminator: Option<Terminator>,
}

/// An SSA instruction we care about.
#[derive(Debug, Clone)]
pub enum Instruction {
    Gep { result: String, offset: usize },
    Load { result: String, source: String },
    Trunc { result: String, source: String },
    Store { dest: String, value: i64 },
}

/// A block terminator.
#[derive(Debug, Clone)]
pub enum Terminator {
    BrCond {
        cond: String,
        true_bb: String,
        false_bb: String,
    },
    Br(String),
    Ret,
    Unreachable,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Best-effort demangling of Rust symbol names.
fn demangle_rust(name: &str) -> String {
    let Some(caps) = Regex::new(r"_ZN(.+)E").unwrap().captures(name) else {
        return name.to_string();
    };
    let encoded = &caps[1];
    let mut parts = Vec::new();
    let bytes = encoded.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let mut num_str = String::new();
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            num_str.push(bytes[i] as char);
            i += 1;
        }
        if num_str.is_empty() {
            break;
        }
        let length: usize = num_str.parse().unwrap_or(0);
        if i + length > bytes.len() {
            break;
        }
        let segment = &encoded[i..i + length];
        // Skip hash suffixes (e.g., "h1234abcdef")
        let is_hash = segment.len() > 2
            && segment.starts_with('h')
            && segment[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !is_hash {
            parts.push(segment.to_string());
        }
        i += length;
    }
    parts.join("::")
}

/// Parse LLVM IR text into a module with function-level GEP/load/store info.
pub fn parse_llvm_ir(ir_text: &str) -> LlvmModule {
    let mut module = LlvmModule {
        source_filename: None,
        struct_types: HashMap::new(),
        functions: Vec::new(),
    };

    let re_source = Regex::new(r#"source_filename = "(.+)""#).unwrap();
    let re_struct = Regex::new(r#"%"?(.+?)"?\s*=\s*type\s*\{(.+)\}"#).unwrap();
    let re_define = Regex::new(r"define\s+\S+\s+@(\S+)\((.+?)\).*\{").unwrap();
    let re_bb = Regex::new(r"^(\w+):").unwrap();
    let re_gep =
        Regex::new(r"%(\w+)\s*=\s*getelementptr\s+inbounds\s+\w+,\s*ptr\s+%self,\s*i64\s+(\d+)")
            .unwrap();
    let re_load = Regex::new(r"%(\w+)\s*=\s*load\s+(\w+),\s*ptr\s+%(\w+)").unwrap();
    let re_trunc = Regex::new(r"%(\w+)\s*=\s*trunc\s+\S+\s+%(\w+)\s+to\s+(\w+)").unwrap();
    let re_store = Regex::new(r"store\s+(\w+)\s+(\d+),\s*ptr\s+%(\w+)").unwrap();
    let re_store_self = Regex::new(r"store\s+(\w+)\s+(\d+),\s*ptr\s+%self").unwrap();
    let re_br_cond = Regex::new(r"br\s+i1\s+%(\w+),\s*label\s+%(\w+),\s*label\s+%(\w+)").unwrap();
    let re_br = Regex::new(r"br\s+label\s+%(\w+)").unwrap();

    let mut current_fn: Option<usize> = None; // index into module.functions
    let mut current_bb: Option<String> = None;

    for line in ir_text.lines() {
        let stripped = line.trim();

        // Source filename
        if let Some(caps) = re_source.captures(stripped) {
            module.source_filename = Some(caps[1].to_string());
            continue;
        }

        // Struct type definitions
        if let Some(caps) = re_struct.captures(stripped) {
            let name = caps[1].to_string();
            let fields: Vec<String> = caps[2].split(',').map(|f| f.trim().to_string()).collect();
            module.struct_types.insert(name, fields);
            continue;
        }

        // Function definition
        if let Some(caps) = re_define.captures(stripped) {
            let mangled = caps[1].to_string();
            let params = &caps[2];
            let has_self = params.contains("%self");
            let demangled = demangle_rust(&mangled);
            module.functions.push(LlvmFunction {
                mangled,
                demangled,
                has_self,
                basic_blocks: HashMap::new(),
                gep_map: HashMap::new(),
            });
            current_fn = Some(module.functions.len() - 1);
            current_bb = None;
            continue;
        }

        let Some(fn_idx) = current_fn else { continue };

        if stripped == "}" {
            current_fn = None;
            current_bb = None;
            continue;
        }

        // Basic block label
        if let Some(caps) = re_bb.captures(stripped) {
            let bb_name = caps[1].to_string();
            module.functions[fn_idx].basic_blocks.insert(
                bb_name.clone(),
                BasicBlock {
                    instructions: vec![],
                    terminator: None,
                },
            );
            current_bb = Some(bb_name);
            continue;
        }

        // Ensure we have a current basic block (implicit entry)
        if current_bb.is_none() {
            let bb_name = "entry".to_string();
            module.functions[fn_idx].basic_blocks.insert(
                bb_name.clone(),
                BasicBlock {
                    instructions: vec![],
                    terminator: None,
                },
            );
            current_bb = Some(bb_name);
        }
        let bb_name = current_bb.as_ref().unwrap().clone();
        let func = &mut module.functions[fn_idx];
        let bb = func.basic_blocks.get_mut(&bb_name).unwrap();

        // GEP
        if let Some(caps) = re_gep.captures(stripped) {
            let result = caps[1].to_string();
            let offset: usize = caps[2].parse().unwrap_or(0);
            func.gep_map.insert(result.clone(), offset);
            bb.instructions.push(Instruction::Gep { result, offset });
            continue;
        }

        // Load
        if let Some(caps) = re_load.captures(stripped) {
            let result = caps[1].to_string();
            let source = caps[3].to_string();
            bb.instructions.push(Instruction::Load { result, source });
            continue;
        }

        // Trunc
        if let Some(caps) = re_trunc.captures(stripped) {
            let result = caps[1].to_string();
            let source = caps[2].to_string();
            bb.instructions.push(Instruction::Trunc { result, source });
            continue;
        }

        // Store to self
        if re_store_self.is_match(stripped) {
            if let Some(caps) = re_store_self.captures(stripped) {
                let value: i64 = caps[2].parse().unwrap_or(0);
                bb.instructions.push(Instruction::Store {
                    dest: "__self__".to_string(),
                    value,
                });
            }
            continue;
        }

        // Store to named dest
        if let Some(caps) = re_store.captures(stripped) {
            let value: i64 = caps[2].parse().unwrap_or(0);
            let dest = caps[3].to_string();
            bb.instructions.push(Instruction::Store { dest, value });
            continue;
        }

        // Conditional branch
        if let Some(caps) = re_br_cond.captures(stripped) {
            bb.terminator = Some(Terminator::BrCond {
                cond: caps[1].to_string(),
                true_bb: caps[2].to_string(),
                false_bb: caps[3].to_string(),
            });
            continue;
        }

        // Unconditional branch
        if let Some(caps) = re_br.captures(stripped) {
            bb.terminator = Some(Terminator::Br(caps[1].to_string()));
            continue;
        }

        // Return
        if stripped.starts_with("ret ") {
            bb.terminator = Some(Terminator::Ret);
            continue;
        }

        // Unreachable
        if stripped == "unreachable" {
            bb.terminator = Some(Terminator::Unreachable);
        }
    }

    module
}

// ---------------------------------------------------------------------------
// Method analysis
// ---------------------------------------------------------------------------

/// Guard extracted from entry block conditional branch.
#[derive(Debug, Clone)]
pub struct FieldGuard {
    pub field_offset: usize,
    pub condition: GuardCondition,
}

#[derive(Debug, Clone)]
pub enum GuardCondition {
    MustBeTrue,
    MustBeFalse,
}

/// Effect: a store to a struct field.
#[derive(Debug, Clone)]
pub struct FieldEffect {
    pub field_offset: usize,
    pub value: i64,
}

/// Analysis result for a single method.
#[derive(Debug)]
pub struct MethodAnalysis {
    pub guards: Vec<FieldGuard>,
    pub effects: Vec<FieldEffect>,
}

/// Analyze a function to extract guards and effects on struct fields.
pub fn analyze_method(func: &LlvmFunction, num_fields: usize) -> MethodAnalysis {
    // Build SSA → field offset map from GEP chains and loads
    let mut ssa_field_map: HashMap<String, usize> = HashMap::new();

    for bb in func.basic_blocks.values() {
        for inst in &bb.instructions {
            match inst {
                Instruction::Gep { result, offset } => {
                    ssa_field_map.insert(result.clone(), *offset);
                }
                Instruction::Load { result, source } => {
                    if source == "self" {
                        ssa_field_map.insert(result.clone(), 0);
                    } else if let Some(&offset) = func.gep_map.get(source) {
                        ssa_field_map.insert(result.clone(), offset);
                    }
                }
                Instruction::Trunc { result, source } => {
                    if let Some(&offset) = ssa_field_map.get(source) {
                        ssa_field_map.insert(result.clone(), offset);
                    }
                }
                _ => {}
            }
        }
    }

    // Find entry block
    let entry_name = ["start", "entry"]
        .iter()
        .find(|n| func.basic_blocks.contains_key(**n))
        .map(|s| s.to_string())
        .or_else(|| func.basic_blocks.keys().next().cloned());

    let Some(entry_name) = entry_name else {
        return MethodAnalysis {
            guards: vec![],
            effects: vec![],
        };
    };

    // Extract guards from entry block's conditional branch
    let mut guards = Vec::new();
    if let Some(entry_bb) = func.basic_blocks.get(&entry_name)
        && let Some(Terminator::BrCond {
            cond,
            true_bb,
            false_bb,
        }) = &entry_bb.terminator
        && let Some(&field_offset) = ssa_field_map.get(cond)
    {
        let true_is_exit = is_early_exit(&func.basic_blocks, true_bb, &mut HashSet::new());
        let false_is_exit = is_early_exit(&func.basic_blocks, false_bb, &mut HashSet::new());

        if true_is_exit && !false_is_exit {
            guards.push(FieldGuard {
                field_offset,
                condition: GuardCondition::MustBeFalse,
            });
        } else if false_is_exit && !true_is_exit {
            guards.push(FieldGuard {
                field_offset,
                condition: GuardCondition::MustBeTrue,
            });
        }
    }

    // Collect effects: stores to struct fields (skip unreachable blocks)
    let mut effects = Vec::new();
    for bb in func.basic_blocks.values() {
        if matches!(&bb.terminator, Some(Terminator::Unreachable)) {
            continue;
        }
        for inst in &bb.instructions {
            if let Instruction::Store { dest, value } = inst {
                let offset = if dest == "__self__" || dest == "self" {
                    Some(0)
                } else {
                    func.gep_map.get(dest).copied()
                };
                if let Some(off) = offset
                    && off < num_fields
                {
                    effects.push(FieldEffect {
                        field_offset: off,
                        value: *value,
                    });
                }
            }
        }
    }

    MethodAnalysis { guards, effects }
}

/// Check if a basic block is an early exit (ret without stores, or unreachable).
fn is_early_exit(
    bbs: &HashMap<String, BasicBlock>,
    bb_name: &str,
    visited: &mut HashSet<String>,
) -> bool {
    if !visited.insert(bb_name.to_string()) {
        return false;
    }
    let Some(bb) = bbs.get(bb_name) else {
        return false;
    };
    match &bb.terminator {
        Some(Terminator::Unreachable) => true,
        Some(Terminator::Ret) => !bb
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Store { .. })),
        Some(Terminator::Br(target)) if visited.len() < 3 => {
            let has_stores = bb
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Store { .. }));
            if has_stores {
                false
            } else {
                is_early_exit(bbs, target, visited)
            }
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// espec builder
// ---------------------------------------------------------------------------

/// Build an `.espec.json` from an LLVM IR module.
pub fn build_espec(module: &LlvmModule, target_struct: Option<&str>) -> ExtractionSpec {
    // Find methods on the target struct
    let mut method_functions: Vec<&LlvmFunction> = Vec::new();
    let mut struct_name = target_struct.map(|s| s.to_string());

    for func in &module.functions {
        if func.demangled.contains("::") && func.has_self {
            let parts: Vec<&str> = func.demangled.split("::").collect();
            if parts.len() >= 2 {
                let this_struct = parts[parts.len() - 2];
                if struct_name.is_none() {
                    struct_name = Some(this_struct.to_string());
                }
                if struct_name.as_deref() == Some(this_struct) {
                    method_functions.push(func);
                }
            }
        }
    }

    let struct_name = struct_name.unwrap_or_else(|| "Module".to_string());

    // Find struct type or infer from GEP offsets
    let struct_fields = find_struct_fields(module, &method_functions, &struct_name);

    // Identify boolean field indices (i8/i1 fields)
    let bool_field_indices: Vec<usize> = struct_fields
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            let t = f.trim();
            t == "i1" || t == "i8"
        })
        .map(|(i, _)| i)
        .collect();
    let bool_field_indices = if bool_field_indices.is_empty() {
        (0..struct_fields.len().min(2)).collect::<Vec<_>>()
    } else {
        bool_field_indices
    };

    // Build FieldDomains for the boolean fields
    let fields: Vec<FieldDomain> = bool_field_indices
        .iter()
        .map(|&idx| {
            FieldDomain::new(
                format!("f{}", idx),
                mununu_core::adapter::domain::AbstractionType::Boolean,
                None,
                None,
                AbstractValue::Bool(false),
            )
        })
        .collect();

    // Enumerate cross-product states
    let field_refs: Vec<&FieldDomain> = fields.iter().collect();
    let all_states = state_enum::enumerate_cross_product(&field_refs);
    let initial_state = state_enum::initial_state_from_fields(&fields);
    let initial_name = state_enum::make_state_name(&initial_state);

    // Build transitions from method analysis
    let mut transitions: Vec<(String, String, String)> = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let method_names: Vec<String> = method_functions
        .iter()
        .map(|f| {
            f.demangled
                .split("::")
                .last()
                .unwrap_or("unknown")
                .to_string()
        })
        .collect();

    for func in &method_functions {
        let method_name = func.demangled.split("::").last().unwrap_or("unknown");
        let label = format!("ev_{}", method_name);
        let analysis = analyze_method(func, struct_fields.len());

        for state in &all_states {
            // Check guards
            let guards_ok = analysis.guards.iter().all(|g| {
                let field_name = format!("f{}", g.field_offset);
                match state.get(&field_name) {
                    Some(AbstractValue::Bool(val)) => match g.condition {
                        GuardCondition::MustBeTrue => *val,
                        GuardCondition::MustBeFalse => !val,
                    },
                    _ => true, // field not modeled → guard vacuously true
                }
            });

            if !guards_ok {
                continue;
            }

            // Apply effects
            let mut target_state = state.clone();
            for eff in &analysis.effects {
                let field_name = format!("f{}", eff.field_offset);
                if target_state.contains_key(&field_name) {
                    target_state.insert(field_name, AbstractValue::Bool(eff.value != 0));
                }
            }

            let src_name = state_enum::make_state_name(state);
            let dst_name = state_enum::make_state_name(&target_state);

            let key = (src_name.clone(), dst_name.clone(), label.clone());
            if seen.insert(key) {
                transitions.push((src_name, dst_name, label.clone()));
            }
        }
    }

    // Add noop self-loops
    for state in &all_states {
        let name = state_enum::make_state_name(state);
        transitions.push((name.clone(), name, "noop".to_string()));
    }

    // BFS reachability pruning
    let edges: Vec<(&str, &str)> = transitions
        .iter()
        .map(|(s, t, _)| (s.as_str(), t.as_str()))
        .collect();
    let reachable = state_enum::bfs_reachable(&initial_name, &edges);

    let states: Vec<StateDef> = all_states
        .iter()
        .filter_map(|s| {
            let name = state_enum::make_state_name(s);
            if reachable.contains(&name) {
                Some(StateDef::Structured(StateDefStructured {
                    name: name.clone(),
                    initial: name == initial_name,
                }))
            } else {
                None
            }
        })
        .collect();

    let transitions: Vec<TransitionDef> = transitions
        .into_iter()
        .filter(|(s, t, _)| reachable.contains(s) && reachable.contains(t))
        .map(|(from, to, label)| TransitionDef {
            from,
            to,
            label,
            mode: "both".to_string(),
            derived_from: None,
            comment: None,
        })
        .collect();

    let reachable_count = states.len();
    let mut all_labels: Vec<String> = transitions.iter().map(|t| t.label.clone()).collect();
    all_labels.sort();
    all_labels.dedup();

    let automaton_id = format!("{}FSM", struct_name);

    ExtractionSpec {
        schema: Some("extraction_spec_v1".to_string()),
        source: SourceRef {
            repo: None,
            commit: None,
            file: module.source_filename.clone(),
            class: None,
            cve: None,
            ghsa: None,
            issue: Some(format!(
                "Extracted from LLVM IR with GEP analysis. Struct: {}. Methods: {:?}. States: {} reachable.",
                struct_name, method_names, reachable_count
            )),
            fix_pr: None,
            fix_commit: None,
        },
        state_fields: vec![],
        methods: vec![],
        bugs: vec![],
        model_config: ModelConfig {
            context_name: struct_name.to_lowercase(),
            controllable_labels: vec![],
            uncontrollable_labels: all_labels,
            automata: vec![AutomatonDef {
                id: automaton_id.clone(),
                states,
                controllable_labels: vec![],
                transitions,
                fields: vec![],
                note: Some(format!(
                    "LLVM IR GEP-based extraction. {} methods, {} boolean fields.",
                    method_functions.len(),
                    bool_field_indices.len()
                )),
                role: None,
            }],
            composition: None,
            properties: vec![PropertyDef {
                id: "safety".to_string(),
                description: Some("Trivial safety".to_string()),
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

/// Find struct fields from explicit type or infer from GEP offsets.
fn find_struct_fields(
    module: &LlvmModule,
    method_functions: &[&LlvmFunction],
    struct_name: &str,
) -> Vec<String> {
    // Try to find explicit struct type
    let struct_key = module
        .struct_types
        .keys()
        .find(|k| k.to_lowercase().contains(&struct_name.to_lowercase()));

    if let Some(key) = struct_key
        && let Some(fields) = module.struct_types.get(key)
        && !fields.is_empty()
    {
        return fields.clone();
    }

    // Infer from GEP offsets across all methods
    let mut all_offsets: HashSet<usize> = HashSet::new();
    for func in method_functions {
        for &offset in func.gep_map.values() {
            all_offsets.insert(offset);
        }
        for bb in func.basic_blocks.values() {
            for inst in &bb.instructions {
                match inst {
                    Instruction::Load { source, .. } if source == "self" => {
                        all_offsets.insert(0);
                    }
                    Instruction::Store { dest, .. } if dest == "__self__" => {
                        all_offsets.insert(0);
                    }
                    _ => {}
                }
            }
        }
    }

    if all_offsets.is_empty() {
        return vec![];
    }

    let max_offset = *all_offsets.iter().max().unwrap();
    vec!["i8".to_string(); max_offset + 1]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_IR: &str = r#"
source_filename = "test.rs"

%"MyStruct" = type { i8, i8 }

define void @_ZN8MyStruct5startE(ptr align 1 %self) {
start:
  %0 = getelementptr inbounds i8, ptr %self, i64 0
  %1 = load i8, ptr %0
  %2 = trunc i8 %1 to i1
  br i1 %2, label %panic, label %normal

panic:
  unreachable

normal:
  store i8 1, ptr %0
  ret void
}

define void @_ZN8MyStruct5closeE(ptr align 1 %self) {
start:
  %0 = getelementptr inbounds i8, ptr %self, i64 1
  store i8 1, ptr %0
  ret void
}
"#;

    #[test]
    fn parse_source_filename() {
        let module = parse_llvm_ir(SIMPLE_IR);
        assert_eq!(module.source_filename.as_deref(), Some("test.rs"));
    }

    #[test]
    fn parse_struct_types() {
        let module = parse_llvm_ir(SIMPLE_IR);
        assert!(module.struct_types.contains_key("MyStruct"));
        assert_eq!(module.struct_types["MyStruct"].len(), 2);
    }

    #[test]
    fn parse_functions() {
        let module = parse_llvm_ir(SIMPLE_IR);
        assert_eq!(module.functions.len(), 2);
        assert!(module.functions[0].has_self);
        assert!(module.functions[0].demangled.contains("start"));
        assert!(module.functions[1].demangled.contains("close"));
    }

    #[test]
    fn demangle_basic() {
        // _ZN8MyStruct5startE → MyStruct::start
        let result = demangle_rust("_ZN8MyStruct5startE");
        assert_eq!(result, "MyStruct::start");
    }

    #[test]
    fn analyze_guarded_method() {
        let module = parse_llvm_ir(SIMPLE_IR);
        let start_fn = &module.functions[0];
        let analysis = analyze_method(start_fn, 2);

        // start() has a guard: field 0 must be false (early exit on true)
        assert_eq!(analysis.guards.len(), 1);
        assert_eq!(analysis.guards[0].field_offset, 0);
        assert!(matches!(
            analysis.guards[0].condition,
            GuardCondition::MustBeFalse
        ));

        // Effect: store 1 to field 0
        assert!(!analysis.effects.is_empty());
        assert!(
            analysis
                .effects
                .iter()
                .any(|e| e.field_offset == 0 && e.value == 1)
        );
    }

    #[test]
    fn analyze_unguarded_method() {
        let module = parse_llvm_ir(SIMPLE_IR);
        let close_fn = &module.functions[1];
        let analysis = analyze_method(close_fn, 2);

        // close() has no guards
        assert!(analysis.guards.is_empty());

        // Effect: store 1 to field 1
        assert!(
            analysis
                .effects
                .iter()
                .any(|e| e.field_offset == 1 && e.value == 1)
        );
    }

    #[test]
    fn build_espec_from_simple_ir() {
        let module = parse_llvm_ir(SIMPLE_IR);
        let spec = build_espec(&module, None);

        assert_eq!(spec.schema.as_deref(), Some("extraction_spec_v1"));
        assert_eq!(spec.model_config.automata.len(), 1);
        assert_eq!(spec.model_config.properties.len(), 1);

        let aut = &spec.model_config.automata[0];
        // 2 boolean fields → 4 states max, some may be pruned
        assert!(!aut.states.is_empty());
        assert!(aut.states.len() <= 4);
        assert!(!aut.transitions.is_empty());
    }

    #[test]
    fn build_espec_with_target() {
        let module = parse_llvm_ir(SIMPLE_IR);
        let spec = build_espec(&module, Some("MyStruct"));

        assert_eq!(spec.model_config.context_name, "mystruct");
        assert_eq!(spec.model_config.automata[0].id, "MyStructFSM");
    }
}
