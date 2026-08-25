//! XL.3b — BTOR2 1-step `__past` shadow-register synthesis (Tier-2 SVA history).
//!
//! The slang translator (XL.3a) lowers `$past`/`$stable`/`$changed`/`$rose`/
//! `$fell` to atoms over a shadow signal `<base>__past`, and reports the needed
//! base signals in [`crate::adapter::slang::translate::TranslationReport::required_shadows`].
//! This module is the model half of that contract: it augments the BTOR2 design
//! with a 1-step flop per base so those atoms bind.
//!
//! For each base `b` resolving to a state cell **or a primary input** of sort
//! `S`, the augmentation appends (with fresh NIDs above the current max):
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
//!
//! A **primary input** base gets the same flop with no `init`: an input has no
//! reset value to mirror, so `b__past` is unconstrained at cycle 0. That is the
//! faithful reading of `$past` before the first clock edge, which SVA leaves
//! undefined. It also matters in practice — `$past` of an input is what every
//! data-integrity property is written over:
//!
//! ```systemverilog
//! (push && !pop && cnt_q == 0) |=> (d0_q == $past(din))
//! ```
//!
//! Restricting the base to state cells made that whole class unverifiable: the
//! translator declared `required __past shadow registers: din(8)`, nothing built
//! `din__past`, and every engine then correctly reported
//! `predicate references unknown register/signal`. The restriction was
//! conservative rather than semantic — `next` sourced from an `input` node is
//! well-formed BTOR2 and the engines consume it unchanged.
//! This is **exact** (a real added flop, not an abstraction): the augmented
//! design *is* the concrete system the verdict transfers to.
//!
//! This works at the BTOR2 **text** level (append lines) so it composes with
//! [`crate::adapter::btor2::kmts_lift::predicate_cube_lift`] /
//! [`crate::adapter::btor2::bit_blast`], both of which re-parse the text.

use crate::adapter::btor2::ast::{Btor2File, Nid, Node, Operand};
use crate::adapter::btor2::parser;
use crate::adapter::{AdapterError, AdapterErrorKind};

