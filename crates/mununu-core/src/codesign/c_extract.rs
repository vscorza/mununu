//! C extraction via clang shell-out — Document C task C5, slice 2.a.
//!
//! Slice 2.a of C5. Reads a C source file via a subprocess shell-out
//! to `clang -Xclang -ast-dump=json -fsyntax-only` and lifts the user-
//! authored function declarations plus their `@mununu_*` annotations
//! into a [`CExtraction`] record. The annotations are extracted via
//! the slice-1 grammar at [`crate::mununu_annotations::extract_from_c_source`]
//! and matched to functions by line proximity (each Doxygen block sits
//! immediately above the declaration it annotates).
//!
//! ## Why shell-out rather than `clang-sys`
//!
//! The pre-flight investigation at
//! `.claude/plans/scoping-logs/c5-libclang-c-extraction.md` chose
//! shell-out over in-process `clang-sys` because:
//!
//! - Zero new Rust build-time dependencies. Yosys is already invoked
//!   as a subprocess (see [`crate::adapter::yosys`]); the precedent
//!   is established.
//! - `clang` is universally available on dev machines (Apple Command
//!   Line Tools, `apt install clang`, MSVC + LLVM). Library-version
//!   matching is the user's problem, not mununu's build system's.
//! - The AST-dump JSON schema has been stable across clang versions
//!   since ~10. We parse only the fields we need (`kind`, `name`,
//!   `loc.file`, `loc.line`); we tolerate unknown fields.
//! - Mununu makes one parse call per codesign workflow. The
//!   subprocess startup cost is irrelevant at that volume.
//!
//! ## Slice 2.a scope (what's in)
//!
//! - **Function declarations** in the user's file. System-header
//!   functions are filtered out by comparing `loc.file` to the
//!   user-supplied source path.
//! - **Doxygen annotation lifting** via the slice-1 grammar. Each
//!   declaration gets the annotations whose source line falls in the
//!   3-line window immediately above the declaration line (Doxygen
//!   convention).
//! - **A CLI subcommand** `mununu codesign extract-c <file.c>` that
//!   emits one JSON record per function and a JSON array of warnings.
//!
//! ## What's out of scope (slice 2.b and beyond)
//!
//! - **Function body extraction into a CTXDSL automaton.** Doc C
//!   §C.9.5's "register access / polling loops / ISR handlers"
//!   semantics. Slice 2.b's job; needs the body-walking + control-
//!   flow analysis that Doc C §C.5 names as the soundness frontier.
//! - **Type resolution / sizeof.** Slice 2.a only looks at function
//!   *declarations*; it doesn't need types beyond the qualified
//!   identifier already in the JSON.
//! - **Macro expansion.** clang has already expanded macros by the
//!   time we see the AST. Macro-only annotations live or die by
//!   whether the user's preprocessor reaches them, which is a clang
//!   command-line concern (the user passes `-DHAVE_FOO=1` etc.).
//! - **Function pointers, recursion, vendor extensions.** Doc C
//!   §C.9.5 explicitly defers these.
//!
//! ## Soundness posture
//!
//! Slice 2.a is **descriptive**: it surfaces what the C source says
//! at the declaration level. It does not infer behaviour. The
//! `@mununu_guarantee` / `@mununu_assume` clauses the extractor
//! lifts are **vendor / firmware-engineer claims about the function**
//! — the HITL review surface at [`crate::contract::review`] is the
//! gate that turns them into accepted contract clauses (Doc A §A7).

use crate::mununu_annotations::{MununuAnnotation, extract_from_c_source};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::process::Command;

/// Errors raised by [`extract_c_via_clang`].
#[derive(Debug)]
pub enum CExtractError {
    /// `clang` could not be spawned (not installed, not in PATH).
    ClangNotFound { tried: String, message: String },
    /// `clang` ran but returned a non-zero exit code.
    ClangFailed {
        status: String,
        stderr: String,
        invocation: String,
    },
    /// `clang` ran cleanly but produced output that wasn't parseable
    /// as JSON.
    AstJsonInvalid(String),
    /// Failed to read the source file (needed alongside the AST for
    /// annotation extraction).
    SourceReadFailed { path: PathBuf, message: String },
    /// Failed to write a temporary file for clang to read.
    TempFileFailed(String),
}

