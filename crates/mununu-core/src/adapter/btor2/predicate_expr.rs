//! B.1 — compound predicate expressions for the predicate-cube path.
//!
//! Today a cube/SMT predicate is a single atom `register == value`
//! ([`crate::adapter::btor2::kmts_lift::PredicateSpec`]). This module adds
//! **compound predicates** — a boolean combination of register comparisons,
//! e.g. `idle = cnt == 0 && en == 1`, evaluated as ONE cube dimension. A
//! compound predicate does not change `|P|` (it is still one cube bit); only
//! the function that decides that bit's truth changes from an equality test to
//! a recursive boolean evaluation.
//!
//! # The soundness obligation (the §4 PO)
//!
//! There are two evaluators that MUST compute the **same boolean function**
//! over any concrete state, or the cube abstraction is unsound:
//!
//! - [`PredicateExpr::eval`] — explicit evaluation over a concrete register
//!   valuation (the sampling / target-cube-truth path in `kmts_lift`).
//! - [`PredicateExpr::build_constraint`] — the Z3 `Bool` over a BV view (the
//!   SMT may/must-edge path in `smt_must_edge`).
//!
//! The `predicate_expr_eval_matches_smt` differential test enumerates atoms,
//! operators, and assignments and asserts `eval(e, s) == sat(build_constraint(e)
//! under s)`. With that, the cube preservation theorem (PO-1: `may ⊇ concrete`,
//! `must ⊆ concrete`) transfers to compound predicates unchanged — the KMTS
//! machinery is agnostic to whether a predicate is atomic or compound.
//!
//! # Width / masking convention
//!
//! `build_constraint` masks the comparison value to the BV width (matching the
//! simple-atom [`crate::adapter::btor2::smt_must_edge`] path). `eval` compares
//! the register's concrete value (already width-bounded when it comes from the
//! simulator) against the value as-is. The two agree whenever the comparison
//! value fits the register width — the same implicit assumption the existing
//! simple-atom cube path already makes (`next_v == pred.value as u128`).

use std::collections::BTreeSet;

/// Comparison operator for a predicate atom. Unsigned bit-vector semantics
/// (the register is an unsigned bit-vector value), matching the BTOR2 / Z3 BV
/// `bvult`/`bvule`/`bvugt`/`bvuge` operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A compound predicate expression: a boolean combination of register
/// comparisons. The leaf is a single `register <op> value` comparison; the
/// internal nodes are `And` / `Or` / `Not`. No arithmetic — that is the
/// register's job, not the predicate's (keeps the predicate layer inside the
/// audited modal fragment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateExpr {
    /// `register <op> value` (unsigned comparison against a literal).
    Cmp {
        register: String,
        op: CmpOp,
        value: u64,
    },
    /// REL — `lhs <op> rhs` where **both** operands are registers (a
    /// *relational* predicate, e.g. `state_q == state_q__past` for `$stable`, or
    /// `data_o == data_i` for dataflow). Like [`PredicateExpr::Cmp`] it is one
    /// cube bit; the second register is referenced (not a cube dimension), and
    /// the SMT image picks a consistent value for it. Only the SMT path honours
    /// it (the literal-based sampler cannot realise a two-register relation —
    /// the same reason compound predicates are gated to `SmtAllPairs`).
    CmpReg {
        lhs: String,
        op: CmpOp,
        rhs: String,
    },
    And(Box<PredicateExpr>, Box<PredicateExpr>),
    Or(Box<PredicateExpr>, Box<PredicateExpr>),
    Not(Box<PredicateExpr>),
}

impl PredicateExpr {
    /// A simple `register == value` atom — the shape every existing
    /// `PredicateSpec` lowers to. Lets the cube path treat a simple predicate
    /// as a trivial compound without special-casing.
    pub fn eq(register: impl Into<String>, value: u64) -> Self {
        PredicateExpr::Cmp {
            register: register.into(),
            op: CmpOp::Eq,
            value,
        }
    }

