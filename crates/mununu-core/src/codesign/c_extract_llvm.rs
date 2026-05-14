//! C extraction via LLVM IR — phase L1 of the principled lift.
//!
//! Implementation plan:
//! `~/.claude/plans/i-want-you-to-distributed-orbit.md`.
//!
//! ## What phase L1 ships
//!
//! - A subprocess shell-out to `clang -O0 -emit-llvm -S` (the same
//!   binary the AST extractor in [`crate::codesign::c_extract`]
//!   already invokes; no new tool dependency).
//! - The textual `.ll` output is parsed by [`crate::llvm_ir`] into a
//!   structured [`llvm_ir::Module`].
//! - The structured form is returned to the caller as a JSON dump
//!   ([`LlvmExtraction`]). **No automaton synthesis yet** — that
//!   lands in phase L2 (register-access identification) and phase L3
//!   (polling-loop detection).
//!
//! ## Soundness posture
//!
//! Same as the AST path: best-effort, with unrecognised IR
//! instructions preserved as [`llvm_ir::Instruction::Other`] so
//! downstream consumers can still see them. The parser is a
//! line-based regex matcher; it tolerates unknown lines without
//! erroring out.

use crate::llvm_ir::{Module, parse_module};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Errors raised by [`extract_c_via_llvm`].
#[derive(Debug)]
pub enum LlvmExtractError {
    /// `clang` could not be spawned (not installed, not on PATH).
    ClangNotFound { tried: String, message: String },
    /// `clang` ran but returned a non-zero exit code.
    ClangFailed {
        status: String,
        stderr: String,
        invocation: String,
    },
    /// `clang` ran cleanly but produced output the IR parser
    /// rejected.
    IrParseFailed(String),
    /// Failed to read the source file before invoking clang.
    SourceReadFailed { path: PathBuf, message: String },
}

