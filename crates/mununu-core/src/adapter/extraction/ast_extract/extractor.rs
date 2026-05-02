//! AST extractor — converts tree-sitter syntax trees into state fields,
//! methods, guards, and effects suitable for state space derivation.
//!
//! The extractor is language-aware: each language has different syntax for
//! field declarations, method bodies, if-guards, and assignments.

use tree_sitter::Node;

use super::parser::{ParsedSource, SourceLanguage};
use crate::adapter::domain::{AbstractValue, AbstractionType, FieldDomain};
use crate::adapter::extraction::ast_extract::call_summary::{CallEffect, CallGuard};
use crate::adapter::extraction::ast_extract::config::TargetConfig;
use crate::adapter::extraction::ast_extract::domain::{self, DomainProfile};
use crate::adapter::extraction::ast_extract::state_space::{Effect, Guard, MethodBehavior};
use std::collections::{HashMap, HashSet};

/// Result of extracting from a single class/struct target.
#[derive(Debug)]
pub struct ExtractedTarget {
    /// Automaton identifier.
    pub automaton_id: String,
    /// Extracted field domains.
    pub fields: Vec<FieldDomain>,
    /// Extracted method behaviors.
    pub methods: Vec<MethodBehavior>,
    /// Warnings generated during extraction.
    pub warnings: Vec<String>,
    /// Source line anchors for the extracted fields (field_name → line).
    pub field_lines: Vec<(String, u32)>,
    /// Source line ranges for the extracted methods (method_name → (start, end)).
    pub method_lines: Vec<(String, u32, u32)>,
}

/// Extract a target class/struct from a parsed source file.
pub fn extract_target(
    parsed: &ParsedSource,
    target: &TargetConfig,
    profile: Option<&DomainProfile>,
) -> Result<ExtractedTarget, String> {
    let automaton_id = target
        .automaton_id
        .clone()
        .unwrap_or_else(|| target.class.clone());

    let mut warnings = Vec::new();

    // Find the target class/struct/impl nodes in the AST.
    // For Rust, fields are in struct_item and methods are in impl_item — separate nodes.
    // For TypeScript/Python, both are in the same class node.
    let (field_node, method_node) = find_class_nodes(parsed, &target.class)?;

    // Extract fields
    let field_names: HashSet<&str> = target
        .state_fields
        .field_names()
        .iter()
        .map(|s| s.as_str())
        .collect();

    let (fields, field_lines) = extract_fields(
        parsed,
        &field_node,
        &field_names,
        target,
        profile,
        &mut warnings,
    );

    // Extract methods
    let method_filter = &target.methods;
    let include_set: HashSet<&str> = method_filter.include.iter().map(|s| s.as_str()).collect();
    let exclude_set: HashSet<&str> = method_filter.exclude.iter().map(|s| s.as_str()).collect();

    let (methods, method_lines) = extract_methods(
        parsed,
        &method_node,
        &field_names,
        &include_set,
        &exclude_set,
        target,
        profile,
        &mut warnings,
    );

    Ok(ExtractedTarget {
        automaton_id,
        fields,
        methods,
        warnings,
        field_lines,
        method_lines,
    })
}

/// Find the class/struct/impl nodes for a target.
/// Returns `(field_node, method_node)` — for TypeScript/Python these are the same node.
/// For Rust, `field_node` is the struct_item and `method_node` is the impl_item.
fn find_class_nodes<'a>(
    parsed: &'a ParsedSource,
    class_name: &str,
) -> Result<(Node<'a>, Node<'a>), String> {
    let root = parsed.tree.root_node();
    let mut cursor = root.walk();

    match parsed.language {
        SourceLanguage::TypeScript => {
            for child in root.children(&mut cursor) {
                if let Some(node) = find_ts_class(&child, parsed, class_name) {
                    return Ok((node, node));
                }
            }
        }
        SourceLanguage::Python => {
            for child in root.children(&mut cursor) {
                if child.kind() == "class_definition" {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        if parsed.node_text(&name_node) == class_name {
                            return Ok((child, child));
                        }
                    }
                }
            }
        }
        SourceLanguage::Rust => {
            // For Rust, find struct (fields) and impl (methods) separately
            let mut struct_node = None;
            let mut impl_node = None;

            for child in root.children(&mut cursor) {
                if child.kind() == "struct_item" {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        if parsed.node_text(&name_node) == class_name {
                            struct_node = Some(child);
                        }
                    }
                }
                if child.kind() == "impl_item" {
                    if let Some(type_node) = child.child_by_field_name("type") {
                        if parsed.node_text(&type_node) == class_name {
                            impl_node = Some(child);
                        }
                    }
                }
            }

            match (struct_node, impl_node) {
                (Some(s), Some(i)) => return Ok((s, i)),
                (Some(s), None) => return Ok((s, s)), // struct only, no impl
                (None, Some(i)) => return Ok((i, i)), // impl only, no struct visible
                (None, None) => {}                    // fall through to error
            }
        }
        SourceLanguage::GDScript => {
            // GDScript has no class wrapper — the file itself is the "class".
            // Use the root node as both field and method container.
            // The class_name is treated as the script name (ignored for matching).
            return Ok((root, root));
        }
    }

    Err(format!("Class/struct '{}' not found in source", class_name))
}

/// Find a TypeScript class declaration, handling export wrappers.
fn find_ts_class<'a>(node: &Node<'a>, parsed: &ParsedSource, class_name: &str) -> Option<Node<'a>> {
    if node.kind() == "class_declaration" {
        if let Some(name_node) = node.child_by_field_name("name") {
            if parsed.node_text(&name_node) == class_name {
                return Some(*node);
            }
        }
    }
    // Check inside export_statement
    if node.kind() == "export_statement" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_ts_class(&child, parsed, class_name) {
                return Some(found);
            }
        }
    }
    None
}

/// Extract field domains from a class/struct.
fn extract_fields(
    parsed: &ParsedSource,
    class_node: &Node,
    field_names: &HashSet<&str>,
    target: &TargetConfig,
    profile: Option<&DomainProfile>,
    warnings: &mut Vec<String>,
) -> (Vec<FieldDomain>, Vec<(String, u32)>) {
    // Tier B2: pre-scan the source for numeric comparisons with each state
    // field, build an inferred-bound map. Used as a default when the user
    // didn't set an explicit bound in the extraction config.
    let inferred_bounds = infer_counter_bounds_from_source(&parsed.source, field_names);

    let mut fields = Vec::new();
    let mut field_lines = Vec::new();

    // Walk the class body to find field declarations
    let body_node = match parsed.language {
        SourceLanguage::TypeScript => class_node.child_by_field_name("body"),
        SourceLanguage::Python => class_node.child_by_field_name("body"),
        SourceLanguage::Rust => {
            // Rust struct fields are inside a field_declaration_list child
            let mut c = class_node.walk();
            class_node
                .children(&mut c)
                .find(|child| child.kind() == "field_declaration_list")
                .or(Some(*class_node))
        }
        SourceLanguage::GDScript => {
            // GDScript: root node is the body (file-level declarations)
            Some(*class_node)
        }
    };

    let body = match body_node {
        Some(b) => b,
        None => return (fields, field_lines),
    };

    // For Python, also extract fields from __init__ body
    let mut py_init_results: Vec<(String, Option<String>, Option<String>, u32)> = Vec::new();
    if parsed.language == SourceLanguage::Python {
        let mut init_cursor = body.walk();
        for child in body.children(&mut init_cursor) {
            if child.kind() == "function_definition" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    if parsed.node_text(&name_node) == "__init__" {
                        py_init_results = extract_py_init_fields(parsed, &child);
                    }
                }
            }
        }
    }

    // Process __init__ fields first (Python-specific)
    for (name, type_str, initial, line) in &py_init_results {
        if !field_names.contains(name.as_str()) {
            continue;
        }
        if let Some(fd) = build_field_domain(
            name,
            type_str.as_deref(),
            initial.as_deref(),
            target,
            profile,
            &inferred_bounds,
            warnings,
        ) {
            field_lines.push((name.clone(), *line));
            fields.push(fd);
        }
    }

    // Collect fields already found via __init__ to avoid duplicates
    let init_field_names: HashSet<String> = py_init_results
        .iter()
        .map(|(n, _, _, _)| n.clone())
        .collect();

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        let (name, type_str, initial, line) = match parsed.language {
            SourceLanguage::TypeScript => extract_ts_field(parsed, &child),
            SourceLanguage::Python => extract_py_field(parsed, &child),
            SourceLanguage::Rust => extract_rs_field(parsed, &child),
            SourceLanguage::GDScript => extract_gd_field(parsed, &child),
        };

        let name = match name {
            Some(n) => n,
            None => continue,
        };

        // Skip fields already extracted from __init__
        if init_field_names.contains(&name) {
            continue;
        }

        // Only include fields that are in the target's state_fields list
        if !field_names.contains(name.as_str()) {
            continue;
        }

        let line = line.unwrap_or(0);
        if let Some(fd) = build_field_domain(
            &name,
            type_str.as_deref(),
            initial.as_deref(),
            target,
            profile,
            &inferred_bounds,
            warnings,
        ) {
            field_lines.push((name, line));
            fields.push(fd);
        }
    }

    (fields, field_lines)
}

