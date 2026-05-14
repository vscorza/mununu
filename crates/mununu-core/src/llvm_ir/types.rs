//! LLVM IR data types — the structured form a `.ll` file parses into.
//!
//! Designed so that the shapes a C codesign extractor and a Rust
//! source extractor both need are represented in the same enum. The
//! types are deliberately conservative: we expose only the fields
//! either consumer reads today. Adding a new instruction or operand
//! shape is a focused enum-variant addition.
//!
//! Soundness posture: parsing is **best-effort**. Any line the parser
//! does not recognise survives as [`Instruction::Other`], preserving
//! the raw text for diagnostics. Consumers downstream must be
//! resilient to that variant — they should not silently treat it as
//! a no-op.

use std::collections::BTreeMap;

/// A parsed LLVM IR module.
#[derive(Debug, Clone, Default)]
pub struct Module {
    /// `source_filename = "…"` line if present.
    pub source_filename: Option<String>,
    /// `target datalayout = "…"` if present.
    pub data_layout: Option<String>,
    /// `target triple = "…"` if present.
    pub target_triple: Option<String>,
    /// Named struct / union types: `%struct.X = type { … }`.
    /// Stored in declaration order via a `BTreeMap` so the parser's
    /// output is deterministic.
    pub struct_types: BTreeMap<String, StructType>,
    /// Module-level globals: `@name = … <type>`.
    pub globals: Vec<Global>,
    /// Function definitions.
    pub functions: Vec<Function>,
}

/// `%struct.UART_TypeDef = type { %union.anon, %union.anon.0, %union.anon.2 }`
#[derive(Debug, Clone)]
pub struct StructType {
    /// Name as written, including the `%struct.` / `%union.` prefix.
    pub name: String,
    /// Field types in declaration order, raw text. Slice 2.b+ may
    /// resolve these against [`Module::struct_types`] for nested
    /// layouts.
    pub fields: Vec<String>,
}

/// `@name = external constant ptr, align 8` — a module-level global.
#[derive(Debug, Clone)]
pub struct Global {
    pub name: String,
    /// `external`, `internal`, `private`, `dso_local`, ... — preserved
    /// as raw tokens; phase L2 may inspect `external` to flag globals
    /// that bind to peripheral base addresses at link time.
    pub linkage: Vec<String>,
    /// `constant` or `global`. `constant` is what `extern volatile
    /// UART_TypeDef *const UART` lowers to.
    pub kind: GlobalKind,
    /// The declared LLVM type — `ptr`, `i32`, `%struct.X`, etc.
    pub ty: String,
    /// `align N` if present.
    pub align: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKind {
    Constant,
    Global,
}

/// `define [<linkage>] <ret> @name(<params>) ... { <blocks> }`.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    /// Return type as raw text (`void`, `i32`, `ptr`, etc.).
    pub return_type: String,
    /// Parameters in declaration order. Each entry is `(type,
    /// optional SSA name)`. Anonymous params (clang's implicit `%0`,
    /// `%1`, ...) have `name = None` — clang assigns them sequential
    /// SSA names that we'd have to track separately.
    pub parameters: Vec<FunctionParameter>,
    /// Function-level attributes (`noinline`, `nounwind`, ...) as raw
    /// tokens. Phase L6 will inspect these for `@mununu_isr` cross-
    /// references at the IR layer.
    pub attributes: Vec<String>,
    /// Basic blocks in source order. The entry block has the implicit
    /// label `entry` when clang doesn't emit a header for it; we
    /// synthesise that.
    pub basic_blocks: Vec<BasicBlock>,
}

#[derive(Debug, Clone)]
pub struct FunctionParameter {
    pub ty: String,
    pub name: Option<String>,
}

/// A basic block: a label, a sequence of instructions, and a
/// terminator.
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub label: String,
    /// `; preds = …` comment when clang emits one — phase L3 uses
    /// these to reconstruct the CFG.
    pub predecessors: Vec<String>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

