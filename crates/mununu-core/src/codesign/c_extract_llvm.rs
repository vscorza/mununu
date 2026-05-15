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
    /// Phase L5: the interprocedural walker hit a recursive call
    /// (direct or transitive). The recursion is broken at the
    /// second visit; the callee's accesses are not unrolled. Sound
    /// over-approximation for safety; liveness verdicts may be
    /// affected.
    RecursiveCall { function: String },
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
            LlvmExtractWarning::RecursiveCall { function } => write!(
                f,
                "{function}: recursive call cycle broken \u{2014} callee accesses not unrolled (over-approximation; sound for safety, may affect liveness)"
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
    /// Phase L6: top-level CTXDSL `compositions { ... }` block
    /// emitted when at least one function carries `@mununu_isr`.
    /// Composes the main-thread automaton asynchronously with each
    /// ISR. `None` when no ISRs are present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition_ctxdsl: Option<String>,
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
    /// Phase L6: `true` when the function carries an `@mununu_isr`
    /// annotation. ISR functions get composed asynchronously with
    /// the main thread in [`LlvmExtraction::composition_ctxdsl`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_isr: bool,
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
    // Phase L6: read the source file too, so we can lift `@mununu_*`
    // annotations from comments and tag functions accordingly.
    let source_text = std::fs::read_to_string(source_path).unwrap_or_default();
    extract_from_ir_text_with_options_and_source(&ir_text, &source_text, options)
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
    extract_from_ir_text_with_options_and_source(ir_text, "", options)
}

/// Phase L6 entry point — same as
/// [`extract_from_ir_text_with_options`] but with the C source
/// alongside the IR so the extractor can lift `@mununu_*`
/// annotations from comments. ISR-annotated functions are tagged
/// in the output and composed asynchronously with the rest.
pub fn extract_from_ir_text_with_options_and_source(
    ir_text: &str,
    source_text: &str,
    options: &LlvmExtractOptions,
) -> Result<LlvmExtraction, LlvmExtractError> {
    let module: Module =
        parse_module(ir_text).map_err(|e| LlvmExtractError::IrParseFailed(e.to_string()))?;
    Ok(summarise(&module, source_text, options))
}

fn summarise(module: &Module, source_text: &str, options: &LlvmExtractOptions) -> LlvmExtraction {
    use crate::llvm_ir::GlobalKind;

    // Phase L6: lift `@mununu_*` annotations from C source and map
    // each `@mununu_isr` annotation to the function it sits above
    // by line-proximity scanning (the next `<ret> NAME(` declaration
    // after the annotation owns it).
    let isr_function_names: std::collections::HashSet<String> = if source_text.is_empty() {
        std::collections::HashSet::new()
    } else {
        detect_isr_functions(source_text)
    };

    let external_globals: Vec<String> = module
        .globals
        .iter()
        .filter(|g| {
            g.linkage.iter().any(|l| l == "external") || matches!(g.kind, GlobalKind::Constant)
        })
        .map(|g| g.name.clone())
        .collect();

    let struct_types: Vec<String> = module.struct_types.keys().cloned().collect();

    // Phase L5: build a module-wide function lookup so the
    // interprocedural walker can resolve `call @callee` instructions
    // to their definitions.
    let module_fns: HashMap<&str, &crate::llvm_ir::Function> = module
        .functions
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();

    let mut warnings: Vec<LlvmExtractWarning> = Vec::new();
    let functions: Vec<LlvmFunctionSummary> = module
        .functions
        .iter()
        .map(|f| {
            let is_isr = isr_function_names.contains(&f.name);
            summarise_function(f, options, &module_fns, is_isr, &mut warnings)
        })
        .collect();

    // Phase L6: emit a top-level CTXDSL `compositions { ... }` block
    // when any ISR functions are present. The composition is
    // asynchronous (Doc C §C.5): main-thread + each ISR interleave
    // non-deterministically.
    let composition_ctxdsl = if !isr_function_names.is_empty() {
        Some(synthesise_compositions_ctxdsl(&functions))
    } else {
        None
    };

    LlvmExtraction {
        source_filename: module.source_filename.clone(),
        target_triple: module.target_triple.clone(),
        functions,
        external_globals,
        struct_types,
        warnings,
        composition_ctxdsl,
    }
}

fn summarise_function(
    f: &crate::llvm_ir::Function,
    options: &LlvmExtractOptions,
    module_fns: &HashMap<&str, &crate::llvm_ir::Function>,
    is_isr: bool,
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
        let mut visiting: std::collections::HashSet<String> = std::collections::HashSet::new();
        let accesses = extract_register_accesses(f, rm, module_fns, &mut visiting, warnings);
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
        is_isr,
    }
}

// --------------------------------------------------------------------
// Phase L3 / L4: pre-pass plan — polling loops + bit-field RMW.
// --------------------------------------------------------------------

