//! AST extractor — converts tree-sitter syntax trees into state fields,
//! methods, guards, and effects suitable for state space derivation.
//!
//! The extractor is language-aware: each language has different syntax for
//! field declarations, method bodies, if-guards, and assignments.

use tree_sitter::Node;

use super::parser::{ParsedSource, SourceLanguage};
use crate::adapter::extraction::ast_extract::call_summary::{CallEffect, CallGuard};
use crate::adapter::extraction::ast_extract::config::{AbstractionType, TargetConfig};
use crate::adapter::extraction::ast_extract::domain::{self, DomainProfile};
use crate::adapter::extraction::ast_extract::state_space::{
    AbstractValue, Effect, FieldDomain, Guard, MethodBehavior,
};
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
            *line,
            target,
            profile,
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
            line,
            target,
            profile,
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
    _line: u32,
    target: &TargetConfig,
    profile: Option<&DomainProfile>,
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
        .and_then(|a| a.bound);

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

        // Extract guards and effects from method body
        let body = method_node.child_by_field_name("body");
        let (guards, effects) = if let Some(body_node) = body {
            extract_guards_and_effects(parsed, &body_node, field_names, warnings)
        } else {
            (vec![], vec![])
        };

        methods.push(MethodBehavior {
            name: method_name,
            guards,
            effects,
            controllable,
            line_start: Some(line_start),
            line_end: Some(line_end),
        });
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
    };

    let body = match body_node {
        Some(b) => b,
        None => return results,
    };

    let method_kind = match parsed.language {
        SourceLanguage::TypeScript => "method_definition",
        SourceLanguage::Python => "function_definition",
        SourceLanguage::Rust => "function_item",
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
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "assignment_expression" {
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

        // Push children for traversal (depth-first)
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    (guards, effects)
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

/// Invert a guard condition (for early-return pattern detection).
fn invert_guard(guard: CallGuard) -> CallGuard {
    match guard {
        CallGuard::MustBeTrue => CallGuard::MustBeFalse,
        CallGuard::MustBeFalse => CallGuard::MustBeTrue,
        CallGuard::CounterGtZero => CallGuard::CounterEqZero,
        CallGuard::CounterEqZero => CallGuard::CounterGtZero,
        CallGuard::MustBePresent => CallGuard::MustBeAbsent,
        CallGuard::MustBeAbsent => CallGuard::MustBePresent,
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

    // Handle binary operators: && / and / || / or
    if kind == "binary_expression" || kind == "boolean_operator" {
        if let Some(op_node) = condition.child_by_field_name("operator") {
            let op = parsed.node_text(&op_node);
            let is_and = op == "&&" || op == "and";
            let is_or = op == "||" || op == "or";

            if is_and || is_or {
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
                    return result;
                } else {
                    return vec![];
                }
            }
        }
    }

    // Handle ! / not (unary negation): toggle the negate flag
    if kind == "unary_expression" || kind == "not_operator" {
        if let Some(operand) = condition
            .child_by_field_name("argument")
            .or_else(|| condition.child_by_field_name("operand"))
        {
            let text = parsed.node_text(condition);
            if text.starts_with('!') || text.starts_with("not ") {
                return extract_guards_from_condition(
                    parsed,
                    &operand,
                    field_names,
                    !negate,
                    var_field_map,
                );
            }
        }
    }

    // Handle parenthesized_expression: unwrap
    if kind == "parenthesized_expression" {
        let mut cursor = condition.walk();
        for child in condition.children(&mut cursor) {
            if child.kind() != "(" && child.kind() != ")" {
                return extract_guards_from_condition(
                    parsed,
                    &child,
                    field_names,
                    negate,
                    var_field_map,
                );
            }
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
        // TypeScript/Rust: lexical_declaration or let_declaration
        // containing `const x = this.field` or `let x = self.field`
        if node.kind() == "lexical_declaration" || node.kind() == "let_declaration" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "variable_declarator" {
                    if let (Some(name_node), Some(value_node)) = (
                        child.child_by_field_name("name"),
                        child.child_by_field_name("value"),
                    ) {
                        let var_name = parsed.node_text(&name_node).to_string();
                        let val_text = parsed.node_text(&value_node);
                        for &field in field_names.iter() {
                            let this_field = format!("this.{field}");
                            let self_field = format!("self.{field}");
                            if val_text == this_field || val_text == self_field {
                                bindings.insert(var_name.clone(), field.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Python: assignment `x = self.field` (at statement level)
        if node.kind() == "assignment" {
            if let (Some(left), Some(right)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("right"),
            ) {
                if left.kind() == "identifier" {
                    let var_name = parsed.node_text(&left).to_string();
                    let val_text = parsed.node_text(&right);
                    for &field in field_names.iter() {
                        let self_field = format!("self.{field}");
                        if val_text == self_field {
                            bindings.insert(var_name.clone(), field.to_string());
                        }
                    }
                }
            }
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    bindings
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
                    } else {
                        // Unknown assignment — could be setting to a specific value
                        (CallEffect::SetTrue, Some(AbstractValue::Bool(true)))
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
}
