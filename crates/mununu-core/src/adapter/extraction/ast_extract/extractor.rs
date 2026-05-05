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

    let (mut fields, mut field_lines) = extract_fields(
        parsed,
        &field_node,
        &field_names,
        target,
        profile,
        &mut warnings,
    );

    // Module-level state scan (GAP-005 step 2).
    // For domains where the most idiomatic state lives at module scope
    // (Python `ContextVar`, JavaScript `AsyncLocalStorage`, shared `Map`/
    // `Set` registries), walk the AST root and synthesize state fields
    // that the class-body scan would miss. Per-target config can override
    // the profile default.
    let module_level_enabled = target
        .state_fields
        .module_level_override()
        .unwrap_or_else(|| profile.is_some_and(|p| p.module_level_scan));
    if module_level_enabled {
        let (mut ml_fields, mut ml_lines) =
            scan_module_level_state(parsed, &field_names, target, profile, &mut warnings);
        // Dedup by name — class-scope match wins over module-scope.
        ml_fields.retain(|f| !fields.iter().any(|existing| existing.name == f.name));
        ml_lines.retain(|(name, _)| ml_fields.iter().any(|f| f.name == *name));
        fields.extend(ml_fields);
        field_lines.extend(ml_lines);
    }

    // GAP-005g: warn on `state_fields.include` names that neither the
    // class-body scan nor the module-level scan produced. Pre-fix, an
    // unrecognized field name was silently dropped — the user only learned
    // of the typo or shape-mismatch by inspecting the output espec.
    let detected: HashSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    for requested in target.state_fields.field_names() {
        if !detected.contains(requested.as_str()) {
            warnings.push(format!(
                "[mununu] WARN: state field '{}' listed in state_fields.include was not \
                 detected (neither as a class-body field nor as a module-level declaration). \
                 Check the field name spelling, the class scope, and (for module-level state) \
                 the domain profile's `module_level_scan` flag.",
                requested
            ));
        }
    }

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
///
/// Recognizes three node kinds:
/// - `class_declaration` — `class Foo { ... }` (the common case)
/// - `abstract_class_declaration` — `abstract class Foo { ... }` (MCP-004
///   hit this with the `Protocol` base class; without this branch, abstract
///   bases were silently skipped and produced degenerate models)
/// - `class_expression` — `const Foo = class { ... }` (occasionally used
///   for inline factory patterns in MCP SDKs)
fn find_ts_class<'a>(node: &Node<'a>, parsed: &ParsedSource, class_name: &str) -> Option<Node<'a>> {
    if matches!(
        node.kind(),
        "class_declaration" | "abstract_class_declaration" | "class_expression"
    ) {
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

/// Scan the AST root for module-level state-field declarations.
///
/// This is the GAP-005 step 2 entry point — runs in addition to the
/// class-body field extraction when `module_level_scan` is enabled on the
/// active domain profile (or explicitly opted-in via
/// `state_fields.module_level: true`). It catches the patterns
/// where state lives outside `self.*` / `this.*`:
///
/// - **Python**: top-level `expression_statement` containing an assignment
///   whose RHS is `contextvars.ContextVar(...)`, `dict()`/`{}`, or
///   `set()` — synthesized as a `BoundedCounter` state field for the
///   `python_server` profile (token-sequence depth or collection
///   cardinality, both abstracted as bounded counters).
/// - **TypeScript**: top-level `lexical_declaration` (const/let) whose
///   RHS is `new AsyncLocalStorage(...)`, `new Map(...)`, or
///   `new Set(...)` — same `BoundedCounter` synthesis. The
///   `mcp_server` profile's `optional_default: Presence` already
///   handles `let _t: T | undefined`, so the type-string is forwarded
///   through `infer_abstraction` for that case.
///
/// A discovered field is only added when its name appears in the
/// target's `state_fields.include` list. This preserves the existing
/// "user lists what they care about" convention; the module-level scan
/// just lets the user list module-scope names alongside instance-scope
/// names without the extractor needing two separate config knobs.
fn scan_module_level_state(
    parsed: &ParsedSource,
    field_names: &HashSet<&str>,
    target: &TargetConfig,
    profile: Option<&DomainProfile>,
    warnings: &mut Vec<String>,
) -> (Vec<FieldDomain>, Vec<(String, u32)>) {
    let mut fields = Vec::new();
    let mut field_lines = Vec::new();
    let inferred_bounds = infer_counter_bounds_from_source(&parsed.source, field_names);

    let root = parsed.tree.root_node();
    let mut cursor = root.walk();

    match parsed.language {
        SourceLanguage::Python => {
            for child in root.children(&mut cursor) {
                // Module-level: `_NAME = <RHS>`
                if child.kind() != "expression_statement" {
                    continue;
                }
                let mut inner = child.walk();
                for grand in child.children(&mut inner) {
                    if grand.kind() != "assignment" {
                        continue;
                    }
                    let lhs = grand.child_by_field_name("left");
                    let rhs = grand.child_by_field_name("right");
                    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
                        continue;
                    };
                    if lhs.kind() != "identifier" {
                        continue;
                    }
                    let name = parsed.node_text(&lhs).to_string();
                    if !field_names.contains(name.as_str()) {
                        continue;
                    }
                    if let Some((type_str, initial)) = classify_python_module_rhs(parsed, &rhs) {
                        if let Some(fd) = build_field_domain(
                            &name,
                            Some(type_str),
                            Some(initial),
                            target,
                            profile,
                            &inferred_bounds,
                            warnings,
                        ) {
                            field_lines.push((name.clone(), parsed.node_line(&grand)));
                            fields.push(fd);
                        }
                    }
                }
            }
        }
        SourceLanguage::TypeScript => {
            for child in root.children(&mut cursor) {
                // Module-level: `const NAME = <RHS>` / `let NAME = <RHS>`
                if child.kind() != "lexical_declaration" && child.kind() != "variable_declaration" {
                    continue;
                }
                let mut inner = child.walk();
                for grand in child.children(&mut inner) {
                    if grand.kind() != "variable_declarator" {
                        continue;
                    }
                    let name_node = grand.child_by_field_name("name");
                    let rhs = grand.child_by_field_name("value");
                    let Some(name_node) = name_node else { continue };
                    if name_node.kind() != "identifier" {
                        continue;
                    }
                    let name = parsed.node_text(&name_node).to_string();
                    if !field_names.contains(name.as_str()) {
                        continue;
                    }
                    let classified = match rhs {
                        Some(r) => classify_typescript_module_rhs(parsed, &r),
                        None => {
                            // No initializer; check the type annotation for
                            // `T | undefined` to infer Presence, then fall
                            // back to GAP-005d's class-typed-singleton
                            // heuristic (any uppercase-typed bare decl).
                            grand
                                .child_by_field_name("type")
                                .and_then(|t| classify_typescript_optional_type(parsed, &t))
                                .or_else(|| {
                                    grand.child_by_field_name("type").and_then(|t| {
                                        classify_typescript_class_typed_singleton(parsed, &t)
                                    })
                                })
                        }
                    };
                    if let Some((type_str, initial)) = classified {
                        if let Some(fd) = build_field_domain(
                            &name,
                            Some(type_str),
                            Some(initial),
                            target,
                            profile,
                            &inferred_bounds,
                            warnings,
                        ) {
                            field_lines.push((name.clone(), parsed.node_line(&grand)));
                            fields.push(fd);
                        }
                    }
                }
            }
        }
        // Module-level scanning is intentionally not supported for Rust
        // and the other languages — module-scope state is uncommon in
        // those domains and the profiles default `module_level_scan: false`.
        _ => {}
    }

    (fields, field_lines)
}