impl fmt::Display for CExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CExtractError::ClangNotFound { tried, message } => write!(
                f,
                "could not spawn clang (tried `{tried}`): {message}. Install clang via xcode-select --install (macOS) or `apt install clang` (Linux), or pass --clang <path>."
            ),
            CExtractError::ClangFailed {
                status,
                stderr,
                invocation,
            } => write!(
                f,
                "clang exited {status}\ninvocation: {invocation}\nstderr:\n{stderr}"
            ),
            CExtractError::AstJsonInvalid(msg) => {
                write!(f, "clang AST output failed to parse as JSON: {msg}")
            }
            CExtractError::SourceReadFailed { path, message } => write!(
                f,
                "failed to read source file {}: {message}",
                path.display()
            ),
            CExtractError::TempFileFailed(msg) => write!(f, "temp file: {msg}"),
        }
    }
}

impl std::error::Error for CExtractError {}

/// Non-fatal observations from [`extract_c_via_clang`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CExtractWarning {
    /// A `@mununu_*` annotation was present in the source but no
    /// function declaration sat within the proximity window beneath
    /// it. The annotation is preserved in the output's `orphan_annotations`
    /// list for the user to action.
    OrphanAnnotation {
        tag: String,
        value: String,
        source_line: u32,
    },
    /// clang AST node kind `kind` was encountered but ignored — slice
    /// 2.a only walks `FunctionDecl` today. Surfaced once per
    /// distinct kind so the user knows what wasn't lifted.
    UnhandledKind { kind: String },
}

impl fmt::Display for CExtractWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CExtractWarning::OrphanAnnotation {
                tag,
                value,
                source_line,
            } => write!(
                f,
                "orphan annotation @mununu_{tag} (line {source_line}, value='{value}'): no function declaration in the proximity window beneath"
            ),
            CExtractWarning::UnhandledKind { kind } => write!(
                f,
                "AST node kind `{kind}` is not lifted by slice 2.a (function bodies + types are slice 2.b)"
            ),
        }
    }
}

/// A single C function declaration lifted by the extractor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CFunctionDecl {
    /// The function's qualified name as it appears in C source.
    pub name: String,
    /// The function's `qualType` string from clang's AST (e.g.
    /// `"void (uint8_t)"`). Stored verbatim — slice 2.a doesn't
    /// parse it; the HITL review surface can show it raw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// 1-based line number of the function declaration in the source
    /// file.
    pub source_line: u32,
    /// `@mununu_*` annotations attached to this function via a
    /// Doxygen block, single-line `/* */`, or `//` comment in the
    /// 3-line proximity window above the declaration.
    pub annotations: Vec<MununuAnnotation>,
}

/// Output of [`extract_c_via_clang`] — every user-defined function
/// declaration in the source, plus the warnings and orphan
/// annotations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CExtraction {
    /// Functions in declaration order (lowest `source_line` first).
    pub functions: Vec<CFunctionDecl>,
    /// `@mununu_*` annotations whose Doxygen block didn't sit
    /// immediately above a function declaration. Preserved so the
    /// user can see them in the output and decide what to do.
    pub orphan_annotations: Vec<MununuAnnotation>,
    /// Non-fatal warnings from the extraction.
    pub warnings: Vec<CExtractWarning>,
}

/// Configuration for [`extract_c_via_clang`].
#[derive(Debug, Clone, Default)]
pub struct CExtractOptions {
    /// Path to the `clang` binary. Default: `clang` (resolved via
    /// `PATH`).
    pub clang_path: Option<PathBuf>,
    /// Additional include paths to pass to clang as `-I`. Useful when
    /// the source file pulls in headers from a non-system location
    /// (firmware SDKs typically need this).
    pub include_paths: Vec<PathBuf>,
    /// Additional preprocessor defines to pass to clang as `-D`. The
    /// values are passed verbatim, e.g. `["HAVE_DMA=1", "F_CPU=84000000"]`.
    pub defines: Vec<String>,
    /// Additional raw arguments to pass to clang. Power users only.
    pub extra_clang_args: Vec<String>,
}

