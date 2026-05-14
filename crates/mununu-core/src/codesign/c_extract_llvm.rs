//! C extraction via LLVM IR — phases L1 + L2 of the principled lift.
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
//!
//! ## What phase L2 ships
//!
//! - **Register-access identification.** Walks each function's
//!   `load volatile` / `store volatile` instructions, traces their
//!   pointer operands back through `getelementptr` /
//!   `load ptr, @global` / `inttoptr` chains, and matches the
//!   resulting symbolic address against a supplied
//!   [`RegisterMap`](crate::codesign::register_map::RegisterMap).
//! - **Linear automaton synthesis** on the same
//!   [`crate::codesign::coupling::rendezvous_label_name`] alphabet
//!   the AST extractor uses. Phase L3 will add polling-loop
//!   detection on top.
//! - Closes motivating examples 1 (macro base pointer — the IR has
//!   the literal address regardless of how the C source spells it)
//!   and 2 (`*(volatile uint32_t *)0x40010000` — the IR `inttoptr`
//!   literal is matched directly against the register map's
//!   address range).
//!
//! ## Soundness posture
//!
//! Best-effort, with unrecognised IR instructions preserved as
//! [`crate::llvm_ir::Instruction::Other`] so downstream consumers
//! can still see them. The register-access matcher emits a
//! structured [`LlvmExtractWarning::UnresolvedPointer`] when it
//! cannot fully resolve a `load volatile` / `store volatile`'s
//! pointer operand, and a [`LlvmExtractWarning::UnknownAccessor`]
//! when the resolved address is outside any register-map entry's
//! window. Nothing is silent.

use crate::codesign::c_extract::{AccessFlow, RegisterAccess};
use crate::codesign::coupling::AccessKind;
use crate::codesign::register_map::{Register, RegisterMap};
use crate::llvm_ir::{GepIndex, Instruction, Module, PointerOperand, Terminator, parse_module};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// Phase L2: register map to match `load volatile` / `store
    /// volatile` instructions against. When `None`, the extractor
    /// returns only the structural summary (L1 behaviour).
    pub register_map: Option<RegisterMap>,
    /// Phase L2: when `true` and `register_map` is supplied, emit
    /// per-function linear CTXDSL automata on the rendezvous-label
    /// alphabet. Phase L3 will extend this with polling-loop
    /// detection.
    pub synthesize_automaton: bool,
}

/// Phase L2 warnings — the register-access matcher's structured
/// diagnostics. Mirror the AST path's `CExtractWarning` shape so a
/// downstream consumer doesn't have to fork on backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlvmExtractWarning {
    /// A `load volatile` / `store volatile` was found but its
    /// pointer operand could not be resolved through the supported
    /// chain shapes (GEP / inttoptr / load@global / bitcast). The
    /// access is dropped; the user is told.
    UnresolvedPointer { function: String, ssa: String },
    /// The pointer resolved to a symbolic address that does not lie
    /// inside any register-map entry's `[base + offset, base +
    /// offset + width]` window.
    UnknownAccessor { function: String, address: String },
    /// A bit-field load-modify-store sequence was partially
    /// recognised but slice 2 did not fully decompose it. Phase L4
    /// will handle per-field expansion.
    PartialBitfield { function: String, register: String },
}

impl fmt::Display for LlvmExtractWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlvmExtractWarning::UnresolvedPointer { function, ssa } => write!(
                f,
                "{function}: could not resolve pointer operand %{ssa} of a volatile load/store; access dropped"
            ),
            LlvmExtractWarning::UnknownAccessor { function, address } => write!(
                f,
                "{function}: volatile load/store at address {address} does not match any register-map entry; access dropped"
            ),
            LlvmExtractWarning::PartialBitfield { function, register } => write!(
                f,
                "{function}: bit-field load-modify-store on register {register} not yet decomposed (phase L4)"
            ),
        }
    }
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
    /// Phase L2: structured warnings from the register-access
    /// matcher. Empty when no register map was supplied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<LlvmExtractWarning>,
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
    /// Phase L2: register accesses identified in source order. Empty
    /// when no register map was supplied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accesses: Vec<RegisterAccess>,
    /// Phase L2: CTXDSL automaton fragment synthesised from
    /// [`Self::accesses`]. `Some` only when synthesis was requested
    /// via [`LlvmExtractOptions::synthesize_automaton`] and the
    /// function has at least one matched access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automaton_ctxdsl: Option<String>,
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
    extract_from_ir_text_with_options(&ir_text, options)
}