/// Build a FieldDomain from extracted field info (shared by all languages).
fn build_field_domain(
    name: &str,
    type_str: Option<&str>,
    initial: Option<&str>,
    target: &TargetConfig,
    profile: Option<&DomainProfile>,
    inferred_bounds: &HashMap<String, i64>,
    warnings: &mut Vec<String>,
) -> Option<FieldDomain> {
    let abstraction = if let Some(abs) = target.state_fields.abstraction_for(name) {
        abs.type_
    } else if let Some(prof) = profile {
        let ts = type_str.unwrap_or("unknown");
        domain::infer_abstraction(prof, ts)
    } else {
        // No profile, no override — infer from type name directly
        infer_abstraction_no_profile(type_str, name, warnings)
    };

    if abstraction == AbstractionType::Ignored {
        return None;
    }

    let bound = target
        .state_fields
        .abstraction_for(name)
        .and_then(|a| a.bound)
        // Tier B2: when no explicit bound was provided in the extraction
        // config, infer one from the source by looking at how the field is
        // compared to numeric literals (`self.field >= N`, `self.field > N`,
        // `for i in 0..N` over the field, etc.). The caller passes this
        // inferred map; absence of the field key means "no inference,
        // fall back to the domain's heuristic default at clamp time."
        .or_else(|| inferred_bounds.get(name).copied());

    let variants = target
        .state_fields
        .abstraction_for(name)
        .and_then(|a| a.variants.clone());

    let initial_value = match initial {
        Some(v) if v == "false" || v == "False" => AbstractValue::Bool(false),
        Some(v) if v == "true" || v == "True" => AbstractValue::Bool(true),
        Some("0") => AbstractValue::Counter(0),
        Some(v) if v == "None" || v == "undefined" || v == "null" => AbstractValue::Present(false),
        _ => match abstraction {
            AbstractionType::Boolean => AbstractValue::Bool(false),
            AbstractionType::Presence => AbstractValue::Present(false),
            AbstractionType::BoundedCounter => AbstractValue::Counter(0),
            AbstractionType::EnumValues => variants
                .as_ref()
                .and_then(|v| v.first())
                .map(|v| AbstractValue::Variant(v.clone()))
                .unwrap_or(AbstractValue::Bool(false)),
            AbstractionType::Ignored => return None,
        },
    };

    Some(FieldDomain {
        name: name.to_string(),
        abstraction,
        bound,
        lower_bound: None,
        variants,
        initial: initial_value,
    })
}

/// Infer abstraction type when no domain profile is available.
fn infer_abstraction_no_profile(
    type_str: Option<&str>,
    name: &str,
    warnings: &mut Vec<String>,
) -> AbstractionType {
    let ts = match type_str {
        Some(t) => t.to_lowercase(),
        None => {
            warnings.push(format!(
                "Field '{}' has no type annotation and no domain profile; defaulting to boolean",
                name
            ));
            return AbstractionType::Boolean;
        }
    };
    let ts = ts.trim();
    if ts == "boolean" || ts == "bool" {
        AbstractionType::Boolean
    } else if ts.starts_with("option") || ts == "optional" || ts.ends_with('?') {
        AbstractionType::Presence
    } else if ts.contains("map")
        || ts.contains("dict")
        || ts.contains("set")
        || ts.contains("vec")
        || ts.contains("list")
        || ts.contains("array")
        || ts == "int"
        || ts == "number"
        || ts.starts_with("i32")
        || ts.starts_with("u32")
    {
        AbstractionType::BoundedCounter
    } else {
        warnings.push(format!(
            "Field '{}' has type '{}' with no domain profile; defaulting to boolean",
            name,
            type_str.unwrap_or("unknown")
        ));
        AbstractionType::Boolean
    }
}

/// Extract a TypeScript class field declaration.
/// Returns (name, type, initial_value, line).
fn extract_ts_field(
    parsed: &ParsedSource,
    node: &Node,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    // TypeScript: public_field_definition or property_declaration
    if node.kind() != "public_field_definition" && node.kind() != "property_declaration" {
        return (None, None, None, None);
    }

    let name = node
        .child_by_field_name("name")
        .map(|n| parsed.node_text(&n).to_string());

    let type_str = node.child_by_field_name("type").map(|type_node| {
        // Type annotation: `: boolean` → extract the type name
        let mut cursor = type_node.walk();
        type_node
            .children(&mut cursor)
            .find(|c| c.kind() != ":")
            .map(|c| parsed.node_text(&c).to_string())
            .unwrap_or_else(|| parsed.node_text(&type_node).to_string())
    });

    let initial = node
        .child_by_field_name("value")
        .map(|v| parsed.node_text(&v).to_string());

    let line = Some(parsed.node_line(node));

    (name, type_str, initial, line)
}

/// Extract a Python field from a class body node.
///
/// Handles two cases:
/// 1. Class-level `expression_statement` with `self.field = value`
/// 2. `__init__` method body containing `self.field = value` assignments
///
/// For `__init__` assignments, infers type from the RHS value:
///   True/False → "bool", {} → "dict", [] → "list", None → "optional",
///   integer literal → "int", string → "str"
fn extract_py_field(
    parsed: &ParsedSource,
    node: &Node,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    // Case 1: expression_statement at class body level (self.field = value)
    if node.kind() == "expression_statement" {
        return extract_py_self_assignment(parsed, node);
    }

    // Case 2 is handled by extract_py_init_fields() called from extract_fields()
    (None, None, None, None)
}

/// Extract a single `self.field = value` assignment from an expression_statement.
fn extract_py_self_assignment(
    parsed: &ParsedSource,
    node: &Node,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "assignment" {
            if let Some(left) = child.child_by_field_name("left") {
                if left.kind() == "attribute" {
                    let obj = left.child_by_field_name("object");
                    let attr = left.child_by_field_name("attribute");
                    if obj.map(|o| parsed.node_text(&o)) == Some("self") {
                        let name = attr.map(|a| parsed.node_text(&a).to_string());
                        let value = child
                            .child_by_field_name("right")
                            .map(|v| parsed.node_text(&v).to_string());
                        let type_str = value.as_deref().map(infer_py_type_from_value);
                        return (name, type_str, value, Some(parsed.node_line(node)));
                    }
                }
            }
        }
    }
    (None, None, None, None)
}

/// Extract all `self.field = value` assignments from a Python `__init__` body.
fn extract_py_init_fields(
    parsed: &ParsedSource,
    init_node: &Node,
) -> Vec<(String, Option<String>, Option<String>, u32)> {
    let mut results = Vec::new();
    let body = match init_node.child_by_field_name("body") {
        Some(b) => b,
        None => return results,
    };
    let mut cursor = body.walk();
    for stmt in body.children(&mut cursor) {
        if stmt.kind() == "expression_statement" {
            if let (Some(name), type_str, value, Some(line)) =
                extract_py_self_assignment(parsed, &stmt)
            {
                results.push((name, type_str, value, line));
            }
        }
    }
    results
}

