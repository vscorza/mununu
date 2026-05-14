//! Line-based regex parser for LLVM IR text.
//!
//! Handles the textual `.ll` output of `clang -emit-llvm -S` and
//! `rustc --emit=llvm-ir`. The parser is intentionally
//! conservative: it recognises the instruction shapes mununu's two
//! consumers (C codesign extraction, Rust source extraction) need
//! today, and falls through to [`Instruction::Other`] for anything
//! else.
//!
//! ## Soundness posture
//!
//! Parsing is **best-effort and forward-only**: each line is matched
//! against a regex; the first matching regex wins. Mis-parses become
//! [`Instruction::Other`]. The parser does *not* validate that the
//! IR is well-formed — it trusts clang/rustc to emit correct IR.
//!
//! ## What is *not* in scope
//!
//! - Metadata nodes (`!N = …`) — skipped silently. Phase L3+ may
//!   need `!llvm.loop` metadata for loop-detection; the parser will
//!   gain that targeted support when it's needed.
//! - Inline assembly (`call void asm "…"`).
//! - Vector / aggregate constants beyond simple `i32 N` indices.
//! - Floating-point operations.
//! - Most attribute groups (`attributes #N = { … }`) — skipped.

use crate::llvm_ir::types::*;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Errors raised by [`parse_module`].
#[derive(Debug, Clone)]
pub enum ParseError {
    /// The input was empty.
    EmptyInput,
    /// A `define` line was malformed.
    MalformedDefine(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyInput => write!(f, "empty IR input"),
            ParseError::MalformedDefine(line) => {
                write!(f, "could not parse `define` line: {line}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

struct Regexes {
    source_filename: Regex,
    data_layout: Regex,
    target_triple: Regex,
    struct_type: Regex,
    global: Regex,
    define: Regex,
    bb_label: Regex,
    preds_comment: Regex,

    // Instructions
    alloca: Regex,
    load: Regex,
    store: Regex,
    gep: Regex,
    inttoptr: Regex,
    ptrtoint: Regex,
    bitcast: Regex,
    trunc: Regex,
    zext: Regex,
    sext: Regex,
    binop: Regex,
    icmp: Regex,
    call: Regex,
    phi: Regex,

    // Terminators
    ret: Regex,
    br_cond: Regex,
    br: Regex,
}

fn regexes() -> &'static Regexes {
    static R: OnceLock<Regexes> = OnceLock::new();
    R.get_or_init(|| Regexes {
        source_filename: Regex::new(r#"^\s*source_filename\s*=\s*"(.+)""#).unwrap(),
        data_layout: Regex::new(r#"^\s*target\s+datalayout\s*=\s*"(.+)""#).unwrap(),
        target_triple: Regex::new(r#"^\s*target\s+triple\s*=\s*"(.+)""#).unwrap(),
        struct_type: Regex::new(r#"^\s*%([\w."]+?)\s*=\s*type\s*\{(.*)\}\s*$"#).unwrap(),
        global: Regex::new(
            r"^\s*@([\w.]+)\s*=\s*((?:[\w_]+\s+)*)(constant|global)\s+(\S+)(?:.*?align\s+(\d+))?",
        )
        .unwrap(),
        define: Regex::new(r"^\s*define\s+(?:[^@]*?\s)?(\S+)\s+@([\w.]+)\((.*?)\)").unwrap(),
        bb_label: Regex::new(r"^(\w+):\s*(?:;.*)?$").unwrap(),
        preds_comment: Regex::new(r";\s*preds\s*=\s*(.+?)\s*$").unwrap(),

        alloca: Regex::new(r"^\s*%(\w+)\s*=\s*alloca\s+([^,]+?)(?:,\s*align\s+(\d+))?\s*$")
            .unwrap(),
        load: Regex::new(
            r"^\s*%(\w+)\s*=\s*load\s+(volatile\s+)?([^,]+?),\s*ptr\s+(\S+?)(?:,\s*align\s+(\d+))?\s*$",
        )
        .unwrap(),
        store: Regex::new(
            r"^\s*store\s+(volatile\s+)?(\S+)\s+(\S+?),\s*ptr\s+(\S+?)(?:,\s*align\s+(\d+))?\s*$",
        )
        .unwrap(),
        gep: Regex::new(
            r"^\s*%(\w+)\s*=\s*getelementptr\s+(inbounds\s+)?(\S+?),\s*ptr\s+([%@][\w.]+)(.+)\s*$",
        )
        .unwrap(),
        inttoptr: Regex::new(r"^\s*%(\w+)\s*=\s*inttoptr\s+\S+\s+(\S+)\s+to\s+ptr\s*$").unwrap(),
        ptrtoint: Regex::new(r"^\s*%(\w+)\s*=\s*ptrtoint\s+ptr\s+(\S+)\s+to\s+\S+\s*$").unwrap(),
        bitcast: Regex::new(r"^\s*%(\w+)\s*=\s*bitcast\s+\S+\s+(\S+)\s+to\s+\S+\s*$").unwrap(),
        trunc: Regex::new(r"^\s*%(\w+)\s*=\s*trunc\s+\S+\s+(\S+)\s+to\s+\S+\s*$").unwrap(),
        zext: Regex::new(r"^\s*%(\w+)\s*=\s*zext\s+\S+\s+(\S+)\s+to\s+\S+\s*$").unwrap(),
        sext: Regex::new(r"^\s*%(\w+)\s*=\s*sext\s+\S+\s+(\S+)\s+to\s+\S+\s*$").unwrap(),
        binop: Regex::new(
            r"^\s*%(\w+)\s*=\s*(add|sub|mul|udiv|sdiv|urem|srem|shl|lshr|ashr|and|or|xor)\s+(?:nuw\s+|nsw\s+|exact\s+)*(\S+)\s+(\S+?),\s+(\S+?)\s*$",
        )
        .unwrap(),
        icmp: Regex::new(
            r"^\s*%(\w+)\s*=\s*icmp\s+(eq|ne|ugt|uge|ult|ule|sgt|sge|slt|sle)\s+(\S+)\s+(\S+?),\s+(\S+?)\s*$",
        )
        .unwrap(),
        // call covers both `%r = call ...` and `call ...`
        call: Regex::new(
            r"^\s*(?:%(\w+)\s*=\s*)?(?:tail\s+|musttail\s+|notail\s+)?call\s+(?:\S+\s+)?(\S+?)\s+@([\w.]+)\((.*?)\)",
        )
        .unwrap(),
        phi: Regex::new(r"^\s*%(\w+)\s*=\s*phi\s+(\S+)\s+(.+)\s*$").unwrap(),

        ret: Regex::new(r"^\s*ret\s+(?:(void)|(\S+)\s+(.+?))\s*$").unwrap(),
        br_cond: Regex::new(
            r"^\s*br\s+i1\s+(\S+?),\s+label\s+%(\w+),\s+label\s+%(\w+)\s*$",
        )
        .unwrap(),
        br: Regex::new(r"^\s*br\s+label\s+%(\w+)").unwrap(),
    })
}

/// Parse an LLVM IR text module into structured form.
pub fn parse_module(ir_text: &str) -> Result<Module, ParseError> {
    if ir_text.trim().is_empty() {
        return Err(ParseError::EmptyInput);
    }
    let r = regexes();
    let mut module = Module::default();

    // Per-function in-progress state.
    let mut current_fn: Option<Function> = None;
    let mut current_bb: Option<BasicBlock> = None;

    for raw_line in ir_text.lines() {
        let line = strip_trailing_metadata(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }

        // Top-level: module header lines + struct/global declarations.
        if current_fn.is_none() {
            if let Some(caps) = r.source_filename.captures(line) {
                module.source_filename = Some(caps[1].to_string());
                continue;
            }
            if let Some(caps) = r.data_layout.captures(line) {
                module.data_layout = Some(caps[1].to_string());
                continue;
            }
            if let Some(caps) = r.target_triple.captures(line) {
                module.target_triple = Some(caps[1].to_string());
                continue;
            }
            if let Some(caps) = r.struct_type.captures(line) {
                let name = format!("%{}", &caps[1]);
                let fields: Vec<String> = caps[2]
                    .split(',')
                    .map(|f| f.trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
                let st = StructType {
                    name: name.clone(),
                    fields,
                };
                module.struct_types.insert(name, st);
                continue;
            }
            if let Some(caps) = r.global.captures(line) {
                let linkage: Vec<String> = caps
                    .get(2)
                    .map(|m| {
                        m.as_str()
                            .split_whitespace()
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let kind = match &caps[3] {
                    "constant" => GlobalKind::Constant,
                    _ => GlobalKind::Global,
                };
                let ty = caps[4].trim_end_matches(',').to_string();
                let align = caps.get(5).and_then(|m| m.as_str().parse().ok());
                module.globals.push(Global {
                    name: caps[1].to_string(),
                    linkage,
                    kind,
                    ty,
                    align,
                });
                continue;
            }
            if let Some(caps) = r.define.captures(line) {
                let return_type = caps[1].to_string();
                let name = caps[2].to_string();
                let params = parse_parameters(&caps[3]);
                current_fn = Some(Function {
                    name,
                    return_type,
                    parameters: params,
                    attributes: Vec::new(),
                    basic_blocks: Vec::new(),
                });
                // Start the implicit entry block — clang doesn't
                // emit a label for it.
                current_bb = Some(BasicBlock {
                    label: "entry".to_string(),
                    predecessors: Vec::new(),
                    instructions: Vec::new(),
                    terminator: Terminator::Unreachable,
                });
                continue;
            }
            // Unrecognised top-level line — skip silently. Could be
            // attribute group, metadata, !llvm.module.flags, etc.
            continue;
        }

        // Inside a function. Check for end-of-function brace.
        if trimmed == "}" {
            if let Some(bb) = current_bb.take()
                && let Some(func) = current_fn.as_mut()
            {
                func.basic_blocks.push(bb);
            }
            if let Some(func) = current_fn.take() {
                module.functions.push(func);
            }
            continue;
        }

        // Basic-block label line.
        if let Some(caps) = r.bb_label.captures(trimmed) {
            // Flush the current basic block before starting a new one.
            if let Some(bb) = current_bb.take()
                && let Some(func) = current_fn.as_mut()
            {
                func.basic_blocks.push(bb);
            }
            let preds = r
                .preds_comment
                .captures(raw_line)
                .map(|c| {
                    c[1].split(',')
                        .map(|p| p.trim().trim_start_matches('%').to_string())
                        .collect()
                })
                .unwrap_or_default();
            current_bb = Some(BasicBlock {
                label: caps[1].to_string(),
                predecessors: preds,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable,
            });
            continue;
        }

        // An instruction line inside the current basic block.
        let Some(bb) = current_bb.as_mut() else {
            continue;
        };
        match parse_instruction_or_terminator(line, r) {
            ParsedLine::Instruction(instr) => bb.instructions.push(instr),
            ParsedLine::Terminator(term) => bb.terminator = term,
        }
    }

    // Defensive: if the file ended without a `}` close brace, flush
    // whatever we accumulated.
    if let Some(bb) = current_bb.take()
        && let Some(mut func) = current_fn.take()
    {
        func.basic_blocks.push(bb);
        module.functions.push(func);
    }

    Ok(module)
}

enum ParsedLine {
    Instruction(Instruction),
    Terminator(Terminator),
}

/// Try every regex in turn; the first match wins. Anything unknown
/// becomes `Instruction::Other(raw_line)`.
fn parse_instruction_or_terminator(line: &str, r: &Regexes) -> ParsedLine {
    // Terminators first (`br` and `ret` are unambiguous).
    if let Some(caps) = r.br_cond.captures(line) {
        return ParsedLine::Terminator(Terminator::BrCond {
            cond: ValueOperand::parse(&caps[1]),
            if_true: caps[2].to_string(),
            if_false: caps[3].to_string(),
        });
    }
    if let Some(caps) = r.br.captures(line) {
        return ParsedLine::Terminator(Terminator::Br {
            target: caps[1].to_string(),
        });
    }
    if let Some(caps) = r.ret.captures(line) {
        let value = if caps.get(1).is_some() {
            None
        } else {
            caps.get(3).map(|m| ValueOperand::parse(m.as_str()))
        };
        return ParsedLine::Terminator(Terminator::Ret { value });
    }
    if line.trim() == "unreachable" {
        return ParsedLine::Terminator(Terminator::Unreachable);
    }

    // Instructions.
    if let Some(caps) = r.alloca.captures(line) {
        return ParsedLine::Instruction(Instruction::Alloca {
            result: caps[1].to_string(),
            ty: caps[2].trim().to_string(),
            align: caps.get(3).and_then(|m| m.as_str().parse().ok()),
        });
    }
    if let Some(caps) = r.load.captures(line) {
        return ParsedLine::Instruction(Instruction::Load {
            result: caps[1].to_string(),
            volatile: caps.get(2).is_some(),
            ty: caps[3].trim().to_string(),
            source: parse_pointer_operand(&caps[4]),
            align: caps.get(5).and_then(|m| m.as_str().parse().ok()),
        });
    }
    if let Some(caps) = r.store.captures(line) {
        return ParsedLine::Instruction(Instruction::Store {
            volatile: caps.get(1).is_some(),
            ty: caps[2].to_string(),
            value: ValueOperand::parse(&caps[3]),
            dest: parse_pointer_operand(&caps[4]),
            align: caps.get(5).and_then(|m| m.as_str().parse().ok()),
        });
    }
    if let Some(caps) = r.gep.captures(line) {
        let indices_text = caps[5].trim_start_matches(',').trim();
        return ParsedLine::Instruction(Instruction::Gep {
            result: caps[1].to_string(),
            in_bounds: caps.get(2).is_some(),
            ty: caps[3].to_string(),
            base: parse_pointer_operand(&caps[4]),
            indices: parse_gep_indices(indices_text),
        });
    }
    if let Some(caps) = r.inttoptr.captures(line) {
        let raw = caps[2].trim();
        let value: u64 = parse_int_literal(raw).unwrap_or(0);
        return ParsedLine::Instruction(Instruction::IntToPtr {
            result: caps[1].to_string(),
            value,
        });
    }
    if let Some(caps) = r.ptrtoint.captures(line) {
        return ParsedLine::Instruction(Instruction::PtrToInt {
            result: caps[1].to_string(),
            source: caps[2].trim_start_matches('%').to_string(),
        });
    }
    if let Some(caps) = r.bitcast.captures(line) {
        return ParsedLine::Instruction(Instruction::Bitcast {
            result: caps[1].to_string(),
            source: caps[2].trim_start_matches('%').to_string(),
        });
    }
    if let Some(caps) = r.trunc.captures(line) {
        return ParsedLine::Instruction(Instruction::Trunc {
            result: caps[1].to_string(),
            source: caps[2].trim_start_matches('%').to_string(),
        });
    }
    if let Some(caps) = r.zext.captures(line) {
        return ParsedLine::Instruction(Instruction::ZExt {
            result: caps[1].to_string(),
            source: caps[2].trim_start_matches('%').to_string(),
        });
    }
    if let Some(caps) = r.sext.captures(line) {
        return ParsedLine::Instruction(Instruction::SExt {
            result: caps[1].to_string(),
            source: caps[2].trim_start_matches('%').to_string(),
        });
    }
    if let Some(caps) = r.binop.captures(line) {
        return ParsedLine::Instruction(Instruction::BinaryOp {
            result: caps[1].to_string(),
            op: BinaryOp::parse(&caps[2]).expect("regex restricts to known ops"),
            ty: caps[3].to_string(),
            a: ValueOperand::parse(&caps[4]),
            b: ValueOperand::parse(&caps[5]),
        });
    }
    if let Some(caps) = r.icmp.captures(line) {
        return ParsedLine::Instruction(Instruction::Icmp {
            result: caps[1].to_string(),
            pred: IcmpPred::parse(&caps[2]).expect("regex restricts to known preds"),
            ty: caps[3].to_string(),
            a: ValueOperand::parse(&caps[4]),
            b: ValueOperand::parse(&caps[5]),
        });
    }
    if let Some(caps) = r.call.captures(line) {
        let args: Vec<String> = caps[4]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return ParsedLine::Instruction(Instruction::Call {
            result: caps.get(1).map(|m| m.as_str().to_string()),
            return_ty: caps[2].to_string(),
            callee: caps[3].to_string(),
            args,
        });
    }
    if let Some(caps) = r.phi.captures(line) {
        let incoming = parse_phi_incoming(&caps[3]);
        return ParsedLine::Instruction(Instruction::Phi {
            result: caps[1].to_string(),
            ty: caps[2].to_string(),
            incoming,
        });
    }

    ParsedLine::Instruction(Instruction::Other(line.trim().to_string()))
}

fn parse_parameters(text: &str) -> Vec<FunctionParameter> {
    text.split(',')
        .filter_map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Each parameter looks like `<type> [<attrs>...] [%name]`.
            // We scan for the first whitespace-separated `%name`
            // token and treat everything before it as the type.
            let mut name: Option<String> = None;
            let mut ty_parts: Vec<&str> = Vec::new();
            for token in trimmed.split_whitespace() {
                if let Some(rest) = token.strip_prefix('%') {
                    name = Some(rest.to_string());
                    break;
                }
                // Skip known attribute tokens that pad the type list.
                if !matches!(
                    token,
                    "noundef" | "zeroext" | "signext" | "readonly" | "nocapture" | "nonnull"
                ) {
                    ty_parts.push(token);
                }
            }
            Some(FunctionParameter {
                ty: ty_parts.join(" "),
                name,
            })
        })
        .collect()
}

fn parse_gep_indices(text: &str) -> Vec<GepIndex> {
    let mut out = Vec::new();
    // Indices come as `i32 0, i32 1` or `i64 %5, i32 1`. Pair tokens
    // up as (type, value).
    let parts: Vec<&str> = text
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    for part in parts {
        let value_token = part.split_whitespace().nth(1).unwrap_or("");
        if let Some(rest) = value_token.strip_prefix('%') {
            out.push(GepIndex::Dynamic(rest.to_string()));
        } else if let Ok(n) = value_token.parse::<i64>() {
            out.push(GepIndex::Const(n));
        } else if let Some(hex) = value_token.strip_prefix("0x")
            && let Ok(n) = i64::from_str_radix(hex, 16)
        {
            out.push(GepIndex::Const(n));
        }
    }
    out
}

fn parse_pointer_operand(text: &str) -> PointerOperand {
    let trimmed = text.trim().trim_end_matches(',');
    if let Some(rest) = trimmed.strip_prefix('@') {
        return PointerOperand::Global(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix('%') {
        return PointerOperand::Ssa(rest.to_string());
    }
    // Inline `inttoptr (i64 0x40010000 to ptr)` literal.
    if let Some(inner) = trimmed
        .strip_prefix("inttoptr")
        .and_then(|s| s.trim().strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'))
    {
        // inner is e.g. "i64 0x40010000 to ptr"
        let mut tokens = inner.split_whitespace();
        let _ty = tokens.next();
        if let Some(value_tok) = tokens.next()
            && let Some(v) = parse_int_literal(value_tok)
        {
            return PointerOperand::InlineConstAddr(v);
        }
    }
    PointerOperand::Ssa(trimmed.to_string())
}

fn parse_int_literal(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches(',');
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    if let Some(suffix) = s.strip_suffix('u') {
        return suffix.parse().ok();
    }
    s.parse().ok()
}

fn parse_phi_incoming(text: &str) -> Vec<(ValueOperand, String)> {
    // Phi entries look like `[ %v0, %bb0 ], [ %v1, %bb1 ]`.
    let mut out = Vec::new();
    for entry in text.split("],") {
        let trimmed = entry
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim();
        let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
        if parts.len() == 2 {
            out.push((
                ValueOperand::parse(parts[0]),
                parts[1].trim_start_matches('%').to_string(),
            ));
        }
    }
    out
}

/// Drop trailing `!metadata, !N, !something` annotations from a line
/// so the instruction regexes don't have to account for them.
fn strip_trailing_metadata(line: &str) -> &str {
    // Find ", !" — that marks the start of a metadata-annotation
    // suffix on an instruction (e.g. `br label %3, !llvm.loop !6`).
    if let Some(idx) = line.find(", !") {
        return &line[..idx];
    }
    line
}

// Silence an unused-import warning once `BTreeMap` is only used in
// types.rs. (Defensive — currently the parser uses it directly.)
#[allow(dead_code)]
fn _btreemap_dependency_anchor() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The IR from `clang -O0 -emit-llvm -S firmware.c` for the
    /// codesign_uart example. Pasted as a `&str` so the tests don't
    /// require clang to be available.
    const FIRMWARE_EXTERN_IR: &str = r#"; ModuleID = 'firmware.c'
source_filename = "firmware.c"
target datalayout = "e-m:o"
target triple = "x86_64-apple-macosx26.0.0"

%struct.UART_TypeDef = type { %union.anon, %union.anon.0, %union.anon.2 }
%union.anon = type { i32 }
%union.anon.0 = type { i32 }
%union.anon.2 = type { i32 }

@UART = external constant ptr, align 8

define void @uart_send(i8 noundef zeroext %0) #0 {
  %2 = alloca i8, align 1
  store i8 %0, ptr %2, align 1
  br label %3

3:                                                ; preds = %9, %1
  %4 = load ptr, ptr @UART, align 8
  %5 = getelementptr inbounds %struct.UART_TypeDef, ptr %4, i32 0, i32 1
  %6 = load volatile i32, ptr %5, align 4
  %7 = and i32 %6, 1
  %8 = icmp ne i32 %7, 0
  br i1 %8, label %9, label %10

9:                                                ; preds = %3
  br label %3, !llvm.loop !6

10:                                               ; preds = %3
  %11 = load i8, ptr %2, align 1
  %12 = load ptr, ptr @UART, align 8
  %13 = getelementptr inbounds %struct.UART_TypeDef, ptr %12, i32 0, i32 2
  store volatile i8 %11, ptr %13, align 4
  ret void
}
"#;

    #[test]
    fn parses_module_header() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        assert_eq!(m.source_filename.as_deref(), Some("firmware.c"));
        assert!(m.data_layout.is_some());
        assert!(m.target_triple.as_deref().unwrap().contains("x86_64"));
    }

    #[test]
    fn parses_struct_types() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        assert!(m.struct_types.contains_key("%struct.UART_TypeDef"));
        let st = &m.struct_types["%struct.UART_TypeDef"];
        assert_eq!(st.fields.len(), 3);
    }

    #[test]
    fn parses_external_global() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let uart = m.globals.iter().find(|g| g.name == "UART").expect("UART");
        assert_eq!(uart.kind, GlobalKind::Constant);
        assert!(uart.linkage.contains(&"external".to_string()));
        assert_eq!(uart.align, Some(8));
    }

    #[test]
    fn parses_function_signature() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let f = m.functions.iter().find(|f| f.name == "uart_send").unwrap();
        assert_eq!(f.return_type, "void");
        assert_eq!(f.parameters.len(), 1);
        assert_eq!(f.parameters[0].ty, "i8");
        assert_eq!(f.parameters[0].name.as_deref(), Some("0"));
    }

    #[test]
    fn parses_basic_blocks_with_predecessors() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let f = m.functions.iter().find(|f| f.name == "uart_send").unwrap();
        // entry + 3 explicit labels
        assert_eq!(f.basic_blocks.len(), 4, "{:#?}", f.basic_blocks);
        assert_eq!(f.basic_blocks[0].label, "entry");
        let bb3 = f.basic_blocks.iter().find(|b| b.label == "3").unwrap();
        assert!(bb3.predecessors.contains(&"9".to_string()));
        assert!(bb3.predecessors.contains(&"1".to_string()));
    }

    #[test]
    fn parses_alloca_store_branch_in_entry_block() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let f = m.functions.iter().find(|f| f.name == "uart_send").unwrap();
        let entry = &f.basic_blocks[0];
        assert!(matches!(entry.instructions[0], Instruction::Alloca { .. }));
        assert!(matches!(entry.instructions[1], Instruction::Store { .. }));
        assert!(matches!(entry.terminator, Terminator::Br { ref target } if target == "3"));
    }

    #[test]
    fn parses_volatile_load_and_gep_in_polling_block() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let f = m.functions.iter().find(|f| f.name == "uart_send").unwrap();
        let bb3 = f.basic_blocks.iter().find(|b| b.label == "3").unwrap();
        // %4 = load ptr, ptr @UART
        let load_uart = bb3
            .instructions
            .iter()
            .find_map(|i| match i {
                Instruction::Load { result, source, .. } if result == "4" => Some(source.clone()),
                _ => None,
            })
            .expect("found load of @UART");
        assert_eq!(load_uart, PointerOperand::Global("UART".into()));
        // %5 = getelementptr ... ptr %4, i32 0, i32 1
        // Crucially, the base must be Ssa("4") — not the empty string
        // a lazy regex would yield.
        let (gep_base, gep_indices) = bb3
            .instructions
            .iter()
            .find_map(|i| match i {
                Instruction::Gep {
                    result,
                    base,
                    indices,
                    ..
                } if result == "5" => Some((base.clone(), indices.clone())),
                _ => None,
            })
            .expect("found gep producing %5");
        assert_eq!(gep_base, PointerOperand::Ssa("4".into()), "GEP base");
        assert_eq!(
            gep_indices,
            vec![GepIndex::Const(0), GepIndex::Const(1)],
            "field-index-1 GEP"
        );
        // %6 = load volatile i32, ptr %5
        let vol_load = bb3.instructions.iter().any(|i| {
            matches!(i, Instruction::Load { result, volatile, source, .. }
                if result == "6" && *volatile && source == &PointerOperand::Ssa("5".into()))
        });
        assert!(vol_load, "expected volatile load on %5");
    }

    #[test]
    fn parses_polling_block_terminator_as_conditional_branch() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let f = m.functions.iter().find(|f| f.name == "uart_send").unwrap();
        let bb3 = f.basic_blocks.iter().find(|b| b.label == "3").unwrap();
        assert!(matches!(
            &bb3.terminator,
            Terminator::BrCond { if_true, if_false, .. } if if_true == "9" && if_false == "10"
        ));
    }

    #[test]
    fn parses_volatile_store_in_post_loop_block() {
        let m = parse_module(FIRMWARE_EXTERN_IR).unwrap();
        let f = m.functions.iter().find(|f| f.name == "uart_send").unwrap();
        let bb10 = f.basic_blocks.iter().find(|b| b.label == "10").unwrap();
        let store = bb10.instructions.iter().any(|i| {
            matches!(
                i,
                Instruction::Store {
                    volatile: true,
                    dest: PointerOperand::Ssa(s),
                    ..
                } if s == "13"
            )
        });
        assert!(store, "volatile store to %13 (DATA.byte)");
    }

    #[test]
    fn parses_inttoptr_register_access() {
        // Synthetic: `*(volatile uint32_t*)0x40023800 |= (1<<24)`
        // generates an IR with an `inttoptr` either as an instruction
        // or as an inline operand.
        let ir = r#"define void @enable_pll() {
  %1 = inttoptr i64 1073887232 to ptr
  %2 = load volatile i32, ptr %1, align 4
  %3 = or i32 %2, 16777216
  store volatile i32 %3, ptr %1, align 4
  ret void
}
"#;
        let m = parse_module(ir).unwrap();
        let f = &m.functions[0];
        let bb = &f.basic_blocks[0];
        assert!(matches!(
            bb.instructions[0],
            Instruction::IntToPtr {
                value: 1073887232,
                ..
            }
        ));
        // Sanity-check the load chains through the inttoptr result.
        assert!(matches!(
            bb.instructions[1],
            Instruction::Load { volatile: true, .. }
        ));
        assert!(matches!(bb.terminator, Terminator::Ret { value: None }));
    }

    #[test]
    fn errors_on_empty_input() {
        assert!(matches!(
            parse_module("").unwrap_err(),
            ParseError::EmptyInput
        ));
        assert!(matches!(
            parse_module("   \n\n").unwrap_err(),
            ParseError::EmptyInput
        ));
    }

    #[test]
    fn unknown_instructions_survive_as_other() {
        let ir = r#"define void @f() {
  %1 = some-weird-instruction i32 0
  ret void
}
"#;
        let m = parse_module(ir).unwrap();
        let bb = &m.functions[0].basic_blocks[0];
        assert!(matches!(bb.instructions[0], Instruction::Other(_)));
    }

    #[test]
    fn metadata_suffix_is_stripped_before_terminator_parse() {
        let ir = r#"define void @f() {
  br label %2, !llvm.loop !1

2:
  ret void
}
"#;
        let m = parse_module(ir).unwrap();
        let entry = &m.functions[0].basic_blocks[0];
        assert!(matches!(entry.terminator, Terminator::Br { ref target } if target == "2"));
    }
}