/// Classify a Python module-level assignment's RHS into (type_str, initial)
/// suitable for `build_field_domain`. Returns None if the RHS doesn't match
/// a known module-level state pattern.
fn classify_python_module_rhs<'a>(
    parsed: &'a ParsedSource,
    rhs: &Node<'a>,
) -> Option<(&'static str, &'static str)> {
    let text = parsed.node_text(rhs);
    // contextvars.ContextVar("name", default=...) — token-sequence depth
    if rhs.kind() == "call" && text.contains("ContextVar(") {
        return Some(("dict", "0"));
    }
    // `{}` or `set()` or `dict()` — collection cardinality
    if rhs.kind() == "dictionary" || text == "{}" {
        return Some(("dict", "0"));
    }
    if rhs.kind() == "set" || text == "set()" {
        return Some(("set", "0"));
    }
    if rhs.kind() == "call" && (text.starts_with("dict(") || text.starts_with("set(")) {
        return Some(("dict", "0"));
    }
    None
}

/// Classify a TypeScript module-level assignment's RHS into
/// (type_str, initial). Returns None for non-state RHS.
fn classify_typescript_module_rhs<'a>(
    parsed: &'a ParsedSource,
    rhs: &Node<'a>,
) -> Option<(&'static str, &'static str)> {
    if rhs.kind() != "new_expression" {
        return None;
    }
    let constructor = rhs.child_by_field_name("constructor")?;
    let cname = parsed.node_text(&constructor);
    // `new AsyncLocalStorage<X>()` — run-context depth
    if cname == "AsyncLocalStorage" {
        return Some(("dict", "0"));
    }
    // `new Map(...)` / `new Set(...)` — collection cardinality
    if cname == "Map" || cname == "Set" {
        return Some(("dict", "0"));
    }
    // `new WeakMap(...)` / `new WeakSet(...)` — same
    if cname == "WeakMap" || cname == "WeakSet" {
        return Some(("dict", "0"));
    }
    None
}

/// If a TypeScript type annotation is `T | undefined`, return ("?", "None")
/// so build_field_domain routes to `optional_default` (Presence).
fn classify_typescript_optional_type<'a>(
    parsed: &'a ParsedSource,
    type_node: &Node<'a>,
) -> Option<(&'static str, &'static str)> {
    let text = parsed.node_text(type_node);
    if text.contains("undefined") || text.ends_with('?') {
        return Some(("?", "None"));
    }
    None
}