/// Pure-function half of the extractor — takes an IR text and
/// produces the summary. Separated from [`extract_c_via_llvm`] so
/// tests can drive it without needing clang on `$PATH`. Uses
/// default options (no register-map matching).
pub fn extract_from_ir_text(ir_text: &str) -> Result<LlvmExtraction, LlvmExtractError> {
    extract_from_ir_text_with_options(ir_text, &LlvmExtractOptions::default())
}

/// Phase L2 entry point: parse the IR and run the register-access
/// matcher against the supplied register map.
pub fn extract_from_ir_text_with_options(
    ir_text: &str,
    options: &LlvmExtractOptions,
) -> Result<LlvmExtraction, LlvmExtractError> {
    let module: Module =
        parse_module(ir_text).map_err(|e| LlvmExtractError::IrParseFailed(e.to_string()))?;
    Ok(summarise(&module, options))
}

fn summarise(module: &Module, options: &LlvmExtractOptions) -> LlvmExtraction {
    use crate::llvm_ir::GlobalKind;

    let external_globals: Vec<String> = module
        .globals
        .iter()
        .filter(|g| {
            g.linkage.iter().any(|l| l == "external") || matches!(g.kind, GlobalKind::Constant)
        })
        .map(|g| g.name.clone())
        .collect();

    let struct_types: Vec<String> = module.struct_types.keys().cloned().collect();

    let mut warnings: Vec<LlvmExtractWarning> = Vec::new();
    let functions: Vec<LlvmFunctionSummary> = module
        .functions
        .iter()
        .map(|f| summarise_function(f, options, &mut warnings))
        .collect();

    LlvmExtraction {
        source_filename: module.source_filename.clone(),
        target_triple: module.target_triple.clone(),
        functions,
        external_globals,
        struct_types,
        warnings,
    }
}

fn summarise_function(
    f: &crate::llvm_ir::Function,
    options: &LlvmExtractOptions,
    warnings: &mut Vec<LlvmExtractWarning>,
) -> LlvmFunctionSummary {
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

    let (accesses, automaton_ctxdsl) = if let Some(rm) = options.register_map.as_ref() {
        let accesses = extract_register_accesses(f, rm, warnings);
        let automaton = if options.synthesize_automaton && !accesses.is_empty() {
            Some(crate::codesign::c_extract::synthesise_automaton_ctxdsl(
                &f.name, &accesses, rm,
            ))
        } else {
            None
        };
        (accesses, automaton)
    } else {
        (Vec::new(), None)
    };

    LlvmFunctionSummary {
        name: f.name.clone(),
        return_type: f.return_type.clone(),
        num_parameters: f.parameters.len(),
        num_basic_blocks: f.basic_blocks.len(),
        num_instructions,
        num_volatile_loads,
        num_volatile_stores,
        num_inttoptr,
        accesses,
        automaton_ctxdsl,
    }
}

// --------------------------------------------------------------------
// Phase L2: register-access identification from IR.
// --------------------------------------------------------------------

/// A resolved symbolic address — the result of tracing a pointer
/// operand back through GEP / load@global / inttoptr chains.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedAddress {
    /// `@global` was loaded directly, then GEP'd at field index `N`.
    /// E.g. `UART->REG_N` where N is the LLVM struct field index of
    /// REG within the peripheral struct.
    GlobalFieldIndex { global: String, field_index: i64 },
    /// An inline-constant byte address — `*(volatile T *)0x40010000`
    /// or an `inttoptr` instruction with a literal value.
    AbsoluteAddress(u64),
}

