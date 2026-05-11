//! BTOR2 parser.
//!
//! BTOR2 is line-based ASCII; tokens are whitespace-separated. Comments start
//! with `;` and run to end of line. Empty lines and comment-only lines are
//! ignored.

use super::ast::*;
use crate::adapter::{AdapterError, AdapterErrorKind, SourceLocation};
use std::collections::HashMap;

pub fn parse(content: &str) -> Result<Btor2File, AdapterError> {
    let mut file = Btor2File::default();

    for (idx, raw) in content.lines().enumerate() {
        let source_line = idx + 1;
        // Strip comments and trim.
        let line = match raw.find(';') {
            Some(pos) => &raw[..pos],
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 2 {
            return Err(parse_err(source_line, format!("malformed line: {line:?}")));
        }

        let nid = parse_nid(tokens[0], source_line)?;
        let kw = tokens[1];

        let node = parse_node(kw, &tokens[2..], source_line)?;
        let immediates = collect_immediates(kw, &tokens[2..], source_line)?;

        if file.by_nid.contains_key(&nid) {
            return Err(parse_err(source_line, format!("duplicate NID {nid}")));
        }
        file.by_nid.insert(nid, file.lines.len());
        file.lines.push(Line {
            nid,
            node,
            immediates,
            source_line,
        });
    }

    validate(&file)?;
    Ok(file)
}

fn parse_node(kw: &str, args: &[&str], source_line: usize) -> Result<Node, AdapterError> {
    match kw {
        "sort" => parse_sort(args, source_line),
        "input" => Ok(Node::Input {
            sort: parse_nid(arg(args, 0, source_line, "input sort")?, source_line)?,
            symbol: args.get(1).map(|s| s.to_string()),
        }),
        "state" => Ok(Node::State {
            sort: parse_nid(arg(args, 0, source_line, "state sort")?, source_line)?,
            symbol: args.get(1).map(|s| s.to_string()),
        }),
        "init" => {
            let sort = parse_nid(arg(args, 0, source_line, "init sort")?, source_line)?;
            let state = parse_nid(arg(args, 1, source_line, "init state ref")?, source_line)?;
            let value = parse_operand(arg(args, 2, source_line, "init value ref")?, source_line)?;
            Ok(Node::Init { sort, state, value })
        }
        "next" => {
            let sort = parse_nid(arg(args, 0, source_line, "next sort")?, source_line)?;
            let state = parse_nid(arg(args, 1, source_line, "next state ref")?, source_line)?;
            let value = parse_operand(arg(args, 2, source_line, "next value ref")?, source_line)?;
            Ok(Node::Next { sort, state, value })
        }
        "bad" => Ok(Node::Bad {
            signal: parse_operand(arg(args, 0, source_line, "bad signal")?, source_line)?,
        }),
        "constraint" => Ok(Node::Constraint {
            signal: parse_operand(arg(args, 0, source_line, "constraint signal")?, source_line)?,
        }),
        "fair" => Ok(Node::Fair {
            signal: parse_operand(arg(args, 0, source_line, "fair signal")?, source_line)?,
        }),
        "output" => Ok(Node::Output {
            signal: parse_operand(arg(args, 0, source_line, "output signal")?, source_line)?,
            symbol: args.get(1).map(|s| s.to_string()),
        }),
        "justice" => {
            let n = parse_uint(arg(args, 0, source_line, "justice count")?, source_line)?;
            let mut signals = Vec::with_capacity(n as usize);
            for i in 0..n as usize {
                signals.push(parse_operand(
                    arg(args, 1 + i, source_line, "justice signal")?,
                    source_line,
                )?);
            }
            Ok(Node::Justice { signals })
        }
        // Constants
        "zero" | "one" | "ones" | "const" | "constd" | "consth" => {
            parse_const(kw, args, source_line)
        }
        // Operators
        other => parse_op(other, args, source_line),
    }
}

fn parse_sort(args: &[&str], source_line: usize) -> Result<Node, AdapterError> {
    let kind = arg(args, 0, source_line, "sort kind")?;
    match kind {
        "bitvec" => {
            let width = parse_uint(arg(args, 1, source_line, "bitvec width")?, source_line)?;
            Ok(Node::Sort {
                sort: Sort::BitVec { width },
            })
        }
        "array" => {
            let index = parse_nid(arg(args, 1, source_line, "array index sort")?, source_line)?;
            let element = parse_nid(
                arg(args, 2, source_line, "array element sort")?,
                source_line,
            )?;
            Ok(Node::Sort {
                sort: Sort::Array { index, element },
            })
        }
        other => Err(parse_err(
            source_line,
            format!("unknown sort kind '{other}'"),
        )),
    }
}

fn parse_const(kw: &str, args: &[&str], source_line: usize) -> Result<Node, AdapterError> {
    let sort = parse_nid(arg(args, 0, source_line, "const sort")?, source_line)?;
    let value = match kw {
        "zero" => ConstValue::Zero,
        "one" => ConstValue::One,
        "ones" => ConstValue::Ones,
        "const" => ConstValue::Bin(arg(args, 1, source_line, "const bits")?.to_string()),
        "constd" => {
            let raw = arg(args, 1, source_line, "constd value")?;
            let v: i128 = raw
                .parse()
                .map_err(|_| parse_err(source_line, format!("bad decimal literal '{raw}'")))?;
            ConstValue::Dec(v)
        }
        "consth" => ConstValue::Hex(arg(args, 1, source_line, "consth value")?.to_string()),
        _ => unreachable!("parse_const dispatched by keyword"),
    };
    Ok(Node::Const { sort, value })
}

fn parse_op(kw: &str, args: &[&str], source_line: usize) -> Result<Node, AdapterError> {
    let op = Op::from_keyword(kw)
        .ok_or_else(|| parse_err(source_line, format!("unknown BTOR2 keyword '{kw}'")))?;

    let sort = parse_nid(arg(args, 0, source_line, "op sort")?, source_line)?;
    let arity = op.arity();

    let mut operands = Vec::with_capacity(arity);
    for i in 0..arity {
        operands.push(parse_operand(
            arg(args, 1 + i, source_line, "op operand")?,
            source_line,
        )?);
    }

    // Capture the trailing symbol (e.g. `uext SORT NID 0 fill`). The number
    // of immediates depends on the op; for slice it's 2 (upper, lower), for
    // uext/sext it's 1 (amount), and for the rest it's 0. Anything past
    // (1 + arity + immediates) that doesn't start with a comment marker is
    // the symbol.
    let immediates_count = match kw {
        "slice" => 2,
        "uext" | "sext" => 1,
        _ => 0,
    };
    let symbol_idx = 1 + arity + immediates_count;
    let symbol = args
        .get(symbol_idx)
        .filter(|s| !s.starts_with(';'))
        .map(|s| s.to_string());

    Ok(Node::Op {
        sort,
        op,
        args: operands,
        symbol,
    })
}

fn collect_immediates(
    kw: &str,
    args: &[&str],
    source_line: usize,
) -> Result<Vec<u32>, AdapterError> {
    // Slice: <sort> <signal> <upper> <lower>
    if kw == "slice" {
        let upper = parse_uint(arg(args, 2, source_line, "slice upper")?, source_line)?;
        let lower = parse_uint(arg(args, 3, source_line, "slice lower")?, source_line)?;
        return Ok(vec![upper, lower]);
    }
    // uext / sext: <sort> <signal> <amount>
    if kw == "uext" || kw == "sext" {
        let amount = parse_uint(arg(args, 2, source_line, "ext amount")?, source_line)?;
        return Ok(vec![amount]);
    }
    Ok(Vec::new())
}

fn validate(file: &Btor2File) -> Result<(), AdapterError> {
    // Cross-reference check: every operand and sort reference must exist.
    let known: std::collections::HashSet<Nid> = file.by_nid.keys().copied().collect();

    for line in &file.lines {
        match &line.node {
            Node::Input { sort, .. } | Node::State { sort, .. } | Node::Const { sort, .. } => {
                check_ref(*sort, &known, line.source_line)?;
            }
            Node::Op { sort, args, .. } => {
                check_ref(*sort, &known, line.source_line)?;
                for op in args {
                    check_ref(op.nid(), &known, line.source_line)?;
                }
            }
            Node::Init { sort, state, value } | Node::Next { sort, state, value } => {
                check_ref(*sort, &known, line.source_line)?;
                check_ref(*state, &known, line.source_line)?;
                check_ref(value.nid(), &known, line.source_line)?;
            }
            Node::Bad { signal }
            | Node::Constraint { signal }
            | Node::Fair { signal }
            | Node::Output { signal, .. } => {
                check_ref(signal.nid(), &known, line.source_line)?;
            }
            Node::Justice { signals } => {
                for s in signals {
                    check_ref(s.nid(), &known, line.source_line)?;
                }
            }
            Node::Sort { sort } => {
                if let Sort::Array { index, element } = sort {
                    check_ref(*index, &known, line.source_line)?;
                    check_ref(*element, &known, line.source_line)?;
                }
            }
        }
    }
    Ok(())
}

fn check_ref(
    nid: Nid,
    known: &std::collections::HashSet<Nid>,
    source_line: usize,
) -> Result<(), AdapterError> {
    if !known.contains(&nid) {
        return Err(parse_err(
            source_line,
            format!("reference to undefined NID {nid}"),
        ));
    }
    Ok(())
}

fn parse_nid(s: &str, source_line: usize) -> Result<Nid, AdapterError> {
    s.parse::<Nid>()
        .map_err(|_| parse_err(source_line, format!("expected non-negative NID, got '{s}'")))
}

fn parse_operand(s: &str, source_line: usize) -> Result<Operand, AdapterError> {
    let n = s
        .parse::<Nid>()
        .map_err(|_| parse_err(source_line, format!("expected operand NID, got '{s}'")))?;
    Ok(Operand(n))
}

fn parse_uint(s: &str, source_line: usize) -> Result<u32, AdapterError> {
    s.parse::<u32>().map_err(|_| {
        parse_err(
            source_line,
            format!("expected non-negative integer, got '{s}'"),
        )
    })
}

fn arg<'a>(
    args: &'a [&'a str],
    idx: usize,
    source_line: usize,
    what: &str,
) -> Result<&'a str, AdapterError> {
    args.get(idx)
        .copied()
        .ok_or_else(|| parse_err(source_line, format!("missing {what} (index {idx})")))
}