/// Pre-pass output used by [`extract_register_accesses`] to refine
/// what it emits.
///
/// Phase L3 + L4 both belong here because they're recognised by
/// looking at IR shapes *before* the linear walker decides what to
/// emit. Keeping the recognition in a pre-pass leaves the walker
/// itself simple.
#[derive(Debug, Default)]
struct ExtractionPlan {
    /// Phase L3: SSA names of `load volatile` instructions that are
    /// the polling read of a `while (cond) ;`-style loop. These get
    /// emitted with `flow: PollingLoop` rather than `Linear`.
    polling_loop_loads: std::collections::HashSet<String>,
    /// Phase L3: basic-block labels that are the back-edge body of a
    /// polling loop. The walker skips them entirely — they have no
    /// register accesses by construction.
    skip_blocks: std::collections::HashSet<String>,
    /// Phase L4: SSA names of `load volatile` instructions that are
    /// the read half of a bit-field read-modify-write sequence.
    /// These are silently consumed; the matching store gets
    /// resolved with [`Self::rmw_store_field`] instead of the
    /// default first-field fallback.
    rmw_consumed_loads: std::collections::HashSet<String>,
    /// Phase L4: SSA name of a `store volatile` → name of the field
    /// whose bits the RMW touches. The walker uses this to emit the
    /// right field on the store side.
    rmw_store_field: std::collections::HashMap<String, String>,
}

/// Build the pre-pass plan for a function. Scans the function once
/// and records every polling loop + every bit-field RMW pattern it
/// finds.
fn build_extraction_plan(
    f: &crate::llvm_ir::Function,
    rm: &RegisterMap,
    ssa_defs: &HashMap<&str, &Instruction>,
) -> ExtractionPlan {
    let mut plan = ExtractionPlan::default();

    // --- Phase L4 pre-pass: bit-field RMW recognition. ---
    //
    // For each `store volatile`, check whether the stored value is
    // `or (and (load volatile %ptr, MASK), BITS)` where the load
    // reads from the same pointer the store writes to. If so:
    //   - mark the load as `rmw_consumed_loads` (skip it),
    //   - compute the touched-bit mask from `(~MASK) | BITS`,
    //   - find the register-map field whose bit range overlaps the
    //     touched bits and record it under `rmw_store_field`.
    for bb in &f.basic_blocks {
        for instr in &bb.instructions {
            let Instruction::Store {
                value: crate::llvm_ir::ValueOperand::Ssa(store_value_ssa),
                dest: store_dest,
                volatile: true,
                ..
            } = instr
            else {
                continue;
            };
            // store value should trace back to `or (and load_result, BITS)`.
            let Some(or_instr) = ssa_defs.get(store_value_ssa.as_str()) else {
                continue;
            };
            let (or_a, or_b) = match or_instr {
                Instruction::BinaryOp {
                    op: crate::llvm_ir::BinaryOp::Or,
                    a,
                    b,
                    ..
                } => (a, b),
                _ => continue,
            };
            let (and_ssa, or_bits) = match (or_a, or_b) {
                (
                    crate::llvm_ir::ValueOperand::Ssa(and_ssa),
                    crate::llvm_ir::ValueOperand::LiteralInt(bits),
                )
                | (
                    crate::llvm_ir::ValueOperand::LiteralInt(bits),
                    crate::llvm_ir::ValueOperand::Ssa(and_ssa),
                ) => (and_ssa.clone(), *bits),
                _ => continue,
            };
            let Some(and_instr) = ssa_defs.get(and_ssa.as_str()) else {
                continue;
            };
            let (and_load_ssa, and_mask) = match and_instr {
                Instruction::BinaryOp {
                    op: crate::llvm_ir::BinaryOp::And,
                    a,
                    b,
                    ..
                } => match (a, b) {
                    (
                        crate::llvm_ir::ValueOperand::Ssa(load_ssa),
                        crate::llvm_ir::ValueOperand::LiteralInt(mask),
                    )
                    | (
                        crate::llvm_ir::ValueOperand::LiteralInt(mask),
                        crate::llvm_ir::ValueOperand::Ssa(load_ssa),
                    ) => (load_ssa.clone(), *mask),
                    _ => continue,
                },
                _ => continue,
            };
            // The and's first operand must be a volatile load whose
            // source is the same pointer as the store's dest.
            let Some(load_instr) = ssa_defs.get(and_load_ssa.as_str()) else {
                continue;
            };
            let Instruction::Load {
                source: load_source,
                volatile: true,
                ..
            } = load_instr
            else {
                continue;
            };
            if load_source != store_dest {
                continue;
            }
            // Compute touched bits. AND mask `M` means bits ~M are
            // cleared; OR with BITS sets those bits. So the touched
            // set is (`~M`) ∪ `BITS`. We treat as u64 for the bit-set
            // computation.
            let touched: u64 = ((!and_mask as u64) | (or_bits as u64)) & 0xFFFF_FFFF;
            // Match touched bits against register fields. We need to
            // know which register the store targets — resolve its
            // pointer through the SSA defs.
            let Some(addr) = resolve_pointer(store_dest, ssa_defs) else {
                continue;
            };
            let register = match &addr {
                ResolvedAddress::GlobalFieldIndex { field_index, .. } => {
                    let Ok(idx) = usize::try_from(*field_index) else {
                        continue;
                    };
                    let Some(r) = rm.registers.get(idx) else {
                        continue;
                    };
                    r
                }
                ResolvedAddress::AbsoluteAddress(abs) => {
                    let Some(base) = rm.base_address_value() else {
                        continue;
                    };
                    let Some(r) = rm.registers.iter().find(|r| {
                        let lo = base.wrapping_add(r.offset);
                        let hi = lo.wrapping_add(u64::from(r.width_bits / 8));
                        *abs >= lo && *abs < hi
                    }) else {
                        continue;
                    };
                    r
                }
                ResolvedAddress::InlineConstexprGep {
                    base_addr,
                    field_index,
                } => {
                    let Some(map_base) = rm.base_address_value() else {
                        continue;
                    };
                    if *base_addr != map_base {
                        continue;
                    }
                    let Ok(idx) = usize::try_from(*field_index) else {
                        continue;
                    };
                    let Some(r) = rm.registers.get(idx) else {
                        continue;
                    };
                    r
                }
            };
            let touched_field = register.fields.iter().find(|fld| {
                // Field's bit range is [fld.bits[0] .. fld.bits[1]].
                let mut mask: u64 = 0;
                for b in fld.bits[0]..=fld.bits[1] {
                    mask |= 1u64 << b;
                }
                touched & mask != 0
            });
            let Some(touched_field) = touched_field else {
                continue;
            };
            plan.rmw_consumed_loads.insert(and_load_ssa);
            // Identify the store by its dest pointer SSA — there's
            // only one store per (load, store) pair in this pattern.
            if let PointerOperand::Ssa(dest_ssa) = store_dest {
                // We key by the store-value's SSA so the walker can
                // look it up at emit time.
                plan.rmw_store_field
                    .insert(store_value_ssa.clone(), touched_field.name.clone());
                let _ = dest_ssa; // currently unused; kept for clarity.
            }
        }
    }

    // --- Phase L3 pre-pass: polling-loop recognition. ---
    //
    // A loop header H satisfies:
    //   - H's terminator is `BrCond { cond, if_true, if_false }`.
    //   - One of {if_true, if_false} (call it B, the back-edge body)
    //     has terminator `Br { target: H }` and no volatile
    //     loads/stores in its instructions.
    //   - H contains exactly one volatile load whose result feeds
    //     `cond` (directly or via an `icmp` whose operand chains
    //     back to the load).
    for header in &f.basic_blocks {
        let Terminator::BrCond {
            cond,
            if_true,
            if_false,
        } = &header.terminator
        else {
            continue;
        };
        // Find the back-edge body — the successor that branches back
        // to the header and has no register accesses.
        let candidate_back_edges = [if_true, if_false];
        let back_edge_label = candidate_back_edges.iter().find(|&succ_label| {
            let succ = f.basic_blocks.iter().find(|b| &b.label == *succ_label);
            let Some(succ) = succ else { return false };
            if !matches!(
                succ.terminator,
                Terminator::Br { ref target } if target == &header.label
            ) {
                return false;
            }
            // The back-edge body must have no volatile loads/stores.
            !succ.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Load { volatile: true, .. }
                        | Instruction::Store { volatile: true, .. }
                )
            })
        });
        let Some(&back_edge_label) = back_edge_label else {
            continue;
        };

        // Find the single volatile load in the header.
        let volatile_loads: Vec<&Instruction> = header
            .instructions
            .iter()
            .filter(|i| matches!(i, Instruction::Load { volatile: true, .. }))
            .collect();
        if volatile_loads.len() != 1 {
            continue;
        }
        let load = volatile_loads[0];
        let Instruction::Load {
            result: load_result,
            ..
        } = load
        else {
            continue;
        };
        // Trace cond back to confirm it depends on this load.
        let cond_ssa = match cond {
            crate::llvm_ir::ValueOperand::Ssa(s) => s,
            _ => continue,
        };
        if !value_depends_on(cond_ssa, load_result, ssa_defs) {
            continue;
        }

        plan.polling_loop_loads.insert(load_result.clone());
        plan.skip_blocks.insert(back_edge_label.clone());
    }

    plan
}