/// An SSA-form instruction. The enum covers the shapes the phase
/// L2+ consumers need; anything else falls through to [`Self::Other`]
/// with the raw text preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// `%result = alloca <ty>, align N`
    Alloca {
        result: String,
        ty: String,
        align: Option<u64>,
    },
    /// `%result = load [volatile] <ty>, ptr <src>, align N`
    Load {
        result: String,
        ty: String,
        source: PointerOperand,
        volatile: bool,
        align: Option<u64>,
    },
    /// `store [volatile] <ty> <val>, ptr <dest>, align N`
    Store {
        value: ValueOperand,
        ty: String,
        dest: PointerOperand,
        volatile: bool,
        align: Option<u64>,
    },
    /// `%result = getelementptr [inbounds] <ty>, ptr <base>, <indices>`
    ///
    /// `indices` is the list of `i32 N` / `i64 N` arguments after the
    /// base pointer. Each entry is the *literal* integer index;
    /// non-constant indices are flagged via [`GepIndex::Dynamic`].
    Gep {
        result: String,
        ty: String,
        base: PointerOperand,
        indices: Vec<GepIndex>,
        in_bounds: bool,
    },
    /// `%result = inttoptr <int-ty> <literal> to ptr` — common for
    /// `*(volatile uint32_t *)0x40010000` register accesses.
    IntToPtr { result: String, value: u64 },
    /// `%result = ptrtoint ptr <src> to <int-ty>`
    PtrToInt { result: String, source: String },
    /// `%result = bitcast <ty> <src> to <ty2>`
    Bitcast { result: String, source: String },
    /// `%result = trunc <ty> <src> to <ty2>` — bit-field unpacking.
    Trunc { result: String, source: String },
    /// `%result = zext <ty> <src> to <ty2>`
    ZExt { result: String, source: String },
    /// `%result = sext <ty> <src> to <ty2>`
    SExt { result: String, source: String },
    /// `%result = <op> <ty> <a>, <b>` — integer binary ops (and / or
    /// / xor / shl / lshr / ashr / add / sub / mul / udiv / sdiv /
    /// urem / srem). Bit-field load-modify-store sequences use the
    /// bitwise subset heavily.
    BinaryOp {
        result: String,
        op: BinaryOp,
        ty: String,
        a: ValueOperand,
        b: ValueOperand,
    },
    /// `%result = icmp <pred> <ty> <a>, <b>`
    Icmp {
        result: String,
        pred: IcmpPred,
        ty: String,
        a: ValueOperand,
        b: ValueOperand,
    },
    /// `[%result = ]call <ret> @func(<args>)` — phase L5 walks the
    /// call graph through these.
    Call {
        result: Option<String>,
        return_ty: String,
        callee: String,
        args: Vec<String>,
    },
    /// `%result = phi <ty> [ <v0>, %bb0 ], [ <v1>, %bb1 ], ...` —
    /// phase L3 needs these to reason about loops.
    Phi {
        result: String,
        ty: String,
        incoming: Vec<(ValueOperand, String)>,
    },
    /// Anything the parser does not recognise. The raw line is
    /// preserved verbatim (no leading/trailing whitespace).
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GepIndex {
    /// `i32 0`, `i32 1`, `i64 2` — a constant index value.
    Const(i64),
    /// `%ssa` — a runtime-computed index.
    Dynamic(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    UDiv,
    SDiv,
    URem,
    SRem,
    Shl,
    LShr,
    AShr,
    And,
    Or,
    Xor,
}

impl BinaryOp {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "add" => Self::Add,
            "sub" => Self::Sub,
            "mul" => Self::Mul,
            "udiv" => Self::UDiv,
            "sdiv" => Self::SDiv,
            "urem" => Self::URem,
            "srem" => Self::SRem,
            "shl" => Self::Shl,
            "lshr" => Self::LShr,
            "ashr" => Self::AShr,
            "and" => Self::And,
            "or" => Self::Or,
            "xor" => Self::Xor,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    Trunc,
    ZExt,
    SExt,
    Bitcast,
    PtrToInt,
    IntToPtr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcmpPred {
    Eq,
    Ne,
    Ugt,
    Uge,
    Ult,
    Ule,
    Sgt,
    Sge,
    Slt,
    Sle,
}

impl IcmpPred {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "eq" => Self::Eq,
            "ne" => Self::Ne,
            "ugt" => Self::Ugt,
            "uge" => Self::Uge,
            "ult" => Self::Ult,
            "ule" => Self::Ule,
            "sgt" => Self::Sgt,
            "sge" => Self::Sge,
            "slt" => Self::Slt,
            "sle" => Self::Sle,
            _ => return None,
        })
    }
}