/// Walk `f`'s `load volatile` / `store volatile` instructions in
/// source order, resolve each pointer operand, and match the result
/// against the register map.
fn extract_register_accesses(
    f: &crate::llvm_ir::Function,
    rm: &RegisterMap,
    warnings: &mut Vec<LlvmExtractWarning>,
) -> Vec<RegisterAccess> {
    // Build an SSA-name → producing-instruction lookup over the
    // whole function. Phase L2 only needs read-only traversal; the
    // lookup is keyed by the SSA `result` field. Source-order
    // iteration on the basic-block list gives us the natural
    // dominance ordering for the in-source-order patterns clang
    // emits at `-O0`.
    let mut ssa_defs: HashMap<&str, &Instruction> = HashMap::new();
    for bb in &f.basic_blocks {
        for instr in &bb.instructions {
            if let Some(result) = instruction_result(instr) {
                ssa_defs.insert(result, instr);
            }
        }
    }

    let mut accesses: Vec<RegisterAccess> = Vec::new();
    let mut current_line: u32 = 0;
    for bb in &f.basic_blocks {
        // Approximate source-line ordering by basic-block traversal —
        // phase L2 doesn't have real source-line info because the IR
        // doesn't carry !dbg metadata by default. We use a monotonic
        // counter so the synthesiser's S0 → S1 → ... numbering stays
        // stable.
        for instr in &bb.instructions {
            current_line = current_line.saturating_add(1);
            let (pointer, kind) = match instr {
                Instruction::Load {
                    volatile: true,
                    source,
                    ..
                } => (source, AccessKind::Read),
                Instruction::Store {
                    volatile: true,
                    dest,
                    ..
                } => (dest, AccessKind::Write),
                _ => continue,
            };
            match resolve_pointer(pointer, &ssa_defs) {
                Some(addr) => {
                    if let Some(access) = match_address_to_register(&addr, rm, kind, current_line) {
                        accesses.push(access);
                    } else {
                        warnings.push(LlvmExtractWarning::UnknownAccessor {
                            function: f.name.clone(),
                            address: format!("{addr:?}"),
                        });
                    }
                }
                None => {
                    let ssa_label = match pointer {
                        PointerOperand::Ssa(s) => s.clone(),
                        PointerOperand::Global(g) => format!("@{g}"),
                        PointerOperand::InlineConstAddr(a) => format!("0x{a:x}"),
                    };
                    warnings.push(LlvmExtractWarning::UnresolvedPointer {
                        function: f.name.clone(),
                        ssa: ssa_label,
                    });
                }
            }
        }
        // Walk through conditional/unconditional branches to keep the
        // current-line counter advancing across basic blocks.
        if let Terminator::Br { .. } | Terminator::BrCond { .. } = bb.terminator {
            current_line = current_line.saturating_add(1);
        }
    }
    accesses
}

/// Return the SSA-result name of an instruction, if it produces one.
fn instruction_result(instr: &Instruction) -> Option<&str> {
    Some(match instr {
        Instruction::Alloca { result, .. } => result,
        Instruction::Load { result, .. } => result,
        Instruction::Gep { result, .. } => result,
        Instruction::IntToPtr { result, .. } => result,
        Instruction::PtrToInt { result, .. } => result,
        Instruction::Bitcast { result, .. } => result,
        Instruction::Trunc { result, .. } => result,
        Instruction::ZExt { result, .. } => result,
        Instruction::SExt { result, .. } => result,
        Instruction::BinaryOp { result, .. } => result,
        Instruction::Icmp { result, .. } => result,
        Instruction::Phi { result, .. } => result,
        Instruction::Call {
            result: Some(r), ..
        } => r,
        Instruction::Call { result: None, .. } => return None,
        Instruction::Store { .. } => return None,
        Instruction::Other(_) => return None,
    })
}

