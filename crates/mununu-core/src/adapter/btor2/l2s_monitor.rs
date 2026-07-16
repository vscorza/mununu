//! P2 — liveness-to-safety `bad` monitor for the response property
//! `AG(a → AF b)` (Biere–Artho–Schuppan).
//!
//! The classifier [`crate::adapter::liveness_rescue::reduce_response_af`] recognises
//! the property; this module emits the `bad` line that makes its violation — a
//! **reachable `b`-free cycle carrying an outstanding request** — a reachability
//! query the portfolio decides. Like [`crate::adapter::btor2::bad_monitor`] and
//! [`crate::adapter::btor2::shadow`], it works at the BTOR2 **text** level: it
//! appends lines with fresh NIDs above the current max, so it composes with the
//! bit-blaster / lift / model checker that re-parse the text.
//!
//! # The construction
//!
//! Let `a`, `b` be the request / grant atoms. The augmentation adds:
//!
//! - `pending` (latch, init 0): `next = (pending ∨ a) ∧ ¬b` — an outstanding
//!   request, set by `a`, cleared by `b`.
//! - `save` (fresh 1-bit input) and `saved` (latch, init 0): a nondeterministic,
//!   at-most-once snapshot trigger — `save_en = save ∧ ¬saved ∧ pending` (only
//!   snapshot when a request is outstanding); `next(saved) = saved ∨ save_en`.
//! - a **shadow** copy `sh_i` of every original state cell `s_i`:
//!   `next(sh_i) = save_en ? s_i : sh_i` (no init — free, only read after a save).
//! - `looped = saved ∧ ⋀_i (s_i == sh_i)` — the snapshotted state has recurred, so
//!   a cycle closed.
//! - `b_seen` (latch, init 0): whether `b` held anywhere on the loop —
//!   `next = save_en ? b : (saved ? (b_seen ∨ b) : 0)`.
//! - `bad = looped ∧ ¬b_seen` (the `and` carries [`bad_monitor::BAD_COND_SYMBOL`]).
//!
//! A reachable `bad` ⇒ a reachable cycle on which an outstanding request is never
//! granted ⇒ `AG(a → AF b)` **VIOLATED**; an unreachable `bad` (a portfolio safety
//! proof) ⇒ **HOLDS**. The construction is **sound** (a `bad` witness is a genuine
//! `b`-free lasso through a pending request) and **complete** (a real violation has
//! a suffix cycle on which `pending` stays 1 and `b` never holds, which the
//! nondeterministic `save` can snapshot).

use crate::adapter::btor2::ast::{Nid, Node};
use crate::adapter::btor2::bad_monitor::{
    BAD_COND_SYMBOL, btor2_cmp_keyword, find_or_make_bool_sort, input_nid_and_sort,
    output_nid_and_sort, state_nid_and_sort,
};
use crate::adapter::btor2::parser;
use crate::adapter::btor2::predicate_expr::CmpOp;
use crate::adapter::{AdapterError, AdapterErrorKind};

fn err(message: String) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::UnsupportedConstruct,
        message,
        location: None,
    }
}

/// A BTOR2-text line appender that hands out fresh NIDs above the design's max.
struct Emit {
    next: Nid,
    lines: Vec<String>,
}

impl Emit {
    /// Append `"<fresh-nid> <body>"` and return the fresh NID.
    fn push(&mut self, body: impl AsRef<str>) -> Nid {
        let nid = self.next;
        self.next += 1;
        self.lines.push(format!("{nid} {}", body.as_ref()));
        nid
    }
}

/// Emit the request / grant atom `signal ⋈ value` as a 1-bit node, returning its
/// NID. `signal` may resolve to a state cell (value-alias / reset-mux aware) or a
/// primary input.
fn emit_atom(
    file: &crate::adapter::btor2::ast::Btor2File,
    e: &mut Emit,
    bool_sort: Nid,
    atom: (&str, CmpOp, u128),
    reset_pinned: bool,
    role: &str,
) -> Result<Nid, AdapterError> {
    let (signal, op, value) = atom;
    // Resolve against a state cell, a primary input, or a named output port. The
    // output case (registered grants like `level1` / `q`) is essential: the standard
    // lift keeps port names but drops state-cell names, so a grant signal surfaces
    // only as an `output` — comparing its driver net per-cycle is a sound observation.
    let (nid, sort) = state_nid_and_sort(file, signal, reset_pinned)
        .or_else(|| input_nid_and_sort(file, signal))
        .or_else(|| output_nid_and_sort(file, signal))
        .ok_or_else(|| {
            err(format!(
                "adapter/btor2/l2s_monitor: {role} `{signal}` is not a state cell, a primary \
                 input, or a named output, so the response monitor cannot bind it"
            ))
        })?;
    let cst = e.push(format!("constd {sort} {value}"));
    Ok(e.push(format!("{} {bool_sort} {nid} {cst}", btor2_cmp_keyword(op))))
}