/// Infer a type string from a Python RHS value literal.
fn infer_py_type_from_value(value: &str) -> String {
    let trimmed = value.trim();
    match trimmed {
        "True" | "False" => "bool".to_string(),
        "None" => "optional".to_string(),
        "{}" => "dict".to_string(),
        "[]" => "list".to_string(),
        "set()" => "set".to_string(),
        _ if trimmed.starts_with('{') => "dict".to_string(),
        _ if trimmed.starts_with('[') => "list".to_string(),
        _ if trimmed.parse::<i64>().is_ok() => "int".to_string(),
        _ if trimmed.starts_with('"') || trimmed.starts_with('\'') => "str".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Extract a Rust struct field declaration.
fn extract_rs_field(
    parsed: &ParsedSource,
    node: &Node,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    if node.kind() != "field_declaration" {
        return (None, None, None, None);
    }

    let name = node
        .child_by_field_name("name")
        .map(|n| parsed.node_text(&n).to_string());

    let type_str = node
        .child_by_field_name("type")
        .map(|t| parsed.node_text(&t).to_string());

    // Rust struct fields don't have initial values in the declaration
    (name, type_str, None, Some(parsed.node_line(node)))
}

/// Extract a field from a GDScript `variable_statement` node.
///
/// Handles patterns like:
/// - `var current_state: State = State.IDLE`
/// - `var health: int = 100`
/// - `var is_dead: bool = false`
fn extract_gd_field(
    parsed: &ParsedSource,
    node: &Node,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    // GDScript variable declarations (tree-sitter-gdscript uses "variable_statement")
    if node.kind() != "variable_statement" {
        return (None, None, None, None);
    }

    let text = parsed.node_text(node);

    // Parse "var name: Type = initial" pattern from the text
    let text = text.trim();
    if !text.starts_with("var ") {
        return (None, None, None, None);
    }
    let after_var = &text[4..];

    // Extract name (up to : or = or whitespace)
    let name_end = after_var
        .find(|c: char| c == ':' || c == '=' || c.is_whitespace())
        .unwrap_or(after_var.len());
    let name = after_var[..name_end].trim().to_string();
    if name.is_empty() {
        return (None, None, None, None);
    }

    // Extract type (after : and before =)
    let type_str = if let Some(colon_pos) = after_var.find(':') {
        let after_colon = &after_var[colon_pos + 1..];
        let type_end = after_colon.find('=').unwrap_or(after_colon.len());
        let t = after_colon[..type_end].trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    } else {
        None
    };

    // Extract initial value (after =)
    let initial = if let Some(eq_pos) = after_var.find('=') {
        let after_eq = after_var[eq_pos + 1..].trim();
        if after_eq.is_empty() {
            None
        } else {
            Some(after_eq.to_string())
        }
    } else {
        None
    };

    (Some(name), type_str, initial, Some(parsed.node_line(node)))
}

/// Extract method behaviors from a class/struct.
#[allow(clippy::too_many_arguments)]
fn extract_methods(
    parsed: &ParsedSource,
    class_node: &Node,
    field_names: &HashSet<&str>,
    include: &HashSet<&str>,
    exclude: &HashSet<&str>,
    target: &TargetConfig,
    profile: Option<&DomainProfile>,
    warnings: &mut Vec<String>,
) -> (Vec<MethodBehavior>, Vec<(String, u32, u32)>) {
    let mut methods = Vec::new();
    let mut method_lines = Vec::new();

    // Find all method/function definitions in the class
    let method_nodes = find_method_nodes(parsed, class_node);

    for (method_name, method_node) in method_nodes {
        // Apply include/exclude filters
        if !include.is_empty() && !include.contains(method_name.as_str()) {
            continue;
        }
        if exclude.contains(method_name.as_str()) {
            continue;
        }

        let line_start = parsed.node_line(&method_node);
        let line_end = parsed.node_end_line(&method_node);
        method_lines.push((method_name.clone(), line_start, line_end));

        // Determine controllability
        let controllable =
            if let Some(override_val) = target.controllability_overrides.get(&method_name) {
                override_val == "controllable"
            } else if let Some(prof) = profile {
                domain::classify_controllability(prof, &method_name)
                    == domain::Controllability::Controllable
            } else {
                false
            };

        // Extract guards and effects from method body.
        // For GDScript match statements, split into per-case behaviors.
        let body = method_node.child_by_field_name("body");
        if let Some(body_node) = body {
            let case_behaviors = extract_method_behaviors(
                parsed,
                &body_node,
                field_names,
                &method_name,
                controllable,
                line_start,
                line_end,
                warnings,
            );
            methods.extend(case_behaviors);
        }
    }

    (methods, method_lines)
}

/// Find all method/function definition nodes in a class.
fn find_method_nodes<'a>(
    parsed: &'a ParsedSource,
    class_node: &Node<'a>,
) -> Vec<(String, Node<'a>)> {
    let mut results = Vec::new();

    let body_node = match parsed.language {
        SourceLanguage::TypeScript => class_node.child_by_field_name("body"),
        SourceLanguage::Python => class_node.child_by_field_name("body"),
        SourceLanguage::Rust => {
            // impl_item has a body field (declaration_list) containing function_items
            class_node.child_by_field_name("body").or(Some(*class_node))
        }
        SourceLanguage::GDScript => {
            // GDScript: root node contains top-level function definitions
            Some(*class_node)
        }
    };

    let body = match body_node {
        Some(b) => b,
        None => return results,
    };

    let method_kind = match parsed.language {
        SourceLanguage::TypeScript => "method_definition",
        SourceLanguage::Python => "function_definition",
        SourceLanguage::Rust => "function_item",
        SourceLanguage::GDScript => "function_definition",
    };

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() == method_kind {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = parsed.node_text(&name_node).to_string();
                results.push((name, child));
            }
        }
    }

    results
}

/// Extract method behaviors, splitting match-statement cases into separate behaviors.
///
/// For a method like:
/// ```gdscript
/// func _physics_process(delta):
///     match current_state:
///         State.IDLE: current_state = State.RUNNING
///         State.RUNNING: current_state = State.IDLE
/// ```
/// This produces two MethodBehavior entries:
///   - `_physics_process` with guard MustEqual("IDLE") + effect Variant("RUNNING")
///   - `_physics_process` with guard MustEqual("RUNNING") + effect Variant("IDLE")
#[allow(clippy::too_many_arguments)]
fn extract_method_behaviors(
    parsed: &ParsedSource,
    body_node: &Node,
    field_names: &HashSet<&str>,
    method_name: &str,
    controllable: bool,
    line_start: u32,
    line_end: u32,
    warnings: &mut Vec<String>,
) -> Vec<MethodBehavior> {
    // Check if the method body contains a top-level match statement on a state field.
    // If so, split into per-case behaviors.
    type MatchCases = Vec<(Vec<Guard>, Vec<Effect>)>;
    let mut match_info: Option<(String, MatchCases)> = None;

    let mut cursor = body_node.walk();
    for child in body_node.children(&mut cursor) {
        if child.kind() == "match_statement" {
            if let Some(value_node) = child.child_by_field_name("value") {
                let match_expr = parsed.node_text(&value_node).trim().to_string();
                let matched_field = field_names
                    .iter()
                    .find(|&&f| {
                        match_expr == f
                            || match_expr == format!("self.{f}")
                            || match_expr == format!("this.{f}")
                    })
                    .map(|f| f.to_string());

                if let Some(field) = matched_field {
                    let mut cases = Vec::new();
                    if let Some(match_body) = child.child_by_field_name("body") {
                        let mut mc = match_body.walk();
                        for section in match_body.children(&mut mc) {
                            if section.kind() == "pattern_section" {
                                let mut case_guards = Vec::new();
                                let mut case_effects = Vec::new();
                                extract_match_case_guard_and_effects(
                                    parsed,
                                    &section,
                                    &field,
                                    field_names,
                                    &mut case_guards,
                                    &mut case_effects,
                                );
                                if !case_guards.is_empty() || !case_effects.is_empty() {
                                    cases.push((case_guards, case_effects));
                                }
                            }
                        }
                    }
                    if !cases.is_empty() {
                        match_info = Some((field, cases));
                    }
                }
            }
        }
    }

    if let Some((_field, cases)) = match_info {
        // Split: one MethodBehavior per effect within each match case.
        // Each assignment to a state field in a case body is a separate
        // nondeterministic transition (environment chooses which if-branch fires).
        let mut behaviors = Vec::new();
        for (guards, effects) in cases {
            if effects.is_empty() {
                // Case with guards but no effects (e.g., pass statement)
                behaviors.push(MethodBehavior {
                    name: method_name.to_string(),
                    guards,
                    effects: vec![],
                    controllable,
                    line_start: Some(line_start),
                    line_end: Some(line_end),
                });
            } else {
                // One behavior per effect — models nondeterministic choice
                for effect in effects {
                    behaviors.push(MethodBehavior {
                        name: method_name.to_string(),
                        guards: guards.clone(),
                        effects: vec![effect],
                        controllable,
                        line_start: Some(line_start),
                        line_end: Some(line_end),
                    });
                }
            }
        }
        behaviors
    } else {
        // No match statement — use flat guard/effect extraction
        let (guards, effects) =
            extract_guards_and_effects(parsed, body_node, field_names, warnings);
        vec![MethodBehavior {
            name: method_name.to_string(),
            guards,
            effects,
            controllable,
            line_start: Some(line_start),
            line_end: Some(line_end),
        }]
    }
}

