//! XL.3b — BTOR2 1-step `__past` shadow-register synthesis (Tier-2 SVA history).
//!
//! The slang translator (XL.3a) lowers `$past`/`$stable`/`$changed`/`$rose`/
//! `$fell` to atoms over a shadow signal `<base>__past`, and reports the needed
//! base signals in [`crate::adapter::slang::translate::TranslationReport::required_shadows`].
//! This module is the model half of that contract: it augments the BTOR2 design
//! with a 1-step flop per base so those atoms bind.
//!
//! For each base `b` resolving to a state cell of sort `S`, the augmentation
//! appends (with fresh NIDs above the current max):
//!
//! ```text
//! <n1> state <S> b__past
//! <n2> next  <S> <n1> <b_nid>          ; next(b__past) = b  → b__past(t+1) = b(t)
//! <n3> init  <S> <n1> <b_init_value>   ; only when b itself has an init
//! ```
//!
//! `next(b__past) = b` makes `b__past` hold `b`'s previous-cycle value at every
//! cycle ≥ 1. The `init` mirrors `b`'s own reset value, so at cycle 0
//! `b__past == b`, making `$stable` true / `$rose`/`$fell` false at the first
//! cycle — the standard "no history before the first clock edge" convention.
//! This is **exact** (a real added flop, not an abstraction): the augmented
//! design *is* the concrete system the verdict transfers to.
//!
//! This works at the BTOR2 **text** level (append lines) so it composes with
//! [`crate::adapter::btor2::kmts_lift::predicate_cube_lift`] /
//! [`crate::adapter::btor2::bit_blast`], both of which re-parse the text.

use crate::adapter::btor2::ast::{Nid, Node, Operand};
use crate::adapter::btor2::parser;
use crate::adapter::{AdapterError, AdapterErrorKind};

/// Augment BTOR2 `content` with a 1-step `<base>__past` shadow flop for each
/// base signal in `bases`. Returns the augmented BTOR2 text.
///
/// - A base that already has a `<base>__past` state is skipped (idempotent), so
///   re-augmenting is a no-op.
/// - A base that does not resolve to a BTOR2 state cell is an error (the
///   `$past`/`$stable` atom would never bind) — never silently dropped.
/// - `bases` is deduplicated internally; order of appended flops follows first
///   appearance.
pub fn augment_with_past_shadows(content: &str, bases: &[&str]) -> Result<String, AdapterError> {
    if bases.is_empty() {
        return Ok(content.to_string());
    }

    let file = parser::parse(content).map_err(|mut e| {
        e.message = format!("adapter/btor2/shadow: {}", e.message);
        e
    })?;
    let symbols = parser::collect_symbols(&file);
    let existing_state_symbols: std::collections::HashSet<&str> = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, Node::State { .. }))
        .filter_map(|l| symbols.get(&l.nid).map(String::as_str))
        .collect();

    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut appended: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for &base in bases {
        if !seen.insert(base) {
            continue; // dedup repeated bases
        }
        let shadow_symbol = format!("{base}__past");
        if existing_state_symbols.contains(shadow_symbol.as_str()) {
            continue; // already augmented — idempotent
        }

        let source_nid =
            parser::resolve_state_by_symbol(&file, base).ok_or_else(|| AdapterError {
                kind: AdapterErrorKind::UnsupportedConstruct,
                message: format!(
                    "adapter/btor2/shadow: base signal `{base}` does not resolve to a BTOR2 state \
                 cell, so its `{shadow_symbol}` shadow flop cannot be synthesised (the Tier-2 \
                 history atom would never bind). Check the SVA signal name matches a register."
                ),
                location: None,
            })?;

        // `resolve_state_by_symbol` returns a state NID; read its sort.
        let sort_nid = match file.lookup(source_nid).map(|l| &l.node) {
            Some(Node::State { sort, .. }) => *sort,
            _ => {
                return Err(AdapterError {
                    kind: AdapterErrorKind::UnsupportedConstruct,
                    message: format!(
                        "adapter/btor2/shadow: `{base}` resolved to NID {source_nid}, which is \
                         not a state line; cannot synthesise its shadow flop."
                    ),
                    location: None,
                });
            }
        };

        // Locate the source state's init (if any) to mirror onto the shadow.
        let source_init: Option<Operand> = file.lines.iter().find_map(|l| match &l.node {
            Node::Init { state, value, .. } if *state == source_nid => Some(*value),
            _ => None,
        });

        let shadow_state = next_nid;
        next_nid += 1;
        let shadow_next = next_nid;
        next_nid += 1;

        appended.push(format!("{shadow_state} state {sort_nid} {shadow_symbol}"));
        // next(shadow) = source's current value (referenced by the source NID).
        appended.push(format!(
            "{shadow_next} next {sort_nid} {shadow_state} {source_nid}"
        ));
        if let Some(init) = source_init {
            let shadow_init = next_nid;
            next_nid += 1;
            appended.push(format!(
                "{shadow_init} init {sort_nid} {shadow_state} {}",
                operand_text(init)
            ));
        }
    }

    if appended.is_empty() {
        return Ok(content.to_string());
    }
    Ok(format!("{}\n{}\n", content.trim_end(), appended.join("\n")))
}

