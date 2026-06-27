//! XL.1b/XL.1c — Tier-1 SVA → mu-calculus translation over slang's `--ast-json`.
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
//! | `cover property (b)` **recoverability lens** (XL.2) | `nu Y. ((mu X. (b \|\| <> X)) && [] Y)` (AG EF b) |
//! | `disable iff (r) P` | gate the body: `(r \|\| body)` (vacuous while disabled) |
//! | **Tier-2 history (XL.3)** `$stable(x)` | `(x == x__past)` |
//! | `$changed(x)` | `(x != x__past)` |
//! | `$rose(x)` (1-bit) | `(x && !x__past)` |
//! | `$fell(x)` (1-bit) | `(!x && x__past)` |
//! | `$past(x)` (depth 1) | the shadow atom `x__past` (a value; used inside a comparison) |
//!
//! Implication `a → b` is emitted as `!a || b` — the mu-calculus parser has no
//! `->` operator, and the rewrite is exact.
//!
//! **Tier-2 shadow registers (XL.3).** `$past`/`$stable`/`$changed`/`$rose`/
//! `$fell` read a signal's *previous-cycle* value, encoded as an atom over a
//! 1-step shadow `<sig>__past`. The translator emits the atom and records the
//! requirement in [`TranslationReport::required_shadows`]; the BTOR2
//! model-augmentation step (XL.3b) synthesises the shadow flop
//! (`next(<sig>__past) = <sig>`) so the atom binds. The Tier-2 rewrites are
//! exact (added flops, no abstraction). `$past` depth > 1 is rejected.
//!
//! **Atoms (XL.1b + XL.1c).** A boolean leaf is one of:
//! - a 1-bit signal (`NamedValue` → identifier atom);
//! - a multi-bit signal in boolean position (vector → `(sig != 0)`, the implicit
//!   reduction-or SV applies to a vector condition);
//! - a comparison `sig OP k` (`==`/`!=`/`<`/`>`/`<=`/`>=` against an integer);
//! - a **reduction** (XL.1c): `|x`→`(x != 0)`, `~|x`→`(x == 0)`, `&x`→`(x == 2^W-1)`,
//!   `~&x`→`(x != 2^W-1)` (width `W` from the operand's slang type);
//! - a **compare-to-zero of a boolean expr** (XL.1c): `bexpr !== '0`→`bexpr`,
//!   `bexpr === '0`→`!bexpr` — this is how OpenTitan's `` `ASSERT `` macro encodes
//!   `disable iff ((!rst_ni) !== '0)`.
//!
//! Every XL.1c rewrite above is **exact** (semantics-preserving), so it adds no
//! new soundness regime over XL.1b. Boolean structure (`!`, `&&`, `||`) recurses.
//! Anything outside the fragment — bit-arithmetic, bit-select indexing (`sig[i]`),
//! reduction-xor/xnor (parity), system calls (`$onehot0`, `$isunknown`),
//! sequences (`##`, `[*n]`), `$past`, etc. — is **rejected with a reason, never
//! silently dropped** (claims-integrity), and every emitted formula is validated
//! through the mu-calculus parser as a safety net.
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
    /// XL.2 (= Track I.3) recoverability companion — `Some` only for `Cover`
    /// (`EF φ`) assertions. Carries the `AG EF φ` lens
    /// `nu Y. ((mu X. (φ || <>X)) && [] Y)`: "from every reachable state, φ is
    /// still reachable." Surfaces the Track-B recoverability wedge directly from
    /// the design's own covers — a branching question the SVA author could not
    /// phrase. Sound at this νμ alternation only on the must-edge path
    /// (`btor2 cegar --must-edge-inference smt-hyper-must`; cf. the V.7-c csrng
    /// showcase). Forming it is XL.2; auto-checking it on a TRUE cover and
    /// attaching the I.1 countertrace is the XL.6 endpoint's job.
    pub recoverability_companion: Option<String>,
}

/// An assertion outside the Tier-1 fragment, recorded (never silently dropped).
#[derive(Debug, Clone)]
pub struct UnsupportedAssertion {
    pub name: String,
    pub kind: Option<SvaKind>,
    pub reason: String,
}