/// A pointer operand — what appears after `ptr` in a load / store /
/// GEP base position. Carrying the distinction between SSA names,
/// globals, and inline constant pointers lets phase L2 do
/// address-range matching without re-parsing strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerOperand {
    /// `%ssa` — refers to the result of an earlier instruction.
    Ssa(String),
    /// `@global` — refers to a module-level global.
    Global(String),
    /// `inttoptr (i64 N to ptr)` — inline constant pointer literal.
    /// Captured as a u64 so phase L2 can match against the
    /// register-map's base-address window.
    InlineConstAddr(u64),
    /// `getelementptr inbounds (T, ptr inttoptr (i64 N to ptr), i32 0, i32 K)` —
    /// an inline GEP constexpr clang generates when the base pointer
    /// is a `#define` macro. Captured as base-address + field index
    /// so phase L2's `match_address_to_register` can use the same
    /// `GlobalFieldIndex` lookup path. Captures one inline-GEP form;
    /// other shapes still surface as `Ssa(raw_text)` and trigger an
    /// `UnresolvedPointer` warning.
    InlineGep { base_addr: u64, field_index: i64 },
}

/// A value operand — what appears in non-pointer positions (the RHS
/// of stores, the operands of binary ops, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueOperand {
    Ssa(String),
    /// A literal integer (`42`, `-1`, etc.).
    LiteralInt(i64),
    /// `undef` / `poison`.
    Undef,
    /// Anything we did not classify — preserved as raw text.
    Other(String),
}

impl ValueOperand {
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix('%') {
            return Self::Ssa(rest.to_string());
        }
        if s == "undef" || s == "poison" {
            return Self::Undef;
        }
        if let Ok(n) = s.parse::<i64>() {
            return Self::LiteralInt(n);
        }
        // Handle `0x…` and `0b…` literals too.
        if let Some(hex) = s.strip_prefix("0x")
            && let Ok(n) = i64::from_str_radix(hex, 16)
        {
            return Self::LiteralInt(n);
        }
        Self::Other(s.to_string())
    }
}

/// A block terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminator {
    /// `ret <ty> <val>` or `ret void`.
    Ret { value: Option<ValueOperand> },
    /// `br label %target`.
    Br { target: String },
    /// `br i1 <cond>, label %true, label %false`.
    BrCond {
        cond: ValueOperand,
        if_true: String,
        if_false: String,
    },
    /// `switch <ty> <val>, label %default [ <case>, label %target … ]`.
    Switch {
        value: ValueOperand,
        default: String,
        cases: Vec<(i64, String)>,
    },
    /// `unreachable` / `resume` / unknown.
    Unreachable,
    /// Anything not in the above list — preserved verbatim.
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_operand_parses_ssa_literal_and_undef() {
        assert_eq!(ValueOperand::parse("%5"), ValueOperand::Ssa("5".into()));
        assert_eq!(ValueOperand::parse("42"), ValueOperand::LiteralInt(42));
        assert_eq!(ValueOperand::parse("-1"), ValueOperand::LiteralInt(-1));
        assert_eq!(ValueOperand::parse("undef"), ValueOperand::Undef);
        assert_eq!(
            ValueOperand::parse("@global_thing"),
            ValueOperand::Other("@global_thing".into())
        );
    }

    #[test]
    fn binary_op_parse_round_trips_known_ops() {
        for op_str in ["add", "and", "or", "shl", "lshr"] {
            assert!(BinaryOp::parse(op_str).is_some(), "{op_str}");
        }
        assert!(BinaryOp::parse("bogus").is_none());
    }

    #[test]
    fn icmp_pred_parse_covers_signed_and_unsigned() {
        for p in ["eq", "ne", "ult", "sle"] {
            assert!(IcmpPred::parse(p).is_some(), "{p}");
        }
        assert!(IcmpPred::parse("xx").is_none());
    }
}