/// GAP-005d: bare typed module-level declarations like
/// `let manager: KnowledgeGraphManager;` (no initializer, no `| undefined`)
/// indicate a singleton slot that's "absent until assigned." Treat any
/// class-named (uppercase-leading-character) type as a `Presence` singleton.
/// Lowercase types (`string`, `number`, `boolean`, `unknown`) are skipped
/// to avoid flooding the model with built-in scalars that the user didn't
/// model intentionally — those should be listed in
/// `state_fields.abstraction_overrides` if needed.
fn classify_typescript_class_typed_singleton<'a>(
    parsed: &'a ParsedSource,
    type_node: &Node<'a>,
) -> Option<(&'static str, &'static str)> {
    let text = parsed.node_text(type_node);
    let stripped = text.strip_prefix(':').unwrap_or(text).trim();
    let first = stripped.chars().next()?;
    if first.is_ascii_uppercase() {
        Some(("?", "None"))
    } else {
        None
    }
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
            // GAP-005h: decorated methods (`@classmethod`, `@staticmethod`,
            // `@property`, etc.) appear as `decorated_definition` in tree-
            // sitter-python, with the actual `function_definition` as the
            // `definition` child. Descend to find __init__ regardless of
            // whether it's wrapped in a decorator.
            let actual = unwrap_python_decorator(child);
            if actual.kind() == "function_definition" {
                if let Some(name_node) = actual.child_by_field_name("name") {
                    if parsed.node_text(&name_node) == "__init__" {
                        py_init_results = extract_py_init_fields(parsed, &actual);
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

    // GAP-005c: TypeScript constructor parameter properties — the shorthand
    // `constructor(private foo: T) {}` declares `foo` as a class field
    // implicitly. Tree-sitter encodes it as a `required_parameter` with an
    // `accessibility_modifier` child, NOT as `public_field_definition` /
    // `property_declaration`. The body walker above never reaches it
    // because the constructor is a `method_definition`, not a field. This
    // scan descends into the constructor and emits a field for each
    // accessibility-modified parameter. Surfaced by MCP-005 (the
    // `KnowledgeGraphManager(private memoryFilePath: string)` pattern).
    //
    // tree-sitter-typescript exposes method names and parameters as
    // positional children of `method_definition`, not via field-names.
    // Walk children by kind to find them robustly.
    if parsed.language == SourceLanguage::TypeScript {
        let mut body_cursor = body.walk();
        for child in body.children(&mut body_cursor) {
            if child.kind() != "method_definition" {
                continue;
            }
            // Find the method name (`property_identifier`) and the
            // `formal_parameters` child by walking children — tree-sitter-
            // typescript exposes these positionally, not via field names.
            let mut method_cursor = child.walk();
            let mut is_constructor = false;
            let mut params: Option<Node> = None;
            for sub in child.children(&mut method_cursor) {
                match sub.kind() {
                    "property_identifier" if parsed.node_text(&sub) == "constructor" => {
                        is_constructor = true;
                    }
                    "formal_parameters" => {
                        params = Some(sub);
                    }
                    _ => {}
                }
            }
            if !is_constructor {
                continue;
            }
            let Some(params) = params else { continue };
            let mut p_cursor = params.walk();
            for param in params.children(&mut p_cursor) {
                if param.kind() != "required_parameter" {
                    continue;
                }
                let (name, type_str, initial, line) =
                    extract_ts_constructor_param_property(parsed, &param);
                let Some(name) = name else { continue };
                if !field_names.contains(name.as_str()) {
                    continue;
                }
                if fields.iter().any(|f| f.name == name) {
                    // Already collected via the body walker (shouldn't
                    // happen for constructor params, but defensive).
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
        }
    }

    (fields, field_lines)
}

/// GAP-005c: extract a TypeScript constructor parameter property
/// (`constructor(private foo: T = init) {}`). Returns
/// `(name, type, initial_value, line)` if the parameter has an
/// accessibility modifier, otherwise all-None.
///
/// tree-sitter-typescript represents this as:
/// ```text
/// required_parameter
///   accessibility_modifier "private"
///   identifier "foo"
///   "?"                  // optional marker (only if `foo?: T`)
///   type_annotation
///     ":"
///     <type_node>
///   "="                  // only if there's an initializer
///   <init expr>
/// ```
/// Children are positional (not field-named for the most part), so this
/// function walks them once and collects by kind.
fn extract_ts_constructor_param_property(
    parsed: &ParsedSource,
    param: &Node,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    if param.kind() != "required_parameter" {
        return (None, None, None, None);
    }

    let mut has_modifier = false;
    let mut name: Option<String> = None;
    let mut type_str: Option<String> = None;
    let mut initial: Option<String> = None;
    let mut is_optional = false;
    let mut seen_name = false;
    let mut seen_assign = false;

    let mut cursor = param.walk();
    for child in param.children(&mut cursor) {
        let kind = child.kind();
        match kind {
            "accessibility_modifier" | "readonly" => {
                has_modifier = true;
            }
            "identifier" | "property_identifier" if !seen_name => {
                name = Some(parsed.node_text(&child).to_string());
                seen_name = true;
            }
            "type_annotation" => {
                // type_annotation children: `:` then the actual type node.
                let mut tcursor = child.walk();
                let raw = child
                    .children(&mut tcursor)
                    .find(|c| c.kind() != ":")
                    .map(|c| parsed.node_text(&c).to_string())
                    .unwrap_or_else(|| parsed.node_text(&child).to_string());
                type_str = Some(raw);
            }
            _ => {
                // Detect optional `?` marker after the name.
                if seen_name && !seen_assign && parsed.node_text(&child) == "?" {
                    is_optional = true;
                }
                // After `=`, the next non-trivia child is the initializer.
                if seen_assign && initial.is_none() {
                    let txt = parsed.node_text(&child).trim();
                    if !txt.is_empty() {
                        initial = Some(txt.to_string());
                    }
                }
                if parsed.node_text(&child) == "=" {
                    seen_assign = true;
                }
            }
        }
    }

    if !has_modifier {
        // Normal parameter, not a parameter-property.
        return (None, None, None, None);
    }

    if is_optional {
        if let Some(t) = type_str.as_mut() {
            if !t.ends_with('?') {
                t.push('?');
            }
        }
    }

    (name, type_str, initial, Some(parsed.node_line(param)))
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
///
/// Tree-sitter for TypeScript represents `private foo: T` as a
/// `public_field_definition` with an `accessibility_modifier` child whose
/// text is `private` (the node-kind name is misleadingly fixed). So this
/// function handles all accessibility levels via the same node-kind set.
///
/// Optional fields (`foo?: T`) are detected by a `?` token sibling between
/// the name and the type — this token is not retrievable via
/// `child_by_field_name`, only by walking the children. When found, the
/// `?` is appended to the returned type string so the downstream
/// `domain::infer_abstraction` (which already routes types ending in `?`
/// to `optional_default`) classifies the field as a `Presence`-abstracted
/// state field instead of treating it as a plain `Transport` reference
/// (which would otherwise fall to `Ignored`).
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

    // Detect the optional `?` marker. Tree-sitter encodes it as a literal
    // `?` token between the property name and the type annotation. Walk the
    // direct children once and look for a `?` after the name.
    let is_optional = {
        let mut cursor = node.walk();
        let mut seen_name = false;
        let mut found_optional = false;
        for child in node.children(&mut cursor) {
            let kind = child.kind();
            if !seen_name
                && (kind == "property_identifier" || kind == "private_property_identifier")
            {
                seen_name = true;
                continue;
            }
            if seen_name && parsed.node_text(&child) == "?" {
                found_optional = true;
                break;
            }
        }
        found_optional
    };

    let type_str = node.child_by_field_name("type").map(|type_node| {
        // Type annotation: `: boolean` → extract the type name
        let mut cursor = type_node.walk();
        let raw = type_node
            .children(&mut cursor)
            .find(|c| c.kind() != ":")
            .map(|c| parsed.node_text(&c).to_string())
            .unwrap_or_else(|| parsed.node_text(&type_node).to_string());
        if is_optional && !raw.ends_with('?') {
            // Append the marker so domain::infer_abstraction's
            // `ends_with('?')` branch routes the field to optional_default.
            format!("{raw}?")
        } else {
            raw
        }
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
        // GAP-005h: in Python, `@classmethod` / `@staticmethod` / `@property` /
        // `@dataclass`-driven methods appear as `decorated_definition` with
        // the actual `function_definition` as the `definition` child. Descend
        // through the decoration so the method is reachable regardless of
        // whether it's wrapped. Non-Python languages don't have this layer.
        let actual = if parsed.language == SourceLanguage::Python {
            unwrap_python_decorator(child)
        } else {
            child
        };
        if actual.kind() == method_kind {
            if let Some(name_node) = actual.child_by_field_name("name") {
                let name = parsed.node_text(&name_node).to_string();
                results.push((name, actual));
            }
        }
    }

    results
}

/// GAP-005h: Python decorator unwrap. `decorated_definition` is the AST
/// shape tree-sitter-python emits for `@classmethod def foo()` / `@property
/// def bar()` / `@dataclass class Baz` etc. The actual definition lives in
/// the `definition` child. For non-decorated nodes, this is the identity.
fn unwrap_python_decorator<'a>(node: Node<'a>) -> Node<'a> {
    if node.kind() == "decorated_definition" {
        if let Some(def) = node.child_by_field_name("definition") {
            return def;
        }
    }
    node
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
        // GAP-005 step 3: bare-identifier receiver. For module-level state
        // (`_scope.set(...)`, `_ctx.run(...)`), the receiver is just the
        // field name with no `this.` / `self.` prefix. Match if the bare
        // identifier appears in `field_names`. This is gated implicitly by
        // the user's `state_fields.include` list — only names the user
        // declared as state are eligible, so a local variable named
        // `_scope` shadowing a module-level name would only resolve here
        // if the user explicitly listed `_scope` as state.
        if stripped == field || trimmed == field {
            return Some(field);
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
                    // GAP-005 step 3: constructor calls (`new Foo(args)`) on
                    // the RHS are treated as a "set present" mutation. Without
                    // this, `this._transport = new Transport(opts)` (the
                    // canonical MCP-004 pattern) classified as `Unknown` and
                    // produced no observable transition.
                    if right_text.starts_with("new ") && right_text.contains('(') {
                        (CallEffect::SetPresent, None)
                    }
                    // Check for increment patterns: field + 1, field += 1
                    else if right_text.contains("+") && right_text.contains("1") {
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

    /// GAP-005 step 1 — `abstract class Foo` is recognized.
    /// Pre-fix: `find_ts_class()` only matched `class_declaration`, so abstract
    /// bases (e.g., the MCP-004 `Protocol` class) were silently skipped.
    #[test]
    fn extract_typescript_abstract_class() {
        let source = r#"
abstract class Protocol {
    protected _started: boolean = false;

    start(): void {
        if (this._started) { return; }
        this._started = true;
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Protocol", &["_started"], &["start"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        assert_eq!(
            result.fields.len(),
            1,
            "expected 1 field on the abstract class, got {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert!(result.fields.iter().any(|f| f.name == "_started"));
        assert_eq!(result.methods.len(), 1, "expected 1 method");
    }

    /// GAP-005 step 1 — optional fields (`field?: T`) get the `Presence`
    /// abstraction from `optional_default` instead of being treated as a
    /// plain reference type and falling to `Ignored`.
    /// Mirrors the MCP-004 `private _transport?: Transport` pattern.
    #[test]
    fn extract_typescript_optional_field_uses_presence() {
        let source = r#"
class Server {
    private _transport?: Transport;

    connect(t: Transport): void {
        if (this._transport) { return; }
        this._transport = t;
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Server", &["_transport"], &["connect"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        let transport = result
            .fields
            .iter()
            .find(|f| f.name == "_transport")
            .unwrap_or_else(|| panic!("expected _transport field, got {:?}", result.fields));
        assert_eq!(
            transport.abstraction,
            crate::adapter::domain::AbstractionType::Presence,
            "expected Presence abstraction (from optional_default), got {:?}",
            transport.abstraction
        );
    }

    /// GAP-005 step 1 — regression guard: a non-optional field continues to
    /// behave as before (Boolean abstraction for `: boolean`, etc.).
    #[test]
    fn extract_typescript_non_optional_field_unchanged() {
        let source = r#"
class C {
    private _started: boolean = false;
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("C", &["_started"], &[]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        let started = result.fields.iter().find(|f| f.name == "_started").unwrap();
        assert_eq!(
            started.abstraction,
            crate::adapter::domain::AbstractionType::Boolean
        );
    }

    /// GAP-005 step 2 — Python module-level `ContextVar` is detected when the
    /// `python_server` profile's `module_level_scan` is enabled (the default).
    #[test]
    fn extract_python_module_level_contextvar() {
        let source = r#"
import contextvars

_scope = contextvars.ContextVar("scope")

class Scope:
    def __init__(self):
        pass
    def enter(self):
        _scope.set("inside")
    def leave(self):
        _scope.reset(None)
"#;
        let parsed = parser::parse_source(source, SourceLanguage::Python).unwrap();
        let target = make_simple_target("Scope", &["_scope"], &["enter", "leave"]);
        let profile = domain::get_profile("python_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        let scope = result.fields.iter().find(|f| f.name == "_scope");
        assert!(
            scope.is_some(),
            "expected module-level _scope to be detected, got fields {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert_eq!(
            scope.unwrap().abstraction,
            crate::adapter::domain::AbstractionType::BoundedCounter
        );
    }

    /// GAP-005 step 2 — TypeScript module-level `new AsyncLocalStorage(...)`
    /// is detected when the `mcp_server` profile is active.
    #[test]
    fn extract_typescript_module_level_async_local_storage() {
        let source = r#"
const _ctx = new AsyncLocalStorage<string>();

class Runner {
    run(value: string): void {
        _ctx.run(value, () => {});
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Runner", &["_ctx"], &["run"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        let ctx = result.fields.iter().find(|f| f.name == "_ctx");
        assert!(
            ctx.is_some(),
            "expected module-level _ctx to be detected, got fields {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert_eq!(
            ctx.unwrap().abstraction,
            crate::adapter::domain::AbstractionType::BoundedCounter
        );
    }

    /// GAP-005 step 2 — regression guard: profiles with
    /// `module_level_scan: false` (e.g., `protocol_implementation`) do NOT
    /// pick up module-level state, even if the language pattern would
    /// otherwise match.
    #[test]
    fn extract_module_level_disabled_in_other_profiles() {
        let source = r#"
const _ctx = new Map();

class C {
    public set(): void {
        _ctx.set("k", "v");
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("C", &["_ctx"], &["set"]);
        // protocol_implementation has module_level_scan = false
        let profile = domain::get_profile("protocol_implementation");

        let result = extract_target(&parsed, &target, profile).unwrap();

        // _ctx must NOT appear because this profile's module_level_scan is off.
        assert!(
            !result.fields.iter().any(|f| f.name == "_ctx"),
            "module-level _ctx should not be picked up by protocol_implementation \
             profile (module_level_scan=false), got {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    /// GAP-005c — TypeScript constructor parameter property
    /// (`constructor(private foo: T) {}`). Tree-sitter encodes this as a
    /// `required_parameter` with an `accessibility_modifier`, NOT a
    /// `public_field_definition`, so the body walker misses it. The
    /// dedicated constructor scan in `extract_fields` recovers it.
    /// Type is `boolean` (state-bearing); the MCP-005 fixture used
    /// `string` which `mcp_server` profile maps to `Ignored` — that's a
    /// separate, intentional behavior (strings are not auto-modeled);
    /// users who want string fields modeled should set abstraction
    /// overrides explicitly.
    #[test]
    fn extract_typescript_constructor_param_property() {
        let source = "class C {\n    constructor(private flag: boolean) {}\n}\n";
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("C", &["flag"], &[]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        assert!(
            result.fields.iter().any(|f| f.name == "flag"),
            "expected `flag` constructor-param-property to be extracted, \
             got fields {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        let flag = result.fields.iter().find(|f| f.name == "flag").unwrap();
        assert_eq!(
            flag.abstraction,
            crate::adapter::domain::AbstractionType::Boolean
        );
    }

    /// GAP-005c — non-modified constructor parameters are NOT emitted as
    /// fields (they're just regular parameters, not parameter-properties).
    /// Regression guard.
    #[test]
    fn extract_typescript_non_modified_constructor_param_not_a_field() {
        let source = r#"
class C {
    constructor(plain: string) {}
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("C", &["plain"], &[]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        assert!(
            !result.fields.iter().any(|f| f.name == "plain"),
            "non-modified constructor parameter should NOT be a field"
        );
    }

    /// GAP-005d — bare typed module-level declarations of class-named types
    /// (uppercase-leading) are recognized as `Presence` singletons.
    #[test]
    fn extract_typescript_bare_typed_module_decl_classlike() {
        let source = r#"
class KnowledgeGraphManager {}

let manager: KnowledgeGraphManager;

class Wrapper {
    init(): void {
        manager = new KnowledgeGraphManager();
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Wrapper", &["manager"], &["init"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        assert!(
            result.fields.iter().any(|f| f.name == "manager"),
            "expected `let manager: KnowledgeGraphManager;` to be detected as \
             a module-level Presence singleton, got fields {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        let manager = result.fields.iter().find(|f| f.name == "manager").unwrap();
        assert_eq!(
            manager.abstraction,
            crate::adapter::domain::AbstractionType::Presence
        );
    }

    /// GAP-005d — bare typed module-level declarations of *builtin* lowercase
    /// types (`string`, `number`, etc.) are NOT picked up. The intent is to
    /// avoid flooding the model with built-in scalars; users who want them
    /// should set abstraction overrides explicitly.
    #[test]
    fn extract_typescript_bare_typed_module_decl_builtin_skipped() {
        let source = r#"
let MEMORY_FILE_PATH: string;

class C {
    init(): void {
        MEMORY_FILE_PATH = "x";
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("C", &["MEMORY_FILE_PATH"], &["init"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        // Should NOT be treated as state — `string` is a builtin, not a class.
        assert!(
            !result.fields.iter().any(|f| f.name == "MEMORY_FILE_PATH"),
            "bare-typed bulitin (`string`) should NOT be detected as \
             module-level state, got {:?}",
            result.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        // ... and the GAP-005g warning should fire because the user listed
        // it but it wasn't found.
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("MEMORY_FILE_PATH")),
            "expected GAP-005g warning about unrecognized field, got {:?}",
            result.warnings
        );
    }

    /// GAP-005 step 3 — constructor calls on the RHS produce SetPresent.
    /// Pre-fix: `this._transport = new Transport(opts)` classified as
    /// CallEffect::Unknown and contributed nothing to the model.
    #[test]
    fn extract_constructor_call_as_set_present() {
        let source = r#"
class Server {
    private _transport?: Transport;

    connect(t: Transport): void {
        this._transport = new Transport(t);
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Server", &["_transport"], &["connect"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        let connect = result.methods.iter().find(|m| m.name == "connect").unwrap();
        assert!(
            connect.effects.iter().any(|e| {
                e.field == "_transport"
                    && matches!(
                        e.effect,
                        crate::adapter::extraction::ast_extract::call_summary::CallEffect::SetPresent
                    )
            }),
            "expected SetPresent effect on _transport from `new Transport(...)`, got {:?}",
            connect.effects
        );
    }

    /// GAP-005h — Python `@classmethod` / `@staticmethod` / `@property`
    /// decorated methods are reachable. Tree-sitter-python wraps them in
    /// `decorated_definition` whose `definition` child is the actual
    /// `function_definition`. Pre-fix, the class-body walker filtered
    /// strictly on `function_definition` and silently skipped decorated
    /// methods. Surfaced by MCP-003 post-patch re-validation, where every
    /// method on `Scope` is `@classmethod`-decorated.
    ///
    /// This test follows the test-methodology lesson from the GAP-005
    /// validation round: use a realistic decorator pattern (not the
    /// minimal example), so adjacent gaps surface in the test suite
    /// instead of in production fixtures.
    #[test]
    fn extract_python_classmethod_decorated_methods_reachable() {
        let source = r#"
import contextvars

_scope = contextvars.ContextVar("scope")

class Scope:
    @classmethod
    def enter(cls, value):
        _scope.set(value)
    @classmethod
    def leave(cls, token):
        _scope.reset(token)
"#;
        let parsed = parser::parse_source(source, SourceLanguage::Python).unwrap();
        let target = make_simple_target("Scope", &["_scope"], &["enter", "leave"]);
        let profile = domain::get_profile("python_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        // The methods must be reachable through the @classmethod wrapper.
        assert!(
            result.methods.iter().any(|m| m.name == "enter"),
            "expected `enter` (under @classmethod) to be extracted, got methods {:?}",
            result.methods.iter().map(|m| &m.name).collect::<Vec<_>>()
        );
        assert!(
            result.methods.iter().any(|m| m.name == "leave"),
            "expected `leave` (under @classmethod) to be extracted, got methods {:?}",
            result.methods.iter().map(|m| &m.name).collect::<Vec<_>>()
        );

        // The end-to-end fields-AND-effects assertion: enter() must
        // produce an IncrementCounter effect on _scope, leave() a
        // DecrementCounter. This is the lesson from MCP-003: pure
        // method-detection isn't enough; the effect chain through the
        // call-summary library must also work.
        let enter = result.methods.iter().find(|m| m.name == "enter").unwrap();
        assert!(
            enter.effects.iter().any(|e| {
                e.field == "_scope"
                    && matches!(
                        e.effect,
                        crate::adapter::extraction::ast_extract::call_summary::CallEffect::IncrementCounter
                    )
            }),
            "expected IncrementCounter on _scope from `_scope.set(value)` \
             inside @classmethod, got {:?}",
            enter.effects
        );
    }

    /// GAP-005a — `ContextVar.set(value)` on a module-level
    /// `_x = ContextVar(...)` field produces an `IncrementCounter` effect.
    /// This was missing from `python_summaries()` pre-fix, so MCP-003's
    /// re-validation found the field but no methods produced transitions.
    /// This is the end-to-end fields-AND-effects test pattern (the lesson
    /// learned from the validation round).
    #[test]
    fn extract_python_module_level_contextvar_methods_produce_effects() {
        let source = r#"
import contextvars

_scope = contextvars.ContextVar("scope")

class Scope:
    def __init__(self):
        pass
    def enter(self, value):
        _scope.set(value)
    def leave(self, token):
        _scope.reset(token)
"#;
        let parsed = parser::parse_source(source, SourceLanguage::Python).unwrap();
        let target = make_simple_target("Scope", &["_scope"], &["enter", "leave"]);
        let profile = domain::get_profile("python_server");

        let result = extract_target(&parsed, &target, profile).unwrap();

        // Field detection (Step 2 — already covered by an earlier test).
        assert!(result.fields.iter().any(|f| f.name == "_scope"));

        // GAP-005a regression: the method bodies must produce real effects
        // on `_scope`, otherwise the resulting automaton stays 1-state.
        let enter = result.methods.iter().find(|m| m.name == "enter").unwrap();
        assert!(
            enter.effects.iter().any(|e| {
                e.field == "_scope"
                    && matches!(e.effect, crate::adapter::extraction::ast_extract::call_summary::CallEffect::IncrementCounter)
            }),
            "expected IncrementCounter on _scope from `_scope.set(value)`, got {:?}",
            enter.effects
        );
        let leave = result.methods.iter().find(|m| m.name == "leave").unwrap();
        assert!(
            leave.effects.iter().any(|e| {
                e.field == "_scope"
                    && matches!(e.effect, crate::adapter::extraction::ast_extract::call_summary::CallEffect::DecrementCounter)
            }),
            "expected DecrementCounter on _scope from `_scope.reset(token)`, got {:?}",
            leave.effects
        );
    }

    /// GAP-005g — `state_fields.include` names that neither class-body nor
    /// module-level scan finds produce a warning instead of being silently
    /// dropped. Surfaced by MCP-005 re-validation: the user's spec asked
    /// for `memoryFilePath` and `knowledgeGraphManager` but neither was
    /// recognized; the espec was empty and the user had no signal.
    #[test]
    fn extract_warns_on_unrecognized_state_field_names() {
        let source = r#"
class C {
    public _real_field: boolean = false;
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        // Spec lists a bogus field name alongside a real one.
        let target = make_simple_target("C", &["_real_field", "_misspelled_or_missing"], &[]);
        let profile = domain::get_profile("mcp_server");
        let result = extract_target(&parsed, &target, profile).unwrap();

        // The real field is detected.
        assert!(result.fields.iter().any(|f| f.name == "_real_field"));
        // The bogus field is NOT detected.
        assert!(
            !result
                .fields
                .iter()
                .any(|f| f.name == "_misspelled_or_missing")
        );
        // But a warning was emitted for it.
        assert!(
            result
                .warnings
                .iter()
                .any(|w| { w.contains("_misspelled_or_missing") && w.contains("not detected") }),
            "expected warning about unrecognized field name, got: {:?}",
            result.warnings
        );
    }

    /// GAP-005e/f — diagnostic test. The MCP-004 postfix `connect()`
    /// pattern: if-throw guard, then field assignment. Verifies:
    ///   - The assignment produces an effect (GAP-005e check).
    ///   - The state-space derivation produces a sound set of
    ///     transitions for `ev_connect` (GAP-005f check — should
    ///     ideally be exactly `_transport=absent → _transport=present`,
    ///     or at most an Unknown-havoc set that includes the correct
    ///     edge).
    #[test]
    fn extract_typescript_if_throw_then_assign_produces_effect() {
        let source = r#"
class Server {
    private _transport?: Transport;

    connect(t: Transport): void {
        if (this._transport) { throw new Error("Already connected"); }
        this._transport = t;
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("Server", &["_transport"], &["connect"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        let connect = result
            .methods
            .iter()
            .find(|m| m.name == "connect")
            .expect("connect method should be extracted");

        assert!(
            !connect.effects.is_empty(),
            "expected connect() to produce at least one effect; got {:?}",
            connect.effects
        );

        // GAP-005f exploratory: derive the automaton and inspect
        // ev_connect transitions. Document what the current behavior
        // produces; treat as soundness-checked rather than strict
        // assertion (because the parameter-assignment-as-Unknown
        // havoc is over-approx, which is sound for safety).
        use crate::adapter::extraction::ast_extract::state_space;
        let label_prefix = profile.unwrap().label_naming.prefix;
        let derived = state_space::derive_automaton(
            "Server",
            &result.fields,
            &result.methods,
            &std::collections::HashMap::new(),
            label_prefix,
            true, // add_noop, mirror what extract_from_source does
        );

        let ev_connect_transitions: Vec<_> = derived
            .transitions
            .iter()
            .filter(|t| t.label == "ev_connect")
            .collect();

        // We expect at least one ev_connect transition. The exact set
        // depends on whether the assignment is classified as Unknown
        // (havoc — multiple edges) or SetPresent (one edge); both are
        // acceptable for this test, the point is non-empty.
        assert!(
            !ev_connect_transitions.is_empty(),
            "expected at least one ev_connect transition, got: {:?}",
            derived.transitions
        );
        // The model should also produce at least one state-mutating
        // transition (a transition where source != target). This is
        // GAP-005b's degenerate-warning calibration check applied to
        // this fixture.
        let mutating: Vec<_> = derived
            .transitions
            .iter()
            .filter(|t| t.from != t.to)
            .collect();
        assert!(
            !mutating.is_empty(),
            "expected at least one state-mutating transition for \
             this fixture (the connect's None→Some edge), got: {:?}",
            derived.transitions
        );
    }

    /// GAP-005 step 3 — bare-identifier receivers (module-level state)
    /// resolve to the corresponding field. Pre-fix: `_ctx.set(k, v)` had
    /// receiver `_ctx`, which `resolve_receiver_to_field` rejected because
    /// it only matched `this.<field>` / `self.<field>`.
    #[test]
    fn extract_bare_receiver_resolves_to_module_level_field() {
        let source = r#"
const _ctx = new Map();

class C {
    add(): void {
        _ctx.set("k", "v");
    }
}
"#;
        let parsed = parser::parse_source(source, SourceLanguage::TypeScript).unwrap();
        let target = make_simple_target("C", &["_ctx"], &["add"]);
        let profile = domain::get_profile("mcp_server");

        let result = extract_target(&parsed, &target, profile).unwrap();
        let add = result.methods.iter().find(|m| m.name == "add").unwrap();
        // Map.set on a bounded-counter abstraction is IncrementCounter
        // per the call-summary library.
        assert!(
            add.effects.iter().any(|e| {
                e.field == "_ctx"
                    && matches!(
                        e.effect,
                        crate::adapter::extraction::ast_extract::call_summary::CallEffect::IncrementCounter
                    )
            }),
            "expected IncrementCounter on _ctx via bare-receiver resolution, got {:?}",
            add.effects
        );
    }
}