/// Resolve a pointer operand to a symbolic address by walking back
/// through GEP / `load ptr, ptr @global` / `inttoptr` / `bitcast`
/// chains. Returns `None` when the chain is too complex (e.g.,
/// dynamic GEP indices, multi-step pointer arithmetic).
fn resolve_pointer(
    pointer: &PointerOperand,
    ssa_defs: &HashMap<&str, &Instruction>,
) -> Option<ResolvedAddress> {
    match pointer {
        PointerOperand::Global(name) => Some(ResolvedAddress::GlobalFieldIndex {
            global: name.clone(),
            field_index: 0,
        }),
        PointerOperand::InlineConstAddr(addr) => Some(ResolvedAddress::AbsoluteAddress(*addr)),
        PointerOperand::Ssa(name) => {
            let def = ssa_defs.get(name.as_str())?;
            resolve_instruction_as_address(def, ssa_defs)
        }
    }
}

fn resolve_instruction_as_address(
    instr: &Instruction,
    ssa_defs: &HashMap<&str, &Instruction>,
) -> Option<ResolvedAddress> {
    match instr {
        // GEP: trace the base and add the field index. Phase L2 only
        // handles the canonical `GEP %struct.T, ptr %base, i32 0,
        // i32 K` shape — index 0 (no offset), then the field index
        // K. Anything more complex returns None (drops the access
        // with an UnresolvedPointer warning).
        Instruction::Gep { base, indices, .. } => {
            // The first index must be 0 (struct-base indirection);
            // the second index is the field-position.
            let mut field_index: Option<i64> = None;
            for (i, idx) in indices.iter().enumerate() {
                match idx {
                    GepIndex::Const(n) if i == 0 && *n != 0 => return None,
                    GepIndex::Const(n) if i == 1 => {
                        field_index = Some(*n);
                    }
                    GepIndex::Const(_) if i > 1 => {
                        // Nested GEP — phase L4 will handle bit-field
                        // sub-indexing. For L2, drop the access.
                        return None;
                    }
                    GepIndex::Dynamic(_) => return None,
                    _ => {}
                }
            }
            let field_index = field_index?;
            let base_addr = resolve_pointer(base, ssa_defs)?;
            match base_addr {
                ResolvedAddress::GlobalFieldIndex { global, .. } => {
                    Some(ResolvedAddress::GlobalFieldIndex {
                        global,
                        field_index,
                    })
                }
                ResolvedAddress::AbsoluteAddress(_) => {
                    // GEP through a literal address — not yet
                    // supported; would need struct-layout info to
                    // compute byte offset.
                    None
                }
            }
        }
        // `load ptr, ptr @global`: the load *result* is the
        // peripheral base pointer value. Treat it as a synonym for
        // `@global` in subsequent GEPs.
        Instruction::Load {
            source: PointerOperand::Global(name),
            volatile: false,
            ..
        } => Some(ResolvedAddress::GlobalFieldIndex {
            global: name.clone(),
            field_index: 0,
        }),
        Instruction::IntToPtr { value, .. } => Some(ResolvedAddress::AbsoluteAddress(*value)),
        Instruction::Bitcast { source, .. } => {
            // Bitcast preserves address; resolve the source SSA.
            let def = ssa_defs.get(source.as_str())?;
            resolve_instruction_as_address(def, ssa_defs)
        }
        _ => None,
    }
}

