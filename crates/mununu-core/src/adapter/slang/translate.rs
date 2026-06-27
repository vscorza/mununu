//! XL.1b — Tier-1 SVA → mu-calculus translation over slang's `--ast-json`.
//!
//! Walks the JSON [`crate::adapter::slang::run_ast_json`] produces, finds every
//! `ConcurrentAssertion`, and translates the Tier-1 fragment to a mu-calculus
//! formula string that [`crate::mu_calculus::parser::parse`] accepts and the
//! cube/CEGAR evaluator can check.
//!
//! **Tier-1 fragment + encoding** (the [XL.0] schema mapping):
//!
//! | SVA | mu-calculus |
//! |---|---|
//! | `assert property (b)` | `nu X. (b && [] X)` (AG b) |
//! | `a \|-> b` | `nu X. ((!a \|\| b) && [] X)` (AG(a→b)) |
//! | `a \|=> b` | `nu X. ((!a \|\| [] b) && [] X)` (AG(a→AX b)) |
//! | `cover property (b)` | `mu X. (b \|\| <> X)` (EF b) |
//! | `disable iff (r) P` | gate the body: `(r \|\| body)` (vacuous while disabled) |
//!
//! Implication `a → b` is emitted as `!a || b` — the mu-calculus parser has no
//! `->` operator, and the rewrite is exact.
//!
//! **Atoms.** A boolean leaf is a signal (`NamedValue` → identifier atom) or a
//! comparison (`sig == k`, parsed as a comparison predicate). Boolean structure
//! (`!`, `&&`, `||`) recurses. Anything outside this fragment — reductions,
//! arithmetic, indexing, sequences (`##`, `[*n]`), `$past`, etc. — is **rejected
//! with a reason, never silently dropped** (claims-integrity), and every emitted
//! formula is validated through the mu-calculus parser as a safety net.
//!
//! [XL.0]: ../../../../.claude/plans/measurements/XL-0-sva-parser-spike-2026-06-26.md

use serde_json::Value;

use crate::adapter::{AdapterError, AdapterErrorKind};

/// The three concurrent-assertion kinds slang reports (`assertionKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvaKind {
    Assert,
    Assume,
    Cover,
}

/// A successfully-translated assertion.
#[derive(Debug, Clone)]
pub struct TranslatedAssertion {
    /// Best-effort name: `<module>_sva_<index>` (slang's `--ast-json` does not
    /// attach the SV label to the assertion node — the label lives on a separate
    /// symbol-table entry; XL.1c can recover it).
    pub name: String,
    pub kind: SvaKind,
    /// mu-calculus formula; guaranteed to parse via
    /// [`crate::mu_calculus::parser::parse`].
    pub formula: String,
}

/// An assertion outside the Tier-1 fragment, recorded (never silently dropped).
#[derive(Debug, Clone)]
pub struct UnsupportedAssertion {
    pub name: String,
    pub kind: Option<SvaKind>,
    pub reason: String,
}

/// Result of translating a whole `--ast-json` document.
#[derive(Debug, Clone, Default)]
pub struct TranslationReport {
    pub translated: Vec<TranslatedAssertion>,
    pub unsupported: Vec<UnsupportedAssertion>,
}

impl TranslationReport {
    /// Total concurrent assertions seen (translated + unsupported).
    pub fn total(&self) -> usize {
        self.translated.len() + self.unsupported.len()
    }
}

/// Translate every concurrent assertion in a slang `--ast-json` document.
pub fn translate_ast_json(json: &str) -> Result<TranslationReport, AdapterError> {
    let root: Value = serde_json::from_str(json).map_err(|e| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: format!("adapter/slang/translate: --ast-json is not valid JSON: {e}"),
        location: None,
    })?;

    let mut found: Vec<(String, &Value)> = Vec::new();
    collect_assertions(&root, "design", &mut found);

    let mut report = TranslationReport::default();
    for (idx, (module, node)) in found.iter().enumerate() {
        let name = format!("{module}_sva_{idx}");
        let kind = match node.get("assertionKind").and_then(Value::as_str) {
            Some("Assert") => SvaKind::Assert,
            Some("Assume") => SvaKind::Assume,
            Some(s) if s.starts_with("Cover") => SvaKind::Cover,
            other => {
                report.unsupported.push(UnsupportedAssertion {
                    name,
                    kind: None,
                    reason: format!("unknown assertionKind: {other:?}"),
                });
                continue;
            }
        };
        let Some(spec) = node.get("propertySpec") else {
            report.unsupported.push(UnsupportedAssertion {
                name,
                kind: Some(kind),
                reason: "assertion has no propertySpec".to_string(),
            });
            continue;
        };
        match translate_one(spec, kind) {
            Ok(formula) => report.translated.push(TranslatedAssertion {
                name,
                kind,
                formula,
            }),
            Err(reason) => report.unsupported.push(UnsupportedAssertion {
                name,
                kind: Some(kind),
                reason,
            }),
        }
    }
    Ok(report)
}

