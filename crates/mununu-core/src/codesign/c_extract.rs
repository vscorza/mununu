//! C extraction via clang shell-out — Document C task C5, slices 2.a + 2.b.
//!
//! Slice 2.a reads a C source file via a subprocess shell-out to
//! `clang -Xclang -ast-dump=json -fsyntax-only` and lifts the user-
//! authored function declarations plus their `@mununu_*` annotations
//! into a [`CExtraction`] record. The annotations are extracted via
//! the slice-1 grammar at [`crate::mununu_annotations::extract_from_c_source`]
//! and matched to functions by line proximity (each Doxygen block sits
//! immediately above the declaration it annotates).
//!
//! Slice 2.b extends the extractor to walk each function's *body*,
//! recognise register-access expressions (chains of `MemberExpr` /
//! `DeclRefExpr` reconstructed back to the C accessor string the
//! programmer would have typed, e.g. `UART->CTRL.bit.tx_start`),
//! classify each access as a read or a write, and synthesise a
//! linear CTXDSL automaton on the [`crate::codesign::coupling`]
//! rendezvous-label alphabet. The synthesis is opt-in via
//! [`CExtractOptions::register_map`] + [`CExtractOptions::synthesize_automaton`]
//! so slice 2.a's behaviour is unchanged when no register map is
//! supplied.
//!
//! ### Slice 2.b scope and non-scope
//!
//! - **In scope:** linear function bodies (sequences of assignments
//!   and expression statements). Each statement contributes zero or
//!   one access; statements with multiple accesses contribute them in
//!   left-to-right source order (RHS reads before the LHS write).
//! - **Slice 2.c (this iteration):** `while (single_register_read) ;`
//!   (or `{}`) is recognised as a *state-creating* construct — the
//!   synthesiser emits a dedicated `Loop_i` state with a self-loop
//!   plus a same-label exit, faithfully reproducing the Doc C §C.4
//!   hand-authored polling shape. Anything more than the canonical
//!   polling idiom — non-trivial condition, side-effecting body —
//!   still falls back to slice-2.b linearisation with the structured
//!   [`CExtractWarning::NonLinearControlFlow`] warning.
//! - **Other control flow (linearised):** `if`, `for`, `do`, `switch`
//!   bodies are walked inline and a `NonLinearControlFlow` warning is
//!   emitted per occurrence. Sound for safety (over-approximation),
//!   unsound for liveness; the user is told.
//! - **Out of scope (future):** function calls into other firmware
//!   functions, indirect calls through function pointers, ISR
//!   entry/exit semantics. The body walker stops at the call boundary.
//!
//! ### Correctness bounds and the principled alternative
//!
//! What the extractor claims and what it does *not* claim is bounded
//! explicitly in [`docs/design/c-extraction-correctness-scope.md`](../../../../docs/design/c-extraction-correctness-scope.md).
//! Re-read that document before opening any PR that adds a slice
//! beyond 2.c. The principled alternative (LLVM-IR / CFG / predicate
//! abstraction) is scoped there too, with the triggers that would
//! justify switching paths.
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

use crate::codesign::coupling::{AccessKind, rendezvous_label_name};
use crate::codesign::register_map::{Field, Register, RegisterMap};
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
    /// Slice 2.b: a register-access expression was reconstructed but
    /// no field in the supplied [`RegisterMap`] has a matching
    /// `c_accessor`. The access is dropped from the synthesised
    /// automaton; the user is told.
    UnknownAccessor {
        function: String,
        accessor: String,
        source_line: u32,
    },
    /// Slice 2.b: the function body contains a control-flow construct
    /// (`while`, `if`, `for`, `do`, `switch`) that slice 2.b does not
    /// handle natively. The body is linearised — accesses inside the
    /// construct are walked inline as if the branch were always taken
    /// — and this warning is emitted. Sound for safety properties
    /// (over-approximation) but unsound for liveness.
    NonLinearControlFlow {
        function: String,
        construct: String,
        source_line: u32,
    },
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
            CExtractWarning::UnknownAccessor {
                function,
                accessor,
                source_line,
            } => write!(
                f,
                "{function} (line {source_line}): accessor `{accessor}` does not match any field's c_accessor in the supplied register map; dropped from synthesised automaton"
            ),
            CExtractWarning::NonLinearControlFlow {
                function,
                construct,
                source_line,
            } => write!(
                f,
                "{function} (line {source_line}): `{construct}` linearised by slice 2.b — body walked as if the branch were always taken (over-approximation; sound for safety, unsound for liveness)"
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
    /// Slice 2.b: register accesses lifted from the function body, in
    /// source order. Empty when no register map was supplied or when
    /// the function has no body (forward declaration).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accesses: Vec<RegisterAccess>,
    /// Slice 2.b: CTXDSL automaton fragment synthesised from
    /// [`Self::accesses`]. `Some` only when synthesis was requested
    /// via [`CExtractOptions::synthesize_automaton`] *and* the
    /// function had at least one matched access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automaton_ctxdsl: Option<String>,
}

/// A single register access reconstructed from a function body
/// (slice 2.b).
///
/// The `accessor` field is the C expression as the programmer typed
/// it (`UART->CTRL.bit.tx_start`), recovered by walking the clang
/// AST's `MemberExpr` / `DeclRefExpr` chain. The `register` and
/// `field` fields are the matched register-map entries. If matching
/// fails the access is not emitted — a
/// [`CExtractWarning::UnknownAccessor`] is recorded instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterAccess {
    /// Whether the access is a read or a write of the matched
    /// register field. Writes are detected via assignment-expression
    /// LHS; everything else lifted by the body walker is a read.
    pub kind: AccessKind,
    /// The register's `name` from the supplied [`RegisterMap`].
    pub register: String,
    /// The field's `name`. `None` when the matched accessor refers to
    /// the whole register (e.g. `UART->DATA` on a data-payload
    /// register with no declared fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// The C accessor string as reconstructed from the AST.
    pub accessor: String,
    /// 1-based source line of the statement containing the access.
    pub source_line: u32,
    /// Slice 2.c: control-flow context for this access. Defaults to
    /// [`AccessFlow::Linear`] — slice 2.b's behaviour. Polling loops
    /// detected in slice 2.c emit `PollingLoop`, which makes the
    /// automaton synthesiser create a state with a self-loop on the
    /// access label rather than chaining through a fresh state.
    #[serde(default, skip_serializing_if = "AccessFlow::is_linear")]
    pub flow: AccessFlow,
}