fn parse_err(source_line: usize, msg: String) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: msg,
        location: Some(SourceLocation {
            line: source_line,
            column: 0,
        }),
    }
}

/// Compute symbol-table style symbols for inputs and states (best-effort).
/// Used by the bit-blaster to populate human-readable signal names.
///
/// Yosys's `write_btor` typically attaches synthetic names like
/// `$auto$async2sync.cc:234:execute$30` to state lines, while the
/// user's actual register names (`fill`, `state`, …) appear as no-op
/// `uext _ NID 0 NAME` alias ops. To surface user-visible names on
/// `valuations { … }` blocks, we also walk the alias chain: for each Op
/// with a symbol, we trace its operand graph backward looking for the
/// first reachable State and attach the symbol to it (only when the
/// state has no user-visible symbol of its own — synthetic compiler
/// names are recognised by their `$auto$` / `$0` prefixes and treated
/// as overridable).
pub fn collect_symbols(file: &Btor2File) -> HashMap<Nid, String> {
    let mut out = HashMap::new();

    // Pass 1: collect direct Input/State symbols.
    for line in &file.lines {
        match &line.node {
            Node::Input {
                symbol: Some(s), ..
            }
            | Node::State {
                symbol: Some(s), ..
            } => {
                out.insert(line.nid, s.clone());
            }
            _ => {}
        }
    }

    // Pass 2: trace `Op { symbol: Some(NAME) }` aliases (Yosys's `uext _ _ 0 NAME`
    // pattern) back to their underlying state lines and attach the user-
    // visible name. Only overrides synthetic compiler-generated names.
    let is_synthetic = |s: &str| s.starts_with('$');
    for line in &file.lines {
        if let Node::Op {
            args,
            symbol: Some(name),
            ..
        } = &line.node
        {
            if let Some(state_nid) = trace_to_state(file, args) {
                let entry = out.entry(state_nid);
                match entry {
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(name.clone());
                    }
                    std::collections::hash_map::Entry::Occupied(mut o) => {
                        if is_synthetic(o.get()) {
                            o.insert(name.clone());
                        }
                    }
                }
            }
        }
    }
    out
}