/// Extract C function declarations + their `@mununu_*` annotations
/// from a source file via clang's `-ast-dump=json` mode.
///
/// `source_path` is the path of the file to extract from (so we can
/// filter system-header functions out of the AST) and is read into
/// memory for the annotation pass; the clang process reads the same
/// path independently.
pub fn extract_c_via_clang(
    source_path: &std::path::Path,
    options: &CExtractOptions,
) -> Result<CExtraction, CExtractError> {
    let source_text =
        std::fs::read_to_string(source_path).map_err(|e| CExtractError::SourceReadFailed {
            path: source_path.to_path_buf(),
            message: e.to_string(),
        })?;

    let clang = options
        .clang_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("clang"));

    let mut cmd = Command::new(&clang);
    cmd.arg("-Xclang")
        .arg("-ast-dump=json")
        .arg("-fsyntax-only")
        .arg("-fno-color-diagnostics");
    for inc in &options.include_paths {
        cmd.arg("-I").arg(inc);
    }
    for define in &options.defines {
        cmd.arg(format!("-D{define}"));
    }
    for raw in &options.extra_clang_args {
        cmd.arg(raw);
    }
    cmd.arg(source_path);

    let invocation = format!("{:?}", cmd);
    let output = cmd.output().map_err(|e| CExtractError::ClangNotFound {
        tried: clang.display().to_string(),
        message: e.to_string(),
    })?;

    if !output.status.success() {
        // clang errors and warnings go to stderr; the AST goes to
        // stdout. Surface stderr so the user can see what went wrong.
        return Err(CExtractError::ClangFailed {
            status: format!("{}", output.status),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            invocation,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    extract_from_ast_json(&stdout, &source_text, source_path)
}

/// Pure-function half of the extractor: takes already-parsed AST JSON
/// (as a string) + the source text + the source path, and returns
/// the [`CExtraction`]. Separated from [`extract_c_via_clang`] so
/// tests can drive it without depending on `clang` being on `PATH`.
pub fn extract_from_ast_json(
    ast_json: &str,
    source_text: &str,
    source_path: &std::path::Path,
) -> Result<CExtraction, CExtractError> {
    let ast: serde_json::Value =
        serde_json::from_str(ast_json).map_err(|e| CExtractError::AstJsonInvalid(e.to_string()))?;

    // Find user-defined function declarations in the AST. Clang's
    // root is a `TranslationUnitDecl`; we walk its `inner[]` once,
    // not recursively, because function declarations sit at TU
    // scope (slice 2.a doesn't lift class methods or nested
    // declarations).
    let source_file_name = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let mut functions: Vec<CFunctionDecl> = Vec::new();
    let mut unhandled_kinds: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut current_loc_file: Option<String> = None;

    if let Some(inner) = ast.get("inner").and_then(|i| i.as_array()) {
        for node in inner {
            let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            // Update the "current file" hint — clang's AST uses a
            // sparse `loc.file` field: only the first declaration in
            // each file carries the full path; subsequent nodes
            // inherit it implicitly. We track that ourselves.
            if let Some(file) = node
                .get("loc")
                .and_then(|l| l.get("file"))
                .and_then(|f| f.as_str())
            {
                current_loc_file = Some(file.to_string());
            }
            match kind {
                "FunctionDecl" => {
                    // Filter: only emit functions whose loc.file
                    // matches the source we extracted from. clang
                    // emits the entire translation unit including
                    // system headers; we only want the user's file.
                    let from_user_file = matches!(
                        current_loc_file.as_deref(),
                        Some(file) if file.ends_with(source_file_name)
                    );
                    if !from_user_file {
                        continue;
                    }
                    let name = node
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("<unnamed>")
                        .to_string();
                    let signature = node
                        .get("type")
                        .and_then(|t| t.get("qualType"))
                        .and_then(|q| q.as_str())
                        .map(str::to_string);
                    let source_line = node
                        .get("loc")
                        .and_then(|l| l.get("line"))
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0) as u32;
                    functions.push(CFunctionDecl {
                        name,
                        signature,
                        source_line,
                        annotations: Vec::new(),
                    });
                }
                // Slice 2.a deliberately ignores everything else.
                // Surface a single warning per distinct kind.
                k if !k.is_empty()
                    && !matches!(
                        k,
                        "TypedefDecl"
                            | "TranslationUnitDecl"
                            | "BuiltinType"
                            | "Pointer"
                            | "ElaboratedType"
                            | "RecordType"
                            | "BuiltinTemplateDecl"
                            | "TypeAliasTemplateDecl"
                    ) =>
                {
                    // Same filter — only warn for user-file kinds, so
                    // the user doesn't get noise about system headers.
                    let from_user_file = matches!(
                        current_loc_file.as_deref(),
                        Some(file) if file.ends_with(source_file_name)
                    );
                    if from_user_file {
                        unhandled_kinds.insert(k.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    // Annotation pass: run slice-1's parser, then attach each
    // annotation to the function declaration immediately below it
    // (proximity window: annotation.line < function.source_line and
    // function.source_line - annotation.line <= 6).
    let annotations = extract_from_c_source(source_text);
    let mut used_annotation_idx: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    for func in &mut functions {
        for (idx, ann) in annotations.iter().enumerate() {
            if used_annotation_idx.contains(&idx) {
                continue;
            }
            if let Some(ann_line) = ann.source_line
                && ann_line < func.source_line
                && func.source_line - ann_line <= 6
            {
                func.annotations.push(ann.clone());
                used_annotation_idx.insert(idx);
            }
        }
    }

    let orphan_annotations: Vec<MununuAnnotation> = annotations
        .iter()
        .enumerate()
        .filter(|(idx, _)| !used_annotation_idx.contains(idx))
        .map(|(_, a)| a.clone())
        .collect();

    let mut warnings: Vec<CExtractWarning> = Vec::new();
    for orphan in &orphan_annotations {
        warnings.push(CExtractWarning::OrphanAnnotation {
            tag: orphan.tag.name().to_string(),
            value: orphan.value.clone(),
            source_line: orphan.source_line.unwrap_or(0),
        });
    }
    for kind in unhandled_kinds {
        warnings.push(CExtractWarning::UnhandledKind { kind });
    }

    // Sort functions by source line for stable, human-readable output.
    functions.sort_by_key(|f| f.source_line);

    Ok(CExtraction {
        functions,
        orphan_annotations,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mununu_annotations::MununuTag;

    fn fake_ast_json(funcs: &[(&str, u32, &str, &str)]) -> String {
        // Build a minimal TranslationUnitDecl with the given function
        // declarations. `funcs` items are (name, source_line,
        // file_path, qualType).
        let inner: Vec<serde_json::Value> = funcs
            .iter()
            .map(|(name, line, file, qual_type)| {
                serde_json::json!({
                    "id": "0x100",
                    "kind": "FunctionDecl",
                    "name": name,
                    "loc": { "file": file, "line": line },
                    "type": { "qualType": qual_type }
                })
            })
            .collect();
        serde_json::json!({
            "id": "0x0",
            "kind": "TranslationUnitDecl",
            "loc": {},
            "inner": inner,
        })
        .to_string()
    }

    #[test]
    fn extracts_a_user_file_function_declaration() {
        let source = "void foo(void);\n";
        let ast = fake_ast_json(&[("foo", 1, "uart.c", "void (void)")]);
        let extraction =
            extract_from_ast_json(&ast, source, std::path::Path::new("uart.c")).unwrap();
        assert_eq!(extraction.functions.len(), 1);
        assert_eq!(extraction.functions[0].name, "foo");
        assert_eq!(
            extraction.functions[0].signature.as_deref(),
            Some("void (void)")
        );
        assert_eq!(extraction.functions[0].source_line, 1);
        assert!(extraction.functions[0].annotations.is_empty());
    }

    #[test]
    fn filters_out_functions_from_other_files() {
        // clang emits FunctionDecls for system-header symbols too.
        // We only want the user's file.
        let source = "void user_fn(void);\n";
        let ast = fake_ast_json(&[
            ("user_fn", 1, "uart.c", "void (void)"),
            (
                "printf",
                12,
                "/usr/include/stdio.h",
                "int (const char *, ...)",
            ),
        ]);
        let extraction =
            extract_from_ast_json(&ast, source, std::path::Path::new("uart.c")).unwrap();
        assert_eq!(extraction.functions.len(), 1);
        assert_eq!(extraction.functions[0].name, "user_fn");
    }

    #[test]
    fn lifts_doxygen_annotation_above_function() {
        let source = "/**\n\
                      * @mununu_guarantee G(start -> eventually done)\n\
                      */\n\
                      void uart_send(uint8_t byte);\n";
        let ast = fake_ast_json(&[("uart_send", 4, "uart.c", "void (uint8_t)")]);
        let extraction =
            extract_from_ast_json(&ast, source, std::path::Path::new("uart.c")).unwrap();
        assert_eq!(extraction.functions.len(), 1);
        assert_eq!(extraction.functions[0].annotations.len(), 1);
        let ann = &extraction.functions[0].annotations[0];
        assert_eq!(ann.tag, MununuTag::Guarantee);
        assert_eq!(ann.value, "G(start -> eventually done)");
    }

    #[test]
    fn lifts_multiple_annotations_from_one_doxygen_block() {
        let source = "/**\n\
                      * @brief Sends a byte\n\
                      * @mununu_guarantee G(start -> eventually done)\n\
                      * @mununu_assume    G(start -> !reset)\n\
                      */\n\
                      void uart_send(uint8_t byte);\n";
        // Doxygen block is lines 1-5; the function decl is on line 6.
        let ast = fake_ast_json(&[("uart_send", 6, "uart.c", "void (uint8_t)")]);
        let extraction =
            extract_from_ast_json(&ast, source, std::path::Path::new("uart.c")).unwrap();
        assert_eq!(extraction.functions[0].annotations.len(), 2);
    }

    #[test]
    fn proximity_window_caps_at_6_lines() {
        // Annotation more than 6 lines above the declaration → orphan.
        let mut source = String::new();
        source.push_str("// @mununu_assume G(p -> q)\n"); // line 1
        for _ in 0..10 {
            source.push('\n');
        }
        source.push_str("void fn(void);\n"); // line 12
        let ast = fake_ast_json(&[("fn", 12, "f.c", "void (void)")]);
        let extraction = extract_from_ast_json(&ast, &source, std::path::Path::new("f.c")).unwrap();
        assert!(
            extraction.functions[0].annotations.is_empty(),
            "annotation 11 lines above is out of range"
        );
        assert_eq!(extraction.orphan_annotations.len(), 1);
        assert!(matches!(
            extraction.warnings.first(),
            Some(CExtractWarning::OrphanAnnotation { .. })
        ));
    }

    #[test]
    fn annotation_used_by_one_function_isnt_reattached_to_a_later_one() {
        let source = "/**\n\
                      * @mununu_guarantee G(a -> b)\n\
                      */\n\
                      void first(void);\n\
                      \n\
                      void second(void);\n";
        let ast = fake_ast_json(&[
            ("first", 4, "f.c", "void (void)"),
            ("second", 6, "f.c", "void (void)"),
        ]);
        let extraction = extract_from_ast_json(&ast, source, std::path::Path::new("f.c")).unwrap();
        assert_eq!(extraction.functions[0].annotations.len(), 1);
        assert_eq!(extraction.functions[1].annotations.len(), 0);
    }

    #[test]
    fn orphan_annotation_with_no_function_below_surfaces_in_warnings() {
        let source = "// @mununu_blackbox\n\
                      // (no function below)\n";
        let ast = fake_ast_json(&[]);
        let extraction = extract_from_ast_json(&ast, source, std::path::Path::new("f.c")).unwrap();
        assert_eq!(extraction.functions.len(), 0);
        assert_eq!(extraction.orphan_annotations.len(), 1);
        assert_eq!(extraction.warnings.len(), 1);
    }

    #[test]
    fn invalid_ast_json_produces_error() {
        let result = extract_from_ast_json("{not valid json", "", std::path::Path::new("f.c"));
        assert!(matches!(result, Err(CExtractError::AstJsonInvalid(_))));
    }

    #[test]
    fn empty_translation_unit_returns_no_functions_no_orphans() {
        let ast = r#"{"kind": "TranslationUnitDecl", "inner": []}"#;
        let extraction = extract_from_ast_json(ast, "", std::path::Path::new("f.c")).unwrap();
        assert!(extraction.functions.is_empty());
        assert!(extraction.orphan_annotations.is_empty());
        assert!(extraction.warnings.is_empty());
    }

    #[test]
    fn user_file_match_uses_filename_not_path() {
        // clang's loc.file may be a full path (`/Users/.../uart.c`)
        // or a relative path (`uart.c`). The match should be on the
        // filename suffix.
        let source = "void foo(void);\n";
        let ast = fake_ast_json(&[("foo", 1, "/Users/some/path/uart.c", "void (void)")]);
        let extraction =
            extract_from_ast_json(&ast, source, std::path::Path::new("uart.c")).unwrap();
        assert_eq!(extraction.functions.len(), 1);
    }

    #[test]
    fn functions_sorted_by_source_line() {
        let source = "void a(void);\n\nvoid b(void);\n";
        // Feed them in reverse order on purpose.
        let ast = fake_ast_json(&[
            ("b", 3, "f.c", "void (void)"),
            ("a", 1, "f.c", "void (void)"),
        ]);
        let extraction = extract_from_ast_json(&ast, source, std::path::Path::new("f.c")).unwrap();
        assert_eq!(extraction.functions[0].name, "a");
        assert_eq!(extraction.functions[1].name, "b");
    }

    #[test]
    fn round_trips_through_serde() {
        let source = "/** @mununu_blackbox */\nvoid foo(void);\n";
        let ast = fake_ast_json(&[("foo", 2, "f.c", "void (void)")]);
        let extraction = extract_from_ast_json(&ast, source, std::path::Path::new("f.c")).unwrap();
        let json = serde_json::to_string_pretty(&extraction).unwrap();
        // Confirm the structure is JSON-serialisable (the
        // CExtraction shape is what the CLI emits).
        assert!(json.contains("\"functions\""));
        assert!(json.contains("\"foo\""));
        assert!(json.contains("\"blackbox\""));
    }

    #[test]
    fn ignores_system_typedefs_without_warning() {
        // Clang's AST starts with hundreds of builtin TypedefDecls
        // (__int128_t, __uint128_t, ...). They must NOT produce
        // UnhandledKind warnings — they're not in the user's file.
        let ast = serde_json::json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "TypedefDecl", "loc": { "file": "<built-in>" }, "name": "__int128_t" },
                { "kind": "FunctionDecl", "loc": { "file": "user.c", "line": 1 }, "name": "foo", "type": { "qualType": "void (void)" } }
            ]
        })
        .to_string();
        let extraction = extract_from_ast_json(&ast, "", std::path::Path::new("user.c")).unwrap();
        assert_eq!(extraction.functions.len(), 1);
        // No UnhandledKind for builtin typedefs.
        let unhandled = extraction
            .warnings
            .iter()
            .filter(|w| matches!(w, CExtractWarning::UnhandledKind { .. }))
            .count();
        assert_eq!(unhandled, 0);
    }
}