/// Control-flow context for a [`RegisterAccess`] (slice 2.c).
///
/// `Linear` is the slice-2.b default — one access produces one
/// transition from the previous state to a new state.
///
/// `PollingLoop` is the slice-2.c special case for `while (cond) ;`
/// (or `while (cond) {}` with an empty body) where `cond` is a
/// single register-access read. It produces *three* transitions on
/// the same label, all sharing one new state:
/// - `prev → Loop_<i>` (enter the loop — read returns "stay polling")
/// - `Loop_<i> → Loop_<i>` (loop iteration — read still busy)
/// - `Loop_<i> → next` (exit the loop — read returns "go")
///
/// Only the *exit* transition advances state. This is the smallest
/// faithful encoding of a polling loop: the verifier sees that the
/// firmware may stay in `Loop_<i>` arbitrarily long (over-
/// approximation — sound for safety) and that it eventually leaves
/// via the same label (matching the hand-authored Doc C §C.4
/// `firmware.ctxdsl` shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessFlow {
    /// Default. One linear transition `prev → next` on the access
    /// label.
    #[default]
    Linear,
    /// Polling-loop pattern. Three transitions on the same label
    /// sharing one new state — see [`AccessFlow`] doc-comment.
    PollingLoop,
}

impl AccessFlow {
    /// Used by `#[serde(skip_serializing_if = "AccessFlow::is_linear")]`
    /// to keep the wire format identical to slice 2.b when the access
    /// is the default linear flow.
    pub fn is_linear(&self) -> bool {
        matches!(self, AccessFlow::Linear)
    }
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
    /// Slice 2.b: register map to match `MemberExpr` chains against.
    /// When `None`, function bodies are not walked and
    /// [`CFunctionDecl::accesses`] is left empty (slice 2.a
    /// behaviour).
    pub register_map: Option<RegisterMap>,
    /// Slice 2.b: when `true` *and* `register_map` is supplied, fill
    /// in [`CFunctionDecl::automaton_ctxdsl`] for every function
    /// that has at least one matched access.
    pub synthesize_automaton: bool,
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
    extract_from_ast_json_with_options(&stdout, &source_text, source_path, options)
}

/// Pure-function half of the extractor: takes already-parsed AST JSON
/// (as a string) + the source text + the source path, and returns
/// the [`CExtraction`]. Separated from [`extract_c_via_clang`] so
/// tests can drive it without depending on `clang` being on `PATH`.
///
/// This wrapper preserves slice 2.a's signature: it calls
/// [`extract_from_ast_json_with_options`] with [`CExtractOptions::default()`],
/// which leaves [`CFunctionDecl::accesses`] empty.
pub fn extract_from_ast_json(
    ast_json: &str,
    source_text: &str,
    source_path: &std::path::Path,
) -> Result<CExtraction, CExtractError> {
    extract_from_ast_json_with_options(
        ast_json,
        source_text,
        source_path,
        &CExtractOptions::default(),
    )
}