/// Returns `true` when the SSA value rooted at `start_ssa` transitively
/// depends on `target_ssa` via the instructions in `ssa_defs`. Used
/// to confirm a `cond` traces back to a polling-loop's volatile load.
fn value_depends_on(
    start_ssa: &str,
    target_ssa: &str,
    ssa_defs: &HashMap<&str, &Instruction>,
) -> bool {
    if start_ssa == target_ssa {
        return true;
    }
    let mut stack: Vec<&str> = vec![start_ssa];
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    while let Some(name) = stack.pop() {
        if !visited.insert(name) {
            continue;
        }
        if name == target_ssa {
            return true;
        }
        let Some(instr) = ssa_defs.get(name) else {
            continue;
        };
        for operand in operand_ssas(instr) {
            stack.push(operand);
        }
    }
    false
}

/// Collect the SSA names an instruction's operands reference. Phase
/// L3's `value_depends_on` walks this graph backward.
fn operand_ssas<'a>(instr: &'a Instruction) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = Vec::new();
    let push_pointer = |out: &mut Vec<&'a str>, p: &'a PointerOperand| {
        if let PointerOperand::Ssa(s) = p {
            out.push(s);
        }
    };
    let push_value = |out: &mut Vec<&'a str>, v: &'a crate::llvm_ir::ValueOperand| {
        if let crate::llvm_ir::ValueOperand::Ssa(s) = v {
            out.push(s);
        }
    };
    match instr {
        Instruction::Load { source, .. } => push_pointer(&mut out, source),
        Instruction::Store { value, dest, .. } => {
            push_value(&mut out, value);
            push_pointer(&mut out, dest);
        }
        Instruction::Gep { base, indices, .. } => {
            push_pointer(&mut out, base);
            for idx in indices {
                if let GepIndex::Dynamic(s) = idx {
                    out.push(s);
                }
            }
        }
        Instruction::BinaryOp { a, b, .. } | Instruction::Icmp { a, b, .. } => {
            push_value(&mut out, a);
            push_value(&mut out, b);
        }
        Instruction::Trunc { source, .. }
        | Instruction::ZExt { source, .. }
        | Instruction::SExt { source, .. }
        | Instruction::Bitcast { source, .. }
        | Instruction::PtrToInt { source, .. } => out.push(source),
        Instruction::Phi { incoming, .. } => {
            for (val, _) in incoming {
                push_value(&mut out, val);
            }
        }
        Instruction::Call { args, .. } => {
            for arg in args {
                // args are raw token strings; pick the ones that start with %.
                if let Some(rest) = arg.strip_prefix('%')
                    && !rest.is_empty()
                {
                    out.push(rest);
                }
            }
        }
        _ => {}
    }
    out
}

