//! BTOR2 emitter — serialise a [`Btor2File`] AST back to BTOR2 text.
//!
//! The inverse of [`super::parser::parse`]. It renders each [`Line`] from its
//! typed [`Node`] (NOT from the retained `source_line`), so it faithfully emits
//! a model that has been **transformed** — a cone-of-influence reduction, an
//! input pinned to a constant, a `constraint` folded in — not just the text that
//! was read. That is what makes it the L2-seam hand-off (`Btor2Emit`) to the
//! subprocess safety engines (btormc / Pono / ABC / AVR): mununu reduces a model
//! internally, emits it, and an external engine decides the reduced query.
//!
//! Fidelity: `parse ∘ emit ∘ parse` is a fixed point — emitting a parsed file
//! and re-parsing yields an identical AST (NIDs, sorts, ops, immediates, and
//! properties all preserved). Optional symbols are emitted when present. The
//! output is a valid BTOR2 file consumable by any conforming tool.

use super::ast::{Btor2File, ConstValue, Line, Node, Operand, Sort};

/// Serialise `file` to BTOR2 text (one node per line, declaration order, trailing newline).
pub fn emit_btor2(file: &Btor2File) -> String {
    let mut out = String::new();
    for line in &file.lines {
        emit_line(line, &mut out);
        out.push('\n');
    }
    out
}

/// An operand renders as its signed NID: a negated operand (`-N`, BTOR2's
/// bit-not shorthand) keeps its sign.
fn operand(op: Operand) -> String {
    op.0.to_string()
}

fn emit_line(line: &Line, out: &mut String) {
    use std::fmt::Write;
    let nid = line.nid;
    match &line.node {
        Node::Sort { sort } => match sort {
            Sort::BitVec { width } => {
                let _ = write!(out, "{nid} sort bitvec {width}");
            }
            Sort::Array { index, element } => {
                let _ = write!(out, "{nid} sort array {index} {element}");
            }
        },
        Node::Input { sort, symbol } => {
            let _ = write!(out, "{nid} input {sort}");
            emit_symbol(symbol, out);
        }
        Node::State { sort, symbol } => {
            let _ = write!(out, "{nid} state {sort}");
            emit_symbol(symbol, out);
        }
        Node::Const { sort, value } => match value {
            ConstValue::Zero => {
                let _ = write!(out, "{nid} zero {sort}");
            }
            ConstValue::One => {
                let _ = write!(out, "{nid} one {sort}");
            }
            ConstValue::Ones => {
                let _ = write!(out, "{nid} ones {sort}");
            }
            ConstValue::Bin(s) => {
                let _ = write!(out, "{nid} const {sort} {s}");
            }
            ConstValue::Dec(d) => {
                let _ = write!(out, "{nid} constd {sort} {d}");
            }
            ConstValue::Hex(s) => {
                let _ = write!(out, "{nid} consth {sort} {s}");
            }
        },
        Node::Op {
            sort,
            op,
            args,
            symbol,
        } => {
            let _ = write!(out, "{nid} {} {sort}", op.keyword());
            for a in args {
                let _ = write!(out, " {}", operand(*a));
            }
            // Operator immediates follow the operand list (`slice … <upper> <lower>`,
            // `uext / sext … <amount>`).
            for imm in &line.immediates {
                let _ = write!(out, " {imm}");
            }
            emit_symbol(symbol, out);
        }
        Node::Init { sort, state, value } => {
            let _ = write!(out, "{nid} init {sort} {state} {}", operand(*value));
        }
        Node::Next { sort, state, value } => {
            let _ = write!(out, "{nid} next {sort} {state} {}", operand(*value));
        }
        Node::Bad { signal } => {
            let _ = write!(out, "{nid} bad {}", operand(*signal));
        }
        Node::Constraint { signal } => {
            let _ = write!(out, "{nid} constraint {}", operand(*signal));
        }
        Node::Fair { signal } => {
            let _ = write!(out, "{nid} fair {}", operand(*signal));
        }
        Node::Output { signal, symbol } => {
            let _ = write!(out, "{nid} output {}", operand(*signal));
            emit_symbol(symbol, out);
        }
        Node::Justice { signals } => {
            let _ = write!(out, "{nid} justice {}", signals.len());
            for s in signals {
                let _ = write!(out, " {}", operand(*s));
            }
        }
    }
}

fn emit_symbol(symbol: &Option<String>, out: &mut String) {
    use std::fmt::Write;
    if let Some(s) = symbol {
        let _ = write!(out, " {s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser;

    /// `parse ∘ emit ∘ parse` is a fixed point: emitting a parsed file and
    /// re-parsing yields an AST that re-emits identically.
    fn assert_roundtrip(src: &str) {
        let file = parser::parse(src).expect("parse original");
        let emitted = emit_btor2(&file);
        let reparsed = parser::parse(&emitted).expect("re-parse emitted");
        let re_emitted = emit_btor2(&reparsed);
        assert_eq!(
            emitted, re_emitted,
            "emit is not idempotent under re-parse\n--- emitted ---\n{emitted}\n--- re-emitted ---\n{re_emitted}"
        );
        assert_eq!(
            file.lines.len(),
            reparsed.lines.len(),
            "re-parse changed the line count"
        );
    }

    #[test]
    fn roundtrip_counter_with_bad() {
        assert_roundtrip(
            "1 sort bitvec 3\n2 zero 1\n3 state 1\n4 init 1 3 2\n5 one 1\n6 add 1 3 5\n\
             7 next 1 3 6\n8 ones 1\n9 sort bitvec 1\n10 eq 9 3 8\n11 bad 10\n",
        );
    }

    #[test]
    fn roundtrip_slice_ext_immediates_and_symbols() {
        // Exercises slice (2 immediates), uext/sext (1 immediate), a named
        // input/state, an output symbol, and a negated operand.
        assert_roundtrip(
            "1 sort bitvec 8\n2 sort bitvec 4\n3 sort bitvec 1\n\
             4 input 1 din\n5 state 1 acc\n6 slice 2 5 3 0\n7 uext 1 6 4\n\
             8 sext 1 6 4\n9 not 1 4\n10 and 1 4 9\n11 eq 3 5 4 is_eq\n\
             12 output 5 acc_o\n13 bad 11\n",
        );
    }

    #[test]
    fn roundtrip_constraint_fair_justice_consts() {
        assert_roundtrip(
            "1 sort bitvec 4\n2 input 1 c\n3 state 1 s\n4 const 1 1010\n\
             5 constd 1 -3\n6 consth 1 a\n7 fair 2\n8 constraint 2\n\
             9 justice 2 2 3\n",
        );
    }

    #[test]
    fn emitted_preserves_bad_reachability_verdict() {
        use crate::adapter::btor2::symbolic_bitblast::exact_bad_reachable;
        // A reachable-bad counter: emit → re-check that the emitted model decides
        // the SAME reachability verdict (a semantic, not just syntactic, round-trip).
        const COUNTER: &str = "1 sort bitvec 3\n2 zero 1\n3 state 1\n4 init 1 3 2\n5 one 1\n6 add 1 3 5\n\
             7 next 1 3 6\n8 ones 1\n9 sort bitvec 1\n10 eq 9 3 8\n11 bad 10\n";
        let file = parser::parse(COUNTER).expect("parse");
        let emitted = emit_btor2(&file);
        assert_eq!(
            exact_bad_reachable(&emitted),
            Ok(true),
            "the emitted model must preserve the reachable-bad verdict"
        );
    }
}