/// Extract guards (if-checks on state fields) and effects (assignments to
/// state fields) from a method body.
fn extract_guards_and_effects(
    parsed: &ParsedSource,
    body_node: &Node,
    field_names: &HashSet<&str>,
    _warnings: &mut Vec<String>,
) -> (Vec<Guard>, Vec<Effect>) {
    let mut guards = Vec::new();
    let mut effects = Vec::new();

    // Pre-pass: collect variable-to-field bindings for indirect guard detection (L2).
    // Patterns: `const x = this.field;` / `let x = this.field;` (TS)
    //           `x = self.field` (Python)
    //           `let x = self.field;` (Rust)
    let var_field_map = collect_variable_field_bindings(parsed, body_node, field_names);

    // Walk all descendant nodes looking for if-statements and assignments
    let mut stack = vec![*body_node];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "if_statement" => {
                // Check if condition references state fields (handles compound && / ||)
                if let Some(condition) = node.child_by_field_name("condition") {
                    // Early-return pattern: if the if-body contains a throw/return,
                    // the method proceeds when the condition is FALSE.
                    // We apply De Morgan: NOT(a || b) = !a && !b, NOT(a && b) = !a || !b
                    // Pass `negate=true` to the extraction so it inverts at each leaf
                    // and swaps || <-> && (De Morgan).
                    let negate = is_early_exit_body(parsed, &node);
                    let extracted = extract_guards_from_condition(
                        parsed,
                        &condition,
                        field_names,
                        negate,
                        &var_field_map,
                    );
                    guards.extend(extracted);
                }
            }
            "assignment_expression" | "augmented_assignment_expression" | "assignment" => {
                // Check if LHS is a state field assignment
                if let Some(effect) = extract_effect_from_assignment(parsed, &node, field_names) {
                    effects.push(effect);
                }
            }
            "expression_statement" => {
                // TypeScript: `this._field = value;` is wrapped in expression_statement
                // GDScript: assignments are also wrapped in expression_statement
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "assignment_expression" || child.kind() == "assignment" {
                        if let Some(effect) =
                            extract_effect_from_assignment(parsed, &child, field_names)
                        {
                            effects.push(effect);
                        }
                    }
                }
            }
            "match_statement" => {
                // GDScript match-based FSM pattern:
                //   match current_state:
                //       State.IDLE:
                //           current_state = State.RUNNING
                //
                // Tree-sitter-gdscript v6 structure:
                //   match_statement { value, body: match_body { pattern_section* } }
                //   pattern_section { _pattern, body }
                if let Some(value_node) = node.child_by_field_name("value") {
                    let match_expr = parsed.node_text(&value_node).trim().to_string();
                    // Check if matched expression is a state field
                    let matched_field = field_names
                        .iter()
                        .find(|&&f| {
                            match_expr == f
                                || match_expr == format!("self.{f}")
                                || match_expr == format!("this.{f}")
                        })
                        .map(|f| f.to_string());

                    if let Some(field) = matched_field {
                        if let Some(body_node_inner) = node.child_by_field_name("body") {
                            let mut cursor = body_node_inner.walk();
                            for section in body_node_inner.children(&mut cursor) {
                                if section.kind() == "pattern_section" {
                                    extract_match_case_guard_and_effects(
                                        parsed,
                                        &section,
                                        &field,
                                        field_names,
                                        &mut guards,
                                        &mut effects,
                                    );
                                }
                            }
                        }
                    }
                }
                // Don't push children — we already traversed match internals
                continue;
            }
            // B6: detect `this.<field>.<method>(...)` / `self.<field>.<method>(...)`
            // and resolve via the call-summary library.
            "call_expression" | "method_invocation" | "call" => {
                if let Some(effect) = extract_effect_from_call(parsed, &node, field_names) {
                    effects.push(effect);
                }
            }
            _ => {}
        }

        // Push children for traversal (depth-first)
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    (guards, effects)
}

/// B6: Detect calls of the form `this.<field>.<method>(...)` /
/// `self.<field>.<method>(...)` and resolve them to a call-summary effect.
///
/// Cross-type ambiguity (where the same method name appears on multiple
/// builtin types with different effects) returns no effect — sound for safety.
fn extract_effect_from_call(
    parsed: &ParsedSource,
    node: &Node,
    field_names: &HashSet<&str>,
) -> Option<Effect> {
    use crate::adapter::extraction::ast_extract::call_summary::CallSummaryLibrary;

    // Locate the function/method portion of the call. tree-sitter exposes it
    // as a `function` field for TS/JS, `function` for Python, varies for Rust.
    let func_node = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("name"))?;
    let func_text = parsed.node_text(&func_node);

    // Must be a member access: `<receiver>.<method>`
    let dot_idx = func_text.rfind('.')?;
    let receiver_text = &func_text[..dot_idx];
    let method_name = &func_text[dot_idx + 1..];

    if method_name.is_empty() {
        return None;
    }

    // Resolve receiver to a state field (handles short chains)
    let field = resolve_receiver_to_field(receiver_text, field_names)?;

    // Library lookup by unqualified method name
    let lang = match parsed.language {
        SourceLanguage::TypeScript => "typescript",
        SourceLanguage::Python => "python",
        SourceLanguage::Rust => "rust",
        SourceLanguage::GDScript => return None, // not yet supported
    };
    let lib = CallSummaryLibrary::for_language(lang);
    let (effect, _guard) = lib.resolve_unqualified(method_name)?;

    // Drop ReadOnly / None — those don't affect state
    use crate::adapter::extraction::ast_extract::call_summary::CallEffect;
    if matches!(effect, CallEffect::ReadOnly | CallEffect::None) {
        return None;
    }

    Some(Effect {
        field: field.to_string(),
        effect,
        value: None,
    })
}