impl fmt::Display for LlvmExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlvmExtractError::ClangNotFound { tried, message } => write!(
                f,
                "could not spawn clang (tried `{tried}`): {message}. Install via xcode-select --install (macOS) or `apt install clang` (Linux), or pass --clang <path>."
            ),
            LlvmExtractError::ClangFailed {
                status,
                stderr,
                invocation,
            } => write!(
                f,
                "clang exited {status}\ninvocation: {invocation}\nstderr:\n{stderr}"
            ),
            LlvmExtractError::IrParseFailed(msg) => {
                write!(f, "LLVM IR parser rejected clang output: {msg}")
            }
            LlvmExtractError::SourceReadFailed { path, message } => {
                write!(
                    f,
                    "failed to read source file {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for LlvmExtractError {}

/// Configuration for [`extract_c_via_llvm`]. Mirrors the AST path's
/// `CExtractOptions` shape so a CLI / API can thread options through
/// either backend transparently.
#[derive(Debug, Clone, Default)]
pub struct LlvmExtractOptions {
    /// Path to the `clang` binary. Default: `clang` (resolved via
    /// PATH).
    pub clang_path: Option<PathBuf>,
    /// Additional include paths (`-I`).
    pub include_paths: Vec<PathBuf>,
    /// Preprocessor defines (`-D`).
    pub defines: Vec<String>,
    /// Extra raw arguments to clang.
    pub extra_clang_args: Vec<String>,
}

/// Output of [`extract_c_via_llvm`]. Phase L1 emits the parsed IR as
/// a JSON record so the CLI / HTTP surface can inspect what clang
/// produced.
///
/// **Note**: the structured [`llvm_ir::Module`] is not yet
/// `Serialize` — phase L1 emits a *summary* (function names,
/// per-function basic-block + instruction counts, register-touching
/// candidates) for diagnostic purposes. Phase L2 will replace this
/// with a richer `RegisterAccess` list once the matcher lands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlvmExtraction {
    /// Source filename as recorded by clang.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_filename: Option<String>,
    /// Target triple from the `.ll` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_triple: Option<String>,
    /// Per-function structural summary.
    pub functions: Vec<LlvmFunctionSummary>,
    /// Module-level external globals (typical for peripheral base
    /// pointers like `@UART`).
    pub external_globals: Vec<String>,
    /// Named struct types declared at module scope (peripheral
    /// register-bank layouts typically land here).
    pub struct_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlvmFunctionSummary {
    pub name: String,
    pub return_type: String,
    pub num_parameters: usize,
    pub num_basic_blocks: usize,
    pub num_instructions: usize,
    /// Number of `load volatile` / `store volatile` instructions —
    /// the candidate register accesses phase L2 will match against
    /// the register map.
    pub num_volatile_loads: usize,
    pub num_volatile_stores: usize,
    /// Number of `inttoptr` instructions — the candidate
    /// MMIO-by-literal-address accesses (the Example 2 idiom).
    pub num_inttoptr: usize,
}

/// Extract LLVM IR from a C source file and produce a structural
/// summary.
pub fn extract_c_via_llvm(
    source_path: &Path,
    options: &LlvmExtractOptions,
) -> Result<LlvmExtraction, LlvmExtractError> {
    // Touch the file to surface a friendly error before clang runs.
    if !source_path.exists() {
        return Err(LlvmExtractError::SourceReadFailed {
            path: source_path.to_path_buf(),
            message: "file not found".to_string(),
        });
    }

    let clang = options
        .clang_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("clang"));

    let mut cmd = Command::new(&clang);
    cmd.arg("-O0")
        .arg("-emit-llvm")
        .arg("-S")
        .arg("-o")
        .arg("-") // stdout
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

    let invocation = format!("{cmd:?}");
    let output = cmd.output().map_err(|e| LlvmExtractError::ClangNotFound {
        tried: clang.display().to_string(),
        message: e.to_string(),
    })?;
    if !output.status.success() {
        return Err(LlvmExtractError::ClangFailed {
            status: format!("{}", output.status),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            invocation,
        });
    }
    let ir_text = String::from_utf8_lossy(&output.stdout).into_owned();
    extract_from_ir_text(&ir_text)
}

/// Pure-function half of the extractor — takes an IR text and
/// produces the summary. Separated from [`extract_c_via_llvm`] so
/// tests can drive it without needing clang on `$PATH`.
pub fn extract_from_ir_text(ir_text: &str) -> Result<LlvmExtraction, LlvmExtractError> {
    let module: Module =
        parse_module(ir_text).map_err(|e| LlvmExtractError::IrParseFailed(e.to_string()))?;
    Ok(summarise(&module))
}

fn summarise(module: &Module) -> LlvmExtraction {
    use crate::llvm_ir::{GlobalKind, Instruction};

    let external_globals: Vec<String> = module
        .globals
        .iter()
        .filter(|g| {
            g.linkage.iter().any(|l| l == "external") || matches!(g.kind, GlobalKind::Constant)
        })
        .map(|g| g.name.clone())
        .collect();

    let struct_types: Vec<String> = module.struct_types.keys().cloned().collect();

    let functions = module
        .functions
        .iter()
        .map(|f| {
            let num_instructions: usize = f.basic_blocks.iter().map(|b| b.instructions.len()).sum();
            let num_volatile_loads = f
                .basic_blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .filter(|i| matches!(i, Instruction::Load { volatile: true, .. }))
                .count();
            let num_volatile_stores = f
                .basic_blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .filter(|i| matches!(i, Instruction::Store { volatile: true, .. }))
                .count();
            let num_inttoptr = f
                .basic_blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .filter(|i| matches!(i, Instruction::IntToPtr { .. }))
                .count();
            LlvmFunctionSummary {
                name: f.name.clone(),
                return_type: f.return_type.clone(),
                num_parameters: f.parameters.len(),
                num_basic_blocks: f.basic_blocks.len(),
                num_instructions,
                num_volatile_loads,
                num_volatile_stores,
                num_inttoptr,
            }
        })
        .collect();

    LlvmExtraction {
        source_filename: module.source_filename.clone(),
        target_triple: module.target_triple.clone(),
        functions,
        external_globals,
        struct_types,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRMWARE_IR: &str = r#"; ModuleID = 'firmware.c'
source_filename = "firmware.c"
target triple = "x86_64-apple-macosx26.0.0"

%struct.UART_TypeDef = type { %union.anon, %union.anon.0, %union.anon.2 }

@UART = external constant ptr, align 8

define void @uart_send(i8 noundef zeroext %0) {
  %2 = alloca i8, align 1
  store i8 %0, ptr %2, align 1
  br label %3

3:
  %4 = load ptr, ptr @UART, align 8
  %5 = getelementptr inbounds %struct.UART_TypeDef, ptr %4, i32 0, i32 1
  %6 = load volatile i32, ptr %5, align 4
  %7 = and i32 %6, 1
  %8 = icmp ne i32 %7, 0
  br i1 %8, label %9, label %10

9:
  br label %3

10:
  %11 = load i8, ptr %2, align 1
  %12 = load ptr, ptr @UART, align 8
  %13 = getelementptr inbounds %struct.UART_TypeDef, ptr %12, i32 0, i32 2
  store volatile i8 %11, ptr %13, align 4
  ret void
}
"#;

    #[test]
    fn summarises_uart_send_function() {
        let ext = extract_from_ir_text(FIRMWARE_IR).unwrap();
        assert_eq!(ext.functions.len(), 1);
        let f = &ext.functions[0];
        assert_eq!(f.name, "uart_send");
        assert_eq!(f.return_type, "void");
        assert_eq!(f.num_parameters, 1);
        // entry + %3 + %9 + %10
        assert_eq!(f.num_basic_blocks, 4);
        assert_eq!(f.num_volatile_loads, 1);
        assert_eq!(f.num_volatile_stores, 1);
        assert_eq!(f.num_inttoptr, 0);
    }

    #[test]
    fn surfaces_external_globals_and_struct_types() {
        let ext = extract_from_ir_text(FIRMWARE_IR).unwrap();
        assert!(ext.external_globals.contains(&"UART".to_string()));
        assert!(ext.struct_types.iter().any(|s| s == "%struct.UART_TypeDef"));
    }

    #[test]
    fn summary_round_trips_through_serde() {
        let ext = extract_from_ir_text(FIRMWARE_IR).unwrap();
        let json = serde_json::to_string_pretty(&ext).unwrap();
        let parsed: LlvmExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.functions.len(), 1);
        assert_eq!(parsed.functions[0].name, "uart_send");
    }

    #[test]
    fn empty_ir_input_errors() {
        assert!(matches!(
            extract_from_ir_text("").unwrap_err(),
            LlvmExtractError::IrParseFailed(_)
        ));
    }

    #[test]
    fn counts_inttoptr_register_access_candidates() {
        let ir = r#"define void @enable_pll() {
  %1 = inttoptr i64 1073887232 to ptr
  %2 = load volatile i32, ptr %1, align 4
  %3 = or i32 %2, 16777216
  store volatile i32 %3, ptr %1, align 4
  ret void
}
"#;
        let ext = extract_from_ir_text(ir).unwrap();
        assert_eq!(ext.functions[0].num_inttoptr, 1);
        assert_eq!(ext.functions[0].num_volatile_loads, 1);
        assert_eq!(ext.functions[0].num_volatile_stores, 1);
    }
}