    /// A relational `lhs == rhs` atom over two registers (REL). The
    /// `$stable(x)` / `$changed(x)` translator forms emit `x <op> x__past`.
    pub fn eq_reg(lhs: impl Into<String>, rhs: impl Into<String>) -> Self {
        PredicateExpr::CmpReg {
            lhs: lhs.into(),
            op: CmpOp::Eq,
            rhs: rhs.into(),
        }
    }

    /// Explicit evaluation over a concrete register valuation. Registers absent
    /// from `regs` default to `0` — matching the cube lifter's `.unwrap_or(0)`
    /// convention for next-state registers that no transition wrote.
    pub fn eval(&self, regs: &std::collections::HashMap<String, u128>) -> bool {
        match self {
            PredicateExpr::Cmp {
                register,
                op,
                value,
            } => {
                let lhs = regs.get(register).copied().unwrap_or(0);
                cmp_apply(*op, lhs, *value as u128)
            }
            PredicateExpr::CmpReg { lhs, op, rhs } => {
                let l = regs.get(lhs).copied().unwrap_or(0);
                let r = regs.get(rhs).copied().unwrap_or(0);
                cmp_apply(*op, l, r)
            }
            PredicateExpr::And(a, b) => a.eval(regs) && b.eval(regs),
            PredicateExpr::Or(a, b) => a.eval(regs) || b.eval(regs),
            PredicateExpr::Not(a) => !a.eval(regs),
        }
    }

    /// All distinct register names referenced by the expression, sorted. Used
    /// to resolve + width-check every register a compound predicate touches
    /// (the simple atom touches exactly one; a compound may touch several).
    pub fn registers(&self) -> Vec<String> {
        let mut set = BTreeSet::new();
        self.collect_registers(&mut set);
        set.into_iter().collect()
    }

    fn collect_registers(&self, out: &mut BTreeSet<String>) {
        match self {
            PredicateExpr::Cmp { register, .. } => {
                out.insert(register.clone());
            }
            PredicateExpr::CmpReg { lhs, rhs, .. } => {
                out.insert(lhs.clone());
                out.insert(rhs.clone());
            }
            PredicateExpr::And(a, b) | PredicateExpr::Or(a, b) => {
                a.collect_registers(out);
                b.collect_registers(out);
            }
            PredicateExpr::Not(a) => a.collect_registers(out),
        }
    }

    /// SMT encoding: a Z3 `Bool` over the register BVs supplied by `lookup`
    /// (the caller maps a register name to its `state_curr` or `state_next` BV
    /// from the encoded view). Returns `None` if any referenced register is
    /// absent from the view — the caller treats that exactly as it treats a
    /// missing simple-atom register (an `Unknown` must-edge verdict).
    ///
    /// **Caller must hold a [`z3::with_z3_config`] scope.**
    pub fn build_constraint<F>(&self, lookup: &F) -> Option<z3::ast::Bool>
    where
        F: Fn(&str) -> Option<z3::ast::BV>,
    {
        match self {
            PredicateExpr::Cmp {
                register,
                op,
                value,
            } => {
                let bv = lookup(register)?;
                Some(cmp_constraint(&bv, *op, *value))
            }
            PredicateExpr::CmpReg { lhs, op, rhs } => {
                let lbv = lookup(lhs)?;
                let rbv = lookup(rhs)?;
                Some(cmp_constraint_bv(&lbv, *op, &rbv))
            }
            PredicateExpr::And(a, b) => {
                let ca = a.build_constraint(lookup)?;
                let cb = b.build_constraint(lookup)?;
                Some(z3::ast::Bool::and(&[&ca, &cb]))
            }
            PredicateExpr::Or(a, b) => {
                let ca = a.build_constraint(lookup)?;
                let cb = b.build_constraint(lookup)?;
                Some(z3::ast::Bool::or(&[&ca, &cb]))
            }
            PredicateExpr::Not(a) => Some(a.build_constraint(lookup)?.not()),
        }
    }
}

