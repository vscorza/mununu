//! Phase B — auto-detection of concurrency idioms.
//!
//! Given a parsed source file, scans for known concurrency call sites
//! (`asyncio.gather`, `Promise.all`, `multiprocessing.Process`, etc.) and
//! returns structured findings the user can use to populate the
//! `composition.instances[]` / `composition.shared[]` blocks of an
//! extract config.
//!
//! Output format is intentionally **suggestion-grade**, not autoritative:
//! the user reviews each finding and decides what to keep, edit, or
//! discard. Each finding carries provenance (call site, line number,
//! detector identifier) so the audit trail is preserved per the Claims
//! Integrity policy.
//!
//! ## Detection layers (per `phase-b-auto-detection.md` plan)
//!
//! - **B1**: syntactic AST matching (this module's initial scope).
//! - **B2**: shared-label inference from parallelized closure bodies
//!   (TODO; piggybacks on the same call-summary library used by the
//!   existing extractor).
//! - **B3**: resource shape inference (out of scope for v0.1).
//!
//! ## Sources for the detection rules
//!
//! - Python: official asyncio docs (asyncio.gather, asyncio.create_task,
//!   multiprocessing.Process, concurrent.futures.ThreadPoolExecutor).
//! - TypeScript: MDN (Promise.all, Promise.allSettled, Worker).
//! - Concurrency-bug taxonomy: Lu, Park, Seo, Zhou — "Learning from
//!   mistakes: a comprehensive study on real world concurrency bug
//!   characteristics" (ASPLOS 2008). Categories: atomicity violations,
//!   order violations, deadlock.
//! - Model-extraction precedent: Corbett, Dwyer, Hatcliff, Laubach,
//!   Pasareanu, Robby, Zheng — "Bandera: extracting finite-state models
//!   from Java source code" (ICSE 2000). Specifically, the
//!   "Threads & Synchronization Detection" module is the academic
//!   ancestor of this work.

use super::parser::{ParsedSource, SourceLanguage};
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

/// One detected concurrency idiom in source. The detector produces a
/// list of these per file scan; each one is a **suggestion** the user
/// can promote into a `composition.instances[]` / `shared[]` block (or
/// discard, or edit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedConcurrency {
    /// Identifier of the detector that produced this finding (e.g.,
    /// `"python_asyncio_gather"`, `"typescript_promise_all"`). Provides
    /// the audit-trail anchor for the Claims Integrity policy.
    pub detector_id: String,
    /// One-line natural-language description ("asyncio.gather over 2
    /// coroutines"). Surfaced verbatim in CLI / API / UI output.
    pub description: String,
    /// Source-line where the idiom was found (1-indexed). The user
    /// should be able to jump to it in their editor.
    pub line: u32,
    /// Number of parallel branches detected (e.g., 2 for `gather(f, g)`).
    /// `None` if dynamic (the call uses splat / spread, e.g.,
    /// `asyncio.gather(*tasks)`); user must enumerate manually.
    pub branch_count: Option<u32>,
    /// Stub instance names the user can rename (`task_0`, `task_1`, ...).
    /// Empty when `branch_count` is None.
    pub suggested_instance_names: Vec<String>,
    /// Best-effort suggestion for the parallelized closures' identity —
    /// the receiver name, the function name, or whatever short label
    /// the user can use to find the relevant code. Heuristic; may be
    /// `None` when the call site is a complex expression.
    pub suggested_class_hint: Option<String>,
}

/// Top-level entry: scan a parsed source file for concurrency idioms
/// known to the registry for the source's language. Returns all findings
/// in source order. An empty result is the common case (no concurrency
/// patterns present, e.g., a plain data class).
pub fn detect_concurrency(parsed: &ParsedSource) -> Vec<DetectedConcurrency> {
    let mut findings = Vec::new();
    match parsed.language {
        SourceLanguage::Python => {
            scan_python(parsed, &mut findings);
        }
        SourceLanguage::TypeScript => {
            scan_typescript(parsed, &mut findings);
        }
        SourceLanguage::Rust | SourceLanguage::GDScript => {
            // Out of scope for v0.1.
        }
    }
    findings
}

