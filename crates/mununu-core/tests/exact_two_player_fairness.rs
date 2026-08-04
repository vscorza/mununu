//! P2.5-F — FAIRNESS environment-assumption discovery for the two-player RECURRENCE (Büchi) game:
//! the GR(1) 1-pair objective `GF a → GF good`, decided EXACTLY by `exact_two_player_gr1_realizable`
//! over the assumption-latched model, and searched by `discover_game_fairness_assumption`.
//!
//! Validation is by KNOWN ANSWER on a minimal 1-bit "buffer" game whose winning region is computed by
//! hand. State `count ∈ {0,1}`, `good = (count == 1)`. Inputs `c` (controllable = "push") and `e`
//! (environment = "pop"). The transition is a saturating ±1 counter:
//!
//!   count' = (c ∧ ¬e) ∨ (count ∧ ¬(e ∧ ¬c))
//!     c=1,e=0 → 1 (push)      ¬c,e   → 0 (pop)      otherwise → count (hold)
//!
//! Key dynamics: a pop-WITH-push HOLDS (c=1,e=1 → count'=count), so once `count=1` the controller
//! keeps it at 1 by playing c=1 — this is what makes the fairness assumption `GF(e==0)` SUFFICIENT
//! (reach 1 on any e=0 step, then hold). Without the assumption the environment pops forever (e=1) and
//! starves `count`.

use mununu_core::adapter::btor2::concrete_oracle::OracleAtom;
use mununu_core::adapter::btor2::predicate_expr::CmpOp;
use mununu_core::adapter::btor2::symbolic_bitblast::{
    exact_two_player_buchi_realizable, exact_two_player_gr1_realizable,
};
use mununu_core::adapter::recoverability::discover_game_fairness_assumption;

// count' = (c ∧ ¬e) ∨ (count ∧ ¬(e ∧ ¬c)); init count=0.
const BUFFER: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 count
5 zero 1
6 init 1 4 5
7 not 1 3
8 and 1 2 7
9 not 1 2
10 and 1 3 9
11 not 1 10
12 and 1 4 11
13 or 1 8 12
14 next 1 4 13
";

// st' = c, init st=1 — the controller trivially maintains st=1 (Büchi realizable WITHOUT any assumption).
const CTRL_INIT1: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 st
5 one 1
6 init 1 4 5
7 next 1 4 2
";

/// The RECURRENCE game `GF(count==1)` (controller = push) is UNREALIZABLE: the environment pops forever
/// (e=1) and `count` is starved. This is the ⊥ a fairness assumption must rescue.
#[test]
fn buffer_recurrence_is_unrealizable_without_an_assumption() {
    assert_eq!(
        exact_two_player_buchi_realizable(BUFFER, "count == 1", &["c"]),
        Ok(false),
        "the environment pops every cycle ⇒ count never recurrently reaches 1"
    );
}

/// The fairness assumption `GF(e==0)` (the environment pauses popping infinitely often) RESCUES it:
/// `GF(e==0) → GF(count==1)` is REALIZABLE — whenever e=0 the controller pushes to 1, and a pop-with-push
/// holds it there. This is the fairness lever's positive case.
#[test]
fn buffer_recurrence_is_realizable_under_pop_pause_fairness() {
    let assume = OracleAtom::new("e", CmpOp::Eq, 0);
    assert_eq!(
        exact_two_player_gr1_realizable(BUFFER, "count == 1", &assume, &["c"]),
        Ok(true),
        "GF(e==0) → GF(count==1): reach 1 on an e=0 step, hold with c=1"
    );
}

/// SOUNDNESS CONTROL — a USELESS assumption does NOT rescue. `GF(e==1)` (the environment pops infinitely
/// often) is satisfied by popping EVERY cycle, which starves `count` ⇒ `GF(e==1) → GF(count==1)` stays
/// UNREALIZABLE. A fairness lever that "rescued" this would be unsound (fabricating a win).
#[test]
fn buffer_recurrence_is_not_rescued_by_a_useless_assumption() {
    let assume = OracleAtom::new("e", CmpOp::Eq, 1);
    assert_eq!(
        exact_two_player_gr1_realizable(BUFFER, "count == 1", &assume, &["c"]),
        Ok(false),
        "GF(e==1) is met by popping every cycle ⇒ no rescue"
    );
}

/// REDUCTION / no-over-firing control — when the plain Büchi game is ALREADY realizable (st'=c, init
/// st=1), the GR(1) game with any assumption is ALSO realizable (an assumption can only help). The
/// fairness path must not spuriously turn a win into a loss.
#[test]
fn gr1_reduces_to_realizable_when_buchi_already_wins() {
    assert_eq!(
        exact_two_player_buchi_realizable(CTRL_INIT1, "st == 1", &["c"]),
        Ok(true),
    );
    let assume = OracleAtom::new("e", CmpOp::Eq, 0);
    assert_eq!(
        exact_two_player_gr1_realizable(CTRL_INIT1, "st == 1", &assume, &["c"]),
        Ok(true),
        "an assumption never breaks an already-realizable recurrence game"
    );
}

/// DISCOVERY end-to-end: on the unrealizable buffer game, `discover_game_fairness_assumption` finds the
/// enabling fairness assumption `GF(e == 0)` (non-vacuous) — and ONLY that one (not the useless
/// `GF(e == 1)`).
#[test]
fn discovery_finds_the_pop_pause_fairness_assumption() {
    let found = discover_game_fairness_assumption(BUFFER, "count == 1", &["c"]);
    assert!(
        found.iter().any(|a| a.phi == "GF(e == 0)" && a.non_vacuous),
        "expected to discover GF(e == 0): {found:?}"
    );
    assert!(
        !found.iter().any(|a| a.phi == "GF(e == 1)"),
        "must NOT discover the useless GF(e == 1): {found:?}"
    );
}

/// DISCOVERY false-positive control — on an already-realizable recurrence game, discovery returns ∅
/// (no assumption is needed; fabricating one would be a false positive).
#[test]
fn discovery_is_empty_when_recurrence_already_holds() {
    assert!(
        discover_game_fairness_assumption(CTRL_INIT1, "st == 1", &["c"]).is_empty(),
        "a realizable recurrence game needs no fairness assumption"
    );
}