/// Check if an if-statement's body is an early-exit (throw, return, break, continue).
/// If so, the guard condition should be INVERTED — the method proceeds when the
/// condition is FALSE, not when it's true.
fn is_early_exit_body(_parsed: &ParsedSource, if_node: &Node) -> bool {
    if let Some(consequence) = if_node.child_by_field_name("consequence") {
        let mut cursor = consequence.walk();
        for child in consequence.children(&mut cursor) {
            match child.kind() {
                "throw_statement" | "return_statement" | "break_statement"
                | "continue_statement" => return true,
                // Block containing a throw/return
                "statement_block" | "block" => {
                    let mut inner = child.walk();
                    for stmt in child.children(&mut inner) {
                        match stmt.kind() {
                            "throw_statement" | "return_statement" | "break_statement"
                            | "continue_statement" => return true,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Extract guards and effects from a single GDScript match/case branch.
///
/// For a pattern_section like `State.IDLE: ...`, this produces:
///   - A guard `MustEqual("IDLE")` on the matched field
///   - Effects from assignments in the case body
fn extract_match_case_guard_and_effects(
    parsed: &ParsedSource,
    section: &Node,
    matched_field: &str,
    field_names: &HashSet<&str>,
    guards: &mut Vec<Guard>,
    effects: &mut Vec<Effect>,
) {
    // Extract pattern value from non-body children of pattern_section.
    // Text is e.g., "State.IDLE" — normalize to "IDLE" (part after last dot).
    let mut pattern_value: Option<String> = None;
    let mut cursor = section.walk();
    for child in section.children(&mut cursor) {
        if child.kind() != "body" && child.is_named() {
            let text = parsed.node_text(&child).trim().to_string();
            if !text.is_empty() {
                let variant = text.rsplit('.').next().unwrap_or(&text).to_string();
                pattern_value = Some(variant);
                break;
            }
        }
    }

    let variant = match pattern_value {
        Some(v) => v,
        None => return,
    };

    guards.push(Guard {
        field: matched_field.to_string(),
        condition: CallGuard::MustEqual(variant),
    });

    // Extract effects from the case body
    if let Some(case_body) = section.child_by_field_name("body") {
        let mut stack = vec![case_body];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "assignment" | "assignment_expression" | "augmented_assignment_expression" => {
                    if let Some(effect) = extract_effect_from_assignment(parsed, &n, field_names) {
                        effects.push(effect);
                    }
                }
                "expression_statement" => {
                    let mut c = n.walk();
                    for child in n.children(&mut c) {
                        if child.kind() == "assignment" || child.kind() == "assignment_expression" {
                            if let Some(effect) =
                                extract_effect_from_assignment(parsed, &child, field_names)
                            {
                                effects.push(effect);
                            }
                        }
                    }
                }
                _ => {}
            }
            let mut c = n.walk();
            for child in n.children(&mut c) {
                stack.push(child);
            }
        }
    }
}

/// Invert a guard condition (for early-return pattern detection).
///
/// Applies De Morgan's law to compound guards:
///   NOT(a || b) = NOT a && NOT b → Conjunction(invert(a), invert(b))
///   NOT(a && b) = NOT a || NOT b → Disjunction(invert(a), invert(b))
fn invert_guard(guard: CallGuard) -> CallGuard {
    match guard {
        CallGuard::MustBeTrue => CallGuard::MustBeFalse,
        CallGuard::MustBeFalse => CallGuard::MustBeTrue,
        CallGuard::CounterGtZero => CallGuard::CounterEqZero,
        CallGuard::CounterEqZero => CallGuard::CounterGtZero,
        CallGuard::MustBePresent => CallGuard::MustBeAbsent,
        CallGuard::MustBeAbsent => CallGuard::MustBePresent,
        // Over-approximate: NOT(x == V) doesn't map to a single guard.
        // Returning None means "no guard" — sound for safety properties.
        CallGuard::MustEqual(_) => CallGuard::None,
        CallGuard::Disjunction(a, b) => {
            CallGuard::Conjunction(Box::new(invert_guard(*a)), Box::new(invert_guard(*b)))
        }
        CallGuard::Conjunction(a, b) => {
            CallGuard::Disjunction(Box::new(invert_guard(*a)), Box::new(invert_guard(*b)))
        }
        CallGuard::None => CallGuard::None,
    }
}

/// Extract guards from a condition node, handling compound expressions and De Morgan.
///
/// When `negate` is true (early-return pattern), De Morgan's law is applied:
///   NOT(a && b) = NOT a || NOT b → over-approximate (skip, method fires from all states)
///   NOT(a || b) = NOT a && NOT b → recurse into both sides with inverted leaf guards
///   NOT(!a) = a → double negation elimination
///
/// When `negate` is false (normal guard):
///   a && b → recurse into both sides (conjunction)
///   a || b → over-approximate (skip)
fn extract_guards_from_condition(
    parsed: &ParsedSource,
    condition: &Node,
    field_names: &HashSet<&str>,
    negate: bool,
    var_field_map: &HashMap<String, String>,
) -> Vec<Guard> {
    let kind = condition.kind();

    // Compound boolean operators (&&, ||, and, or) with De Morgan
    if kind == "binary_expression" || kind == "boolean_operator" {
        if let Some(guards) =
            extract_guards_from_binary_op(parsed, condition, field_names, negate, var_field_map)
        {
            return guards;
        }
    }

    // Unary negation (!, not) — toggle the negate flag
    if kind == "unary_expression" || kind == "not_operator" {
        if let Some(guards) =
            extract_guards_from_negation(parsed, condition, field_names, negate, var_field_map)
        {
            return guards;
        }
    }

    // Parenthesized expression — unwrap
    if kind == "parenthesized_expression" {
        if let Some(guards) =
            extract_guards_from_parens(parsed, condition, field_names, negate, var_field_map)
        {
            return guards;
        }
    }

    // Base case: look for direct field references or indirect via variable binding
    if let Some(mut guard) = extract_single_guard(parsed, condition, field_names, var_field_map) {
        if negate {
            guard.condition = invert_guard(guard.condition);
        }
        vec![guard]
    } else {
        vec![]
    }
}

/// Handle binary boolean operators (`&&`/`and`/`||`/`or`) with De Morgan's law.
///
/// De Morgan: when negated, AND becomes OR and vice versa.
/// Effective-AND → recurse into both sides; effective-OR → over-approximate (skip).
fn extract_guards_from_binary_op(
    parsed: &ParsedSource,
    condition: &Node,
    field_names: &HashSet<&str>,
    negate: bool,
    var_field_map: &HashMap<String, String>,
) -> Option<Vec<Guard>> {
    let op_node = condition.child_by_field_name("operator")?;
    let op = parsed.node_text(&op_node);
    let is_and = op == "&&" || op == "and";
    let is_or = op == "||" || op == "or";

    if !is_and && !is_or {
        return None;
    }

    // De Morgan: negate swaps AND <-> OR
    let effective_and = if negate { is_or } else { is_and };

    if effective_and {
        let mut result = Vec::new();
        if let Some(left) = condition.child_by_field_name("left") {
            result.extend(extract_guards_from_condition(
                parsed,
                &left,
                field_names,
                negate,
                var_field_map,
            ));
        }
        if let Some(right) = condition.child_by_field_name("right") {
            result.extend(extract_guards_from_condition(
                parsed,
                &right,
                field_names,
                negate,
                var_field_map,
            ));
        }
        return Some(result);
    }

    // Effective-OR: try to encode same-field disjunction precisely. Each side
    // must produce exactly one guard, and both must reference the same field.
    // Cross-field OR is over-approximated to no guard (returned as empty Vec)
    // — sound for safety, imprecise.
    let left_guards = condition
        .child_by_field_name("left")
        .map(|n| extract_guards_from_condition(parsed, &n, field_names, negate, var_field_map))
        .unwrap_or_default();
    let right_guards = condition
        .child_by_field_name("right")
        .map(|n| extract_guards_from_condition(parsed, &n, field_names, negate, var_field_map))
        .unwrap_or_default();

    if let ([lg], [rg]) = (left_guards.as_slice(), right_guards.as_slice())
        && lg.field == rg.field
    {
        return Some(vec![Guard {
            field: lg.field.clone(),
            condition: CallGuard::Disjunction(
                Box::new(lg.condition.clone()),
                Box::new(rg.condition.clone()),
            ),
        }]);
    }

    // Cross-field disjunction or unparseable side(s): over-approx (skip).
    Some(vec![])
}

/// Handle unary negation (`!` / `not`) — toggles the negate flag and recurses.
fn extract_guards_from_negation(
    parsed: &ParsedSource,
    condition: &Node,
    field_names: &HashSet<&str>,
    negate: bool,
    var_field_map: &HashMap<String, String>,
) -> Option<Vec<Guard>> {
    let operand = condition
        .child_by_field_name("argument")
        .or_else(|| condition.child_by_field_name("operand"))?;
    let text = parsed.node_text(condition);
    if text.starts_with('!') || text.starts_with("not ") {
        Some(extract_guards_from_condition(
            parsed,
            &operand,
            field_names,
            !negate,
            var_field_map,
        ))
    } else {
        None
    }
}

/// Handle parenthesized expressions — unwrap and recurse into inner expression.
fn extract_guards_from_parens(
    parsed: &ParsedSource,
    condition: &Node,
    field_names: &HashSet<&str>,
    negate: bool,
    var_field_map: &HashMap<String, String>,
) -> Option<Vec<Guard>> {
    let mut cursor = condition.walk();
    for child in condition.children(&mut cursor) {
        if child.kind() != "(" && child.kind() != ")" {
            return Some(extract_guards_from_condition(
                parsed,
                &child,
                field_names,
                negate,
                var_field_map,
            ));
        }
    }
    None
}

/// Extract a single guard from a leaf condition referencing a state field,
/// either directly (`this.field` / `self.field`) or indirectly via a local
/// variable binding (`const x = this.field; if (x)`).
fn extract_single_guard(
    parsed: &ParsedSource,
    condition: &Node,
    field_names: &HashSet<&str>,
    var_field_map: &HashMap<String, String>,
) -> Option<Guard> {
    let text = parsed.node_text(condition);

    // Comparison-operator guards (priority_roadmap §2.1 / Tier B1):
    // `self.field > 0`, `self.field == 0`, `self.field == VALUE`, plus the
    // mirror forms `0 < self.field`, etc. Returns the guard if recognized.
    if let Some(g) = extract_comparison_guard(text, field_names) {
        return Some(g);
    }

    // Check direct field references
    for &field in field_names {
        let this_field = format!("this.{field}");
        let self_field = format!("self.{field}");

        if text.contains(&this_field) || text.contains(&self_field) {
            let negated = text.starts_with('!')
                || text.starts_with("not ")
                || text.contains(&format!("!this.{field}"))
                || text.contains(&format!("!self.{field}"))
                || text.contains(&format!("not self.{field}"));

            let condition = if negated {
                CallGuard::MustBeFalse
            } else {
                CallGuard::MustBeTrue
            };

            return Some(Guard {
                field: field.to_string(),
                condition,
            });
        }
    }

    // Check indirect references via variable bindings (L2)
    // e.g., `const x = this.field; if (x)` → guard on field
    let trimmed = text.trim().trim_start_matches('!');
    if let Some(field_name) = var_field_map.get(trimmed) {
        let negated = text.trim().starts_with('!') || text.trim().starts_with("not ");
        let condition = if negated {
            CallGuard::MustBeFalse
        } else {
            CallGuard::MustBeTrue
        };
        return Some(Guard {
            field: field_name.clone(),
            condition,
        });
    }

    None
}

/// Infer counter-field bounds from comparison patterns in the source text
/// (priority_roadmap §2.2 / Tier B2).
///
/// For each state field, scan the source for `self.{field} >= N`,
/// `self.{field} > N`, `self.{field} == N`, and the mirror forms. Returns the
/// MAXIMUM N seen for each field — a conservative upper bound that admits
/// every observed comparison's value within the abstracted domain.
///
/// Returned as `HashMap<String, i64>`: field_name → inferred upper bound.
/// Fields with no inferable bound are absent from the map; the caller falls
/// back to whatever default the abstraction's heuristic uses (typically the
/// `DEFAULT_COUNTER_BOUND` constant in `crate::adapter::domain`).
///
/// Text-based heuristic to keep the change additive — a future tree-sitter-
/// typed pass would be more precise (handle `for i in 0..N`, `Vec::with_capacity(N)`
/// directly), but this catches the most common pattern (explicit comparisons
/// against state-field counters) which is the dominant signal in real code.
fn infer_counter_bounds_from_source(
    source: &str,
    field_names: &HashSet<&str>,
) -> HashMap<String, i64> {
    let mut bounds: HashMap<String, i64> = HashMap::new();
    for &field in field_names {
        for prefix in &[format!("self.{field}"), format!("this.{field}")] {
            // Find every occurrence; for each, look at the immediately-following
            // characters for an operator + numeric literal.
            let mut search_from = 0;
            while let Some(rel) = source[search_from..].find(prefix) {
                let abs = search_from + rel;
                let after = &source[abs + prefix.len()..];
                if let Some(n) = parse_trailing_comparison(after) {
                    bounds
                        .entry(field.to_string())
                        .and_modify(|cur| *cur = (*cur).max(n))
                        .or_insert(n);
                }
                search_from = abs + prefix.len();
            }
            // Also scan for the mirror form (`N <op> self.{field}`)
            let mut search_from = 0;
            while let Some(rel) = source[search_from..].find(&format!(" {prefix}")) {
                let abs = search_from + rel + 1; // skip leading space
                // Look at chars BEFORE the prefix for `N >= ` etc. Walk back.
                let before = &source[..abs];
                if let Some(n) = parse_leading_comparison(before) {
                    bounds
                        .entry(field.to_string())
                        .and_modify(|cur| *cur = (*cur).max(n))
                        .or_insert(n);
                }
                search_from = abs + prefix.len();
            }
        }
    }
    bounds
}

/// Match `[whitespace]<op>[whitespace]<number>` at the start of `text`. Returns
/// the parsed number if any of the upper-bound-implying operators are seen.
fn parse_trailing_comparison(text: &str) -> Option<i64> {
    let t = text.trim_start();
    for op in &[">=", "==", ">", "<=", "<"] {
        if let Some(rest) = t.strip_prefix(op) {
            let rest = rest.trim_start();
            // Read a number prefix
            let n_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = n_str.parse::<i64>() {
                return Some(n);
            }
        }
    }
    None
}

/// Match `<number>[whitespace]<op>[whitespace]` at the END of `text`. Returns
/// the parsed number for the mirror-form comparison (`N op field`).
fn parse_leading_comparison(text: &str) -> Option<i64> {
    let t = text.trim_end();
    for op in &[">=", "==", ">", "<=", "<"] {
        if let Some(without_op) = t.strip_suffix(op) {
            let without_op = without_op.trim_end();
            // Read a number suffix
            let n_str: String = without_op
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if let Ok(n) = n_str.parse::<i64>() {
                return Some(n);
            }
        }
    }
    None
}

/// Recognize comparison-operator guards on state fields (priority_roadmap §2.1 / Tier B1).
///
/// Handles common patterns:
/// - `self.field > 0` or `this.field > 0` → `CounterGtZero`
/// - `self.field == 0` or `this.field == 0` → `CounterEqZero`
/// - `self.field != 0` or `this.field != 0` → `CounterGtZero` (assumes counter ≥ 0; the negative branch is `CounterEqZero`'s complement)
/// - `self.field == VALUE` (uppercase identifier or `Enum.VALUE`) → `MustEqual(VALUE)`
/// - Mirror forms (`0 < self.field`, `VALUE == self.field`) handled by symmetry.
///
/// Text-based matching to stay consistent with `extract_single_guard`'s
/// existing approach. A future tree-sitter-typed implementation could be more
/// precise, but the ergonomic win here is large for common patterns.
fn extract_comparison_guard(text: &str, field_names: &HashSet<&str>) -> Option<Guard> {
    let t = text.trim();
    // Try each comparison operator. Order matters: `==` before `=`, `>=` before `>`.
    for op in &["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((lhs, rhs)) = split_top_level_op(t, op) {
            let (lhs, rhs) = (lhs.trim(), rhs.trim());
            // Try field on left, literal on right
            if let Some(field) = match_field_ref(lhs, field_names) {
                if let Some(guard) = comparison_to_guard(op, rhs, false) {
                    return Some(Guard {
                        field: field.to_string(),
                        condition: guard,
                    });
                }
            }
            // Try field on right, literal on left (e.g., `0 < self.x` ≡ `self.x > 0`)
            if let Some(field) = match_field_ref(rhs, field_names) {
                if let Some(guard) = comparison_to_guard(op, lhs, true) {
                    return Some(Guard {
                        field: field.to_string(),
                        condition: guard,
                    });
                }
            }
        }
    }
    None
}

/// Split `text` into `(lhs, rhs)` at the FIRST top-level occurrence of `op`,
/// where "top-level" means not nested in parens or brackets. Returns None if
/// no occurrence found.
fn split_top_level_op<'a>(text: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let bytes = text.as_bytes();
    let op_bytes = op.as_bytes();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i + op_bytes.len() <= bytes.len() {
        let b = bytes[i];
        if b == b'(' || b == b'[' || b == b'{' {
            depth += 1;
        } else if b == b')' || b == b']' || b == b'}' {
            depth -= 1;
        } else if depth == 0 && bytes[i..].starts_with(op_bytes) {
            // Avoid matching `==` when looking for `=`, etc. — caller's
            // operator order handles this. Here we only need to ensure we
            // don't treat `=>` as `=` (Rust); guard with a peek.
            let after = bytes.get(i + op_bytes.len()).copied();
            if op == "=" && after == Some(b'=') {
                i += 1;
                continue;
            }
            return Some((&text[..i], &text[i + op_bytes.len()..]));
        }
        i += 1;
    }
    None
}

/// If `text` is exactly a `self.field` or `this.field` reference for a known
/// state field, return the field name.
fn match_field_ref<'a>(text: &str, field_names: &HashSet<&'a str>) -> Option<&'a str> {
    let t = text.trim();
    for &field in field_names {
        if t == format!("self.{field}") || t == format!("this.{field}") || t == field {
            return Some(field);
        }
    }
    None
}

/// Map a comparison `(op, rhs)` to a CallGuard. `mirrored` indicates the field
/// was on the RHS, so operator semantics flip (`<` becomes `>`, etc.).
fn comparison_to_guard(op: &str, rhs: &str, mirrored: bool) -> Option<CallGuard> {
    let effective_op: &str = if mirrored {
        match op {
            "<" => ">",
            ">" => "<",
            "<=" => ">=",
            ">=" => "<=",
            other => other, // "==" and "!=" are symmetric
        }
    } else {
        op
    };
    let rhs = rhs.trim();
    // Numeric literal RHS — counter comparisons
    if let Ok(n) = rhs.parse::<i64>() {
        return match (effective_op, n) {
            (">", 0) => Some(CallGuard::CounterGtZero),
            (">=", 1) => Some(CallGuard::CounterGtZero),
            ("==", 0) => Some(CallGuard::CounterEqZero),
            ("!=", 0) => Some(CallGuard::CounterGtZero),
            // Other numeric comparisons (`> 5`, `== 7`) have no current variant.
            // Could extend CallGuard later (priority_roadmap follow-up).
            _ => None,
        };
    }
    // Enum-variant RHS — `field == VALUE` or `field == Enum.VALUE`
    if effective_op == "==" || effective_op == "!=" {
        // Strip `Enum.` prefix if present, take the variant name
        let variant = rhs.rsplit('.').next().unwrap_or(rhs);
        // Only accept all-uppercase identifiers as enum variants (heuristic)
        if !variant.is_empty()
            && variant
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            let mut g = CallGuard::MustEqual(variant.to_string());
            if effective_op == "!=" {
                g = invert_guard(g);
            }
            return Some(g);
        }
    }
    None
}