/// Walk the Python AST for known concurrency call signatures. Currently
/// recognizes `asyncio.gather(...)`. Extends to `asyncio.create_task`,
/// `multiprocessing.Process`, and `ThreadPoolExecutor.submit` in later
/// commits.
fn scan_python(parsed: &ParsedSource, out: &mut Vec<DetectedConcurrency>) {
    let root = parsed.tree.root_node();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        // tree-sitter-python: `call` node with `function: attribute`
        // child (`asyncio.gather`) and `arguments: argument_list` child.
        if node.kind() == "call" {
            if let Some(finding) = match_python_asyncio_gather(parsed, &node) {
                out.push(finding);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Match `asyncio.gather(arg, arg, ...)`. Returns a finding with the
/// branch count + stub instance names when matched, otherwise None.
fn match_python_asyncio_gather(
    parsed: &ParsedSource,
    call_node: &Node,
) -> Option<DetectedConcurrency> {
    let function = call_node.child_by_field_name("function")?;
    if function.kind() != "attribute" {
        return None;
    }
    // `attribute` has children: object, dot, attribute (the rightmost
    // identifier). For `asyncio.gather`, object is `asyncio` and the
    // rightmost identifier is `gather`.
    let object = function.child_by_field_name("object")?;
    let attr = function.child_by_field_name("attribute")?;
    if parsed.node_text(&object) != "asyncio" {
        return None;
    }
    if parsed.node_text(&attr) != "gather" {
        return None;
    }

    let arguments = call_node.child_by_field_name("arguments")?;
    // arguments child is `argument_list`. Count children that aren't
    // delimiters / keyword args.
    let (count, dynamic, hint) = analyze_python_arguments(parsed, &arguments);
    let line = parsed.node_line(call_node);

    let (branch_count, suggested_instance_names) = if dynamic {
        (None, Vec::new())
    } else {
        let names = (0..count).map(|i| format!("task_{i}")).collect();
        (Some(count as u32), names)
    };

    Some(DetectedConcurrency {
        detector_id: "python_asyncio_gather".to_string(),
        description: if dynamic {
            "asyncio.gather over a dynamic argument list (splat / spread); branch count not statically known".to_string()
        } else {
            format!("asyncio.gather over {count} coroutine(s)")
        },
        line,
        branch_count,
        suggested_instance_names,
        suggested_class_hint: hint,
    })
}

/// Walk an `argument_list` and return `(count, dynamic, class_hint)`.
/// `dynamic` is true when any argument is a splat (`*tasks`); the
/// branch count cannot be statically determined in that case.
/// `class_hint` is a best-effort guess at what class the parallelized
/// calls touch — falls back to None for complex expressions.
fn analyze_python_arguments(
    parsed: &ParsedSource,
    arguments: &Node,
) -> (usize, bool, Option<String>) {
    let mut count = 0usize;
    let mut dynamic = false;
    let mut hints: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = arguments.walk();
    for child in arguments.children(&mut cursor) {
        let kind = child.kind();
        // Skip delimiters and keyword args (return_exceptions=True etc.).
        if matches!(kind, "(" | ")" | ",") || kind == "keyword_argument" {
            continue;
        }
        // Splat or list-spread: dynamic count.
        if kind == "list_splat" || kind == "dictionary_splat" {
            dynamic = true;
            continue;
        }
        count += 1;
        // Best-effort class-name hint: if the argument is a method call
        // on a receiver, record the receiver text as a hint.
        if kind == "call" {
            if let Some(func) = child.child_by_field_name("function") {
                if func.kind() == "attribute" {
                    if let Some(receiver) = func.child_by_field_name("object") {
                        let r = parsed.node_text(&receiver).to_string();
                        // Skip self.* — that's the extracted class itself,
                        // not a useful hint about a separate dependency.
                        if r != "self" && !r.is_empty() {
                            hints.insert(r);
                        }
                    }
                }
            }
        }
    }
    let hint = if hints.len() == 1 {
        hints.into_iter().next()
    } else {
        // Multiple distinct receivers (or none) — don't emit a misleading hint.
        None
    };
    (count, dynamic, hint)
}

/// Walk the TypeScript AST for known concurrency call signatures.
/// Currently recognizes `Promise.all(...)` and `Promise.allSettled(...)`.
/// Extends to `new Worker(...)` and `setInterval` in later commits.
fn scan_typescript(parsed: &ParsedSource, out: &mut Vec<DetectedConcurrency>) {
    let root = parsed.tree.root_node();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        // tree-sitter-typescript: `call_expression` with `function:
        // member_expression` (Promise.all) and `arguments` (a single
        // array literal in the canonical case).
        if node.kind() == "call_expression" {
            if let Some(finding) = match_typescript_promise_all(parsed, &node) {
                out.push(finding);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Match `Promise.all([arg, arg, ...])` and `Promise.allSettled([...])`.
/// The two methods have identical detection shape — they differ only in
/// error semantics (allSettled doesn't short-circuit on first rejection)
/// which has no bearing on the modeled topology.
fn match_typescript_promise_all(
    parsed: &ParsedSource,
    call_node: &Node,
) -> Option<DetectedConcurrency> {
    let function = call_node.child_by_field_name("function")?;
    if function.kind() != "member_expression" {
        return None;
    }
    let object = function.child_by_field_name("object")?;
    let property = function.child_by_field_name("property")?;
    if parsed.node_text(&object) != "Promise" {
        return None;
    }
    let method_name = parsed.node_text(&property);
    if method_name != "all" && method_name != "allSettled" {
        return None;
    }

    let arguments = call_node.child_by_field_name("arguments")?;
    // arguments is `arguments` containing one or more values. The
    // canonical Promise.all takes a single iterable argument (typically
    // an array literal). When the argument is a literal array, count
    // its elements. When it's anything else (variable, spread, generator
    // call), treat as dynamic.
    let (count, dynamic, hint) = analyze_typescript_arguments(parsed, &arguments);
    let line = parsed.node_line(call_node);

    let (branch_count, suggested_instance_names) = if dynamic {
        (None, Vec::new())
    } else {
        let names = (0..count).map(|i| format!("task_{i}")).collect();
        (Some(count as u32), names)
    };

    let detector_id = format!("typescript_promise_{method_name}");

    Some(DetectedConcurrency {
        detector_id,
        description: if dynamic {
            format!(
                "Promise.{method_name} over a non-literal iterable; branch count not statically known"
            )
        } else {
            format!("Promise.{method_name} over {count} promise(s)")
        },
        line,
        branch_count,
        suggested_instance_names,
        suggested_class_hint: hint,
    })
}

/// Walk a TS `arguments` node for a `Promise.all(...)` call. Returns
/// `(count, dynamic, class_hint)`. The canonical case is a single
/// array literal with N elements; other shapes (variables, generators,
/// computed iterables) collapse to dynamic.
fn analyze_typescript_arguments(
    parsed: &ParsedSource,
    arguments: &Node,
) -> (usize, bool, Option<String>) {
    // Find the first non-delimiter child.
    let mut cursor = arguments.walk();
    let mut first_arg: Option<Node> = None;
    for child in arguments.children(&mut cursor) {
        if matches!(child.kind(), "(" | ")" | ",") {
            continue;
        }
        first_arg = Some(child);
        break;
    }
    let arg = match first_arg {
        Some(a) => a,
        None => return (0, false, None),
    };

    // Canonical case: array literal `[...]`. Walk its elements.
    if arg.kind() == "array" {
        let mut count = 0usize;
        let mut dynamic = false;
        let mut hints: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut a_cursor = arg.walk();
        for elem in arg.children(&mut a_cursor) {
            let kind = elem.kind();
            if matches!(kind, "[" | "]" | ",") {
                continue;
            }
            if kind == "spread_element" {
                dynamic = true;
                continue;
            }
            count += 1;
            // Best-effort class-hint: if the element is a method call
            // on a receiver, record that receiver. Skip `this.*` since
            // that's the extracted class itself.
            if kind == "call_expression" {
                if let Some(func) = elem.child_by_field_name("function") {
                    if func.kind() == "member_expression" {
                        if let Some(receiver) = func.child_by_field_name("object") {
                            let r = parsed.node_text(&receiver).to_string();
                            if r != "this" && !r.is_empty() {
                                hints.insert(r);
                            }
                        }
                    }
                }
            }
        }
        let hint = if hints.len() == 1 {
            hints.into_iter().next()
        } else {
            None
        };
        return (count, dynamic, hint);
    }

    // Non-literal argument (variable, generator call, spread): dynamic.
    (0, true, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::extraction::ast_extract::parser::{SourceLanguage, parse_source};

    #[test]
    fn detect_python_asyncio_gather_two_calls() {
        let source = r#"
import asyncio

class Worker:
    async def run(self):
        await asyncio.gather(scope.enter("a"), scope.enter("b"))
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(
            findings.len(),
            1,
            "expected one gather finding, got {findings:?}"
        );
        let f = &findings[0];
        assert_eq!(f.detector_id, "python_asyncio_gather");
        assert_eq!(f.branch_count, Some(2));
        assert_eq!(f.suggested_instance_names, vec!["task_0", "task_1"]);
        // Both arguments are method calls on `scope`; expect that as the hint.
        assert_eq!(f.suggested_class_hint.as_deref(), Some("scope"));
    }

    #[test]
    fn detect_python_asyncio_gather_three_calls_distinct_receivers() {
        // Three calls on different receivers — no single class hint.
        let source = r#"
import asyncio

async def main():
    await asyncio.gather(a.f(), b.g(), c.h())
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].branch_count, Some(3));
        assert!(
            findings[0].suggested_class_hint.is_none(),
            "with multiple distinct receivers, hint should be None"
        );
    }

    #[test]
    fn detect_python_asyncio_gather_dynamic_splat() {
        // *tasks: dynamic argument list. Branch count unknown.
        let source = r#"
import asyncio

async def main(tasks):
    await asyncio.gather(*tasks)
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].branch_count.is_none(),
            "splat args produce dynamic branch count"
        );
        assert!(findings[0].suggested_instance_names.is_empty());
        assert!(findings[0].description.contains("dynamic"));
    }

    #[test]
    fn detect_python_no_gather_returns_empty() {
        let source = r#"
import asyncio

class Worker:
    async def run(self):
        result = await self.fetch()
        return result
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        let findings = detect_concurrency(&parsed);
        assert!(findings.is_empty(), "no gather call -> no findings");
    }

    #[test]
    fn detect_python_gather_with_keyword_argument() {
        // `return_exceptions=True` is a keyword arg, not a coroutine.
        let source = r#"
import asyncio

async def main():
    await asyncio.gather(f(), g(), return_exceptions=True)
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(findings.len(), 1);
        // The keyword arg is excluded from the count.
        assert_eq!(findings[0].branch_count, Some(2));
    }

    #[test]
    fn detect_only_asyncio_gather_not_other_attribute_calls() {
        // `asyncio.run(main())` and `asyncio.create_task(...)` are NOT
        // gather; the detector should ignore them. (create_task is a
        // future Phase B detector, not this one.)
        let source = r#"
import asyncio

async def main():
    return await asyncio.run(other())
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        let findings = detect_concurrency(&parsed);
        assert!(
            findings.is_empty(),
            "asyncio.run is not asyncio.gather; detector must not produce a false positive"
        );
    }

    #[test]
    fn detect_typescript_promise_all_two_calls() {
        let source = r#"
class Worker {
    async run(): Promise<void> {
        await Promise.all([client.send("a"), client.send("b")]);
    }
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(
            findings.len(),
            1,
            "expected one Promise.all finding, got {findings:?}"
        );
        let f = &findings[0];
        assert_eq!(f.detector_id, "typescript_promise_all");
        assert_eq!(f.branch_count, Some(2));
        assert_eq!(f.suggested_instance_names, vec!["task_0", "task_1"]);
        assert_eq!(f.suggested_class_hint.as_deref(), Some("client"));
    }

    #[test]
    fn detect_typescript_promise_all_settled() {
        // allSettled has identical detection shape; only error semantics
        // differ, which doesn't affect the modeled topology.
        let source = r#"
async function run() {
    const results = await Promise.allSettled([a.fetch(), b.fetch(), c.fetch()]);
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].detector_id, "typescript_promise_allSettled");
        assert_eq!(findings[0].branch_count, Some(3));
        assert!(
            findings[0].suggested_class_hint.is_none(),
            "three distinct receivers — no consensus hint"
        );
    }

    #[test]
    fn detect_typescript_promise_all_with_spread() {
        // Spread element makes count dynamic.
        let source = r#"
async function run(tasks: Promise<void>[]) {
    await Promise.all([...tasks, extra()]);
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].branch_count.is_none(),
            "spread element produces dynamic branch count"
        );
    }

    #[test]
    fn detect_typescript_promise_all_with_variable_argument() {
        // Non-literal argument — branch count unknown.
        let source = r#"
async function run(tasks: Promise<void>[]) {
    await Promise.all(tasks);
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let findings = detect_concurrency(&parsed);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].branch_count.is_none());
    }

    #[test]
    fn detect_typescript_no_promise_all_returns_empty() {
        let source = r#"
class Worker {
    async run(): Promise<string> {
        return await this.fetch();
    }
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let findings = detect_concurrency(&parsed);
        assert!(findings.is_empty());
    }

    #[test]
    fn detect_typescript_only_promise_all_not_other_member_calls() {
        // Promise.resolve / Promise.race etc. should not match.
        let source = r#"
async function run() {
    return await Promise.resolve(42);
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let findings = detect_concurrency(&parsed);
        assert!(
            findings.is_empty(),
            "Promise.resolve must not match the all/allSettled detector"
        );
    }
}