/// Map a resolved address back to a register-map entry.
///
/// For [`ResolvedAddress::GlobalFieldIndex`], the field index maps
/// 1:1 to a register-by-position in the register map. This is sound
/// for the common case where the C struct's field order matches the
/// register map's declaration order — exactly the codesign_uart
/// pattern. Phase L4 will refine this with byte-offset matching
/// using LLVM data layout when alignment padding diverges.
///
/// For [`ResolvedAddress::AbsoluteAddress`], the literal address is
/// matched against `[base + offset, base + offset + width]` of each
/// register-map entry.
///
/// L2 emits read/write accesses; field selection within a register
/// (which bit of a packed register is touched) is deferred to phase
/// L4. For now, an access to register `R` with `R` having one or
/// more fields produces one access per *field* if the read/write
/// touches the whole register; this is conservative
/// (over-approximation: assumes all fields were touched).
fn match_address_to_register(
    addr: &ResolvedAddress,
    rm: &RegisterMap,
    kind: AccessKind,
    source_line: u32,
) -> Option<RegisterAccess> {
    let register: &Register = match addr {
        ResolvedAddress::GlobalFieldIndex { field_index, .. } => {
            let idx = usize::try_from(*field_index).ok()?;
            rm.registers.get(idx)?
        }
        ResolvedAddress::AbsoluteAddress(abs) => {
            let base = rm.base_address_value()?;
            rm.registers.iter().find(|r| {
                let reg_byte_offset = r.offset;
                let reg_width_bytes = u64::from(r.width_bits / 8);
                let lo = base.wrapping_add(reg_byte_offset);
                let hi = lo.wrapping_add(reg_width_bytes);
                *abs >= lo && *abs < hi
            })?
        }
    };

    // Phase L2: when the register has fields, pick the field that
    // looks like the canonical "first" field (lowest bit-range). A
    // proper per-field expansion is phase L4. When the register has
    // no fields, emit a whole-register access.
    let field_name = register
        .fields
        .iter()
        .min_by_key(|f| f.bits[0])
        .map(|f| f.name.clone());

    Some(RegisterAccess {
        kind,
        register: register.name.clone(),
        field: field_name,
        accessor: format!("(IR-resolved @{addr:?})"),
        source_line,
        flow: AccessFlow::Linear,
    })
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

    // ----------------------------------------------------------------
    // Phase L2 tests — register-access matching from IR.
    // ----------------------------------------------------------------

    use crate::codesign::register_map::{
        AccessPath, Field, Register, RegisterDirection, RegisterMap, VisibilityClass,
    };

    fn uart_register_map() -> RegisterMap {
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
                        sv_signal: None,
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
                        sv_signal: None,
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
                        sv_signal: None,
                        c_accessor: Some("UART->DATA.byte".to_string()),
                        description: None,
                    }],
                },
            ],
        }
    }

    #[test]
    fn l2_extracts_two_volatile_accesses_in_firmware() {
        // The FIRMWARE_IR fixture has one volatile load (STATUS) and
        // one volatile store (DATA). Phase L2 must match both to
        // register-map entries.
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let f = &ext.functions[0];
        assert_eq!(f.accesses.len(), 2, "{:#?}", f.accesses);
        // First access: load volatile from STATUS (field index 1).
        assert_eq!(f.accesses[0].kind, AccessKind::Read);
        assert_eq!(f.accesses[0].register, "STATUS");
        // Second access: store volatile to DATA (field index 2).
        assert_eq!(f.accesses[1].kind, AccessKind::Write);
        assert_eq!(f.accesses[1].register, "DATA");
    }

    #[test]
    fn l2_emits_no_accesses_without_register_map() {
        // Slice-2.a / L1 behaviour preserved when no map is given.
        let ext = extract_from_ir_text(FIRMWARE_IR).unwrap();
        let f = &ext.functions[0];
        assert!(f.accesses.is_empty());
        assert!(f.automaton_ctxdsl.is_none());
        assert!(ext.warnings.is_empty());
    }

    #[test]
    fn l2_synthesises_linear_automaton_when_requested() {
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            synthesize_automaton: true,
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let f = &ext.functions[0];
        let ctxdsl = f.automaton_ctxdsl.as_ref().expect("automaton emitted");
        // Phase L2's linear synthesis: S0 → S1 → S2 with no Loop_i
        // state. Phase L3 will add Loop0 for the polling loop.
        assert!(ctxdsl.contains("state S0 initial"));
        assert!(ctxdsl.contains("state S1"));
        assert!(ctxdsl.contains("state S2"));
        assert!(!ctxdsl.contains("Loop0"), "phase L3 territory");
        // Labels come from the rendezvous-label convention.
        assert!(ctxdsl.contains("rd_status_tx_busy"));
        assert!(ctxdsl.contains("wr_data_byte"));
    }

    #[test]
    fn l2_inttoptr_literal_matches_against_register_window() {
        // Example 2 from the plan: `*(volatile uint32_t *)0x40010004 = 1`
        // → a `store volatile` to an `inttoptr` literal landing inside
        //   STATUS register's [base + 4, base + 8) byte window.
        let ir = r#"source_filename = "f.c"
define void @clear_status() {
  %1 = inttoptr i64 1073807364 to ptr
  store volatile i32 1, ptr %1, align 4
  ret void
}
"#;
        // 0x40010004 = 1073807364.
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        let f = &ext.functions[0];
        assert_eq!(f.accesses.len(), 1);
        assert_eq!(f.accesses[0].kind, AccessKind::Write);
        assert_eq!(f.accesses[0].register, "STATUS");
    }

    #[test]
    fn l2_inttoptr_outside_register_window_emits_unknown_warning() {
        // Address 0x60000000 is outside the UART_LITE peripheral's
        // [0x40010000, 0x4001000C) window. The access must NOT be
        // emitted and an UnknownAccessor warning must surface.
        let ir = r#"define void @stray() {
  %1 = inttoptr i64 1610612736 to ptr
  store volatile i32 1, ptr %1, align 4
  ret void
}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        assert!(ext.functions[0].accesses.is_empty());
        assert!(
            ext.warnings
                .iter()
                .any(|w| matches!(w, LlvmExtractWarning::UnknownAccessor { .. }))
        );
    }

    #[test]
    fn l2_dynamic_gep_index_surfaces_unresolved_warning() {
        // A GEP with a dynamic (SSA) index can't be resolved
        // statically. Phase L2 emits UnresolvedPointer and drops
        // the access.
        let ir = r#"%struct.UART_TypeDef = type { i32, i32, i32 }
@UART = external constant ptr, align 8
define void @bad_index(i32 %0) {
  %2 = load ptr, ptr @UART, align 8
  %3 = getelementptr inbounds %struct.UART_TypeDef, ptr %2, i32 0, i32 %0
  %4 = load volatile i32, ptr %3, align 4
  ret void
}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        assert!(ext.functions[0].accesses.is_empty());
        assert!(
            ext.warnings
                .iter()
                .any(|w| matches!(w, LlvmExtractWarning::UnresolvedPointer { .. })),
            "got warnings: {:?}",
            ext.warnings
        );
    }

    #[test]
    #[ignore]
    fn debug_dump_firmware_ir() {
        let m = crate::llvm_ir::parse_module(FIRMWARE_IR).unwrap();
        for f in &m.functions {
            eprintln!("Function: {}", f.name);
            for bb in &f.basic_blocks {
                eprintln!("  Block {} (preds: {:?})", bb.label, bb.predecessors);
                for (i, instr) in bb.instructions.iter().enumerate() {
                    eprintln!("    [{i}] {instr:?}");
                }
                eprintln!("    term: {:?}", bb.terminator);
            }
        }
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        eprintln!("Accesses: {:#?}", ext.functions[0].accesses);
        eprintln!("Warnings: {:#?}", ext.warnings);
    }

    #[test]
    fn l2_round_trips_warnings_through_serde() {
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let json = serde_json::to_string_pretty(&ext).unwrap();
        let parsed: LlvmExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.functions.len(), 1);
        assert_eq!(parsed.functions[0].accesses.len(), 2);
    }
}