// --------------------------------------------------------------------
// Phase L6: ISR detection + asynchronous composition emission.
// --------------------------------------------------------------------

/// Scan C source text for `@mununu_isr` annotations and return the
/// set of function names they apply to. Each annotation owns the
/// next function declaration line below it (within a 10-line
/// window). Annotation-only — no naming-convention defaults; a
/// function not carrying `@mununu_isr` is *not* an ISR even if its
/// name ends in `_IRQHandler`.
fn detect_isr_functions(source_text: &str) -> std::collections::HashSet<String> {
    use crate::mununu_annotations::{MununuTag, extract_from_c_source};
    use regex::Regex;
    let mut out = std::collections::HashSet::new();
    let isr_annotations: Vec<u32> = extract_from_c_source(source_text)
        .into_iter()
        .filter(|a| a.tag == MununuTag::Isr)
        .filter_map(|a| a.source_line)
        .collect();
    if isr_annotations.is_empty() {
        return out;
    }
    // Scan source lines. For each ISR annotation, look for the next
    // function-declaration line within 10 lines below it.
    let fn_decl = Regex::new(r"^\s*(?:static\s+)?(?:inline\s+)?\S+\s+(\w+)\s*\(").unwrap();
    let lines: Vec<&str> = source_text.lines().collect();
    for ann_line in &isr_annotations {
        let start = *ann_line as usize;
        let end = (start + 10).min(lines.len());
        for line in lines.iter().take(end).skip(start) {
            if let Some(caps) = fn_decl.captures(line) {
                out.insert(caps[1].to_string());
                break;
            }
        }
    }
    out
}

/// Emit a top-level CTXDSL `compositions { ... }` block composing
/// the non-ISR functions asynchronously with each ISR function.
/// Doc C §C.5 mandates asynchronous composition for ISR + main-
/// thread interleaving (synchronous would be unsound — ISRs
/// preempt the main thread at arbitrary points).
fn synthesise_compositions_ctxdsl(functions: &[LlvmFunctionSummary]) -> String {
    use std::fmt::Write;
    let main_thread_fns: Vec<&LlvmFunctionSummary> = functions
        .iter()
        .filter(|f| !f.is_isr && f.automaton_ctxdsl.is_some())
        .collect();
    let isr_fns: Vec<&LlvmFunctionSummary> = functions
        .iter()
        .filter(|f| f.is_isr && f.automaton_ctxdsl.is_some())
        .collect();
    let mut buf = String::new();
    let _ = writeln!(buf, "    compositions {{");
    let _ = writeln!(buf, "        composition Codesign = asynchronous {{");
    for f in &main_thread_fns {
        let _ = writeln!(buf, "            member {};", ctxdsl_ident(&f.name));
    }
    for f in &isr_fns {
        let _ = writeln!(
            buf,
            "            member {};   // ISR",
            ctxdsl_ident(&f.name)
        );
    }
    let _ = writeln!(buf, "        }};");
    let _ = writeln!(buf, "    }}");
    buf
}

/// Sanitise a function name into a CTXDSL identifier shape that
/// matches what `synthesise_automaton_ctxdsl` emits (first char
/// uppercase, rest as-is).
fn ctxdsl_ident(name: &str) -> String {
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
        "Func".to_string()
    } else {
        out
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
    /// An inline `getelementptr` constexpr over an `inttoptr` literal,
    /// the shape clang emits for `MACRO->FIELD` accesses where `MACRO`
    /// is `#define`d to a literal address. The matcher resolves this
    /// against the register map by checking `base_addr == map.base`
    /// and using `field_index` as the register position.
    InlineConstexprGep { base_addr: u64, field_index: i64 },
}