/// Apply a comparison operator to two concrete unsigned values (shared by the
/// `Cmp` literal leaf and the `CmpReg` relational leaf in [`PredicateExpr::eval`]).
fn cmp_apply(op: CmpOp, lhs: u128, rhs: u128) -> bool {
    match op {
        CmpOp::Eq => lhs == rhs,
        CmpOp::Ne => lhs != rhs,
        CmpOp::Lt => lhs < rhs,
        CmpOp::Le => lhs <= rhs,
        CmpOp::Gt => lhs > rhs,
        CmpOp::Ge => lhs >= rhs,
    }
}

/// Build the Z3 `Bool` for a relational `lhs <op> rhs` atom over two BVs (REL).
/// Zero-extends the narrower BV to the wider width first (unsigned semantics) so
/// registers of different widths compare correctly.
fn cmp_constraint_bv(lhs: &z3::ast::BV, op: CmpOp, rhs: &z3::ast::BV) -> z3::ast::Bool {
    let (l, r) = match_widths(lhs, rhs);
    match op {
        CmpOp::Eq => l.eq(&r),
        CmpOp::Ne => l.eq(&r).not(),
        CmpOp::Lt => l.bvult(&r),
        CmpOp::Le => l.bvule(&r),
        CmpOp::Gt => l.bvugt(&r),
        CmpOp::Ge => l.bvuge(&r),
    }
}

/// Zero-extend the narrower of two BVs so both share the wider width.
fn match_widths(a: &z3::ast::BV, b: &z3::ast::BV) -> (z3::ast::BV, z3::ast::BV) {
    let (wa, wb) = (a.get_size(), b.get_size());
    if wa < wb {
        (a.zero_ext(wb - wa), b.clone())
    } else if wb < wa {
        (a.clone(), b.zero_ext(wa - wb))
    } else {
        (a.clone(), b.clone())
    }
}

/// Build the Z3 `Bool` for one `register <op> value` atom over `bv`. Masks the
/// value to the BV width (matching
/// `smt_must_edge::build_predicate_constraint`).
fn cmp_constraint(bv: &z3::ast::BV, op: CmpOp, value: u64) -> z3::ast::Bool {
    let width = bv.get_size();
    let mask: u64 = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let val_bv = z3::ast::BV::from_u64(value & mask, width);
    match op {
        CmpOp::Eq => bv.eq(&val_bv),
        CmpOp::Ne => bv.eq(&val_bv).not(),
        CmpOp::Lt => bv.bvult(&val_bv),
        CmpOp::Le => bv.bvule(&val_bv),
        CmpOp::Gt => bv.bvugt(&val_bv),
        CmpOp::Ge => bv.bvuge(&val_bv),
    }
}

// --------------------------------------------------------------------------
// B.1 — sidecar expr-string parser
//
// The MVP sidecar carries a compound predicate as a human-authored string
// (`"cnt == 0 && en == 1"`), parsed here into a [`PredicateExpr`]. (The
// structured-JSON form is a Phase-FP option; the string is friendlier to
// author.) Hand-rolled tokenizer + recursive-descent parser — no external
// grammar dep, and the supported surface is deliberately small (the §6 scope
// discipline): comparison atoms + `&&` / `||` / `!` / parentheses, nothing
// else (no arithmetic — that is the register's job, not the predicate's).
// --------------------------------------------------------------------------

/// Error parsing a predicate-expression string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateExprParseError {
    pub message: String,
}

impl std::fmt::Display for PredicateExprParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "predicate expression parse error: {}", self.message)
    }
}

impl std::error::Error for PredicateExprParseError {}