/// Render a BTOR2 operand back to text, preserving the `-N` (bit-not) shorthand.
fn operand_text(op: Operand) -> String {
    if op.is_negated() {
        format!("-{}", op.nid())
    } else {
        op.nid().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::ast::Btor2File;
    use std::collections::HashMap;

    // A 4-bit counter `s` that increments each cycle, with `s` reset to 0.
    //   1 sort bitvec 4
    //   2 state 1 s
    //   3 one 1
    //   4 add 1 2 3        ; s + 1
    //   5 init 1 2 ... (zero)
    //   6 next 1 2 4
    const COUNTER: &str = "\
1 sort bitvec 4
2 state 1 s
3 one 1
4 add 1 2 3
5 zero 1
6 init 1 2 5
7 next 1 2 4
";

    #[test]
    fn augment_appends_state_next_init_for_a_base() {
        let out = augment_with_past_shadows(COUNTER, &["s"]).expect("augments");
        let file = parser::parse(&out).expect("augmented btor2 parses");
        let symbols = parser::collect_symbols(&file);
        // A new state `s__past` of the same sort (1) exists.
        let shadow = file
            .lines
            .iter()
            .find(|l| {
                matches!(l.node, Node::State { .. })
                    && symbols.get(&l.nid).map(String::as_str) == Some("s__past")
            })
            .expect("s__past state was appended");
        let Node::State { sort, .. } = shadow.node else {
            unreachable!()
        };
        assert_eq!(sort, 1, "shadow reuses the source's sort");
        // A `next` line links the shadow to the source state (NID 2).
        let has_next = file.lines.iter().any(|l| {
            matches!(
                l.node,
                Node::Next { state, value, .. } if state == shadow.nid && value.nid() == 2
            )
        });
        assert!(has_next, "next(s__past) = s must be present");
        // An `init` line mirrors the source's init (value NID 5, the `zero`).
        let has_init = file.lines.iter().any(|l| {
            matches!(
                l.node,
                Node::Init { state, value, .. } if state == shadow.nid && value.nid() == 5
            )
        });
        assert!(has_init, "init(s__past) must mirror s's init");
    }

    #[test]
    fn shadow_captures_previous_cycle_value() {
        // The soundness proof: after one step from s = 5, the shadow holds 5
        // (s's *previous* value) while s advances to 6.
        let out = augment_with_past_shadows(COUNTER, &["s"]).expect("augments");
        let file = parser::parse(&out).expect("parse");
        let regs = HashMap::from([("s".to_string(), 5u128)]);
        let next =
            crate::adapter::btor2::bit_blast::simulate_one_step(&file, &regs, &HashMap::new())
                .expect("simulate");
        assert_eq!(
            next.get("s__past").copied(),
            Some(5),
            "shadow took s's value"
        );
        assert_eq!(next.get("s").copied(), Some(6), "s advanced (5 + 1)");
    }

    #[test]
    fn cycle0_shadow_equals_source_init() {
        // init mirroring makes $stable true at cycle 0: with s reset to 0, the
        // shadow also resets to 0, so `s == s__past` holds at the first cycle.
        let out = augment_with_past_shadows(COUNTER, &["s"]).expect("augments");
        let file = parser::parse(&out).expect("parse");
        // honor_init: read the init values both cells reset to.
        let s_init = init_value(&file, "s");
        let shadow_init = init_value(&file, "s__past");
        assert_eq!(s_init, shadow_init, "shadow init mirrors source init");
        assert_eq!(s_init, Some(0));
    }

    #[test]
    fn missing_base_is_an_error_not_silent() {
        let err = augment_with_past_shadows(COUNTER, &["nonexistent"]).expect_err("must error");
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
        assert!(
            err.message.contains("nonexistent") && err.message.contains("does not resolve"),
            "error must name the unresolved signal; got: {}",
            err.message
        );
    }

    #[test]
    fn augmentation_is_idempotent() {
        let once = augment_with_past_shadows(COUNTER, &["s"]).expect("augment 1");
        let twice = augment_with_past_shadows(&once, &["s"]).expect("augment 2");
        assert_eq!(once, twice, "re-augmenting an existing shadow is a no-op");
    }

    #[test]
    fn empty_bases_returns_input_unchanged() {
        assert_eq!(augment_with_past_shadows(COUNTER, &[]).unwrap(), COUNTER);
    }

    #[test]
    fn augmented_model_lifts_with_shadow_in_valuations() {
        // The augmentation composes with the explicit-state bit-blast lift: the
        // shadow `s__past` appears as a state-valuation variable, so the
        // evaluator's on-demand `s == s__past` ($stable) comparison atom binds
        // against the lifted Clts. (8 state bits — well under MAX_STATE_BITS.)
        use crate::adapter::AdapterOptions;
        let out = augment_with_past_shadows(COUNTER, &["s"]).expect("augment");
        let lifted = crate::adapter::btor2::bit_blast::translate(&out, &AdapterOptions::default())
            .expect("bit-blast lift of the augmented model");
        let has_shadow = lifted
            .state_valuations
            .values()
            .any(|states| states.values().any(|vars| vars.contains_key("s__past")));
        assert!(
            has_shadow,
            "s__past must appear in the lifted state valuations so $stable binds; got: {:?}",
            lifted.state_valuations
        );
    }

    /// Read the value a state cell initialises to (its `Init` line's value).
    fn init_value(file: &Btor2File, symbol: &str) -> Option<u128> {
        let symbols = parser::collect_symbols(file);
        let state_nid = file.lines.iter().find_map(|l| match &l.node {
            Node::State { .. } if symbols.get(&l.nid).map(String::as_str) == Some(symbol) => {
                Some(l.nid)
            }
            _ => None,
        })?;
        // Read the init operand's constant directly (simulate_one_step seeds
        // states from the caller's map, not from the BTOR2 init lines).
        let init_op = file.lines.iter().find_map(|l| match &l.node {
            Node::Init { state, value, .. } if *state == state_nid => Some(*value),
            _ => None,
        })?;
        match &file.lookup(init_op.nid())?.node {
            Node::Const { value, .. } => Some(const_to_u128(value)),
            _ => None,
        }
    }

    fn const_to_u128(v: &crate::adapter::btor2::ast::ConstValue) -> u128 {
        use crate::adapter::btor2::ast::ConstValue::*;
        match v {
            Zero => 0,
            One => 1,
            Ones => u128::MAX,
            Dec(d) => *d as u128,
            Bin(b) => u128::from_str_radix(b, 2).unwrap_or(0),
            Hex(h) => u128::from_str_radix(h, 16).unwrap_or(0),
        }
    }
}