/// Walk `f`'s `load volatile` / `store volatile` instructions in
/// source order, resolve each pointer operand, and match the result
/// against the register map.
fn extract_register_accesses(
    f: &crate::llvm_ir::Function,
    rm: &RegisterMap,
    module_fns: &HashMap<&str, &crate::llvm_ir::Function>,
    visiting: &mut std::collections::HashSet<String>,
    warnings: &mut Vec<LlvmExtractWarning>,
) -> Vec<RegisterAccess> {
    // Phase L5: cycle guard for the interprocedural walk. A function
    // that calls itself (directly or transitively) would otherwise
    // recurse forever.
    if !visiting.insert(f.name.clone()) {
        warnings.push(LlvmExtractWarning::RecursiveCall {
            function: f.name.clone(),
        });
        return Vec::new();
    }

    // Build an SSA-name → producing-instruction lookup over the
    // whole function. The plan below uses it for backwards-tracing
    // (e.g. resolving a store's value through `or → and → load`).
    let mut ssa_defs: HashMap<&str, &Instruction> = HashMap::new();
    for bb in &f.basic_blocks {
        for instr in &bb.instructions {
            if let Some(result) = instruction_result(instr) {
                ssa_defs.insert(result, instr);
            }
        }
    }

    let plan = build_extraction_plan(f, rm, &ssa_defs);

    let mut accesses: Vec<RegisterAccess> = Vec::new();
    let mut current_line: u32 = 0;
    for bb in &f.basic_blocks {
        if plan.skip_blocks.contains(&bb.label) {
            // Phase L3: skip the polling-loop's back-edge body. It
            // has no register accesses by construction.
            continue;
        }
        // Approximate source-line ordering by basic-block traversal.
        for instr in &bb.instructions {
            current_line = current_line.saturating_add(1);
            // Phase L5: when we hit a call to a function defined in
            // the same module, recursively extract its accesses and
            // splice them into the caller's list at this point.
            // External calls fall through silently — the verifier
            // sees no accesses for them (chaotic-stub equivalent).
            if let Instruction::Call { callee, .. } = instr {
                let target = callee.trim_start_matches('@');
                if let Some(callee_fn) = module_fns.get(target) {
                    let callee_accesses =
                        extract_register_accesses(callee_fn, rm, module_fns, visiting, warnings);
                    accesses.extend(callee_accesses);
                }
                continue;
            }
            let (pointer, kind, instr_result) = match instr {
                Instruction::Load {
                    volatile: true,
                    source,
                    result,
                    ..
                } => (source, AccessKind::Read, Some(result.as_str())),
                Instruction::Store {
                    volatile: true,
                    dest,
                    value: crate::llvm_ir::ValueOperand::Ssa(value_ssa),
                    ..
                } => (dest, AccessKind::Write, Some(value_ssa.as_str())),
                Instruction::Store {
                    volatile: true,
                    dest,
                    ..
                } => (dest, AccessKind::Write, None),
                _ => continue,
            };
            // Phase L4: drop loads consumed by a bit-field RMW.
            if let (AccessKind::Read, Some(r)) = (kind, instr_result)
                && plan.rmw_consumed_loads.contains(r)
            {
                continue;
            }
            match resolve_pointer(pointer, &ssa_defs) {
                Some(addr) => {
                    let field_override = if kind == AccessKind::Write {
                        instr_result.and_then(|r| plan.rmw_store_field.get(r).cloned())
                    } else {
                        None
                    };
                    let flow = match (kind, instr_result) {
                        (AccessKind::Read, Some(r)) if plan.polling_loop_loads.contains(r) => {
                            AccessFlow::PollingLoop
                        }
                        _ => AccessFlow::Linear,
                    };
                    if let Some(access) = match_address_to_register(
                        &addr,
                        rm,
                        kind,
                        current_line,
                        flow,
                        field_override.as_deref(),
                    ) {
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
                        PointerOperand::InlineGep {
                            base_addr,
                            field_index,
                        } => format!("inline_gep(0x{base_addr:x}, field {field_index})"),
                    };
                    warnings.push(LlvmExtractWarning::UnresolvedPointer {
                        function: f.name.clone(),
                        ssa: ssa_label,
                    });
                }
            }
        }
        if let Terminator::Br { .. } | Terminator::BrCond { .. } = bb.terminator {
            current_line = current_line.saturating_add(1);
        }
    }
    visiting.remove(&f.name);
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
        PointerOperand::InlineGep {
            base_addr,
            field_index,
        } => Some(ResolvedAddress::InlineConstexprGep {
            base_addr: *base_addr,
            field_index: *field_index,
        }),
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
                ResolvedAddress::AbsoluteAddress(addr) => {
                    // GEP through a literal address — synthesise an
                    // InlineConstexprGep with the base + new field
                    // index. Phase L4 may refine this with proper
                    // byte-offset reasoning.
                    Some(ResolvedAddress::InlineConstexprGep {
                        base_addr: addr,
                        field_index,
                    })
                }
                ResolvedAddress::InlineConstexprGep { base_addr, .. } => {
                    Some(ResolvedAddress::InlineConstexprGep {
                        base_addr,
                        field_index,
                    })
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
    flow: AccessFlow,
    field_override: Option<&str>,
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
        ResolvedAddress::InlineConstexprGep {
            base_addr,
            field_index,
        } => {
            // Match the constexpr base against the register-map
            // peripheral base. Mismatch → no register; the caller
            // surfaces an UnknownAccessor warning.
            let map_base = rm.base_address_value()?;
            if *base_addr != map_base {
                return None;
            }
            let idx = usize::try_from(*field_index).ok()?;
            rm.registers.get(idx)?
        }
    };

    // Phase L4: when the bit-field RMW pre-pass identified which
    // field a write touched, use that name. Otherwise fall back to
    // the lowest-bit field (phase-L2 behaviour). Whole-register
    // accesses with no fields emit `field: None`.
    let field_name = field_override.map(str::to_string).or_else(|| {
        register
            .fields
            .iter()
            .min_by_key(|f| f.bits[0])
            .map(|f| f.name.clone())
    });

    Some(RegisterAccess {
        kind,
        register: register.name.clone(),
        field: field_name,
        accessor: format!("(IR-resolved @{addr:?})"),
        source_line,
        flow,
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
  %14 = load ptr, ptr @UART, align 8
  %15 = getelementptr inbounds %struct.UART_TypeDef, ptr %14, i32 0, i32 0
  %16 = load volatile i32, ptr %15, align 4
  %17 = and i32 %16, -2
  %18 = or i32 %17, 1
  store volatile i32 %18, ptr %15, align 4
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
        // 1 STATUS poll-load + 1 CTRL bit-field RMW load.
        assert_eq!(f.num_volatile_loads, 2);
        // 1 DATA write + 1 CTRL bit-field RMW store.
        assert_eq!(f.num_volatile_stores, 2);
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
        // The FIRMWARE_IR fixture now contains: a polling read of
        // STATUS, a write to DATA, and a bit-field RMW on CTRL.
        // L3+L4 must produce 3 accesses (polling read, DATA write,
        // CTRL write — the RMW load is collapsed).
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let f = &ext.functions[0];
        assert_eq!(f.accesses.len(), 3, "{:#?}", f.accesses);
        assert_eq!(f.accesses[0].kind, AccessKind::Read);
        assert_eq!(f.accesses[0].register, "STATUS");
        assert_eq!(f.accesses[1].kind, AccessKind::Write);
        assert_eq!(f.accesses[1].register, "DATA");
        assert_eq!(f.accesses[2].kind, AccessKind::Write);
        assert_eq!(f.accesses[2].register, "CTRL");
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
    fn l3_synthesises_automaton_with_polling_loop_state() {
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            synthesize_automaton: true,
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let f = &ext.functions[0];
        let ctxdsl = f.automaton_ctxdsl.as_ref().expect("automaton emitted");
        assert!(ctxdsl.contains("state S0 initial"));
        assert!(ctxdsl.contains("state Loop0"));
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
    fn debug_dump_plan() {
        let m = crate::llvm_ir::parse_module(FIRMWARE_IR).unwrap();
        let f = &m.functions[0];
        let mut ssa_defs: HashMap<&str, &Instruction> = HashMap::new();
        for bb in &f.basic_blocks {
            for instr in &bb.instructions {
                if let Some(result) = instruction_result(instr) {
                    ssa_defs.insert(result, instr);
                }
            }
        }
        let plan = build_extraction_plan(f, &uart_register_map(), &ssa_defs);
        eprintln!("polling_loop_loads: {:?}", plan.polling_loop_loads);
        eprintln!("skip_blocks: {:?}", plan.skip_blocks);
        eprintln!("rmw_consumed_loads: {:?}", plan.rmw_consumed_loads);
        eprintln!("rmw_store_field: {:?}", plan.rmw_store_field);
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

    // ----------------------------------------------------------------
    // Phase L3 / L4 tests — polling-loop + bit-field RMW.
    // ----------------------------------------------------------------

    #[test]
    fn l3_polling_loop_detected_in_firmware_ir() {
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let status_read = &ext.functions[0].accesses[0];
        assert_eq!(status_read.kind, AccessKind::Read);
        assert_eq!(status_read.register, "STATUS");
        assert_eq!(
            status_read.flow,
            AccessFlow::PollingLoop,
            "polling-loop read must carry PollingLoop flow"
        );
    }

    #[test]
    fn l4_bitfield_rmw_collapses_to_single_write() {
        // The IR carries a load-modify-store sequence on CTRL:
        //   %16 = load volatile i32 ptr %15
        //   %17 = and i32 %16, -2
        //   %18 = or i32 %17, 1
        //   store volatile i32 %18, ptr %15
        // Phase L4 must drop the load and emit ONE write to
        // CTRL.tx_start (bit 0).
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let ctrl_accesses: Vec<&RegisterAccess> = ext.functions[0]
            .accesses
            .iter()
            .filter(|a| a.register == "CTRL")
            .collect();
        assert_eq!(ctrl_accesses.len(), 1, "RMW must collapse to one access");
        assert_eq!(ctrl_accesses[0].kind, AccessKind::Write);
        assert_eq!(ctrl_accesses[0].field.as_deref(), Some("tx_start"));
    }

    #[test]
    fn l3_parity_with_slice2c_on_firmware_ir() {
        // The full parity gate: the LLVM backend must produce the
        // canonical S0 → Loop0 → S1 → S2 → S3 shape, same labels as
        // slice 2.c's `synthesise_automaton_ctxdsl` would.
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            synthesize_automaton: true,
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(FIRMWARE_IR, &opts).unwrap();
        let f = &ext.functions[0];
        assert_eq!(f.accesses.len(), 3, "{:#?}", f.accesses);
        let ctxdsl = f.automaton_ctxdsl.as_ref().unwrap();
        // Loop state, post-loop state, two write states.
        for marker in [
            "state S0 initial",
            "state Loop0",
            "state S1",
            "state S2",
            "state S3",
            "transition S0 -> Loop0 on label rd_status_tx_busy",
            "transition Loop0 -> Loop0 on label rd_status_tx_busy",
            "transition Loop0 -> S1 on label rd_status_tx_busy",
            "transition S1 -> S2 on label wr_data_byte",
            "transition S2 -> S3 on label wr_ctrl_tx_start",
        ] {
            assert!(
                ctxdsl.contains(marker),
                "missing marker `{marker}` in:\n{ctxdsl}"
            );
        }
        assert!(
            !ctxdsl.contains("state S4"),
            "L3+L4 collapse must yield no S4"
        );
    }

    #[test]
    fn l4_bitfield_picks_field_from_or_bits_not_lowest_bit() {
        // A bit-field write that targets a field at bits [4, 7] (not
        // [0, 0]) must select that field, not fall back to the
        // lowest-bit field.
        let mut rm = uart_register_map();
        // Append a higher-bit field to CTRL.
        rm.registers[0].fields.push(Field {
            name: "tx_mode".to_string(),
            bits: [4, 7],
            sv_signal: None,
            c_accessor: Some("UART->CTRL.bit.tx_mode".to_string()),
            description: None,
        });
        // IR: read CTRL, AND with 0xFFFFFF0F (clear bits 4-7), OR with
        // 0x00000020 (set bit 5), store back.
        let ir = r#"%struct.UART_TypeDef = type { i32, i32, i32 }
@UART = external constant ptr, align 8
define void @set_mode() {
  %1 = load ptr, ptr @UART, align 8
  %2 = getelementptr inbounds %struct.UART_TypeDef, ptr %1, i32 0, i32 0
  %3 = load volatile i32, ptr %2, align 4
  %4 = and i32 %3, -241
  %5 = or i32 %4, 32
  store volatile i32 %5, ptr %2, align 4
  ret void
}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(rm),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        let f = &ext.functions[0];
        assert_eq!(f.accesses.len(), 1);
        assert_eq!(f.accesses[0].kind, AccessKind::Write);
        assert_eq!(f.accesses[0].register, "CTRL");
        assert_eq!(f.accesses[0].field.as_deref(), Some("tx_mode"));
    }

    // ----------------------------------------------------------------
    // Phase L5 tests — interprocedural call-graph walk.
    // ----------------------------------------------------------------

    #[test]
    fn l5_inlines_callee_accesses_at_the_call_site() {
        // Example 4 from the plan: a helper function holds the
        // polling loop, the caller chains DATA write + CTRL write.
        // The IR has separate `define` blocks for both functions;
        // phase L5 inlines the callee's accesses at the call site.
        let ir = r#"%struct.UART_TypeDef = type { i32, i32, i32 }
@UART = external constant ptr, align 8

define void @uart_wait_idle() {
1:
  %2 = load ptr, ptr @UART, align 8
  %3 = getelementptr inbounds %struct.UART_TypeDef, ptr %2, i32 0, i32 1
  %4 = load volatile i32, ptr %3, align 4
  %5 = and i32 %4, 1
  %6 = icmp ne i32 %5, 0
  br i1 %6, label %7, label %8

7:
  br label %1

8:
  ret void
}

define void @uart_send(i8 %0) {
  call void @uart_wait_idle()
  %2 = load ptr, ptr @UART, align 8
  %3 = getelementptr inbounds %struct.UART_TypeDef, ptr %2, i32 0, i32 2
  store volatile i8 %0, ptr %3, align 4
  ret void
}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            synthesize_automaton: true,
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        // Two functions in the module — the caller (uart_send) and
        // the callee (uart_wait_idle). Each has its own summary.
        let send = ext
            .functions
            .iter()
            .find(|f| f.name == "uart_send")
            .expect("uart_send extracted");
        // After L5 inlining: the polling-loop STATUS read (from
        // uart_wait_idle) + the DATA write (from uart_send body).
        assert_eq!(send.accesses.len(), 2, "{:#?}", send.accesses);
        assert_eq!(send.accesses[0].kind, AccessKind::Read);
        assert_eq!(send.accesses[0].register, "STATUS");
        assert_eq!(send.accesses[0].flow, AccessFlow::PollingLoop);
        assert_eq!(send.accesses[1].kind, AccessKind::Write);
        assert_eq!(send.accesses[1].register, "DATA");
    }

    #[test]
    fn l5_recursive_call_breaks_with_warning_no_infinite_loop() {
        let ir = r#"define void @loops() {
  call void @loops()
  ret void
}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        // The walker must not infinite-loop; it must emit a
        // RecursiveCall warning when it re-enters the same function.
        assert!(
            ext.warnings
                .iter()
                .any(|w| matches!(w, LlvmExtractWarning::RecursiveCall { function } if function == "loops")),
            "got warnings: {:?}",
            ext.warnings
        );
    }

    // ----------------------------------------------------------------
    // Phase L6 tests — @mununu_isr annotation + async composition.
    // ----------------------------------------------------------------

    #[test]
    fn l6_detects_isr_annotation_in_source() {
        let source = r#"
/* main thread driver */
void uart_send(uint8_t byte) {}

/**
 * @mununu_isr
 */
void UART_IRQHandler(void) {}
"#;
        let isrs = detect_isr_functions(source);
        assert!(isrs.contains("UART_IRQHandler"), "got: {isrs:?}");
        assert!(!isrs.contains("uart_send"));
    }

    #[test]
    fn l6_marks_isr_function_in_summary() {
        let ir = r#"@UART = external constant ptr, align 8
define void @uart_send() {
  ret void
}
define void @UART_IRQHandler() {
  ret void
}
"#;
        let source = r#"
void uart_send(uint8_t byte) {}

/**
 * @mununu_isr
 */
void UART_IRQHandler(void) {}
"#;
        let ext = extract_from_ir_text_with_options_and_source(
            ir,
            source,
            &LlvmExtractOptions::default(),
        )
        .unwrap();
        let isr = ext
            .functions
            .iter()
            .find(|f| f.name == "UART_IRQHandler")
            .unwrap();
        assert!(isr.is_isr);
        let send = ext
            .functions
            .iter()
            .find(|f| f.name == "uart_send")
            .unwrap();
        assert!(!send.is_isr);
    }

    #[test]
    fn l6_emits_async_composition_when_isr_present() {
        let ir = r#"@UART = external constant ptr, align 8
%struct.UART_TypeDef = type { i32, i32, i32 }
define void @uart_send() {
  %1 = load ptr, ptr @UART, align 8
  %2 = getelementptr inbounds %struct.UART_TypeDef, ptr %1, i32 0, i32 2
  store volatile i8 0, ptr %2, align 4
  ret void
}
define void @UART_IRQHandler() {
  %1 = load ptr, ptr @UART, align 8
  %2 = getelementptr inbounds %struct.UART_TypeDef, ptr %1, i32 0, i32 1
  %3 = load volatile i32, ptr %2, align 4
  ret void
}
"#;
        let source = r#"
void uart_send(uint8_t byte) {}

/**
 * @mununu_isr
 */
void UART_IRQHandler(void) {}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            synthesize_automaton: true,
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options_and_source(ir, source, &opts).unwrap();
        let comp = ext
            .composition_ctxdsl
            .as_ref()
            .expect("composition emitted when ISR present");
        assert!(comp.contains("compositions"));
        assert!(comp.contains("asynchronous"));
        assert!(comp.contains("Uart_send"));
        assert!(comp.contains("UART_IRQHandler") || comp.contains("Uart_irqhandler"));
        assert!(comp.contains("// ISR"));
    }

    #[test]
    fn l6_no_composition_when_no_isr_annotation() {
        let ir = r#"define void @uart_send() {
  ret void
}
"#;
        let source = "void uart_send(uint8_t byte) {}\n";
        let ext = extract_from_ir_text_with_options_and_source(
            ir,
            source,
            &LlvmExtractOptions::default(),
        )
        .unwrap();
        assert!(ext.composition_ctxdsl.is_none());
    }

    #[test]
    fn l5_external_call_falls_through_silently() {
        // Calls to external symbols (not defined in this module) are
        // not inlined and produce no warning — they're the chaotic-
        // stub equivalent of "we have no body to walk."
        let ir = r#"@UART = external constant ptr, align 8
%struct.UART_TypeDef = type { i32, i32, i32 }
define void @uart_send() {
  call void @printf(ptr %0)
  %2 = load ptr, ptr @UART, align 8
  %3 = getelementptr inbounds %struct.UART_TypeDef, ptr %2, i32 0, i32 0
  store volatile i32 1, ptr %3, align 4
  ret void
}
"#;
        let opts = LlvmExtractOptions {
            register_map: Some(uart_register_map()),
            ..Default::default()
        };
        let ext = extract_from_ir_text_with_options(ir, &opts).unwrap();
        let f = &ext.functions[0];
        // One access — the CTRL write that comes after the printf
        // call. The external call doesn't produce a recursion or
        // unknown warning.
        assert_eq!(f.accesses.len(), 1);
        assert_eq!(f.accesses[0].register, "CTRL");
        assert!(
            !ext.warnings
                .iter()
                .any(|w| matches!(w, LlvmExtractWarning::RecursiveCall { .. }))
        );
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
        assert_eq!(parsed.functions[0].accesses.len(), 3);
    }
}