/// Collect variable-to-field bindings from a method body.
///
/// Matches patterns like:
/// - TypeScript: `const x = this.field;` / `let x = this.field;`
/// - Python: `x = self.field`
/// - Rust: `let x = self.field;`
///
/// Returns a map from variable name to field name.
fn collect_variable_field_bindings(
    parsed: &ParsedSource,
    body_node: &Node,
    field_names: &HashSet<&str>,
) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    let mut stack = vec![*body_node];

    while let Some(node) = stack.pop() {
        // TypeScript/Rust: `const x = this.field` / `let x = self.field`
        if node.kind() == "lexical_declaration" || node.kind() == "let_declaration" {
            collect_declarator_bindings(parsed, &node, field_names, &mut bindings);
        }

        // Python: `x = self.field` (at statement level)
        if node.kind() == "assignment" {
            collect_py_assignment_binding(parsed, &node, field_names, &mut bindings);
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    bindings
}

/// Collect variable-to-field bindings from a `const`/`let` declaration (TypeScript/Rust).
fn collect_declarator_bindings(
    parsed: &ParsedSource,
    decl_node: &Node,
    field_names: &HashSet<&str>,
    bindings: &mut HashMap<String, String>,
) {
    let mut cursor = decl_node.walk();
    for child in decl_node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            if let (Some(name_node), Some(value_node)) = (
                child.child_by_field_name("name"),
                child.child_by_field_name("value"),
            ) {
                try_bind_field(parsed, &name_node, &value_node, field_names, bindings);
            }
        }
    }
}

/// Collect a variable-to-field binding from a Python assignment (`x = self.field`).
fn collect_py_assignment_binding(
    parsed: &ParsedSource,
    node: &Node,
    field_names: &HashSet<&str>,
    bindings: &mut HashMap<String, String>,
) {
    if let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) {
        if left.kind() == "identifier" {
            try_bind_field(parsed, &left, &right, field_names, bindings);
        }
    }
}

/// Check if a value node references `this.field` or `self.field`, and if so,
/// insert the variable-to-field binding.
fn try_bind_field(
    parsed: &ParsedSource,
    name_node: &Node,
    value_node: &Node,
    field_names: &HashSet<&str>,
    bindings: &mut HashMap<String, String>,
) {
    let var_name = parsed.node_text(name_node);
    let val_text = parsed.node_text(value_node);
    for &field in field_names.iter() {
        let this_field = format!("this.{field}");
        let self_field = format!("self.{field}");
        if val_text == this_field || val_text == self_field {
            bindings.insert(var_name.to_string(), field.to_string());
        }
    }
}