/// Augment BTOR2 `content` with a 1-step `<base>__past` shadow flop for each
/// base signal in `bases`. Returns the augmented BTOR2 text.
///
/// - A base that already has a `<base>__past` state is skipped (idempotent), so
///   re-augmenting is a no-op.
/// - A base resolves to a state cell or a primary input. A state cell's `init`
///   is mirrored onto the shadow; an input has none to mirror, so its shadow is
///   unconstrained at cycle 0 (SVA leaves `$past` undefined there).
/// - A base that resolves to neither is an error (the `$past`/`$stable` atom
///   would never bind) — never silently dropped.
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

        // A base is either a state cell or a primary input. Both are legitimate
        // `$past` sources and both produce the same `next(shadow) = source`
        // flop; they differ only in where the shadow's cycle-0 value comes from.
        let source = resolve_shadow_source(&file, base).ok_or_else(|| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/btor2/shadow: base signal `{base}` resolves to neither a BTOR2 state \
                 cell nor a primary input, so its `{shadow_symbol}` shadow flop cannot be \
                 synthesised (the Tier-2 history atom would never bind). Check the SVA signal \
                 name matches a register or a module input."
            ),
            location: None,
        })?;
        let sort_nid = source.sort();

        // btormc's parser requires an `init` value's NID to be BELOW the state's,
        // so the zero an input shadow inits from is allocated FIRST — everything
        // appended here sits above every pre-existing NID, and allocating it after
        // the state would emit a model the external oracle rejects while mununu's
        // own order-agnostic parser read it happily. A state cell's init needs no
        // such care: it reuses the source's own value node, already far below.
        let mut zero_nid = None;
        if matches!(source, ShadowSource::PrimaryInput { .. }) {
            zero_nid = Some(next_nid);
            appended.push(format!("{next_nid} zero {sort_nid}"));
            next_nid += 1;
        }

        let shadow_state = next_nid;
        next_nid += 1;
        let shadow_next = next_nid;
        next_nid += 1;

        appended.push(format!("{shadow_state} state {sort_nid} {shadow_symbol}"));
        // next(shadow) = source's current value (referenced by the source NID).
        appended.push(format!(
            "{shadow_next} next {sort_nid} {shadow_state} {}",
            source.nid()
        ));

        match source {
            // Mirror the state cell's init so cycle 0 has `shadow == source` — the
            // "no history before the first clock edge" convention. A source with
            // no init leaves the shadow init-less too, which keeps the pair
            // consistent under whatever a given engine does with an init-less
            // cell: they move together or not at all.
            ShadowSource::StateCell { nid, .. } => {
                let source_init = file.lines.iter().find_map(|l| match &l.node {
                    Node::Init { state, value, .. } if *state == nid => Some(*value),
                    _ => None,
                });
                if let Some(init) = source_init {
                    let shadow_init = next_nid;
                    next_nid += 1;
                    appended.push(format!(
                        "{shadow_init} init {sort_nid} {shadow_state} {}",
                        operand_text(init)
                    ));
                }
            }

            // SOUNDNESS: an input has no init to mirror, so the shadow's cycle-0
            // value is CHOSEN — pinned to zero rather than left free.
            //
            // Free would be the more faithful reading of SVA, where `$past` is
            // undefined before the first clock edge. It is not a choice this
            // adapter can make: an init-less state cell means different things to
            // different engines. The cube and exact engines default it to 0
            // (`state_cell_init_values` / `initial_state_bdd`, per the
            // `setundef -zero` power-up); the reachability portfolio leaves it
            // FREE, per BTOR2's nondeterministic-init semantics. That is exactly
            // the verdict DISAGREEMENT `reset_init::inject_zero_init` exists to
            // close — and it cannot close this one, because it runs on the
            // pre-augmentation BTOR2, before `augment_with_past_shadows` appends
            // the shadow. A free shadow would escape the mitigation by
            // construction.
            //
            // What is given up is bounded, and observable in only one shape. The
            // lift puts a `|=>` consequent under a `[]`, so it is evaluated only
            // at states that HAVE a predecessor — where the shadow holds a value
            // the design actually drove. A `|->` consequent is evaluated at the
            // initial state too, and there it reads this invented zero. So every
            // data-integrity property (`|=>` by construction) is unaffected; a
            // `|->` over `$past` of an input is not, and its cycle-0 verdict does
            // not transfer to hardware. Every later cycle is exact — a real added
            // flop, not an abstraction. Pinning also matches the power-up the rest
            // of the adapter already assumes, so every engine decides one model.
            //
            // Full argument, per-engine ledger, and why no engine may assume
            // stutter equivalence here: docs/design/past-shadow-soundness.md
            ShadowSource::PrimaryInput { .. } => {
                let zero = zero_nid.expect("an input source allocates its zero above");
                let shadow_init = next_nid;
                next_nid += 1;
                appended.push(format!(
                    "{shadow_init} init {sort_nid} {shadow_state} {zero}"
                ));
            }
        }
    }

    if appended.is_empty() {
        return Ok(content.to_string());
    }
    Ok(format!("{}\n{}\n", content.trim_end(), appended.join("\n")))
}

/// Can `base` be a `$past` shadow source in this model?
///
/// Exported so a caller that pre-filters bases uses the SAME rule as the
/// augmentation itself. Two independent answers to "what is a valid base" is
/// precisely how the input case stayed broken: the augmentation was taught to
/// accept an input while `verify_auto`'s filter still asked
/// `resolve_state_by_symbol`, so input bases were dropped before they ever
/// reached here and the fix was inert on the only path users take.
pub fn can_shadow(file: &Btor2File, base: &str) -> bool {
    resolve_shadow_source(file, base).is_some()
}

/// Resolve a `$past` base to its source line: `(nid, sort, is_input)`.
///
/// A state cell is tried first, matching the historical behaviour and keeping
/// the symbol-distance heuristic in `resolve_state_by_symbol` authoritative
/// when a name could mean either. A primary input is the fallback.
/// Where a `__past` shadow takes its value from, and hence how its cycle 0 is set.
#[derive(Clone, Copy)]
enum ShadowSource {
    /// A BTOR2 state cell — the shadow mirrors this cell's `init`, or its absence.
    StateCell { nid: Nid, sort: Nid },
    /// A primary input — there is no `init` to mirror, so the shadow's cycle-0
    /// value is chosen. See the SOUNDNESS note in `augment_with_past_shadows`.
    PrimaryInput { nid: Nid, sort: Nid },
}