/// XL.3 (Tier-2): a base signal whose previous-cycle value a translated
/// assertion needs. The BTOR2 model-augmentation step (XL.3b) must synthesise a
/// 1-step shadow flop named `<base>__past` of this width — `next(<base>__past)
/// = <base>` — so the `<base>__past` atoms the translator emits actually bind.
/// Reported only for *successfully* translated assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowSignal {
    pub base: String,
    pub width: u32,
}

/// Result of translating a whole `--ast-json` document.
#[derive(Debug, Clone, Default)]
pub struct TranslationReport {
    pub translated: Vec<TranslatedAssertion>,
    pub unsupported: Vec<UnsupportedAssertion>,
    /// XL.3: 1-step `__past` shadow registers the translated formulas reference
    /// (deduped by base). The XL.3b BTOR2 augmentation consumes this; an empty
    /// vec means no Tier-2 history was used.
    pub required_shadows: Vec<ShadowSignal>,
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
            Ok(formula) => {
                // XL.2: a cover's `EF φ` gets its `AG EF φ` recoverability lens.
                // (Omitted if the companion somehow fails to parse — never ship
                // a formula the evaluator can't read.)
                let recoverability_companion = (kind == SvaKind::Cover)
                    .then(|| recoverability_companion_formula(&formula).ok())
                    .flatten();
                // XL.3: record the `__past` shadow flops this assertion needs
                // (only for assertions that actually translated).
                collect_shadow_signals(spec, &mut report.required_shadows);
                report.translated.push(TranslatedAssertion {
                    name,
                    kind,
                    formula,
                    recoverability_companion,
                });
            }
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

/// XL.3: walk a (translated) assertion's spec and record every base signal a
/// Tier-2 history call (`$past`/`$stable`/`$changed`/`$rose`/`$fell`) reads, so
/// the BTOR2 augmentation knows which `__past` shadow flops to synthesise.
/// Deduped by base name. Only call on specs that translated successfully.
fn collect_shadow_signals(node: &Value, out: &mut Vec<ShadowSignal>) {
    match node {
        Value::Object(map) => {
            if map.get("kind").and_then(Value::as_str) == Some("Call")
                && matches!(
                    map.get("subroutine").and_then(Value::as_str),
                    Some("$past" | "$stable" | "$changed" | "$rose" | "$fell")
                )
                && let Ok((base, width)) = call_arg_signal(node)
                && !out.iter().any(|s| s.base == base)
            {
                out.push(ShadowSignal {
                    base: base.to_string(),
                    width,
                });
            }
            for v in map.values() {
                collect_shadow_signals(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_shadow_signals(v, out);
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

/// XL.2 (= Track I.3): the *recoverability companion* of a cover's `EF φ`.
///
/// Wraps the already-formed `EF φ` (`mu X. (φ || <>X)`) into `AG EF φ`
/// = `nu Y. ((EF φ) && [] Y)` — "from every reachable state, φ is still
/// reachable." The formula is identical in shape to the one the V.7-c csrng
/// recoverability showcase checks by hand; this just forms it automatically
/// from a `cover` assertion. Validated through the mu-calculus parser so a cover
/// only carries a companion the evaluator can actually run.
fn recoverability_companion_formula(ef_formula: &str) -> Result<String, String> {
    let companion = format!("nu Y. (({ef_formula}) && [] Y)");
    crate::mu_calculus::parser::parse(&companion)
        .map_err(|e| format!("recoverability companion failed to parse ({e:?}): {companion}"))?;
    Ok(companion)
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
    let expr = unwrap(expr);
    let k = expr.get("kind").and_then(Value::as_str).unwrap_or("?");
    match k {
        // A bare signal in boolean position: 1-bit → identifier atom; a vector
        // is implicitly reduction-or'd by SV in a condition → `(sig != 0)`.
        "NamedValue" => {
            let name = signal_name(expr)?;
            if signal_width(expr) > 1 {
                Ok(format!("({name} != 0)"))
            } else {
                Ok(name.to_string())
            }
        }
        "UnaryOp" => {
            let op = expr.get("op").and_then(Value::as_str).unwrap_or("?");
            let operand = unwrap(child(expr, "operand")?);
            match op {
                "LogicalNot" => Ok(format!("(!({}))", bool_expr(operand)?)),
                // XL.1c reductions over a vector — exact rewrites to comparisons.
                "BitwiseOr" => reduction_to_cmp(operand, "!=", false), //  |x  → x != 0
                "BitwiseNor" => reduction_to_cmp(operand, "==", false), // ~|x  → x == 0
                "BitwiseAnd" => reduction_to_cmp(operand, "==", true), //  &x  → x == all-ones
                "BitwiseNand" => reduction_to_cmp(operand, "!=", true), // ~&x → x != all-ones
                // `~x` (full bitwise-not): logical-not on a 1-bit operand only.
                "BitwiseNot" => bitwise_not(operand),
                "BitwiseXor" | "BitwiseXnor" => Err(format!(
                    "reduction-{} (parity) is not expressible as a propositional atom",
                    if op == "BitwiseXor" { "xor" } else { "xnor" }
                )),
                other => Err(format!("unsupported unary op: {other}")),
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
                "Equality" | "CaseEquality" => compare(expr, "=="),
                "Inequality" | "CaseInequality" => compare(expr, "!="),
                "LessThan" => compare(expr, "<"),
                "GreaterThan" => compare(expr, ">"),
                "LessThanEqual" => compare(expr, "<="),
                "GreaterThanEqual" => compare(expr, ">="),
                other => Err(format!("unsupported binary op: {other}")),
            }
        }
        // Bit-select `sig[i]` — distinguish constant (needs a bit-level model
        // predicate; H.1 seeding) from dynamic (index is a signal; not a
        // propositional atom at all). Both rejected, never silently dropped.
        "ElementSelect" => {
            let selector = unwrap(child(expr, "selector")?);
            if sv_integer(selector).is_some() {
                Err(
                    "constant bit-select `sig[k]` needs a bit-level model predicate \
                     (H.1 auto-seeding); not a Tier-1c atom"
                        .to_string(),
                )
            } else {
                Err("dynamic bit-select `sig[idx]` (index is a signal) is not \
                     expressible as a propositional atom"
                    .to_string())
            }
        }
        // XL.3 (Tier-2 history). `$stable`/`$changed`/`$rose`/`$fell`/`$past`
        // lower to atoms over a 1-step shadow register `<sig>__past` (the XL.3b
        // BTOR2 model-augmentation step synthesises that flop). All exact.
        "Call" => {
            let sub = expr
                .get("subroutine")
                .and_then(Value::as_str)
                .unwrap_or("?");
            match sub {
                "$stable" | "$changed" => {
                    let (sig, _w) = call_arg_signal(expr)?;
                    let shadow = past_shadow_name(sig);
                    let cmp = if sub == "$stable" { "==" } else { "!=" };
                    Ok(format!("({sig} {cmp} {shadow})"))
                }
                "$rose" | "$fell" => {
                    let (sig, w) = call_arg_signal(expr)?;
                    if w != 1 {
                        return Err(format!(
                            "{sub}(<{w}-bit>) needs a bit-select; Tier-2 supports \
                             {sub} on 1-bit signals only"
                        ));
                    }
                    let shadow = past_shadow_name(sig);
                    Ok(if sub == "$rose" {
                        format!("({sig} && (!({shadow})))") // 1 now, 0 last cycle
                    } else {
                        format!("((!({sig})) && {shadow})") // 0 now, 1 last cycle
                    })
                }
                // `$past(x)` is a value; in boolean position a vector → `!= 0`.
                "$past" => {
                    let (sig, w) = call_arg_signal(expr)?;
                    let shadow = past_shadow_name(sig);
                    Ok(if w > 1 {
                        format!("({shadow} != 0)")
                    } else {
                        shadow
                    })
                }
                other => Err(format!(
                    "system/subroutine call `{other}` (e.g. `$onehot0`, `$isunknown`) \
                     is not in the Tier-1c/Tier-2 fragment"
                )),
            }
        }
        other => Err(format!("unsupported expression kind: {other}")),
    }
}

/// Restructured comparison handler (XL.1c). Accepts, in order:
/// 1. `signal OP literal` → `(sig OP lit)`;
/// 2. `literal OP signal` → `(sig OP' lit)` with the operator flipped;
/// 3. `bool-expr ==/!= 0`  → `!bexpr` / `bexpr` (exact, since `bool != 0 ≡ bool`).
fn compare(expr: &Value, op: &str) -> Result<String, String> {
    let left = unwrap(child(expr, "left")?);
    let right = unwrap(child(expr, "right")?);

    // (1) signal OP literal
    if let (Ok(sig), Some(lit)) = (signal_name(left), sv_integer(right)) {
        return Ok(format!("({sig} {op} {lit})"));
    }
    // (2) literal OP signal  → flip to signal OP' literal
    if let (Some(lit), Ok(sig)) = (sv_integer(left), signal_name(right)) {
        return Ok(format!("({sig} {} {lit})", flip_op(op)));
    }
    // (3) boolean-expr ==/!= 0
    if matches!(op, "==" | "!=") {
        let other = if sv_integer(right) == Some(0) {
            Some(left)
        } else if sv_integer(left) == Some(0) {
            Some(right)
        } else {
            None
        };
        if let Some(other) = other {
            let b = bool_expr(other)?;
            return Ok(if op == "!=" { b } else { format!("(!({b}))") });
        }
    }
    // (4) XL.3 history comparison: `signal OP $past(signal)` (the explicit form
    // of `$stable`/`$changed`). Gated on at least one side being `$past(...)` so
    // a general `signalA OP signalB` (which could mask an unbound named constant)
    // is still conservatively rejected.
    if (is_past_call(left) || is_past_call(right))
        && let (Some(l), Some(r)) = (cmp_signal_atom(left), cmp_signal_atom(right))
    {
        return Ok(format!("({l} {op} {r})"));
    }
    Err(format!(
        "comparison must be `signal {op} literal`, `literal {op} signal`, or a \
         `boolean-expr ==/!= 0` form; got left={:?} right={:?}",
        left.get("kind").and_then(Value::as_str),
        right.get("kind").and_then(Value::as_str),
    ))
}

/// A reduction over a single vector signal → an exact comparison atom.
/// `need_all_ones` selects the `&`/`~&` RHS (`2^W - 1` from the operand width)
/// versus the `|`/`~|` RHS (`0`).
fn reduction_to_cmp(operand: &Value, op: &str, need_all_ones: bool) -> Result<String, String> {
    let sig = signal_name(operand)
        .map_err(|_| format!("reduction `{op}` over a non-signal operand (only `OP signal`)"))?;
    let rhs: i64 = if need_all_ones {
        let w = signal_width(operand);
        if w == 0 || w >= 63 {
            return Err(format!(
                "reduction-and/nand width {w} unusable for an all-ones i64 literal"
            ));
        }
        (1i64 << w) - 1
    } else {
        0
    };
    Ok(format!("({sig} {op} {rhs})"))
}

/// The `__past` shadow-register name for a base signal — the XL.3 naming
/// contract shared with the BTOR2 model-augmentation step (XL.3b synthesises a
/// 1-step flop with `next(<base>__past) = <base>` so the atom binds).
fn past_shadow_name(base: &str) -> String {
    format!("{base}__past")
}

/// True if `expr` (modulo `Conversion` peeling) is a `$past(...)` call.
fn is_past_call(expr: &Value) -> bool {
    let expr = unwrap(expr);
    expr.get("kind").and_then(Value::as_str) == Some("Call")
        && expr.get("subroutine").and_then(Value::as_str) == Some("$past")
}

/// A comparison operand that resolves to a mu-calc atom name: a plain signal, or
/// `$past(signal)` → its `__past` shadow. `None` for anything else.
fn cmp_signal_atom(expr: &Value) -> Option<String> {
    let expr = unwrap(expr);
    if is_past_call(expr) {
        return call_arg_signal(expr)
            .ok()
            .map(|(sig, _)| past_shadow_name(sig));
    }
    signal_name(expr).ok().map(|s| s.to_string())
}

/// A Tier-2 history call (`$past`/`$stable`/`$changed`/`$rose`/`$fell`) takes
/// exactly one signal argument; return its `(name, width)`. Rejects depth>1
/// `$past` (≥2 args), multi-arg forms, and non-signal arguments.
fn call_arg_signal(call: &Value) -> Result<(&str, u32), String> {
    let sub = call
        .get("subroutine")
        .and_then(Value::as_str)
        .unwrap_or("$?");
    let args = call
        .get("arguments")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{sub} without an argument list"))?;
    if args.len() != 1 {
        return Err(format!(
            "{sub} with {} arguments; Tier-2 supports single-signal depth-1 history only",
            args.len()
        ));
    }
    let arg = unwrap(&args[0]);
    let sig = signal_name(arg).map_err(|_| format!("{sub} argument is not a plain signal"))?;
    Ok((sig, signal_width(arg)))
}

/// `~x` in boolean position: logical-not on a 1-bit operand; a vector `~x` is a
/// value transform, not a boolean, so it is rejected.
fn bitwise_not(operand: &Value) -> Result<String, String> {
    if signal_width(operand) == 1 {
        Ok(format!("(!({}))", bool_expr(operand)?))
    } else {
        Err(format!(
            "bitwise-not `~` over a {}-bit vector is a value transform, not a boolean",
            signal_width(operand)
        ))
    }
}

/// Peel slang `Conversion` (width-cast) wrappers to the underlying expression.
fn unwrap(mut e: &Value) -> &Value {
    while e.get("kind").and_then(Value::as_str) == Some("Conversion") {
        match e.get("operand") {
            Some(o) => e = o,
            None => break,
        }
    }
    e
}

/// Bit-width of an expression from its slang `type` string. `logic[7:0]`→8,
/// `logic`/`bit`/`reg`→1, unknown→1 (the safe scalar default).
fn signal_width(expr: &Value) -> u32 {
    let Some(ty) = expr.get("type").and_then(Value::as_str) else {
        return 1;
    };
    match (ty.find('['), ty.find(']')) {
        (Some(l), Some(r)) if r > l => {
            let inner = &ty[l + 1..r];
            if let Some((hi, lo)) = inner.split_once(':')
                && let (Ok(hi), Ok(lo)) = (hi.trim().parse::<i64>(), lo.trim().parse::<i64>())
            {
                return (hi - lo).unsigned_abs() as u32 + 1;
            }
            1
        }
        _ => 1,
    }
}

/// Flip a comparison operator (for the `literal OP signal` normalisation).
fn flip_op(op: &str) -> &str {
    match op {
        "<" => ">",
        ">" => "<",
        "<=" => ">=",
        ">=" => "<=",
        other => other,
    }
}

/// Extract the signal name from a `NamedValue` (`"symbol": "<id> name"`),
/// peeling `Conversion` wrappers first.
fn signal_name(expr: &Value) -> Result<&str, String> {
    let expr = unwrap(expr);
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

/// Parse an integer-literal expression to its value. Handles plain decimal,
/// based (`8'd5`, `4'hF`, `3'o7`, `1'b0`) and unbased-unsized (`'0`/`'1`,
/// serialised by slang as `1'b0`/`1'b1`) forms. `Conversion`-wrapped literals
/// are peeled first.
fn sv_integer(expr: &Value) -> Option<i64> {
    let expr = unwrap(expr);
    let k = expr.get("kind").and_then(Value::as_str)?;
    if !k.contains("IntegerLiteral") {
        return None;
    }
    let raw = expr
        .get("constant")
        .or_else(|| expr.get("value"))
        .and_then(Value::as_str)?;
    parse_sv_literal(raw)
}

/// Parse SystemVerilog integer-literal text. `"8"`→8, `"8'd5"`→5, `"1'b0"`→0,
/// `"4'hff"`→255, `"3'b101"`→5. Returns `None` for X/Z digits or malformed text.
fn parse_sv_literal(raw: &str) -> Option<i64> {
    let s: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_')
        .collect();
    let Some(tick) = s.find('\'') else {
        return s.parse::<i64>().ok();
    };
    // After the tick: optional signedness (`s`/`S`), then a base char, then digits.
    let rest = &s[tick + 1..];
    let rest = rest.strip_prefix(['s', 'S']).unwrap_or(rest);
    let mut chars = rest.chars();
    let base = chars.next()?;
    let digits: String = chars.collect();
    let radix = match base.to_ascii_lowercase() {
        'b' => 2,
        'o' => 8,
        'd' => 10,
        'h' => 16,
        _ => return None,
    };
    i64::from_str_radix(&digits, radix).ok()
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
    // Frozen `slang --ast-json` snapshot of the 5 Tier-2 history forms
    // ($stable / $changed / $rose / $fell / $past). Regenerate via:
    //   slang --ast-json testdata/tier2.ast.json testdata/tier2.sv --single-unit
    const TIER2_JSON: &str = include_str!("testdata/tier2.ast.json");

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
        // Reduction-xor `^x` (parity) is out of the fragment — not expressible
        // as a propositional atom, so it must be rejected (never dropped).
        let spec = serde_json::json!({
            "kind": "Simple",
            "expr": {"kind": "UnaryOp", "op": "BitwiseXor",
                     "operand": {"kind": "NamedValue", "symbol": "1 req", "type": "logic[7:0]"}}
        });
        let err = translate_one(&spec, SvaKind::Assert).expect_err("must reject");
        assert!(err.contains("parity"), "got: {err}");
    }

    #[test]
    fn xl1c_reduction_or_becomes_nonzero_comparison() {
        // `|gnt_o |-> ready_i` — the reduction-or LHS → `(gnt_o != 0)`.
        let spec = serde_json::json!({
            "kind": "Binary",
            "op": "OverlappedImplication",
            "left": {"kind": "Simple", "expr":
                {"kind": "UnaryOp", "op": "BitwiseOr",
                 "operand": {"kind": "NamedValue", "symbol": "1 gnt_o", "type": "logic[7:0]"}}},
            "right": {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "2 ready_i"}}
        });
        let f = translate_one(&spec, SvaKind::Assert).expect("translates");
        assert!(f.contains("(gnt_o != 0)"), "reduction-or → != 0; got {f}");
        assert!(f.contains("ready_i"));
        crate::mu_calculus::parser::parse(&f).expect("parses");
    }

    #[test]
    fn xl1c_reduction_and_uses_operand_width_all_ones() {
        // `&x` over logic[7:0] → `(x == 255)`.
        let expr = serde_json::json!({
            "kind": "UnaryOp", "op": "BitwiseAnd",
            "operand": {"kind": "NamedValue", "symbol": "1 x", "type": "logic[7:0]"}
        });
        assert_eq!(bool_expr(&expr).unwrap(), "(x == 255)");
        // `~&x` → `(x != 255)`.
        let nand = serde_json::json!({
            "kind": "UnaryOp", "op": "BitwiseNand",
            "operand": {"kind": "NamedValue", "symbol": "1 x", "type": "logic[2:0]"}
        });
        assert_eq!(bool_expr(&nand).unwrap(), "(x != 7)");
    }

    #[test]
    fn xl1c_disable_iff_macro_form_translates() {
        // The OpenTitan `ASSERT` macro encodes `disable iff ((!rst_ni) !== '0)`:
        //   condition = CaseInequality((!rst_ni), '0)  →  `(!(rst_ni))`.
        let spec = serde_json::json!({
            "kind": "DisableIff",
            "condition": {
                "kind": "BinaryOp", "op": "CaseInequality",
                "left": {"kind": "UnaryOp", "op": "LogicalNot",
                         "operand": {"kind": "NamedValue", "symbol": "1 rst_ni", "type": "logic"}},
                "right": {"kind": "UnbasedUnsizedIntegerLiteral", "value": "1'b0"}
            },
            "expr": {"kind": "Simple", "expr": {"kind": "NamedValue", "symbol": "2 a"}}
        });
        let body = property_body(&spec).expect("translates");
        assert_eq!(body, "((!(rst_ni)) || a)");
    }

    #[test]
    fn xl1c_sv_literal_parsing() {
        assert_eq!(parse_sv_literal("8"), Some(8));
        assert_eq!(parse_sv_literal("1'b0"), Some(0)); // the `'0` form (was the digit-filter bug)
        assert_eq!(parse_sv_literal("8'd5"), Some(5));
        assert_eq!(parse_sv_literal("4'hff"), Some(255));
        assert_eq!(parse_sv_literal("3'b101"), Some(5));
        assert_eq!(parse_sv_literal("16'sd42"), Some(42));
        assert_eq!(parse_sv_literal("garbage"), None);
    }

    #[test]
    fn xl1c_signal_width_from_type() {
        let mk = |t: &str| serde_json::json!({"kind": "NamedValue", "symbol": "1 s", "type": t});
        assert_eq!(signal_width(&mk("logic[7:0]")), 8);
        assert_eq!(signal_width(&mk("logic[2:0]")), 3);
        assert_eq!(signal_width(&mk("logic")), 1);
        assert_eq!(signal_width(&mk("bit")), 1);
    }

    #[test]
    fn xl1c_dynamic_bit_select_is_rejected_with_reason() {
        // `req_i[idx_o]` — selector is a signal → dynamic, not a Tier-1c atom.
        let expr = serde_json::json!({
            "kind": "ElementSelect",
            "selector": {"kind": "NamedValue", "symbol": "1 idx_o", "type": "logic[2:0]"},
            "value": {"kind": "NamedValue", "symbol": "2 req_i", "type": "logic[7:0]"}
        });
        let err = bool_expr(&expr).expect_err("dynamic index must reject");
        assert!(err.contains("dynamic bit-select"), "got: {err}");
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
        // Real OpenTitan SVA. XL.1c lifts the reduction-or / disable-iff-macro
        // properties (`|gnt_o |-> ready_i`, etc.); the bit-arithmetic, dynamic
        // bit-select, and `$onehot0`/`$isunknown` ones stay honestly unsupported.
        assert!(
            report.total() >= 13,
            "all 13 concurrent assertions should be seen"
        );
        assert!(
            report.translated.len() >= 6,
            "XL.1c should translate the 6 reduction/comparison ASSERTs \
             (GntImpliesReady/Valid, ReqAndReadyImplyGrant, ReqImpliesValid, \
             ReadyAndValidImplyGrant, NoReadyValidNoGrant); got {} translated, \
             unsupported: {:?}",
            report.translated.len(),
            report
                .unsupported
                .iter()
                .map(|u| &u.reason)
                .collect::<Vec<_>>()
        );
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

    #[test]
    fn xl2_cover_gets_recoverability_companion() {
        // The Tier-1 fixture's `cover property (a && b)` → EF, plus the AG-EF lens.
        let report = translate_ast_json(TIER1_JSON).expect("valid ast-json");
        let cover = report
            .translated
            .iter()
            .find(|t| t.kind == SvaKind::Cover)
            .expect("fixture has a cover");
        let companion = cover
            .recoverability_companion
            .as_ref()
            .expect("a cover carries its recoverability companion");
        // AG EF shape: outer nu over the cover's EF (mu), closed with `[] Y`.
        assert!(companion.starts_with("nu Y. (("), "got {companion}");
        assert!(
            companion.contains(&cover.formula),
            "companion embeds the EF formula"
        );
        assert!(companion.ends_with(") && [] Y)"), "got {companion}");
        crate::mu_calculus::parser::parse(companion).expect("companion parses");
    }

    #[test]
    fn xl2_non_cover_assertions_have_no_companion() {
        // EF→AG-EF only makes sense for covers; asserts/assumes are AG-shaped.
        let report = translate_ast_json(TIER1_JSON).expect("valid ast-json");
        for t in report
            .translated
            .iter()
            .filter(|t| t.kind != SvaKind::Cover)
        {
            assert!(
                t.recoverability_companion.is_none(),
                "{} ({:?}) must not carry a recoverability companion",
                t.name,
                t.kind
            );
        }
    }

    #[test]
    fn xl2_companion_matches_v7c_recoverability_shape() {
        // The V.7-c csrng showcase checks `nu Y. ((mu X. (idle || <> X)) && [] Y)`
        // by hand. XL.2 forms the same AG-EF shape from `cover property (state_q == 55)`.
        let ef = translate_one(
            &serde_json::json!({
                "kind": "Simple",
                "expr": {"kind": "BinaryOp", "op": "Equality",
                         "left":  {"kind": "NamedValue", "symbol": "1 state_q", "type": "logic[5:0]"},
                         "right": {"kind": "IntegerLiteral", "value": "55", "constant": "55"}}
            }),
            SvaKind::Cover,
        )
        .expect("cover translates");
        let companion = recoverability_companion_formula(&ef).expect("companion forms + parses");
        assert_eq!(
            companion,
            "nu Y. ((mu X. (((state_q == 55)) || <> X)) && [] Y)"
        );
    }

    // --- XL.3 (Tier-2 history) -------------------------------------------

    /// Helper: a `$<sub>(<sig>:<width>)` Call expression for the unit tests.
    fn history_call(sub: &str, sig: &str, ty: &str) -> Value {
        serde_json::json!({
            "kind": "Call", "subroutine": sub,
            "arguments": [{"kind": "NamedValue", "symbol": format!("1 {sig}"), "type": ty}]
        })
    }

    #[test]
    fn xl3_stable_and_changed_compare_against_shadow() {
        assert_eq!(
            bool_expr(&history_call("$stable", "state_q", "logic[5:0]")).unwrap(),
            "(state_q == state_q__past)"
        );
        assert_eq!(
            bool_expr(&history_call("$changed", "state_q", "logic[5:0]")).unwrap(),
            "(state_q != state_q__past)"
        );
    }

    #[test]
    fn xl3_rose_fell_one_bit_edges() {
        assert_eq!(
            bool_expr(&history_call("$rose", "v", "logic")).unwrap(),
            "(v && (!(v__past)))"
        );
        assert_eq!(
            bool_expr(&history_call("$fell", "v", "logic")).unwrap(),
            "((!(v)) && v__past)"
        );
    }

    #[test]
    fn xl3_rose_on_vector_is_rejected() {
        let err = bool_expr(&history_call("$rose", "bus", "logic[7:0]")).expect_err("reject");
        assert!(err.contains("1-bit"), "got: {err}");
    }

    #[test]
    fn xl3_past_in_comparison_resolves_to_shadow() {
        // `state_q == $past(state_q)` (the explicit $stable form).
        let cmp = serde_json::json!({
            "kind": "BinaryOp", "op": "Equality",
            "left":  {"kind": "NamedValue", "symbol": "1 state_q", "type": "logic[5:0]"},
            "right": history_call("$past", "state_q", "logic[5:0]")
        });
        assert_eq!(bool_expr(&cmp).unwrap(), "(state_q == state_q__past)");
    }

    #[test]
    fn xl3_past_depth_gt_1_is_rejected() {
        // `$past(x, 2)` — two arguments → out of Tier-2 (depth-1 only).
        let call = serde_json::json!({
            "kind": "Call", "subroutine": "$past",
            "arguments": [
                {"kind": "NamedValue", "symbol": "1 x", "type": "logic"},
                {"kind": "IntegerLiteral", "value": "2", "constant": "2"}
            ]
        });
        let err = bool_expr(&call).expect_err("depth>1 must reject");
        assert!(err.contains("depth-1"), "got: {err}");
    }

    #[test]
    fn xl3_onehot_isunknown_still_rejected() {
        // Tier-2 only adds the history calls; other system calls stay rejected.
        let call = serde_json::json!({
            "kind": "Call", "subroutine": "$onehot0",
            "arguments": [{"kind": "NamedValue", "symbol": "1 gnt", "type": "logic[7:0]"}]
        });
        let err = bool_expr(&call).expect_err("must reject");
        assert!(
            err.contains("$onehot0") || err.contains("not in the"),
            "got: {err}"
        );
    }

    #[test]
    fn xl3_tier2_fixture_translates_all_five_and_records_shadows() {
        let report = translate_ast_json(TIER2_JSON).expect("valid ast-json");
        assert_eq!(report.total(), 5, "fixture has 5 Tier-2 assertions");
        assert_eq!(
            report.unsupported.len(),
            0,
            "all 5 should translate; unsupported: {:?}",
            report.unsupported
        );
        for t in &report.translated {
            crate::mu_calculus::parser::parse(&t.formula)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e:?}", t.name));
        }
        // Shadows: state_q (width 6, from $stable/$changed/$past) + v (width 1,
        // from $rose/$fell), deduped.
        let mut shadows = report.required_shadows.clone();
        shadows.sort_by(|a, b| a.base.cmp(&b.base));
        assert_eq!(
            shadows,
            vec![
                ShadowSignal {
                    base: "state_q".into(),
                    width: 6
                },
                ShadowSignal {
                    base: "v".into(),
                    width: 1
                },
            ],
            "required_shadows must dedup by base + carry width"
        );
    }

    #[test]
    fn xl3_no_shadows_when_no_history() {
        // The Tier-1 fixture uses no history calls → empty shadow set.
        let report = translate_ast_json(TIER1_JSON).expect("valid ast-json");
        assert!(report.required_shadows.is_empty());
    }
}