/// Recursively collect `ConcurrentAssertion` nodes, tracking the nearest
/// enclosing `name` (the module) for a best-effort assertion name.
fn collect_assertions<'a>(node: &'a Value, enclosing: &str, out: &mut Vec<(String, &'a Value)>) {
    match node {
        Value::Object(map) => {
            if map.get("kind").and_then(Value::as_str) == Some("ConcurrentAssertion") {
                out.push((enclosing.to_string(), node));
            }
            // Update the enclosing name for descendants when this node names one.
            let next = map
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(enclosing);
            for v in map.values() {
                collect_assertions(v, next, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_assertions(v, enclosing, out);
            }
        }
        _ => {}
    }
}

/// Translate one assertion's `propertySpec` to a full mu-calculus formula, then
/// validate it parses. `Err(reason)` for any out-of-fragment construct.
fn translate_one(spec: &Value, kind: SvaKind) -> Result<String, String> {
    let body = property_body(spec)?;
    let formula = match kind {
        SvaKind::Assert | SvaKind::Assume => format!("nu X. (({body}) && [] X)"),
        SvaKind::Cover => format!("mu X. (({body}) || <> X)"),
    };
    // Safety net: only ship formulas the evaluator's parser accepts.
    crate::mu_calculus::parser::parse(&formula)
        .map_err(|e| format!("emitted formula failed to parse ({e:?}): {formula}"))?;
    Ok(formula)
}

/// Translate an `AssertionExpr` into the per-step propositional body (the part
/// inside `nu X. (BODY && []X)` / `mu X. (BODY || <>X)`).
fn property_body(spec: &Value) -> Result<String, String> {
    let k = spec.get("kind").and_then(Value::as_str).unwrap_or("?");
    match k {
        // `@(posedge clk) P` — one CLTS step is one clock edge; strip the clock.
        "Clocking" => property_body(child(spec, "expr")?),
        // `disable iff (r) P` — vacuously true while disabled: `(r || body)`.
        "DisableIff" => {
            let cond = bool_expr(child(spec, "condition")?)?;
            let inner = property_body(child(spec, "expr")?)?;
            Ok(format!("({cond} || {inner})"))
        }
        // `a |-> b` / `a |=> b` — left/right are boolean (Tier-1 `Simple`).
        "Binary" => {
            let op = spec.get("op").and_then(Value::as_str).unwrap_or("?");
            let l = simple_bool(child(spec, "left")?)?;
            let r = simple_bool(child(spec, "right")?)?;
            match op {
                "OverlappedImplication" => Ok(format!("(!({l}) || {r})")),
                "NonOverlappedImplication" => Ok(format!("(!({l}) || [] {r})")),
                other => Err(format!("unsupported property operator: {other}")),
            }
        }
        "Simple" => bool_expr(child(spec, "expr")?),
        other => Err(format!("unsupported property kind: {other}")),
    }
}

/// An `AssertionExpr` expected to be a boolean leaf (`Simple`) — Tier-1.
fn simple_bool(spec: &Value) -> Result<String, String> {
    match spec.get("kind").and_then(Value::as_str) {
        Some("Simple") => bool_expr(child(spec, "expr")?),
        other => Err(format!(
            "unsupported operand (Tier-1 expects a boolean, got {other:?})"
        )),
    }
}

/// Translate a boolean `Expression` into mu-calculus atom/connective text.
fn bool_expr(expr: &Value) -> Result<String, String> {
    let k = expr.get("kind").and_then(Value::as_str).unwrap_or("?");
    match k {
        "NamedValue" => signal_name(expr).map(|s| s.to_string()),
        "UnaryOp" => {
            let op = expr.get("op").and_then(Value::as_str).unwrap_or("?");
            if op == "LogicalNot" {
                let inner = bool_expr(child(expr, "operand")?)?;
                Ok(format!("(!({inner}))"))
            } else {
                Err(format!("unsupported unary op: {op}"))
            }
        }
        "BinaryOp" => {
            let op = expr.get("op").and_then(Value::as_str).unwrap_or("?");
            match op {
                "LogicalAnd" => {
                    let l = bool_expr(child(expr, "left")?)?;
                    let r = bool_expr(child(expr, "right")?)?;
                    Ok(format!("({l} && {r})"))
                }
                "LogicalOr" => {
                    let l = bool_expr(child(expr, "left")?)?;
                    let r = bool_expr(child(expr, "right")?)?;
                    Ok(format!("({l} || {r})"))
                }
                // Comparison atoms: only `signal OP literal` (the form the
                // mu-calculus parser accepts as a comparison predicate).
                "Equality" | "CaseEquality" => comparison(expr, "=="),
                "Inequality" | "CaseInequality" => comparison(expr, "!="),
                "LessThan" => comparison(expr, "<"),
                "GreaterThan" => comparison(expr, ">"),
                "LessThanEqual" => comparison(expr, "<="),
                "GreaterThanEqual" => comparison(expr, ">="),
                other => Err(format!("unsupported binary op: {other}")),
            }
        }
        other => Err(format!("unsupported expression kind: {other}")),
    }
}

/// `signal OP literal` → a comparison atom; rejects non-atomic operands so the
/// emitted atom stays in the parser's `identifier OP value` shape.
fn comparison(expr: &Value, op: &str) -> Result<String, String> {
    let lhs = signal_name(child(expr, "left")?)
        .map_err(|_| "comparison LHS is not a plain signal".to_string())?;
    let rhs = integer_literal(child(expr, "right")?)
        .ok_or_else(|| "comparison RHS is not an integer literal".to_string())?;
    Ok(format!("{lhs} {op} {rhs}"))
}

/// Extract the signal name from a `NamedValue` (`"symbol": "<id> name"`).
fn signal_name(expr: &Value) -> Result<&str, String> {
    if expr.get("kind").and_then(Value::as_str) != Some("NamedValue") {
        return Err("expected a NamedValue signal reference".to_string());
    }
    let sym = expr
        .get("symbol")
        .and_then(Value::as_str)
        .ok_or_else(|| "NamedValue without a symbol".to_string())?;
    // slang formats the symbol as "<address> <name>"; take the name token.
    Ok(sym.rsplit(' ').next().unwrap_or(sym))
}

/// Extract a non-negative decimal value from an integer-literal expression.
fn integer_literal(expr: &Value) -> Option<u64> {
    let k = expr.get("kind").and_then(Value::as_str)?;
    if !k.contains("IntegerLiteral") {
        return None;
    }
    // slang serialises the constant as `"constant": "<width>'<base><digits>"` or
    // a plain value; accept the first run of decimal digits.
    let raw = expr
        .get("constant")
        .or_else(|| expr.get("value"))
        .and_then(Value::as_str)?;
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok()
}

fn child<'a>(node: &'a Value, key: &str) -> Result<&'a Value, String> {
    node.get(key)
        .ok_or_else(|| format!("missing field `{key}` on {:?}", node.get("kind")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Frozen `slang --ast-json` snapshot of a 5-assertion Tier-1 module
    // (assert bool / assert |-> / assert |=> / assume / cover). Regenerate via:
    //   slang --ast-json testdata/tier1.ast.json testdata/tier1.sv --single-unit
    const TIER1_JSON: &str = include_str!("testdata/tier1.ast.json");

    #[test]
    fn translates_the_tier1_fixture() {
        let report = translate_ast_json(TIER1_JSON).expect("valid ast-json");
        assert_eq!(report.total(), 5, "the fixture has 5 concurrent assertions");
        assert_eq!(
            report.unsupported.len(),
            0,
            "all 5 fixture assertions are Tier-1; got unsupported: {:?}",
            report.unsupported
        );
        // Every emitted formula must parse (translate_one already validates, but
        // re-confirm here as the contract).
        for t in &report.translated {
            crate::mu_calculus::parser::parse(&t.formula)
                .unwrap_or_else(|e| panic!("formula for {} failed to parse: {e:?}", t.name));
        }
        let by_kind = |k: SvaKind| -> Vec<&str> {
            report
                .translated
                .iter()
                .filter(|t| t.kind == k)
                .map(|t| t.formula.as_str())
                .collect()
        };
        // 3 asserts (bool, |->, |=>), 1 assume, 1 cover.
        assert_eq!(by_kind(SvaKind::Assert).len(), 3);
        assert_eq!(by_kind(SvaKind::Assume).len(), 1);
        assert_eq!(by_kind(SvaKind::Cover).len(), 1);
        // The cover is an EF (mu / <>); the asserts are AG (nu / []).
        assert!(by_kind(SvaKind::Cover)[0].starts_with("mu X."));
        assert!(
            by_kind(SvaKind::Assert)
                .iter()
                .all(|f| f.starts_with("nu X."))
        );
        // The |-> assert encodes implication as `!a || b`.
        assert!(
            by_kind(SvaKind::Assert)
                .iter()
                .any(|f| f.contains("(!(") && f.contains("||")),
            "an implication assert must encode `a -> b` as `!a || b`; got {:?}",
            by_kind(SvaKind::Assert)
        );
    }

    #[test]
    fn implication_body_rewrites_to_disjunction() {
        // a |-> b  →  nu X. ((!(a) || b) && [] X)
        let spec = serde_json::json!({
            "kind": "Binary",
            "op": "OverlappedImplication",
            "left":  {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "1 a"}},
            "right": {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "2 b"}}
        });
        let f = translate_one(&spec, SvaKind::Assert).expect("translates");
        assert_eq!(f, "nu X. (((!(a) || b)) && [] X)");
        crate::mu_calculus::parser::parse(&f).expect("parses");
    }

    #[test]
    fn nonoverlapped_implication_adds_next() {
        // a |=> b  →  next-cycle b
        let spec = serde_json::json!({
            "kind": "Binary",
            "op": "NonOverlappedImplication",
            "left":  {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "1 a"}},
            "right": {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "2 b"}}
        });
        let f = translate_one(&spec, SvaKind::Assert).expect("translates");
        assert!(f.contains("[] b"), "|=> must put b under a next ([]): {f}");
        crate::mu_calculus::parser::parse(&f).expect("parses");
    }

    #[test]
    fn unsupported_construct_is_recorded_not_dropped() {
        // A reduction-or `|req` (UnaryOp other than LogicalNot) is out of Tier-1.
        let spec = serde_json::json!({
            "kind": "Simple",
            "expr": {"kind": "UnaryOp", "op": "ReductionOr",
                     "operand": {"kind": "NamedValue", "symbol": "1 req"}}
        });
        let err = translate_one(&spec, SvaKind::Assert).expect_err("must reject");
        assert!(err.contains("unsupported unary op"), "got: {err}");
    }

    #[test]
    #[ignore = "requires the slang CLI + the M.0 prim_arbiter fixture; run with --ignored"]
    fn e2e_prim_arbiter_real_corpus() {
        use crate::adapter::slang::{locate_slang, run_ast_json};
        use std::path::PathBuf;
        let bin = locate_slang().expect("slang available");
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/verify/m0_opentitan_prim_arbiter/source");
        let json = run_ast_json(&bin, &[dir.join("prim_arbiter_fixed.sv")], &[dir])
            .expect("slang --ast-json on prim_arbiter");
        let report = translate_ast_json(&json).expect("translate");
        eprintln!(
            "prim_arbiter: {} translated, {} unsupported (of {} concurrent assertions)",
            report.translated.len(),
            report.unsupported.len(),
            report.total()
        );
        for u in &report.unsupported {
            eprintln!("  unsupported {}: {}", u.name, u.reason);
        }
        // Real OpenTitan SVA: the boolean-implication properties translate; the
        // reduction-/index-/comparison-heavy ones are honestly reported
        // unsupported (Tier-1 fragment). At least the 13 `ASSERT`s are seen, and
        // some translate, none silently dropped.
        assert!(report.total() >= 13, "all 13 `ASSERT`s should be seen");
        for t in &report.translated {
            crate::mu_calculus::parser::parse(&t.formula).expect("translated formula parses");
        }
    }

    #[test]
    fn disable_iff_gates_the_body() {
        // disable iff (!rst_n) (a)  →  body `((!(rst_n)) || a)`
        let spec = serde_json::json!({
            "kind": "DisableIff",
            "condition": {"kind": "UnaryOp", "op": "LogicalNot",
                          "operand": {"kind": "NamedValue", "symbol": "1 rst_n"}},
            "expr": {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "2 a"}}
        });
        let body = property_body(&spec).expect("translates");
        assert_eq!(body, "((!(rst_n)) || a)");
    }
}