impl ShadowSource {
    /// The NID the shadow's `next` reads from.
    fn nid(self) -> Nid {
        match self {
            Self::StateCell { nid, .. } | Self::PrimaryInput { nid, .. } => nid,
        }
    }

    /// The sort both the shadow flop and its `next` carry.
    fn sort(self) -> Nid {
        match self {
            Self::StateCell { sort, .. } | Self::PrimaryInput { sort, .. } => sort,
        }
    }
}

/// Resolve `base` to the signal a `{base}__past` shadow should sample.
///
/// Precedence is EXACT BEFORE FUZZY, and that ordering is the point.
/// `parser::resolve_state_by_symbol` also matches an `Op`/`Output` line named
/// `base` and walks its cone to the nearest state — a match at distance N. Asking
/// it first would let that distant state outrank an `input` line that carries the
/// name exactly, silently shadowing the wrong signal. So: exact state, then exact
/// input, then the cone walk.
///
/// Tier 1 requires a UNIQUE exact state match; two states sharing a symbol fall
/// through to the walk, which rejects the ambiguity itself (`tied` → `None`)
/// rather than picking one arbitrarily.
fn resolve_shadow_source(file: &Btor2File, base: &str) -> Option<ShadowSource> {
    let mut exact_states = file.lines.iter().filter_map(|l| match &l.node {
        Node::State {
            sort,
            symbol: Some(s),
            ..
        } if s == base => Some(ShadowSource::StateCell {
            nid: l.nid,
            sort: *sort,
        }),
        _ => None,
    });
    if let Some(only) = exact_states.next()
        && exact_states.next().is_none()
    {
        return Some(only);
    }

    let exact_input = file.lines.iter().find_map(|l| match &l.node {
        Node::Input {
            sort,
            symbol: Some(s),
        } if s == base => Some(ShadowSource::PrimaryInput {
            nid: l.nid,
            sort: *sort,
        }),
        _ => None,
    });
    if exact_input.is_some() {
        return exact_input;
    }

    let nid = parser::resolve_state_by_symbol(file, base)?;
    match file.lookup(nid).map(|l| &l.node) {
        Some(Node::State { sort, .. }) => Some(ShadowSource::StateCell { nid, sort: *sort }),
        _ => None,
    }
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

    // A design whose only `$past` source is a primary INPUT:
    //   1 sort bitvec 8
    //   2 input 1 din
    //   3 state 1 q          ; q <= din
    //   4 next 1 3 2
    const INPUT_SOURCE: &str = "1 sort bitvec 8\n2 input 1 din\n3 state 1 q\n4 next 1 3 2\n";

    #[test]
    fn a_primary_input_gets_a_shadow_flop() {
        // The case that made every data-integrity property unverifiable: the
        // translator asks for `din__past`, and before this the augmentation
        // refused because `din` is an input rather than a state cell.
        let out = augment_with_past_shadows(INPUT_SOURCE, &["din"]).expect("input base accepted");
        let file = parser::parse(&out).expect("augmented model parses");
        let symbols = parser::collect_symbols(&file);

        let shadow = file
            .lines
            .iter()
            .find(|l| {
                matches!(l.node, Node::State { .. })
                    && symbols.get(&l.nid).map(String::as_str) == Some("din__past")
            })
            .expect("din__past is a state cell in the augmented model");

        // next(din__past) = din, so the flop holds the input's previous value.
        let din_nid = file
            .lines
            .iter()
            .find(|l| symbols.get(&l.nid).map(String::as_str) == Some("din"))
            .expect("din")
            .nid;
        assert!(
            file.lines.iter().any(|l| matches!(&l.node,
                Node::Next { state, value, .. } if *state == shadow.nid && value.nid() == din_nid)),
            "next(din__past) must be sourced from the input"
        );
    }

    #[test]
    fn an_input_shadow_is_pinned_to_zero_so_every_engine_agrees() {
        // A state cell's shadow mirrors the source's init. An input has none, and
        // leaving the shadow init-less is NOT the safe default here: the cube and
        // exact engines read an init-less cell as 0 while the reach portfolio
        // leaves it free, which is the verdict disagreement `inject_zero_init`
        // exists to prevent — and that pass runs before this one, so it cannot
        // cover a shadow appended afterwards. Pinning here is what keeps the
        // engines deciding the same model.
        let out = augment_with_past_shadows(INPUT_SOURCE, &["din"]).expect("augment");
        let file = parser::parse(&out).expect("parse");
        assert_eq!(
            init_value(&file, "din__past"),
            Some(0),
            "an input-sourced shadow must carry an EXPLICIT zero init"
        );
    }

    #[test]
    fn an_input_shadows_zero_is_declared_before_the_state_it_inits() {
        // btormc's parser requires an `init` value's NID to be BELOW the state's.
        // mununu's own parser is order-agnostic, so getting this wrong produces a
        // model that reads fine in-process and is rejected by the external oracle
        // — a disagreement that would surface as a portfolio failure, not as a
        // parse error here.
        let out = augment_with_past_shadows(INPUT_SOURCE, &["din"]).expect("augment");
        let file = parser::parse(&out).expect("parse");
        let symbols = parser::collect_symbols(&file);
        let shadow_nid = file
            .lines
            .iter()
            .find(|l| symbols.get(&l.nid).map(String::as_str) == Some("din__past"))
            .expect("din__past")
            .nid;
        let init_value_nid = file
            .lines
            .iter()
            .find_map(|l| match &l.node {
                Node::Init { state, value, .. } if *state == shadow_nid => Some(value.nid()),
                _ => None,
            })
            .expect("the shadow carries an init");
        assert!(
            init_value_nid < shadow_nid,
            "init value {init_value_nid} must precede state {shadow_nid}"
        );
    }

    #[test]
    fn an_exact_input_outranks_a_like_named_alias_over_a_distant_state() {
        // `resolve_state_by_symbol` also matches an `output`/op line named `din`
        // and walks its cone to the nearest state — a hit at distance N. If the
        // walk were tried first, that distant `q` would shadow the wrong signal
        // while an `input` line carries the name exactly. Exact beats fuzzy.
        let aliased = "1 sort bitvec 8\n                       2 input 1 din\n                       3 state 1 q\n                       4 next 1 3 2\n                       5 output 3 din\n";
        let out = augment_with_past_shadows(aliased, &["din"]).expect("augment");
        let file = parser::parse(&out).expect("parse");
        let symbols = parser::collect_symbols(&file);
        let shadow_nid = file
            .lines
            .iter()
            .find(|l| symbols.get(&l.nid).map(String::as_str) == Some("din__past"))
            .expect("din__past")
            .nid;
        assert!(
            file.lines.iter().any(|l| matches!(&l.node,
                Node::Next { state, value, .. } if *state == shadow_nid && value.nid() == 2)),
            "next(din__past) must read the INPUT (nid 2), not the aliased state"
        );
    }

    #[test]
    fn a_state_cell_still_wins_over_a_like_named_input() {
        // Resolution order matters: a state cell is tried first, so the
        // symbol-distance heuristic stays authoritative when a name could
        // mean either.
        let both = "1 sort bitvec 8\n2 input 1 s\n3 state 1 s\n4 next 1 3 2\n";
        let out = augment_with_past_shadows(both, &["s"]).expect("augment");
        let file = parser::parse(&out).expect("parse");
        let symbols = parser::collect_symbols(&file);
        let state_nid = file
            .lines
            .iter()
            .find(|l| {
                matches!(l.node, Node::State { .. })
                    && symbols.get(&l.nid).map(String::as_str) == Some("s")
            })
            .expect("state s")
            .nid;
        let shadow_nid = file
            .lines
            .iter()
            .find(|l| symbols.get(&l.nid).map(String::as_str) == Some("s__past"))
            .expect("s__past")
            .nid;
        assert!(
            file.lines.iter().any(|l| matches!(&l.node,
                Node::Next { state, value, .. } if *state == shadow_nid && value.nid() == state_nid)),
            "the STATE cell must win when a name resolves to both"
        );
    }

    #[test]
    fn missing_base_is_an_error_not_silent() {
        let err = augment_with_past_shadows(COUNTER, &["nonexistent"]).expect_err("must error");
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
        // Asserts the INTENT — the message names the signal and says it did
        // not resolve — rather than an exact phrase. The wording changed when
        // inputs became valid bases, and a test that pins prose fails for a
        // reason that has nothing to do with behaviour.
        assert!(
            err.message.contains("nonexistent") && err.message.contains("resolve"),
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