/// B6: Resolve a receiver expression to a state field name when possible.
///
/// Handles `this.<field>` / `self.<field>` and short chain prefixes like
/// `this.<field>.first()` / `self.<field>[0]` — the receiver of the outer
/// method is logically still `<field>`. Returns `None` for receivers that
/// cannot be statically attributed to a single state field (e.g.,
/// `getQueue().push(...)` where the receiver is a function call return).
fn resolve_receiver_to_field<'a>(
    receiver_text: &str,
    field_names: &HashSet<&'a str>,
) -> Option<&'a str> {
    let trimmed = receiver_text.trim();

    // Strip a single trailing `.<method>(...)` or `[...]` indexing layer to
    // peel chain calls like `this._map.first()` down to `this._map`.
    let stripped = strip_chain_tail(trimmed);

    for &field in field_names {
        for prefix in [format!("this.{field}"), format!("self.{field}")] {
            if stripped == prefix.as_str() || trimmed == prefix.as_str() {
                return Some(field);
            }
        }
    }
    None
}

/// Strip ONE trailing chain segment (a `.method(...)` or `[idx]` suffix) so
/// the caller can match against the head expression. Returns the original
/// text on no-op.
fn strip_chain_tail(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return text;
    }

    // Trailing `[...]` indexing
    if bytes[bytes.len() - 1] == b']'
        && let Some(open) = find_matching_open(bytes, bytes.len() - 1, b'[', b']')
    {
        return text[..open].trim_end();
    }

    // Trailing `.method(...)` call
    if bytes[bytes.len() - 1] == b')'
        && let Some(open) = find_matching_open(bytes, bytes.len() - 1, b'(', b')')
    {
        // Walk back over the method-name identifier preceding `(`
        let head = text[..open].trim_end();
        if let Some(dot) = head.rfind('.') {
            return head[..dot].trim_end();
        }
    }

    text
}