/// Append the liveness-to-safety `bad` monitor for `AG(ante → AF cons)` to `content`.
///
/// See the module docs for the construction. Returns an error when an atom does not
/// resolve to a signal.
pub fn emit_response_l2s_monitor(
    content: &str,
    ante: (&str, CmpOp, u128),
    cons: (&str, CmpOp, u128),
    reset_pinned: bool,
) -> Result<String, AdapterError> {
    let file = parser::parse(content).map_err(|mut e| {
        e.message = format!("adapter/btor2/l2s_monitor: {}", e.message);
        e
    })?;

    // Original state cells (before augmentation): the shadow copies these.
    let states: Vec<(Nid, Nid)> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::State { sort, .. } => Some((l.nid, *sort)),
            _ => None,
        })
        .collect();

    let next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut e = Emit {
        next: next_nid,
        lines: Vec::new(),
    };
    let mut prelude: Vec<String> = Vec::new();
    let bool_sort = find_or_make_bool_sort(&file, &mut e.next, &mut prelude);
    e.lines.extend(prelude);

    // Request / grant atoms.
    let a = emit_atom(&file, &mut e, bool_sort, ante, reset_pinned, "antecedent")?;
    let b = emit_atom(&file, &mut e, bool_sort, cons, reset_pinned, "consequent")?;

    // A fresh `zero` of the boolean sort for the 1-bit latch inits.
    let zero1 = e.push(format!("zero {bool_sort}"));

    // pending: next = (pending || a) && !b.
    let pending = e.push(format!("state {bool_sort} mununu_l2s_pending"));
    e.push(format!("init {bool_sort} {pending} {zero1}"));
    let p_or_a = e.push(format!("or {bool_sort} {pending} {a}"));
    let not_b = e.push(format!("not {bool_sort} {b}"));
    let next_pending = e.push(format!("and {bool_sort} {p_or_a} {not_b}"));
    e.push(format!("next {bool_sort} {pending} {next_pending}"));

    // save input + saved latch + save_en = save && !saved && pending.
    let saved = e.push(format!("state {bool_sort} mununu_l2s_saved"));
    e.push(format!("init {bool_sort} {saved} {zero1}"));
    let save_in = e.push(format!("input {bool_sort} mununu_l2s_save"));
    let not_saved = e.push(format!("not {bool_sort} {saved}"));
    let save_np = e.push(format!("and {bool_sort} {save_in} {not_saved}"));
    let save_en = e.push(format!("and {bool_sort} {save_np} {pending}"));
    let next_saved = e.push(format!("or {bool_sort} {saved} {save_en}"));
    e.push(format!("next {bool_sort} {saved} {next_saved}"));

    // Shadow copy of each original state cell + per-cell equality.
    let mut eq_terms: Vec<Nid> = Vec::new();
    for (s_nid, s_sort) in &states {
        let sh = e.push(format!("state {s_sort} mununu_l2s_sh_{s_nid}"));
        // next(sh) = ite(save_en, s, sh); no init — free, only read after a save.
        let nx = e.push(format!("ite {s_sort} {save_en} {s_nid} {sh}"));
        e.push(format!("next {s_sort} {sh} {nx}"));
        eq_terms.push(e.push(format!("eq {bool_sort} {s_nid} {sh}")));
    }
    // all_eq = ⋀ eq_terms; the empty conjunction (stateless design) is `one`.
    let all_eq = if let Some((&first, rest)) = eq_terms.split_first() {
        rest.iter().fold(first, |acc, &t| {
            e.push(format!("and {bool_sort} {acc} {t}"))
        })
    } else {
        e.push(format!("one {bool_sort}"))
    };
    let looped = e.push(format!("and {bool_sort} {saved} {all_eq}"));

    // b_seen: next = ite(save_en, b, ite(saved, b_seen || b, 0)).
    let b_seen = e.push(format!("state {bool_sort} mununu_l2s_bseen"));
    e.push(format!("init {bool_sort} {b_seen} {zero1}"));
    let bseen_or_b = e.push(format!("or {bool_sort} {b_seen} {b}"));
    let inner = e.push(format!("ite {bool_sort} {saved} {bseen_or_b} {zero1}"));
    let next_bseen = e.push(format!("ite {bool_sort} {save_en} {b} {inner}"));
    e.push(format!("next {bool_sort} {b_seen} {next_bseen}"));

    // bad = looped && !b_seen — a b-free closed loop carrying a pending request.
    let not_bseen = e.push(format!("not {bool_sort} {b_seen}"));
    let bad_cond = e.push(format!(
        "and {bool_sort} {looped} {not_bseen} {BAD_COND_SYMBOL}"
    ));
    e.push(format!("bad {bad_cond}"));

    Ok(format!("{}\n{}\n", content.trim_end(), e.lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 2-state request/grant FSM: `st` 0=idle, 1=busy. Input `req` drives idle→busy;
    // busy→idle unconditionally, asserting `grant` (grant = st==1). `req` pending in
    // idle is always granted next cycle ⇒ AG(req→AF grant) HOLDS.
    const RESPONDER: &str = "\
1 sort bitvec 1
2 state 1 st
3 zero 1
4 init 1 2 3
5 input 1 req
6 one 1
7 ite 1 2 3 5
8 next 1 2 7
";

    #[test]
    fn emitted_monitor_parses_and_has_bad_and_latches() {
        // req atom = (st == 0) proxy via input `req`; grant = (st == 1).
        let out = emit_response_l2s_monitor(
            RESPONDER,
            ("req", CmpOp::Eq, 1),
            ("st", CmpOp::Eq, 1),
            false,
        )
        .expect("emit");
        let file = parser::parse(&out).expect("emitted BTOR2 re-parses");
        // The monitor added the l2s latches + a bad line.
        assert!(out.contains("mununu_l2s_pending"), "pending latch present");
        assert!(out.contains("mununu_l2s_saved"), "saved latch present");
        assert!(out.contains("mununu_l2s_save"), "save input present");
        assert!(out.contains("mununu_l2s_bseen"), "b_seen latch present");
        assert!(
            out.contains("mununu_l2s_sh_2"),
            "shadow of state cell 2 present"
        );
        assert!(
            file.lines
                .iter()
                .any(|l| matches!(l.node, Node::Bad { .. })),
            "a bad line is present"
        );
    }

    #[test]
    fn unresolvable_atom_errors() {
        let e = emit_response_l2s_monitor(
            RESPONDER,
            ("nonexistent", CmpOp::Eq, 1),
            ("st", CmpOp::Eq, 1),
            false,
        );
        assert!(e.is_err(), "an atom that binds no signal must error");
    }

    // The real-lift shape: `flatten; opt_clean; dffunmap` keeps port names but drops
    // state-cell names, so a registered grant surfaces only as a named `output` over
    // an *unnamed* state. Binding the grant atom must follow the output to its driver.
    const REGISTERED_OUTPUT: &str = "\
1 sort bitvec 1
2 input 1 req
3 state 1
4 zero 1
5 init 1 3 4
6 next 1 3 2
7 output 3 grant
";

    #[test]
    fn binds_registered_output_grant() {
        // `grant` is a named output over the unnamed state 3; `req` is a primary input.
        let out = emit_response_l2s_monitor(
            REGISTERED_OUTPUT,
            ("req", CmpOp::Eq, 1),
            ("grant", CmpOp::Eq, 1),
            false,
        )
        .expect("a registered-output grant must bind via output_nid_and_sort");
        let file = parser::parse(&out).expect("emitted BTOR2 re-parses");
        assert!(
            file.lines
                .iter()
                .any(|l| matches!(l.node, Node::Bad { .. })),
            "the monitor emits a bad line"
        );
        // The grant comparison binds the output's driver (state 3), not the output line.
        assert!(
            out.contains("eq 1 3 "),
            "grant compares against driver nid 3"
        );
    }
}