/// Slice 2.b entry point: same as [`extract_from_ast_json`] but
/// respects [`CExtractOptions::register_map`] +
/// [`CExtractOptions::synthesize_automaton`]. When a register map is
/// supplied the body of each in-file function is walked for register
/// accesses; when synthesis is also requested the resulting linear
/// sequence is emitted as a CTXDSL automaton fragment on each
/// function.
pub fn extract_from_ast_json_with_options(
    ast_json: &str,
    source_text: &str,
    source_path: &std::path::Path,
    options: &CExtractOptions,
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
    let mut body_warnings: Vec<CExtractWarning> = Vec::new();

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

                    // Slice 2.b: when a register map is supplied,
                    // walk the function body for register accesses.
                    let (accesses, automaton_ctxdsl) = if let Some(rm) =
                        options.register_map.as_ref()
                    {
                        let accesses =
                            extract_accesses_from_function(node, &name, rm, &mut body_warnings);
                        let automaton = if options.synthesize_automaton && !accesses.is_empty() {
                            Some(synthesise_automaton_ctxdsl(&name, &accesses, rm))
                        } else {
                            None
                        };
                        (accesses, automaton)
                    } else {
                        (Vec::new(), None)
                    };

                    functions.push(CFunctionDecl {
                        name,
                        signature,
                        source_line,
                        annotations: Vec::new(),
                        accesses,
                        automaton_ctxdsl,
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
    // Slice 2.b body-walk warnings preserve their statement order so
    // the user can read the diagnostic top-down against their source.
    warnings.extend(body_warnings);

    // Sort functions by source line for stable, human-readable output.
    functions.sort_by_key(|f| f.source_line);

    Ok(CExtraction {
        functions,
        orphan_annotations,
        warnings,
    })
}

// --------------------------------------------------------------------
// Slice 2.b: function-body walker + register-access reconstruction +
// CTXDSL automaton synthesis.
// --------------------------------------------------------------------

/// Walk a `FunctionDecl` node's body for register accesses against
/// the supplied register map.
///
/// Returns the accesses in source order. Emits one
/// [`CExtractWarning::NonLinearControlFlow`] per encountered
/// `while` / `if` / `for` / `do` / `switch`, and one
/// [`CExtractWarning::UnknownAccessor`] per reconstructed accessor
/// that has no matching `c_accessor` in the register map.
///
/// The function's body is the *last* `inner[]` entry whose `kind` is
/// `CompoundStmt` (clang places the body after the parameter
/// declarations). A function with no body — i.e. a forward
/// declaration — returns an empty `Vec`.
fn extract_accesses_from_function(
    func_decl: &serde_json::Value,
    function_name: &str,
    rm: &RegisterMap,
    warnings: &mut Vec<CExtractWarning>,
) -> Vec<RegisterAccess> {
    let body = func_decl
        .get("inner")
        .and_then(|i| i.as_array())
        .and_then(|nodes| {
            nodes
                .iter()
                .rev()
                .find(|n| n.get("kind").and_then(|k| k.as_str()) == Some("CompoundStmt"))
        });
    let Some(body) = body else {
        return Vec::new();
    };

    let mut accesses = Vec::new();
    walk_compound_stmt(body, function_name, rm, &mut accesses, warnings);
    accesses
}

/// Walk a `CompoundStmt` node's statements. Each statement may add
/// zero or more accesses. Control-flow constructs are linearised
/// (their body is walked inline) with a warning.
fn walk_compound_stmt(
    compound: &serde_json::Value,
    function_name: &str,
    rm: &RegisterMap,
    accesses: &mut Vec<RegisterAccess>,
    warnings: &mut Vec<CExtractWarning>,
) {
    let Some(stmts) = compound.get("inner").and_then(|i| i.as_array()) else {
        return;
    };
    for stmt in stmts {
        walk_statement(stmt, function_name, rm, accesses, warnings);
    }
}

/// Walk a single statement.
fn walk_statement(
    stmt: &serde_json::Value,
    function_name: &str,
    rm: &RegisterMap,
    accesses: &mut Vec<RegisterAccess>,
    warnings: &mut Vec<CExtractWarning>,
) {
    let kind = stmt.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    let source_line = statement_source_line(stmt);
    match kind {
        // Assignment: BinaryOperator with opcode "=" → LHS write,
        // RHS reads.
        "BinaryOperator" if stmt.get("opcode").and_then(|o| o.as_str()) == Some("=") => {
            let inner = stmt.get("inner").and_then(|i| i.as_array());
            if let Some(inner) = inner
                && inner.len() >= 2
            {
                // RHS reads first (left-to-right evaluation order).
                collect_reads(
                    &inner[1],
                    function_name,
                    source_line,
                    rm,
                    accesses,
                    warnings,
                );
                // LHS write.
                if let Some(accessor) = reconstruct_c_accessor(&inner[0]) {
                    push_matched_access(
                        AccessKind::Write,
                        accessor,
                        source_line,
                        function_name,
                        rm,
                        accesses,
                        warnings,
                    );
                }
            }
        }
        // Compound assignment (`|=`, `&=`, …) is a read-modify-write
        // by definition. We model both the read and the write on the
        // LHS register field.
        "CompoundAssignOperator" => {
            let inner = stmt.get("inner").and_then(|i| i.as_array());
            if let Some(inner) = inner
                && inner.len() >= 2
            {
                collect_reads(
                    &inner[1],
                    function_name,
                    source_line,
                    rm,
                    accesses,
                    warnings,
                );
                if let Some(accessor) = reconstruct_c_accessor(&inner[0]) {
                    push_matched_access(
                        AccessKind::Read,
                        accessor.clone(),
                        source_line,
                        function_name,
                        rm,
                        accesses,
                        warnings,
                    );
                    push_matched_access(
                        AccessKind::Write,
                        accessor,
                        source_line,
                        function_name,
                        rm,
                        accesses,
                        warnings,
                    );
                }
            }
        }
        // Non-assignment expression statement: any MemberExpr chains
        // inside are reads.
        "ExprStmt" | "CallExpr" | "ImplicitCastExpr" | "DeclRefExpr" | "MemberExpr"
        | "ParenExpr" | "UnaryOperator" | "BinaryOperator" => {
            collect_reads(stmt, function_name, source_line, rm, accesses, warnings);
        }
        // Variable declarations may initialise from a register read.
        "DeclStmt" => {
            if let Some(inner) = stmt.get("inner").and_then(|i| i.as_array()) {
                for child in inner {
                    if child.get("kind").and_then(|k| k.as_str()) == Some("VarDecl")
                        && let Some(init) = child.get("inner").and_then(|i| i.as_array())
                        && let Some(first) = init.first()
                    {
                        collect_reads(first, function_name, source_line, rm, accesses, warnings);
                    }
                }
            }
        }
        // Slice 2.c: a `while (cond) ;` / `while (cond) {}` where
        // `cond` is exactly one register-access read maps to the
        // canonical polling-loop pattern (`PollingLoop` flow). Any
        // other while shape — non-trivial condition, side-effecting
        // body — falls back to slice-2.b linearisation with the
        // standard `NonLinearControlFlow` warning.
        "WhileStmt" => {
            let inner = stmt.get("inner").and_then(|i| i.as_array());
            let cond = inner.and_then(|nodes| nodes.first());
            let body = inner.and_then(|nodes| nodes.get(1));
            let body_is_empty = body.is_none_or(is_empty_or_inert_body);
            let cond_accessor = cond
                .and_then(reconstruct_single_read_accessor)
                .filter(|acc| match_accessor(acc, rm).is_some());

            if let (true, Some(accessor)) = (body_is_empty, cond_accessor) {
                push_matched_access_with_flow(
                    AccessKind::Read,
                    AccessFlow::PollingLoop,
                    accessor,
                    source_line,
                    function_name,
                    rm,
                    accesses,
                    warnings,
                );
            } else {
                warnings.push(CExtractWarning::NonLinearControlFlow {
                    function: function_name.to_string(),
                    construct: kind.to_string(),
                    source_line,
                });
                if let Some(nodes) = inner {
                    for child in nodes {
                        collect_reads(child, function_name, source_line, rm, accesses, warnings);
                        let child_kind = child.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                        if child_kind == "CompoundStmt" {
                            walk_compound_stmt(child, function_name, rm, accesses, warnings);
                        }
                    }
                }
            }
        }
        // Other control flow: linearise. Same shape as slice 2.b.
        "DoStmt" | "ForStmt" | "IfStmt" | "SwitchStmt" => {
            warnings.push(CExtractWarning::NonLinearControlFlow {
                function: function_name.to_string(),
                construct: kind.to_string(),
                source_line,
            });
            if let Some(inner) = stmt.get("inner").and_then(|i| i.as_array()) {
                // Heuristic: for IfStmt the inner is [cond, then, else?];
                // for ForStmt the layout varies. We treat every child as
                // a candidate for read collection + recursive linear walk;
                // this is sound (over-approximation) for slice 2.b/c.
                for child in inner {
                    collect_reads(child, function_name, source_line, rm, accesses, warnings);
                    let child_kind = child.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                    if child_kind == "CompoundStmt" {
                        walk_compound_stmt(child, function_name, rm, accesses, warnings);
                    }
                }
            }
        }
        // Return / break / continue / null statements: ignored.
        "ReturnStmt" | "BreakStmt" | "ContinueStmt" | "NullStmt" => {
            // A `return expr;` carries a read in `inner[0]`.
            if let Some(inner) = stmt.get("inner").and_then(|i| i.as_array())
                && let Some(first) = inner.first()
            {
                collect_reads(first, function_name, source_line, rm, accesses, warnings);
            }
        }
        // Nested compound statement.
        "CompoundStmt" => walk_compound_stmt(stmt, function_name, rm, accesses, warnings),
        _ => {
            // Unknown statement kind — be conservative and still
            // collect any obvious reads we find in its subtree.
            collect_reads(stmt, function_name, source_line, rm, accesses, warnings);
        }
    }
}

/// Recursively scan an expression subtree for any `MemberExpr` /
/// `DeclRefExpr` chains that reconstruct to register accessors, and
/// record them as reads. Stops descending into a `MemberExpr` once
/// matched (the whole chain is one access).
fn collect_reads(
    node: &serde_json::Value,
    function_name: &str,
    source_line: u32,
    rm: &RegisterMap,
    accesses: &mut Vec<RegisterAccess>,
    warnings: &mut Vec<CExtractWarning>,
) {
    let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    if kind == "MemberExpr"
        && let Some(accessor) = reconstruct_c_accessor(node)
    {
        push_matched_access(
            AccessKind::Read,
            accessor,
            source_line,
            function_name,
            rm,
            accesses,
            warnings,
        );
        return;
    }
    if let Some(children) = node.get("inner").and_then(|i| i.as_array()) {
        for child in children {
            collect_reads(child, function_name, source_line, rm, accesses, warnings);
        }
    }
}

/// Look up the reconstructed accessor in the register map and push a
/// matching [`RegisterAccess`]. If no field matches, emit a
/// [`CExtractWarning::UnknownAccessor`].
///
/// Slice 2.b wrapper — always emits a `Linear` access. Slice 2.c
/// constructs `PollingLoop` accesses via
/// [`push_matched_access_with_flow`].
fn push_matched_access(
    kind: AccessKind,
    accessor: String,
    source_line: u32,
    function_name: &str,
    rm: &RegisterMap,
    accesses: &mut Vec<RegisterAccess>,
    warnings: &mut Vec<CExtractWarning>,
) {
    push_matched_access_with_flow(
        kind,
        AccessFlow::Linear,
        accessor,
        source_line,
        function_name,
        rm,
        accesses,
        warnings,
    );
}

/// Slice 2.c entry point. Same as [`push_matched_access`] but lets
/// the caller mark the access as a [`AccessFlow::PollingLoop`].
#[allow(clippy::too_many_arguments)]
fn push_matched_access_with_flow(
    kind: AccessKind,
    flow: AccessFlow,
    accessor: String,
    source_line: u32,
    function_name: &str,
    rm: &RegisterMap,
    accesses: &mut Vec<RegisterAccess>,
    warnings: &mut Vec<CExtractWarning>,
) {
    if let Some((reg, field)) = match_accessor(&accessor, rm) {
        accesses.push(RegisterAccess {
            kind,
            register: reg.name.clone(),
            field: field.map(|f| f.name.clone()),
            accessor,
            source_line,
            flow,
        });
    } else {
        warnings.push(CExtractWarning::UnknownAccessor {
            function: function_name.to_string(),
            accessor,
            source_line,
        });
    }
}

/// Match a reconstructed C accessor string against the
/// [`RegisterMap`]. Returns the matching register + optional field.
/// Exact match on `c_accessor`; no fuzzy matching.
fn match_accessor<'a>(
    accessor: &str,
    rm: &'a RegisterMap,
) -> Option<(&'a Register, Option<&'a Field>)> {
    for reg in &rm.registers {
        for field in &reg.fields {
            if field.c_accessor.as_deref() == Some(accessor) {
                return Some((reg, Some(field)));
            }
        }
        // Whole-register access (no field on the c_accessor side —
        // typical for data-payload registers). The register itself
        // doesn't carry a c_accessor today; fall back to a synthetic
        // expectation of `<PERIPH>->{name}` for now. Slice 2.b is
        // satisfied with field-level matching; whole-register
        // accessors are a slice 2.c concern.
        let _ = reg;
    }
    None
}

/// Reconstruct the C accessor string a programmer would type, by
/// walking a `MemberExpr` chain bottom-up to a `DeclRefExpr`.
///
/// Example: a `MemberExpr` AST for `UART->CTRL.bit.tx_start` has the
/// shape (top-down):
///
/// ```text
/// MemberExpr name="tx_start" isArrow=false
///   inner:
///     MemberExpr name="bit" isArrow=false
///       inner:
///         MemberExpr name="CTRL" isArrow=true
///           inner:
///             ImplicitCastExpr  (LValueToRValue)
///               inner:
///                 DeclRefExpr referencedDecl.name="UART"
/// ```
///
/// We walk the chain, collecting `(name, isArrow)` pairs, then build
/// the accessor string. Returns `None` if any step is not a
/// `MemberExpr`, `ImplicitCastExpr`, `ParenExpr`, or terminal
/// `DeclRefExpr`.
pub fn reconstruct_c_accessor(node: &serde_json::Value) -> Option<String> {
    // Collect the chain bottom-up: outermost MemberExpr is the
    // deepest field; walk towards the DeclRefExpr at the root.
    let mut steps: Vec<(String, bool)> = Vec::new();
    let mut cursor = node;
    let root_name = loop {
        let kind = cursor.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        match kind {
            "MemberExpr" => {
                let name = cursor.get("name").and_then(|n| n.as_str())?.to_string();
                let is_arrow = cursor
                    .get("isArrow")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                steps.push((name, is_arrow));
                cursor = cursor.get("inner").and_then(|i| i.as_array())?.first()?;
            }
            // Transparent wrappers — descend through them.
            "ImplicitCastExpr" | "ParenExpr" | "CStyleCastExpr" => {
                cursor = cursor.get("inner").and_then(|i| i.as_array())?.first()?;
            }
            "DeclRefExpr" => {
                // The root is `referencedDecl.name` — the variable /
                // global the chain bottoms out on.
                let name = cursor
                    .get("referencedDecl")
                    .and_then(|d| d.get("name"))
                    .and_then(|n| n.as_str())?
                    .to_string();
                break name;
            }
            _ => return None,
        }
    };

    // Steps are bottom-up (innermost-name first). Build the string
    // outermost-to-innermost (root → outermost member).
    let mut accessor = root_name;
    for (name, is_arrow) in steps.iter().rev() {
        let sep = if *is_arrow { "->" } else { "." };
        accessor.push_str(sep);
        accessor.push_str(name);
    }
    Some(accessor)
}

/// Slice 2.c: reconstruct the accessor for a *single* register read
/// from a `while`-condition expression.
///
/// A polling loop's condition typically looks like
/// `UART->STATUS.bit.tx_busy` (a single `MemberExpr` chain wrapped in
/// `ImplicitCastExpr(LValueToRValue)`). Anything more complex — a
/// boolean combination, a comparison, a function call — returns
/// `None`, which makes the WhileStmt handler fall back to the
/// slice-2.b linearisation.
fn reconstruct_single_read_accessor(node: &serde_json::Value) -> Option<String> {
    // Walk transparent wrappers down to the first MemberExpr we can
    // hand to `reconstruct_c_accessor`. Anything non-trivial (binary
    // ops, calls, parenthesised compound expressions) returns None.
    let mut cursor = node;
    loop {
        let kind = cursor.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        match kind {
            "ImplicitCastExpr" | "ParenExpr" | "CStyleCastExpr" => {
                cursor = cursor.get("inner").and_then(|i| i.as_array())?.first()?;
            }
            "MemberExpr" => return reconstruct_c_accessor(cursor),
            _ => return None,
        }
    }
}

/// Slice 2.c: a polling loop's body is "inert" if it has no statements,
/// or only statements that do not touch registers (e.g. a single
/// `NullStmt` for `while (cond) ;`, or a `CompoundStmt` whose own
/// `inner` is empty for `while (cond) {}`).
///
/// Returns `true` for the inert cases the slice-2.c PollingLoop
/// encoding faithfully represents. Returns `false` for any body with
/// statements; those fall through to slice-2.b linearisation since
/// representing a side-effecting loop body in a single state would
/// elide information.
fn is_empty_or_inert_body(node: &serde_json::Value) -> bool {
    let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match kind {
        "NullStmt" => true,
        "CompoundStmt" => node
            .get("inner")
            .and_then(|i| i.as_array())
            .map(|stmts| stmts.is_empty())
            .unwrap_or(true),
        _ => false,
    }
}

/// Extract the 1-based source line of a statement node, falling back
/// to 0 when the AST doesn't carry one (clang's `range` and `loc`
/// fields are sparse).
fn statement_source_line(stmt: &serde_json::Value) -> u32 {
    stmt.get("loc")
        .and_then(|l| l.get("line"))
        .and_then(|n| n.as_u64())
        .or_else(|| {
            stmt.get("range")
                .and_then(|r| r.get("begin"))
                .and_then(|b| b.get("line"))
                .and_then(|n| n.as_u64())
        })
        .unwrap_or(0) as u32
}

/// Synthesise a linear CTXDSL `automaton { … }` block from the
/// sequence of register accesses extracted from a function body.
///
/// The emitted automaton has `N+1` states for a sequence of `N`
/// accesses: an initial state `S0`, then `S1 .. SN` after each
/// access. The labels follow the
/// [`crate::codesign::coupling::rendezvous_label_name`] convention so
/// the firmware automaton synchronises with the peripheral chaotic
/// stub on the same alphabet.
///
/// All firmware-driven write labels are declared as
/// `controllable { … }`; reads are left uncontrollable (the default).
/// This matches Doc A §4 and the per-side classification in
/// [`crate::codesign::coupling::register_map_labels`].
pub fn synthesise_automaton_ctxdsl(
    function_name: &str,
    accesses: &[RegisterAccess],
    rm: &RegisterMap,
) -> String {
    use std::fmt::Write;

    let automaton_name = sanitise_ident_for_ctxdsl(function_name);
    let mut buf = String::new();
    let _ = writeln!(buf, "    automata {{");
    let _ = writeln!(buf, "        automaton {automaton_name} {{");

    // Labels — collect controllable writes for the controllable {…}
    // declaration. Reads default to uncontrollable.
    let mut controllable_labels: Vec<String> = Vec::new();
    for access in accesses {
        let label = rendezvous_label_name(&access.register, access.field.as_deref(), access.kind);
        if access.kind == AccessKind::Write && !controllable_labels.contains(&label) {
            controllable_labels.push(label);
        }
    }
    if !controllable_labels.is_empty() {
        let _ = writeln!(buf, "            controllable {{");
        for label in &controllable_labels {
            let _ = writeln!(buf, "                {label};");
        }
        let _ = writeln!(buf, "            }}");
        let _ = writeln!(buf);
    }

    let _ = writeln!(buf, "            states {{");
    let _ = writeln!(buf, "                state S0 initial;");
    for (i, access) in accesses.iter().enumerate() {
        // Slice 2.c: each PollingLoop access introduces a dedicated
        // `Loop_i` state sitting between S_{i} and S_{i+1}, declared
        // in chronological order (Loop first, then the main-line
        // state the loop exits into).
        if access.flow == AccessFlow::PollingLoop {
            let _ = writeln!(buf, "                state Loop{i};");
        }
        let _ = writeln!(buf, "                state S{idx};", idx = i + 1);
    }
    let _ = writeln!(buf, "            }}");
    let _ = writeln!(buf);

    let _ = writeln!(buf, "            transitions {{");
    for (i, access) in accesses.iter().enumerate() {
        let label = rendezvous_label_name(&access.register, access.field.as_deref(), access.kind);
        let from = format!("S{i}");
        let to = format!("S{next}", next = i + 1);
        match access.flow {
            AccessFlow::Linear => {
                let _ = writeln!(
                    buf,
                    "                transition {from} -> {to} on label {label}; // {accessor}",
                    accessor = access.accessor
                );
            }
            AccessFlow::PollingLoop => {
                let loop_state = format!("Loop{i}");
                // Enter the loop on the read.
                let _ = writeln!(
                    buf,
                    "                transition {from} -> {loop_state} on label {label}; // {accessor} (enter loop)",
                    accessor = access.accessor
                );
                // Loop iteration: still polling — same label, same state.
                let _ = writeln!(
                    buf,
                    "                transition {loop_state} -> {loop_state} on label {label}; // {accessor} (loop iteration)",
                    accessor = access.accessor
                );
                // Exit the loop on the same label — the read returns the
                // value that breaks the polling condition.
                let _ = writeln!(
                    buf,
                    "                transition {loop_state} -> {to} on label {label}; // {accessor} (exit loop)",
                    accessor = access.accessor
                );
            }
        }
    }
    let _ = writeln!(buf, "            }}");
    // Close the `automaton { … }` body.
    let _ = writeln!(buf, "        }}");
    // Close the wrapping `automata { … }` section.
    let _ = writeln!(buf, "    }}");

    let _ = rm; // currently unused but kept for symmetry with coupling.rs.
    buf
}

/// Sanitise a C identifier into a CTXDSL automaton name: first char
/// uppercase, rest preserved (CTXDSL is case-sensitive but the
/// convention is PascalCase for automaton names; we cheaply convert
/// `uart_send` → `Uart_send`). Non-alnum becomes `_`.
fn sanitise_ident_for_ctxdsl(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut first = true;
    for c in name.chars() {
        let safe = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '_'
        };
        if first {
            out.push(safe.to_ascii_uppercase());
            first = false;
        } else {
            out.push(safe);
        }
    }
    if out.is_empty() {
        out.push_str("Func");
    }
    out
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

    // ----------------------------------------------------------------
    // Slice 2.b tests: function-body walker + accessor reconstruction
    // + automaton synthesis.
    // ----------------------------------------------------------------

    use crate::codesign::register_map::{
        AccessPath, Register, RegisterDirection, RegisterMap, VisibilityClass,
    };

    /// Test fixture: a small UART_LITE register map matching the
    /// `examples/industrial/codesign_uart/` shape. Fields carry
    /// `c_accessor` strings so the body walker can match against
    /// reconstructed `MemberExpr` chains.
    fn uart_register_map() -> RegisterMap {
        use crate::codesign::register_map::Field;
        RegisterMap {
            peripheral: "UART_LITE".to_string(),
            base_address: "0x40010000".to_string(),
            description: None,
            contract_uri: None,
            registers: vec![
                Register {
                    name: "CTRL".to_string(),
                    offset: 0,
                    width_bits: 32,
                    direction: RegisterDirection::Rw,
                    visibility_class: VisibilityClass::Control,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![Field {
                        name: "tx_start".to_string(),
                        bits: [0, 0],
                        sv_signal: Some("uart_inst.ctrl_reg[0]".to_string()),
                        c_accessor: Some("UART->CTRL.bit.tx_start".to_string()),
                        description: None,
                    }],
                },
                Register {
                    name: "STATUS".to_string(),
                    offset: 4,
                    width_bits: 32,
                    direction: RegisterDirection::Ro,
                    visibility_class: VisibilityClass::Status,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![Field {
                        name: "tx_busy".to_string(),
                        bits: [0, 0],
                        sv_signal: Some("uart_inst.tx_busy".to_string()),
                        c_accessor: Some("UART->STATUS.bit.tx_busy".to_string()),
                        description: None,
                    }],
                },
                Register {
                    name: "DATA".to_string(),
                    offset: 8,
                    width_bits: 32,
                    direction: RegisterDirection::Rw,
                    visibility_class: VisibilityClass::Data,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![Field {
                        name: "byte".to_string(),
                        bits: [0, 7],
                        sv_signal: Some("uart_inst.data_reg".to_string()),
                        c_accessor: Some("UART->DATA.byte".to_string()),
                        description: None,
                    }],
                },
            ],
        }
    }

    /// Build a `MemberExpr` JSON node matching the clang AST shape
    /// for a `UART->CTRL.bit.tx_start`-style accessor.
    fn member_expr_chain(root: &str, parts: &[(&str, bool)]) -> serde_json::Value {
        // `parts` is from-root-outward: first entry sits on the
        // DeclRefExpr, last entry is the outermost MemberExpr. We
        // build innermost-first, ending at the outermost.
        let mut current = serde_json::json!({
            "kind": "ImplicitCastExpr",
            "inner": [{
                "kind": "DeclRefExpr",
                "referencedDecl": { "name": root },
            }],
        });
        for (name, is_arrow) in parts {
            current = serde_json::json!({
                "kind": "MemberExpr",
                "name": name,
                "isArrow": is_arrow,
                "inner": [current],
            });
        }
        current
    }

    fn fake_ast_with_body(
        func_name: &str,
        func_line: u32,
        file: &str,
        qual_type: &str,
        body_stmts: Vec<serde_json::Value>,
    ) -> String {
        serde_json::json!({
            "kind": "TranslationUnitDecl",
            "inner": [{
                "kind": "FunctionDecl",
                "name": func_name,
                "loc": { "file": file, "line": func_line },
                "type": { "qualType": qual_type },
                "inner": [{
                    "kind": "CompoundStmt",
                    "inner": body_stmts,
                }],
            }],
        })
        .to_string()
    }

    #[test]
    fn reconstructs_arrow_then_dot_chain() {
        // UART->CTRL.bit.tx_start
        let node = member_expr_chain(
            "UART",
            &[("CTRL", true), ("bit", false), ("tx_start", false)],
        );
        assert_eq!(
            reconstruct_c_accessor(&node).as_deref(),
            Some("UART->CTRL.bit.tx_start")
        );
    }

    #[test]
    fn reconstructs_single_arrow_access() {
        // UART->CTRL — direct register-level access (no field).
        let node = member_expr_chain("UART", &[("CTRL", true)]);
        assert_eq!(reconstruct_c_accessor(&node).as_deref(), Some("UART->CTRL"));
    }

    #[test]
    fn reconstruct_returns_none_for_non_member_expr() {
        let node = serde_json::json!({ "kind": "IntegerLiteral", "value": "0" });
        assert_eq!(reconstruct_c_accessor(&node), None);
    }

    #[test]
    fn write_is_detected_as_assignment_lhs() {
        // UART->CTRL.bit.tx_start = 1;
        let lhs = member_expr_chain(
            "UART",
            &[("CTRL", true), ("bit", false), ("tx_start", false)],
        );
        let assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 10 },
            "inner": [lhs, { "kind": "IntegerLiteral", "value": "1" }],
        });
        let ast = fake_ast_with_body("fire_tx", 8, "uart.c", "void (void)", vec![assign]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        let func = &extraction.functions[0];
        assert_eq!(func.accesses.len(), 1, "{:?}", func.accesses);
        assert_eq!(func.accesses[0].kind, AccessKind::Write);
        assert_eq!(func.accesses[0].register, "CTRL");
        assert_eq!(func.accesses[0].field.as_deref(), Some("tx_start"));
        assert_eq!(func.accesses[0].accessor, "UART->CTRL.bit.tx_start");
    }

    #[test]
    fn read_is_detected_in_assignment_rhs() {
        // local_busy = UART->STATUS.bit.tx_busy;
        let rhs = member_expr_chain(
            "UART",
            &[("STATUS", true), ("bit", false), ("tx_busy", false)],
        );
        let assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 5 },
            "inner": [
                // LHS is a plain local variable — not a register
                // accessor. The matcher should ignore it.
                { "kind": "DeclRefExpr", "referencedDecl": { "name": "local_busy" } },
                rhs,
            ],
        });
        let ast = fake_ast_with_body("poll", 4, "uart.c", "void (void)", vec![assign]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        let func = &extraction.functions[0];
        assert_eq!(func.accesses.len(), 1);
        assert_eq!(func.accesses[0].kind, AccessKind::Read);
        assert_eq!(func.accesses[0].register, "STATUS");
        assert_eq!(func.accesses[0].field.as_deref(), Some("tx_busy"));
    }

    #[test]
    fn unknown_accessor_emits_warning_not_access() {
        // UART->BOGUS.bit.something = 0; — accessor not in map.
        let lhs = member_expr_chain(
            "UART",
            &[("BOGUS", true), ("bit", false), ("something", false)],
        );
        let assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 3 },
            "inner": [lhs, { "kind": "IntegerLiteral", "value": "0" }],
        });
        let ast = fake_ast_with_body("fn", 1, "uart.c", "void (void)", vec![assign]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        assert!(extraction.functions[0].accesses.is_empty());
        assert!(
            extraction
                .warnings
                .iter()
                .any(|w| matches!(w, CExtractWarning::UnknownAccessor { .. }))
        );
    }

    #[test]
    fn slice2c_polling_loop_with_empty_body_is_recognised() {
        // while (UART->STATUS.bit.tx_busy) { /* empty body */ }
        // Slice 2.c: this is the canonical polling-loop idiom. The
        // access is marked PollingLoop and NO NonLinearControlFlow
        // warning is emitted.
        let cond = member_expr_chain(
            "UART",
            &[("STATUS", true), ("bit", false), ("tx_busy", false)],
        );
        let while_stmt = serde_json::json!({
            "kind": "WhileStmt",
            "loc": { "line": 7 },
            "inner": [
                cond,
                { "kind": "CompoundStmt", "inner": [] },
            ],
        });
        let ast = fake_ast_with_body("poll", 5, "uart.c", "void (void)", vec![while_stmt]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        let func = &extraction.functions[0];
        assert_eq!(func.accesses.len(), 1);
        assert_eq!(func.accesses[0].kind, AccessKind::Read);
        assert_eq!(func.accesses[0].flow, AccessFlow::PollingLoop);
        assert!(
            !extraction
                .warnings
                .iter()
                .any(|w| matches!(w, CExtractWarning::NonLinearControlFlow { .. })),
            "slice 2.c handles the empty-body polling loop without a linearisation warning"
        );
    }

    #[test]
    fn slice2c_polling_loop_null_body_is_recognised() {
        // while (UART->STATUS.bit.tx_busy) ;   (body is a NullStmt)
        let cond = member_expr_chain(
            "UART",
            &[("STATUS", true), ("bit", false), ("tx_busy", false)],
        );
        let while_stmt = serde_json::json!({
            "kind": "WhileStmt",
            "loc": { "line": 7 },
            "inner": [cond, { "kind": "NullStmt" }],
        });
        let ast = fake_ast_with_body("poll", 5, "uart.c", "void (void)", vec![while_stmt]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        let func = &extraction.functions[0];
        assert_eq!(func.accesses.len(), 1);
        assert_eq!(func.accesses[0].flow, AccessFlow::PollingLoop);
    }

    #[test]
    fn slice2c_falls_back_to_linearisation_for_nontrivial_body() {
        // while (UART->STATUS.bit.tx_busy) { UART->CTRL.bit.tx_start = 1; }
        // The body has a side effect → slice 2.c cannot use the
        // PollingLoop encoding. Falls back to slice-2.b linearisation
        // with the NonLinearControlFlow warning.
        let cond = member_expr_chain(
            "UART",
            &[("STATUS", true), ("bit", false), ("tx_busy", false)],
        );
        let body_lhs = member_expr_chain(
            "UART",
            &[("CTRL", true), ("bit", false), ("tx_start", false)],
        );
        let body_assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 8 },
            "inner": [body_lhs, { "kind": "IntegerLiteral", "value": "1" }],
        });
        let while_stmt = serde_json::json!({
            "kind": "WhileStmt",
            "loc": { "line": 7 },
            "inner": [
                cond,
                { "kind": "CompoundStmt", "inner": [body_assign] },
            ],
        });
        let ast = fake_ast_with_body("poll", 5, "uart.c", "void (void)", vec![while_stmt]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        assert!(
            extraction
                .warnings
                .iter()
                .any(|w| matches!(w, CExtractWarning::NonLinearControlFlow { .. })),
            "non-trivial body must surface the linearisation warning"
        );
        let func = &extraction.functions[0];
        // No access carries the PollingLoop flow — the body forced
        // the slice-2.b linearisation path.
        assert!(
            func.accesses.iter().all(|a| a.flow == AccessFlow::Linear),
            "linearised accesses must not be marked PollingLoop"
        );
        // Every emitted access maps to either the condition's STATUS
        // read or the body's CTRL.tx_start write. The exact count
        // depends on how many times the body subtree is visited; what
        // matters is the flow flag stays Linear.
        assert!(!func.accesses.is_empty());
    }

    #[test]
    fn slice2c_falls_back_when_condition_is_not_a_single_read() {
        // while (UART->STATUS.bit.tx_busy && some_flag) ;
        // The condition is a binary && — slice 2.c cannot recognise
        // it as a single register read. Falls back to slice-2.b
        // linearisation; the single accessor reachable inside the &&
        // is still collected as a Linear read.
        let cond_lhs = member_expr_chain(
            "UART",
            &[("STATUS", true), ("bit", false), ("tx_busy", false)],
        );
        let cond = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "&&",
            "inner": [
                cond_lhs,
                { "kind": "DeclRefExpr", "referencedDecl": { "name": "some_flag" } },
            ],
        });
        let while_stmt = serde_json::json!({
            "kind": "WhileStmt",
            "loc": { "line": 7 },
            "inner": [cond, { "kind": "NullStmt" }],
        });
        let ast = fake_ast_with_body("poll", 5, "uart.c", "void (void)", vec![while_stmt]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        assert!(
            extraction
                .warnings
                .iter()
                .any(|w| matches!(w, CExtractWarning::NonLinearControlFlow { .. }))
        );
        let func = &extraction.functions[0];
        assert_eq!(func.accesses.len(), 1);
        assert_eq!(func.accesses[0].flow, AccessFlow::Linear);
    }

    #[test]
    fn slice2c_polling_loop_emits_three_transitions_on_same_label() {
        // Synthesis test against a PollingLoop access.
        let accesses = vec![RegisterAccess {
            kind: AccessKind::Read,
            register: "STATUS".to_string(),
            field: Some("tx_busy".to_string()),
            accessor: "UART->STATUS.bit.tx_busy".to_string(),
            source_line: 7,
            flow: AccessFlow::PollingLoop,
        }];
        let rm = uart_register_map();
        let ctxdsl = synthesise_automaton_ctxdsl("poll", &accesses, &rm);
        // The Loop state and main-line state both exist.
        assert!(ctxdsl.contains("state Loop0"));
        assert!(ctxdsl.contains("state S1"));
        // Three transitions on the same rd_status_tx_busy label.
        let occurrences = ctxdsl.matches("rd_status_tx_busy").count();
        assert!(
            occurrences >= 3,
            "expected ≥3 rd_status_tx_busy occurrences (enter/iterate/exit), got {occurrences} in:\n{ctxdsl}"
        );
        assert!(ctxdsl.contains("S0 -> Loop0 on label rd_status_tx_busy"));
        assert!(ctxdsl.contains("Loop0 -> Loop0 on label rd_status_tx_busy"));
        assert!(ctxdsl.contains("Loop0 -> S1 on label rd_status_tx_busy"));
    }

    #[test]
    fn no_register_map_means_no_body_walk() {
        // Slice 2.a behaviour preserved: without a register map the
        // body walker is not invoked.
        let lhs = member_expr_chain(
            "UART",
            &[("CTRL", true), ("bit", false), ("tx_start", false)],
        );
        let assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 3 },
            "inner": [lhs, { "kind": "IntegerLiteral", "value": "1" }],
        });
        let ast = fake_ast_with_body("fire_tx", 1, "uart.c", "void (void)", vec![assign]);
        let extraction = extract_from_ast_json(&ast, "", std::path::Path::new("uart.c")).unwrap();
        assert!(extraction.functions[0].accesses.is_empty());
        assert!(extraction.functions[0].automaton_ctxdsl.is_none());
    }

    #[test]
    fn synthesised_automaton_has_n_plus_one_states() {
        // Three accesses (read STATUS, write DATA→byte, write CTRL→tx_start)
        // → four states (S0 .. S3).
        let accesses = vec![
            RegisterAccess {
                kind: AccessKind::Read,
                register: "STATUS".to_string(),
                field: Some("tx_busy".to_string()),
                accessor: "UART->STATUS.bit.tx_busy".to_string(),
                source_line: 5,
                flow: AccessFlow::Linear,
            },
            RegisterAccess {
                kind: AccessKind::Write,
                register: "DATA".to_string(),
                field: Some("byte".to_string()),
                accessor: "UART->DATA.byte".to_string(),
                source_line: 6,
                flow: AccessFlow::Linear,
            },
            RegisterAccess {
                kind: AccessKind::Write,
                register: "CTRL".to_string(),
                field: Some("tx_start".to_string()),
                accessor: "UART->CTRL.bit.tx_start".to_string(),
                source_line: 7,
                flow: AccessFlow::Linear,
            },
        ];
        let rm = uart_register_map();
        let ctxdsl = synthesise_automaton_ctxdsl("uart_send", &accesses, &rm);
        assert!(ctxdsl.contains("state S0 initial"));
        assert!(ctxdsl.contains("state S3"));
        assert!(!ctxdsl.contains("state S4"));
        // Labels follow the coupling.rs convention.
        assert!(ctxdsl.contains("rd_status_tx_busy"));
        assert!(ctxdsl.contains("wr_data_byte"));
        assert!(ctxdsl.contains("wr_ctrl_tx_start"));
        // Controllable block lists write labels.
        assert!(ctxdsl.contains("controllable"));
        assert!(ctxdsl.contains("transition S0 -> S1 on label rd_status_tx_busy"));
        assert!(ctxdsl.contains("transition S1 -> S2 on label wr_data_byte"));
        assert!(ctxdsl.contains("transition S2 -> S3 on label wr_ctrl_tx_start"));
    }

    #[test]
    fn end_to_end_uart_send_synthesises_a_three_step_automaton() {
        // while (UART->STATUS.bit.tx_busy) {}
        // UART->DATA.byte = byte;
        // UART->CTRL.bit.tx_start = 1;
        let status_read = member_expr_chain(
            "UART",
            &[("STATUS", true), ("bit", false), ("tx_busy", false)],
        );
        let while_stmt = serde_json::json!({
            "kind": "WhileStmt",
            "loc": { "line": 5 },
            "inner": [status_read, { "kind": "CompoundStmt", "inner": [] }],
        });
        let data_write_lhs = member_expr_chain("UART", &[("DATA", true), ("byte", false)]);
        let data_assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 6 },
            "inner": [data_write_lhs, { "kind": "DeclRefExpr", "referencedDecl": { "name": "byte" } }],
        });
        let ctrl_write_lhs = member_expr_chain(
            "UART",
            &[("CTRL", true), ("bit", false), ("tx_start", false)],
        );
        let ctrl_assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 7 },
            "inner": [ctrl_write_lhs, { "kind": "IntegerLiteral", "value": "1" }],
        });
        let ast = fake_ast_with_body(
            "uart_send",
            3,
            "uart_driver.c",
            "void (uint8_t)",
            vec![while_stmt, data_assign, ctrl_assign],
        );
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            synthesize_automaton: true,
            ..Default::default()
        };
        let extraction = extract_from_ast_json_with_options(
            &ast,
            "",
            std::path::Path::new("uart_driver.c"),
            &opts,
        )
        .unwrap();
        let func = &extraction.functions[0];
        assert_eq!(func.accesses.len(), 3);
        assert_eq!(func.accesses[0].kind, AccessKind::Read);
        assert_eq!(func.accesses[0].register, "STATUS");
        // Slice 2.c: the polling loop's status read is marked
        // PollingLoop so the synthesiser emits the Polling state.
        assert_eq!(func.accesses[0].flow, AccessFlow::PollingLoop);
        assert_eq!(func.accesses[1].kind, AccessKind::Write);
        assert_eq!(func.accesses[1].register, "DATA");
        assert_eq!(func.accesses[1].flow, AccessFlow::Linear);
        assert_eq!(func.accesses[2].kind, AccessKind::Write);
        assert_eq!(func.accesses[2].register, "CTRL");
        assert_eq!(func.accesses[2].flow, AccessFlow::Linear);
        let ctxdsl = func.automaton_ctxdsl.as_ref().expect("automaton emitted");
        assert!(ctxdsl.contains("automaton Uart_send"));
        // The synthesised automaton has the canonical Polling state.
        assert!(ctxdsl.contains("state Loop0"));
        assert!(ctxdsl.contains("rd_status_tx_busy"));
        assert!(ctxdsl.contains("wr_data_byte"));
        assert!(ctxdsl.contains("wr_ctrl_tx_start"));
    }

    #[test]
    fn compound_assignment_emits_read_then_write_on_same_field() {
        // UART->CTRL.bit.tx_start |= 1;
        let lhs = member_expr_chain(
            "UART",
            &[("CTRL", true), ("bit", false), ("tx_start", false)],
        );
        let stmt = serde_json::json!({
            "kind": "CompoundAssignOperator",
            "opcode": "|=",
            "loc": { "line": 3 },
            "inner": [lhs, { "kind": "IntegerLiteral", "value": "1" }],
        });
        let ast = fake_ast_with_body("set_start", 1, "uart.c", "void (void)", vec![stmt]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        let accesses = &extraction.functions[0].accesses;
        assert_eq!(accesses.len(), 2);
        assert_eq!(accesses[0].kind, AccessKind::Read);
        assert_eq!(accesses[1].kind, AccessKind::Write);
        assert_eq!(accesses[0].register, "CTRL");
        assert_eq!(accesses[1].register, "CTRL");
    }

    #[test]
    fn synthesis_skipped_when_no_matched_accesses() {
        // A function with no register accesses gets no automaton even
        // if synthesize_automaton is true.
        let assign = serde_json::json!({
            "kind": "BinaryOperator",
            "opcode": "=",
            "loc": { "line": 3 },
            "inner": [
                { "kind": "DeclRefExpr", "referencedDecl": { "name": "x" } },
                { "kind": "IntegerLiteral", "value": "1" },
            ],
        });
        let ast = fake_ast_with_body("noop", 1, "uart.c", "void (void)", vec![assign]);
        let rm = uart_register_map();
        let opts = CExtractOptions {
            register_map: Some(rm),
            synthesize_automaton: true,
            ..Default::default()
        };
        let extraction =
            extract_from_ast_json_with_options(&ast, "", std::path::Path::new("uart.c"), &opts)
                .unwrap();
        assert!(extraction.functions[0].accesses.is_empty());
        assert!(extraction.functions[0].automaton_ctxdsl.is_none());
    }

    #[test]
    fn sanitise_ident_for_ctxdsl_pascal_cases_first_char() {
        assert_eq!(sanitise_ident_for_ctxdsl("uart_send"), "Uart_send");
        assert_eq!(sanitise_ident_for_ctxdsl("Foo"), "Foo");
        assert_eq!(sanitise_ident_for_ctxdsl(""), "Func");
        assert_eq!(sanitise_ident_for_ctxdsl("123bad"), "123bad");
    }
}