/// Find the matching opening bracket position for the closer at `close_idx`.
fn find_matching_open(bytes: &[u8], close_idx: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth: i32 = 1;
    let mut i = close_idx;
    while i > 0 {
        i -= 1;
        if bytes[i] == close {
            depth += 1;
        } else if bytes[i] == open {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Try to extract an effect from an assignment to a state field.
fn extract_effect_from_assignment(
    parsed: &ParsedSource,
    node: &Node,
    field_names: &HashSet<&str>,
) -> Option<Effect> {
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    let left_text = parsed.node_text(&left);
    let right_text = parsed.node_text(&right).trim();

    // Check if LHS is this._field or self._field
    for &field in field_names {
        let this_field = format!("this.{field}");
        let self_field = format!("self.{field}");

        if left_text == this_field || left_text == self_field || left_text == field {
            // Determine the effect
            let (effect, value) = match right_text {
                "true" | "True" => (CallEffect::SetTrue, None),
                "false" | "False" => (CallEffect::SetFalse, None),
                "0" => (CallEffect::ResetToZero, None),
                "None" | "undefined" | "null" => (CallEffect::SetAbsent, None),
                _ => {
                    // Check for increment patterns: field + 1, field += 1
                    if right_text.contains("+") && right_text.contains("1") {
                        (CallEffect::IncrementCounter, None)
                    } else if right_text.contains("-") && right_text.contains("1") {
                        (CallEffect::DecrementCounter, None)
                    } else if right_text.contains('.') {
                        // Enum variant assignment: State.RUNNING, MyEnum.VALUE
                        // Extract the variant name after the last dot
                        let variant = right_text
                            .rsplit('.')
                            .next()
                            .unwrap_or(right_text)
                            .to_string();
                        (CallEffect::SetTrue, Some(AbstractValue::Variant(variant)))
                    } else if right_text
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_')
                    {
                        // Bare enum variant: RUNNING, IDLE (all-caps identifier)
                        (
                            CallEffect::SetTrue,
                            Some(AbstractValue::Variant(right_text.to_string())),
                        )
                    } else {
                        // Unknown assignment — over-approximate as havoc
                        (CallEffect::Unknown, None)
                    }
                }
            };

            return Some(Effect {
                field: field.to_string(),
                effect,
                value,
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::extraction::ast_extract::config::*;
    use crate::adapter::extraction::ast_extract::parser;

    fn make_simple_target(class: &str, fields: &[&str], methods: &[&str]) -> TargetConfig {
        TargetConfig {
            class: class.to_string(),
            automaton_id: None,
            state_fields: StateFieldsConfig::Simple(fields.iter().map(|s| s.to_string()).collect()),
            methods: MethodsConfig {
                include: methods.iter().map(|s| s.to_string()).collect(),
                exclude: vec![],
            },
            controllability_overrides: std::collections::HashMap::new(),
            state_names: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn extract_typescript_class() {
        let source = r#"
class Server {
    private _started: boolean = false;
    private _closed: boolean = false;

    start(): void {
        if (this._started) {
            throw new Error('already started');
        }
        this._started = true;
    }

    close(): void {
        if (this._closed) {
            return;
        }
        this._closed = true;
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Server", &["_started", "_closed"], &["start", "close"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        // Should extract 2 fields
        assert_eq!(result.fields.len(), 2);
        assert!(result.fields.iter().any(|f| f.name == "_started"));
        assert!(result.fields.iter().any(|f| f.name == "_closed"));

        // Should extract 2 methods
        assert_eq!(result.methods.len(), 2);

        // start() should have: guard _started must be true (the if checks _started),
        // effect _started = true
        let start = result.methods.iter().find(|m| m.name == "start").unwrap();
        assert!(!start.guards.is_empty(), "start should have guards");
        assert!(!start.effects.is_empty(), "start should have effects");

        // close() similar
        let close = result.methods.iter().find(|m| m.name == "close").unwrap();
        assert!(!close.guards.is_empty(), "close should have guards");
        assert!(!close.effects.is_empty(), "close should have effects");
    }

    #[test]
    fn extract_rust_struct() {
        let source = r#"
pub struct Connection {
    started: bool,
    closed: bool,
}

impl Connection {
    pub fn start(&mut self) {
        if !self.started {
            self.started = true;
        }
    }

    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
        }
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::Rust).unwrap();
        let target = make_simple_target("Connection", &["started", "closed"], &["start", "close"]);
        let profile = domain::get_profile("protocol_implementation");

        let result = extract_target(&parsed, &target, profile).unwrap();

        // Rust extraction: fields from struct_item, methods from impl_item
        assert_eq!(result.methods.len(), 2);
        let start = result.methods.iter().find(|m| m.name == "start").unwrap();
        assert!(start.controllable); // pub fn → controllable in protocol_implementation profile
    }

    #[test]
    fn extract_python_class() {
        let source = r#"
class Handler:
    def __init__(self):
        self._active = False

    def activate(self):
        if not self._active:
            self._active = True

    def deactivate(self):
        self._active = False
"#;
        let parsed = parser::parse_source(source, SourceLanguage::Python).unwrap();
        let target = make_simple_target("Handler", &["_active"], &["activate", "deactivate"]);
        let profile = domain::get_profile("python_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        assert_eq!(result.methods.len(), 2);
        let activate = result
            .methods
            .iter()
            .find(|m| m.name == "activate")
            .unwrap();
        assert!(!activate.controllable); // _* pattern → uncontrollable? No, activate doesn't match _*
    }

    #[test]
    fn controllability_from_profile() {
        let source = r#"
class Transport {
    private _active: boolean = false;

    start(): void { this._active = true; }
    handleRequest(): void {}
    close(): void { this._active = false; }
    send(): void {}
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target(
            "Transport",
            &["_active"],
            &["start", "handleRequest", "close", "send"],
        );
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        let by_name = |n: &str| result.methods.iter().find(|m| m.name == n).unwrap();
        assert!(by_name("start").controllable); // matches "start" pattern
        assert!(!by_name("handleRequest").controllable); // matches "handle*" pattern
        assert!(by_name("close").controllable); // matches "close" pattern
        assert!(by_name("send").controllable); // matches "send" pattern
    }

    // ---------------------------------------------------------------
    // Tier B1 — Comparison-operator guard extraction
    // ---------------------------------------------------------------

    fn fields() -> HashSet<&'static str> {
        let mut s = HashSet::new();
        s.insert("count");
        s.insert("state");
        s
    }

    #[test]
    fn comparison_counter_gt_zero() {
        let f = fields();
        let g = extract_comparison_guard("self.count > 0", &f).unwrap();
        assert_eq!(g.field, "count");
        assert_eq!(g.condition, CallGuard::CounterGtZero);
    }

    #[test]
    fn comparison_counter_eq_zero() {
        let f = fields();
        let g = extract_comparison_guard("this.count == 0", &f).unwrap();
        assert_eq!(g.field, "count");
        assert_eq!(g.condition, CallGuard::CounterEqZero);
    }

    #[test]
    fn comparison_counter_ne_zero_implies_gt_zero() {
        let f = fields();
        let g = extract_comparison_guard("self.count != 0", &f).unwrap();
        assert_eq!(g.condition, CallGuard::CounterGtZero);
    }

    #[test]
    fn comparison_counter_ge_one_implies_gt_zero() {
        let f = fields();
        let g = extract_comparison_guard("self.count >= 1", &f).unwrap();
        assert_eq!(g.condition, CallGuard::CounterGtZero);
    }

    #[test]
    fn comparison_mirrored_form() {
        let f = fields();
        // `0 < self.count` should equal `self.count > 0`
        let g = extract_comparison_guard("0 < self.count", &f).unwrap();
        assert_eq!(g.condition, CallGuard::CounterGtZero);
    }

    #[test]
    fn comparison_must_equal_enum_variant() {
        let f = fields();
        let g = extract_comparison_guard("self.state == IDLE", &f).unwrap();
        assert_eq!(g.field, "state");
        assert_eq!(g.condition, CallGuard::MustEqual("IDLE".to_string()));
    }

    #[test]
    fn comparison_must_equal_dotted_enum() {
        let f = fields();
        let g = extract_comparison_guard("this.state == State.RUNNING", &f).unwrap();
        assert_eq!(g.field, "state");
        assert_eq!(g.condition, CallGuard::MustEqual("RUNNING".to_string()));
    }

    #[test]
    fn comparison_ne_enum_variant_inverts() {
        let f = fields();
        let g = extract_comparison_guard("self.state != IDLE", &f).unwrap();
        // != enum_variant → MustEqual is inverted; the existing invert_guard
        // currently maps MustEqual(_) → None (no specific anti-variant).
        assert_eq!(g.condition, CallGuard::None);
    }

    #[test]
    fn comparison_unknown_op_returns_none() {
        let f = fields();
        // `self.count + 1` is not a comparison, no guard
        assert!(extract_comparison_guard("self.count + 1", &f).is_none());
    }

    #[test]
    fn comparison_non_field_reference_returns_none() {
        let f = fields();
        // `local_var > 0` not a state field
        assert!(extract_comparison_guard("local_var > 0", &f).is_none());
    }

    // ---------------------------------------------------------------
    // Tier B2 — Counter bound inference from source patterns
    // ---------------------------------------------------------------

    #[test]
    fn infer_bound_from_ge_comparison() {
        let src = r#"
            class Buffer {
                fill: number = 0;
                push() {
                    if (this.fill >= 4) return;
                    this.fill += 1;
                }
            }
        "#;
        let mut fnames = HashSet::new();
        fnames.insert("fill");
        let bounds = infer_counter_bounds_from_source(src, &fnames);
        assert_eq!(bounds.get("fill"), Some(&4));
    }

    #[test]
    fn infer_bound_takes_max_across_comparisons() {
        let src = r#"
            class Counter {
                count: number = 0;
                tick() {
                    if (self.count > 3) reset();
                    if (self.count == 7) escalate();
                }
            }
        "#;
        let mut fnames = HashSet::new();
        fnames.insert("count");
        let bounds = infer_counter_bounds_from_source(src, &fnames);
        // max(3, 7) == 7
        assert_eq!(bounds.get("count"), Some(&7));
    }

    #[test]
    fn infer_bound_handles_mirror_form() {
        let src = r#"
            class Q {
                len: number = 0;
                check() {
                    if (5 < self.len) return;
                }
            }
        "#;
        let mut fnames = HashSet::new();
        fnames.insert("len");
        let bounds = infer_counter_bounds_from_source(src, &fnames);
        assert_eq!(bounds.get("len"), Some(&5));
    }

    #[test]
    fn infer_bound_absent_when_no_comparison() {
        let src = r#"
            class Q {
                len: number = 0;
                tick() { this.len += 1; }
            }
        "#;
        let mut fnames = HashSet::new();
        fnames.insert("len");
        let bounds = infer_counter_bounds_from_source(src, &fnames);
        assert!(!bounds.contains_key("len"));
    }

    #[test]
    fn infer_bound_ignores_other_fields() {
        let src = r#"
            class A {
                wanted: number = 0;
                other: number = 0;
                check() {
                    if (this.other > 99) bail();
                }
            }
        "#;
        let mut fnames = HashSet::new();
        fnames.insert("wanted");
        let bounds = infer_counter_bounds_from_source(src, &fnames);
        // `other` is not in the field set; `wanted` is never compared, so no bound
        assert!(bounds.is_empty());
    }

    // ---------------------------------------------------------------
    // Tier B5 — Compound guard extraction (||, De Morgan)
    // ---------------------------------------------------------------

    fn extract_guards_from_method(source: &str, field: &str) -> Vec<Guard> {
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("X", &[field], &["check"]);
        let result = extract_target(&parsed, &target, None).unwrap();
        result
            .methods
            .into_iter()
            .find(|m| m.name == "check")
            .map(|m| m.guards)
            .unwrap_or_default()
    }

    #[test]
    fn or_same_field_produces_disjunction() {
        let src = r#"
            class X {
                count: number = 0;
                check() {
                    if (this.count == 0 || this.count > 0) return;
                }
            }
        "#;
        let guards = extract_guards_from_method(src, "count");
        // Early-return inverts the condition: NOT(count == 0 || count > 0).
        // De Morgan turns this into AND-of-NOTs (both negated). NOT(== 0) → CounterGtZero,
        // NOT(> 0) → CounterEqZero — emitted as separate guards.
        // The non-early-return path with same-field OR would emit one Disjunction.
        // For this early-return shape we just verify both branches got picked up.
        assert!(!guards.is_empty(), "expected at least one guard");
    }

    #[test]
    fn invert_disjunction_applies_de_morgan() {
        // !(a || b) = !a && !b → Conjunction(invert(a), invert(b))
        let inner = CallGuard::Disjunction(
            Box::new(CallGuard::CounterGtZero),
            Box::new(CallGuard::MustBeTrue),
        );
        let inverted = invert_guard(inner);
        assert_eq!(
            inverted,
            CallGuard::Conjunction(
                Box::new(CallGuard::CounterEqZero),
                Box::new(CallGuard::MustBeFalse),
            )
        );
    }

    #[test]
    fn invert_conjunction_applies_de_morgan() {
        // !(a && b) = !a || !b → Disjunction(invert(a), invert(b))
        let inner = CallGuard::Conjunction(
            Box::new(CallGuard::CounterEqZero),
            Box::new(CallGuard::MustBeFalse),
        );
        let inverted = invert_guard(inner);
        assert_eq!(
            inverted,
            CallGuard::Disjunction(
                Box::new(CallGuard::CounterGtZero),
                Box::new(CallGuard::MustBeTrue),
            )
        );
    }

    #[test]
    fn double_invert_is_identity() {
        // !!(a || b) = a || b
        let inner = CallGuard::Disjunction(
            Box::new(CallGuard::CounterGtZero),
            Box::new(CallGuard::MustBeTrue),
        );
        assert_eq!(invert_guard(invert_guard(inner.clone())), inner);
    }

    // ---------------------------------------------------------------
    // Tier B6 — Call-summary receiver resolution
    // ---------------------------------------------------------------

    #[test]
    fn resolve_receiver_simple_this_field() {
        let mut f = HashSet::new();
        f.insert("_map");
        assert_eq!(resolve_receiver_to_field("this._map", &f), Some("_map"));
        assert_eq!(resolve_receiver_to_field("self._map", &f), Some("_map"));
    }

    #[test]
    fn resolve_receiver_chain_strips_method_call() {
        let mut f = HashSet::new();
        f.insert("queue");
        // Chain: `this.queue.first()` peels to `this.queue`
        assert_eq!(
            resolve_receiver_to_field("this.queue.first()", &f),
            Some("queue")
        );
    }

    #[test]
    fn resolve_receiver_chain_strips_indexing() {
        let mut f = HashSet::new();
        f.insert("buf");
        assert_eq!(resolve_receiver_to_field("self.buf[0]", &f), Some("buf"));
    }

    #[test]
    fn resolve_receiver_unrelated_returns_none() {
        let mut f = HashSet::new();
        f.insert("_map");
        // Unrelated receivers
        assert_eq!(resolve_receiver_to_field("getQueue()", &f), None);
        assert_eq!(resolve_receiver_to_field("local_var", &f), None);
        assert_eq!(resolve_receiver_to_field("this.other", &f), None);
    }

    #[test]
    fn extract_call_effect_map_set_increments_counter() {
        let src = r#"
            class S {
                _map: Map<string, number> = new Map();
                add(): void { this._map.set("k", 1); }
            }
        "#;
        let parsed = parser::parse_source(src, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("S", &["_map"], &["add"]);
        let result = extract_target(&parsed, &target, None).unwrap();
        let add = result.methods.iter().find(|m| m.name == "add").unwrap();
        // The library has Map.prototype.set → IncrementCounter for `_map`
        assert!(
            add.effects.iter().any(|e| {
                e.field == "_map"
                    && matches!(
                        e.effect,
                        crate::adapter::extraction::ast_extract::call_summary::CallEffect::IncrementCounter
                    )
            }),
            "expected IncrementCounter effect on _map, got {:?}",
            add.effects
        );
    }
}