/// Walk the operand graph backward from a list of operands, returning
/// the nid of the first State node reachable. Returns `None` if no state
/// is found within a small bounded traversal (cycles + width are guarded
/// to keep this O(n) overall across the whole file).
fn trace_to_state(file: &Btor2File, args: &[crate::adapter::btor2::ast::Operand]) -> Option<Nid> {
    let mut stack: Vec<Nid> = args.iter().map(|o| o.nid()).collect();
    let mut seen: std::collections::HashSet<Nid> = std::collections::HashSet::new();
    while let Some(nid) = stack.pop() {
        if !seen.insert(nid) {
            continue;
        }
        let Some(line) = file.lookup(nid) else {
            continue;
        };
        match &line.node {
            Node::State { .. } => return Some(line.nid),
            Node::Op { args, .. } => {
                for o in args {
                    stack.push(o.nid());
                }
            }
            _ => {}
        }
    }
    None
}

/// Resolve the bit-vector width of a sort node. Returns `None` if the
/// referenced node is an array sort or unresolvable.
pub fn bv_width(file: &Btor2File, sort_nid: Nid) -> Option<u32> {
    match &file.lookup(sort_nid)?.node {
        Node::Sort {
            sort: Sort::BitVec { width },
        } => Some(*width),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_safety_circuit() {
        // 1-bit toggle latch with a "bad" output that fires when latch=1.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 one 1
4 state 1 q
5 init 1 4 2
6 not 1 4
7 next 1 4 6
8 bad 4
"#;
        let file = parse(src).expect("parse");
        assert_eq!(file.lines.len(), 8);
        assert!(matches!(
            file.lookup(1).unwrap().node,
            Node::Sort {
                sort: Sort::BitVec { width: 1 }
            }
        ));
        assert_eq!(file.states().count(), 1);
        assert_eq!(file.bads().count(), 1);
    }

    #[test]
    fn rejects_undefined_reference() {
        let src = "1 sort bitvec 1\n2 not 1 99\n";
        let err = parse(src).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
        assert!(err.message.contains("undefined NID 99"));
    }

    #[test]
    fn handles_comments_and_blank_lines() {
        let src = r#"
; this is a comment
1 sort bitvec 1
; mid-file comment

2 zero 1   ; trailing comment
"#;
        let file = parse(src).expect("parse");
        assert_eq!(file.lines.len(), 2);
    }

    #[test]
    fn parses_negated_operand() {
        let src = "1 sort bitvec 1\n2 input 1 a\n3 not 1 -2\n";
        let file = parse(src).expect("parse");
        if let Node::Op { args, .. } = &file.lookup(3).unwrap().node {
            assert!(args[0].is_negated());
            assert_eq!(args[0].nid(), 2);
        } else {
            panic!("expected Op node");
        }
    }

    #[test]
    fn parses_slice_immediates() {
        let src = "1 sort bitvec 8\n2 sort bitvec 4\n3 input 1 a\n4 slice 2 3 7 4\n";
        let file = parse(src).expect("parse");
        let line = file.lookup(4).unwrap();
        assert_eq!(line.immediates, vec![7, 4]);
    }

    #[test]
    fn parses_uext_immediate() {
        let src = "1 sort bitvec 4\n2 sort bitvec 8\n3 input 1 a\n4 uext 2 3 4\n";
        let file = parse(src).expect("parse");
        assert_eq!(file.lookup(4).unwrap().immediates, vec![4]);
    }

    #[test]
    fn parses_justice_set() {
        let src = "1 sort bitvec 1\n2 input 1 a\n3 input 1 b\n4 justice 2 2 3\n";
        let file = parse(src).expect("parse");
        if let Node::Justice { signals } = &file.lookup(4).unwrap().node {
            assert_eq!(signals.len(), 2);
        } else {
            panic!("expected Justice node");
        }
    }
}