fn perr(message: impl Into<String>) -> PredicateExprParseError {
    PredicateExprParseError {
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Int(u64),
    Op(CmpOp),
    And,
    Or,
    Not,
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<Vec<Token>, PredicateExprParseError> {
    let chars: Vec<char> = s.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                tokens.push(Token::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                tokens.push(Token::Or);
                i += 2;
            }
            '=' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Op(CmpOp::Eq));
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Op(CmpOp::Ne));
                i += 2;
            }
            '!' => {
                tokens.push(Token::Not);
                i += 1;
            }
            '<' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Op(CmpOp::Le));
                i += 2;
            }
            '<' => {
                tokens.push(Token::Op(CmpOp::Lt));
                i += 1;
            }
            '>' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Op(CmpOp::Ge));
                i += 2;
            }
            '>' => {
                tokens.push(Token::Op(CmpOp::Gt));
                i += 1;
            }
            '0'..='9' => {
                let start = i;
                // 0x-hex or decimal.
                if c == '0' && matches!(chars.get(i + 1), Some('x') | Some('X')) {
                    i += 2;
                    let hex_start = i;
                    while i < chars.len() && chars[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    let hex: String = chars[hex_start..i].iter().collect();
                    if hex.is_empty() {
                        return Err(perr("empty hex literal after `0x`"));
                    }
                    let v = u64::from_str_radix(&hex, 16)
                        .map_err(|e| perr(format!("bad hex literal `0x{hex}`: {e}")))?;
                    tokens.push(Token::Int(v));
                } else {
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let dec: String = chars[start..i].iter().collect();
                    let v = dec
                        .parse::<u64>()
                        .map_err(|e| perr(format!("bad decimal literal `{dec}`: {e}")))?;
                    tokens.push(Token::Int(v));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric()
                        || chars[i] == '_'
                        || chars[i] == '$'
                        || chars[i] == '.')
                {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(ident));
            }
            other => {
                return Err(perr(format!("unexpected character `{other}`")));
            }
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Result<PredicateExpr, PredicateExprParseError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.pos += 1;
            let rhs = self.parse_and()?;
            lhs = PredicateExpr::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<PredicateExpr, PredicateExprParseError> {
        let mut lhs = self.parse_not()?;
        while matches!(self.peek(), Some(Token::And)) {
            self.pos += 1;
            let rhs = self.parse_not()?;
            lhs = PredicateExpr::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<PredicateExpr, PredicateExprParseError> {
        if matches!(self.peek(), Some(Token::Not)) {
            self.pos += 1;
            let inner = self.parse_not()?;
            Ok(PredicateExpr::Not(Box::new(inner)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<PredicateExpr, PredicateExprParseError> {
        match self.peek() {
            Some(Token::LParen) => {
                self.pos += 1;
                let e = self.parse_or()?;
                match self.next() {
                    Some(Token::RParen) => Ok(e),
                    _ => Err(perr("expected `)`")),
                }
            }
            Some(Token::Ident(_)) => self.parse_atom(),
            other => Err(perr(format!(
                "expected an atom or `(`, found {:?}",
                other.cloned()
            ))),
        }
    }

    fn parse_atom(&mut self) -> Result<PredicateExpr, PredicateExprParseError> {
        let register = match self.next() {
            Some(Token::Ident(s)) => s,
            other => return Err(perr(format!("expected a register name, found {other:?}"))),
        };
        let op = match self.next() {
            Some(Token::Op(o)) => o,
            other => {
                return Err(perr(format!(
                    "expected a comparison operator after `{register}`, found {other:?}"
                )));
            }
        };
        // RHS is an integer literal (→ `Cmp`) or a register name (→ `CmpReg`,
        // the REL relational form, e.g. `state_q == state_q__past`).
        match self.next() {
            Some(Token::Int(value)) => Ok(PredicateExpr::Cmp {
                register,
                op,
                value,
            }),
            Some(Token::Ident(rhs)) => Ok(PredicateExpr::CmpReg {
                lhs: register,
                op,
                rhs,
            }),
            other => Err(perr(format!(
                "expected an integer value or a register after `{register} <op>`, found {other:?}"
            ))),
        }
    }
}

/// Parse a sidecar predicate-expression string into a [`PredicateExpr`].
///
/// Grammar (precedence low→high): `||`, `&&`, unary `!`, parentheses, then
/// comparison atoms `<register> <op> <rhs>` with `op ∈ { ==, !=, <, <=, >,
/// >= }`. Registers are identifiers (`[A-Za-z_][A-Za-z0-9_$.]*`; `$`/`.` are
/// allowed for BTOR2 hierarchical symbol names). The `<rhs>` is a decimal or
/// `0x`-hex integer (→ `Cmp`) **or another register** (→ `CmpReg`, the REL
/// relational form). `&&` binds tighter than `||`; `!` is unary prefix.
///
/// Examples: `cnt == 0 && en == 1`, `!(state == 5) || done >= 1`,
/// `state_q == state_q__past` (relational — REL).
pub fn parse_predicate_expr(s: &str) -> Result<PredicateExpr, PredicateExprParseError> {
    let tokens = tokenize(s)?;
    if tokens.is_empty() {
        return Err(perr("empty predicate expression"));
    }
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
    };
    let expr = parser.parse_or()?;
    if parser.pos != tokens.len() {
        return Err(perr(format!(
            "unexpected trailing tokens starting at {:?}",
            tokens.get(parser.pos)
        )));
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn regs(pairs: &[(&str, u128)]) -> HashMap<String, u128> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn eval_simple_eq_atom() {
        let e = PredicateExpr::eq("cnt", 0);
        assert!(e.eval(&regs(&[("cnt", 0)])));
        assert!(!e.eval(&regs(&[("cnt", 1)])));
        // Absent register defaults to 0.
        assert!(e.eval(&regs(&[])));
    }

    #[test]
    fn eval_compound_and_or_not() {
        // idle = cnt == 0 && en == 1
        let idle = PredicateExpr::And(
            Box::new(PredicateExpr::eq("cnt", 0)),
            Box::new(PredicateExpr::eq("en", 1)),
        );
        assert!(idle.eval(&regs(&[("cnt", 0), ("en", 1)])));
        assert!(!idle.eval(&regs(&[("cnt", 0), ("en", 0)])));
        assert!(!idle.eval(&regs(&[("cnt", 3), ("en", 1)])));

        // busy = !(cnt == 0) || en == 0
        let busy = PredicateExpr::Or(
            Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("cnt", 0)))),
            Box::new(PredicateExpr::eq("en", 0)),
        );
        assert!(busy.eval(&regs(&[("cnt", 2), ("en", 1)])));
        assert!(busy.eval(&regs(&[("cnt", 0), ("en", 0)])));
        assert!(!busy.eval(&regs(&[("cnt", 0), ("en", 1)])));
    }

    #[test]
    fn eval_ordering_operators() {
        let lt = PredicateExpr::Cmp {
            register: "x".into(),
            op: CmpOp::Lt,
            value: 4,
        };
        assert!(lt.eval(&regs(&[("x", 3)])));
        assert!(!lt.eval(&regs(&[("x", 4)])));
        let ge = PredicateExpr::Cmp {
            register: "x".into(),
            op: CmpOp::Ge,
            value: 4,
        };
        assert!(ge.eval(&regs(&[("x", 4)])));
        assert!(!ge.eval(&regs(&[("x", 3)])));
    }

    #[test]
    fn parse_simple_atom() {
        assert_eq!(
            parse_predicate_expr("cnt == 0").unwrap(),
            PredicateExpr::eq("cnt", 0)
        );
        assert_eq!(
            parse_predicate_expr("state_q != 41").unwrap(),
            PredicateExpr::Cmp {
                register: "state_q".into(),
                op: CmpOp::Ne,
                value: 41
            }
        );
    }

    #[test]
    fn parse_all_comparison_ops_and_hex() {
        let cases = [
            ("a == 1", CmpOp::Eq),
            ("a != 1", CmpOp::Ne),
            ("a < 1", CmpOp::Lt),
            ("a <= 1", CmpOp::Le),
            ("a > 1", CmpOp::Gt),
            ("a >= 1", CmpOp::Ge),
        ];
        for (src, op) in cases {
            assert_eq!(
                parse_predicate_expr(src).unwrap(),
                PredicateExpr::Cmp {
                    register: "a".into(),
                    op,
                    value: 1
                },
                "parsing {src}"
            );
        }
        // 0x-hex value.
        assert_eq!(
            parse_predicate_expr("x == 0x1f").unwrap(),
            PredicateExpr::eq("x", 31)
        );
    }

    #[test]
    fn parse_compound_and_precedence() {
        // idle = cnt == 0 && en == 1
        assert_eq!(
            parse_predicate_expr("cnt == 0 && en == 1").unwrap(),
            PredicateExpr::And(
                Box::new(PredicateExpr::eq("cnt", 0)),
                Box::new(PredicateExpr::eq("en", 1)),
            )
        );
        // && binds tighter than ||: a==1 || b==2 && c==3  ==  Or(a, And(b, c))
        assert_eq!(
            parse_predicate_expr("a == 1 || b == 2 && c == 3").unwrap(),
            PredicateExpr::Or(
                Box::new(PredicateExpr::eq("a", 1)),
                Box::new(PredicateExpr::And(
                    Box::new(PredicateExpr::eq("b", 2)),
                    Box::new(PredicateExpr::eq("c", 3)),
                )),
            )
        );
        // Parens override precedence + unary !.
        assert_eq!(
            parse_predicate_expr("!(state == 5) || done >= 1").unwrap(),
            PredicateExpr::Or(
                Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("state", 5)))),
                Box::new(PredicateExpr::Cmp {
                    register: "done".into(),
                    op: CmpOp::Ge,
                    value: 1
                }),
            )
        );
    }

    #[test]
    fn parse_round_trips_through_eval() {
        let e = parse_predicate_expr("cnt == 0 && en == 1").unwrap();
        assert!(e.eval(&regs(&[("cnt", 0), ("en", 1)])));
        assert!(!e.eval(&regs(&[("cnt", 1), ("en", 1)])));
    }

    // --- REL (relational predicates: register == register) ---------------

    #[test]
    fn parse_relational_atom_to_cmpreg() {
        // `state_q == state_q__past` ($stable) parses to a CmpReg, not a Cmp.
        assert_eq!(
            parse_predicate_expr("state_q == state_q__past").unwrap(),
            PredicateExpr::eq_reg("state_q", "state_q__past")
        );
        assert_eq!(
            parse_predicate_expr("data_o != data_i").unwrap(),
            PredicateExpr::CmpReg {
                lhs: "data_o".into(),
                op: CmpOp::Ne,
                rhs: "data_i".into(),
            }
        );
        // Literal RHS still parses to Cmp (no regression).
        assert_eq!(
            parse_predicate_expr("state_q == 5").unwrap(),
            PredicateExpr::eq("state_q", 5)
        );
    }

    #[test]
    fn eval_relational_atom() {
        let stable = PredicateExpr::eq_reg("x", "x_past");
        assert!(stable.eval(&regs(&[("x", 7), ("x_past", 7)]))); // $stable true
        assert!(!stable.eval(&regs(&[("x", 7), ("x_past", 6)]))); // changed
        // A relational atom inside a compound (the real $stable-under-disable shape).
        let e = parse_predicate_expr("rst == 0 && state == state_past").unwrap();
        assert!(e.eval(&regs(&[("rst", 0), ("state", 3), ("state_past", 3)])));
        assert!(!e.eval(&regs(&[("rst", 0), ("state", 3), ("state_past", 2)])));
        assert!(!e.eval(&regs(&[("rst", 1), ("state", 3), ("state_past", 3)])));
    }

    #[test]
    fn registers_collects_both_sides_of_a_relational_atom() {
        let e = parse_predicate_expr("a == b && c != a").unwrap();
        assert_eq!(
            e.registers(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    // The §4 soundness obligation, extended to REL: eval ≡ SMT for register-vs-
    // register leaves (the ONLY new proof obligation relational predicates add).
    #[test]
    fn relational_eval_matches_smt() {
        let exprs: Vec<PredicateExpr> = vec![
            PredicateExpr::eq_reg("a", "b"),
            PredicateExpr::CmpReg {
                lhs: "a".into(),
                op: CmpOp::Ne,
                rhs: "b".into(),
            },
            PredicateExpr::CmpReg {
                lhs: "a".into(),
                op: CmpOp::Lt,
                rhs: "b".into(),
            },
            PredicateExpr::CmpReg {
                lhs: "a".into(),
                op: CmpOp::Ge,
                rhs: "b".into(),
            },
            // relational leaf combined with a literal leaf + boolean structure
            PredicateExpr::And(
                Box::new(PredicateExpr::eq("a", 0)),
                Box::new(PredicateExpr::eq_reg("a", "b")),
            ),
            PredicateExpr::Not(Box::new(PredicateExpr::eq_reg("a", "b"))),
        ];

        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            for e in &exprs {
                for a in 0u64..4 {
                    for b in 0u64..4 {
                        let want = e.eval(&regs(&[("a", a as u128), ("b", b as u128)]));
                        let a_bv = z3::ast::BV::new_const("a", 2);
                        let b_bv = z3::ast::BV::new_const("b", 2);
                        let lookup = |name: &str| -> Option<z3::ast::BV> {
                            match name {
                                "a" => Some(a_bv.clone()),
                                "b" => Some(b_bv.clone()),
                                _ => None,
                            }
                        };
                        let constraint = e.build_constraint(&lookup).expect("all regs present");
                        let solver = z3::Solver::new();
                        let a_val = z3::ast::BV::from_u64(a, 2);
                        let b_val = z3::ast::BV::from_u64(b, 2);
                        let a_pin = a_bv.eq(&a_val);
                        let b_pin = b_bv.eq(&b_val);
                        solver.assert(&a_pin);
                        solver.assert(&b_pin);
                        solver.assert(&constraint);
                        let got = matches!(solver.check(), z3::SatResult::Sat);
                        assert_eq!(
                            want, got,
                            "eval/SMT disagree for {e:?} at a={a}, b={b}: eval={want}, smt={got}"
                        );
                    }
                }
            }
        });
    }

    #[test]
    fn relational_eval_matches_smt_mismatched_widths() {
        // a is 2-bit, b is 4-bit → zero-extend a; eval (u128) must agree.
        let e = PredicateExpr::eq_reg("a", "b");
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            for a in 0u64..4 {
                for b in 0u64..16 {
                    let want = e.eval(&regs(&[("a", a as u128), ("b", b as u128)]));
                    let a_bv = z3::ast::BV::new_const("a", 2);
                    let b_bv = z3::ast::BV::new_const("b", 4);
                    let lookup = |name: &str| -> Option<z3::ast::BV> {
                        match name {
                            "a" => Some(a_bv.clone()),
                            "b" => Some(b_bv.clone()),
                            _ => None,
                        }
                    };
                    let constraint = e.build_constraint(&lookup).expect("present");
                    let solver = z3::Solver::new();
                    let a_val = z3::ast::BV::from_u64(a, 2);
                    let b_val = z3::ast::BV::from_u64(b, 4);
                    let a_pin = a_bv.eq(&a_val);
                    let b_pin = b_bv.eq(&b_val);
                    solver.assert(&a_pin);
                    solver.assert(&b_pin);
                    solver.assert(&constraint);
                    let got = matches!(solver.check(), z3::SatResult::Sat);
                    assert_eq!(
                        want, got,
                        "width-mismatch eval/SMT disagree at a={a}, b={b}"
                    );
                }
            }
        });
    }

    #[test]
    fn parse_rejects_malformed() {
        // Missing value, missing op, trailing op, unclosed paren, empty,
        // bad char, arithmetic (out of scope).
        for bad in [
            "",
            "cnt ==",
            "cnt 0",
            "cnt == 0 &&",
            "(cnt == 0",
            "cnt == 0)",
            "cnt == ", // op then nothing
            "cnt + 1 == 0",
            "cnt == 0x",
        ] {
            assert!(
                parse_predicate_expr(bad).is_err(),
                "expected parse error for {bad:?}"
            );
        }
    }

    #[test]
    fn registers_collects_all_referenced_sorted() {
        let e = PredicateExpr::And(
            Box::new(PredicateExpr::Or(
                Box::new(PredicateExpr::eq("b", 1)),
                Box::new(PredicateExpr::eq("a", 0)),
            )),
            Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("a", 2)))),
        );
        assert_eq!(e.registers(), vec!["a".to_string(), "b".to_string()]);
    }

    // The §4 soundness obligation: the explicit evaluator and the SMT
    // encoding compute the SAME boolean function over every assignment. If
    // they ever diverge, compound predicates are unsound on the cube path.
    #[test]
    fn predicate_expr_eval_matches_smt() {
        // Two 2-bit registers a, b ranging 0..=3. A spread of expressions
        // covering every operator + And/Or/Not nesting.
        let exprs: Vec<PredicateExpr> = vec![
            PredicateExpr::eq("a", 0),
            PredicateExpr::Cmp {
                register: "a".into(),
                op: CmpOp::Ne,
                value: 2,
            },
            PredicateExpr::Cmp {
                register: "a".into(),
                op: CmpOp::Lt,
                value: 2,
            },
            PredicateExpr::Cmp {
                register: "b".into(),
                op: CmpOp::Ge,
                value: 1,
            },
            PredicateExpr::And(
                Box::new(PredicateExpr::eq("a", 0)),
                Box::new(PredicateExpr::Cmp {
                    register: "b".into(),
                    op: CmpOp::Gt,
                    value: 1,
                }),
            ),
            PredicateExpr::Or(
                Box::new(PredicateExpr::Not(Box::new(PredicateExpr::eq("a", 3)))),
                Box::new(PredicateExpr::Cmp {
                    register: "b".into(),
                    op: CmpOp::Le,
                    value: 1,
                }),
            ),
            PredicateExpr::Not(Box::new(PredicateExpr::And(
                Box::new(PredicateExpr::eq("a", 1)),
                Box::new(PredicateExpr::eq("b", 1)),
            ))),
        ];

        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            for e in &exprs {
                for a in 0u64..4 {
                    for b in 0u64..4 {
                        let want = e.eval(&regs(&[("a", a as u128), ("b", b as u128)]));

                        let a_bv = z3::ast::BV::new_const("a", 2);
                        let b_bv = z3::ast::BV::new_const("b", 2);
                        let lookup = |name: &str| -> Option<z3::ast::BV> {
                            match name {
                                "a" => Some(a_bv.clone()),
                                "b" => Some(b_bv.clone()),
                                _ => None,
                            }
                        };
                        let constraint = e.build_constraint(&lookup).expect("all regs present");

                        let solver = z3::Solver::new();
                        // Pin the assignment (named bindings, mirroring the
                        // smt_must_edge assert pattern).
                        let a_val = z3::ast::BV::from_u64(a, 2);
                        let b_val = z3::ast::BV::from_u64(b, 2);
                        let a_pin = a_bv.eq(&a_val);
                        let b_pin = b_bv.eq(&b_val);
                        solver.assert(&a_pin);
                        solver.assert(&b_pin);
                        solver.assert(&constraint);
                        let got = matches!(solver.check(), z3::SatResult::Sat);

                        assert_eq!(
                            want, got,
                            "eval/SMT disagree for {e:?} at a={a}, b={b}: eval={want}, smt={got}"
                        );
                    }
                }
            }
        });
    }
}
