//! P2.5-F — the KMTS (3-valued predicate-cube) TWO-PLAYER game backend (`kmts_two_player_verdict` +
//! `AbstractRelation::cpre_controllable` / `cpre_environment`): the SCALE backend of the unified verifier
//! (`.claude/plans/unified-two-player-mu-calculus-verifier.md`, §2 Stage 4), for games past the exact
//! BDD cap. The controllable predecessor plays the exact concrete `∀env ∃ctrl` game within each concrete
//! state and lifts to the cube by the may/must (∀x / ∃x) abstraction.
//!
//! Validation:
//! - **precise ⇒ equals exact**: with a predicate that pins the (1-bit) state, the cube is exact, so the
//!   3-valued verdict is definite and equals `exact_two_player_verdict`.
//! - **coarse ⇒ sound `⊥`**: too few predicates to track multi-step reachability give `Unknown` — sound
//!   (never contradicting the exact oracle), the CEGAR-refinement trigger (a follow-up for games).
//! - **coarse-but-definite**: an unreachable target makes the whole non-target region one uniformly
//!   non-winning cube, so a single predicate already decides `False` — abstraction giving a definite
//!   verdict without pinning every state.
//! - **industrial soundness**: on the real OpenTitan `edn` / `otbn` fixtures the KMTS two-player verdict
//!   never contradicts the exact two-player oracle (definite ⇒ same verdict; else `⊥`).

use mununu_core::adapter::btor2::predicate_expr::{PredicateExpr, parse_predicate_atom_bool};
use mununu_core::adapter::btor2::symbolic_bitblast::{
    ExactVerdict, MustSemantics, exact_two_player_verdict, kmts_two_player_verdict,
};
use mununu_core::mu_calculus::parser;
use mununu_core::mu_calculus::trit::Trit;

fn preds(list: &[(&str, &str)]) -> Vec<(String, PredicateExpr)> {
    list.iter()
        .map(|(n, e)| {
            (
                n.to_string(),
                parse_predicate_atom_bool(e).expect("atom parses"),
            )
        })
        .collect()
}

/// st' = c (controller sets the state) / st' = c & ¬e (environment blocks) — the 1-bit games from the
/// exact suite, reused here so KMTS-with-a-precise-predicate can be checked against exact.
const CTRL: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 st
5 zero 1
6 init 1 4 5
7 next 1 4 2
";
const ENVBLK: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 st
5 zero 1
6 init 1 4 5
7 not 1 3
8 and 1 2 7
9 next 1 4 8
";

/// Precise predicate (the 1-bit state pinned) ⇒ the cube is exact ⇒ the 3-valued verdict is definite and
/// equals `exact_two_player_verdict`: True where the controller wins (`CTRL`), False where the
/// environment blocks (`ENVBLK`).
#[test]
fn kmts_matches_exact_with_a_precise_predicate() {
    let f = parser::parse("mu X. ((st == 1) || <(ctrl = controllable)> X)").unwrap();
    let p = preds(&[("st == 1", "st == 1")]);
    for (name, btor, want_kmts, want_exact) in [
        ("CTRL", CTRL, Trit::True, ExactVerdict::Holds),
        ("ENVBLK", ENVBLK, Trit::False, ExactVerdict::Violated),
    ] {
        let kmts =
            kmts_two_player_verdict(btor, &f, &p, &["c"], MustSemantics::ForallExists).unwrap();
        let exact = exact_two_player_verdict(btor, &f, &["c"]).unwrap();
        assert_eq!(kmts, want_kmts, "{name}: KMTS two-player verdict");
        assert_eq!(exact, want_exact, "{name}: exact two-player verdict");
    }
}

// A 0 -(c)-> 1 -> 2 chain: reaching st==2 needs two controller steps. Controllable c.
const CHAIN: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 input 2 c
4 state 1 st
5 zero 1
6 init 1 4 5
7 one 1
8 constd 1 2
9 eq 2 4 5
10 eq 2 4 7
11 ite 1 3 7 5
12 ite 1 10 8 4
13 ite 1 9 11 12
14 next 1 4 13
";

/// Coarse predicates ⇒ sound `⊥`: with only `st == 2` the fixpoint cannot grow the must-region across
/// the two-step chain (the non-target cube lumps state 1, which reaches the target in one step, with
/// state 0, which does not), so the verdict is `Unknown` — sound (exact says Holds; KMTS abstains, never
/// contradicts). This is the CEGAR trigger.
#[test]
fn kmts_coarse_predicate_yields_bottom() {
    let f = parser::parse("mu X. ((st == 2) || <(ctrl = controllable)> X)").unwrap();
    let p = preds(&[("st == 2", "st == 2")]);
    let kmts = kmts_two_player_verdict(CHAIN, &f, &p, &["c"], MustSemantics::ForallExists).unwrap();
    assert_eq!(kmts, Trit::Unknown, "coarse abstraction abstains (⊥)");
    // Sound: exact decides Holds, KMTS's ⊥ does not contradict it.
    assert_eq!(
        exact_two_player_verdict(CHAIN, &f, &["c"]).unwrap(),
        ExactVerdict::Holds
    );
}

// st' = 2 unconditionally: the target st==1 is unreachable (0 -> 2 -> 2).
const TRAP: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 input 2 c
4 state 1 st
5 zero 1
6 init 1 4 5
7 constd 1 2
8 next 1 4 7
";

/// Coarse-but-definite: an unreachable target makes the whole non-target region one uniformly
/// non-winning cube, so a SINGLE predicate already decides `False` — abstraction giving a definite
/// verdict without pinning every state. Matches exact (Violated).
#[test]
fn kmts_coarse_predicate_decides_unreachable_target() {
    let f = parser::parse("mu X. ((st == 1) || <(ctrl = controllable)> X)").unwrap();
    let p = preds(&[("st == 1", "st == 1")]);
    let kmts = kmts_two_player_verdict(TRAP, &f, &p, &["c"], MustSemantics::ForallExists).unwrap();
    assert_eq!(
        kmts,
        Trit::False,
        "unreachable target ⇒ definite False, coarsely"
    );
    assert_eq!(
        exact_two_player_verdict(TRAP, &f, &["c"]).unwrap(),
        ExactVerdict::Violated
    );
}

// ---- Industrial soundness: KMTS two-player never contradicts the exact two-player oracle -------------

const EDN: &str = include_str!("fixtures/wall_classes/edn_boot_sm.btor");
const OTBN: &str = include_str!("fixtures/wall_classes/otbn_start_stop_fsm.btor");
const EDN_ALL: &[&str] = &[
    "auto_req_mode_i",
    "boot_req_mode_i",
    "clk_i",
    "cmd_sent_i",
    "csrng_ack_err_i",
    "csrng_cmd_ack_i",
    "edn_enable_i",
    "local_escalate_i",
    "max_reqs_cnt_zero_i",
    "rst_ni",
    "sw_cmd_req_load_i",
];
const OTBN_ALL: &[&str] = &[
    "rma_req_i",
    "escalate_i",
    "urnd_reseed_ack_i",
    "secure_wipe_req_i",
    "start_i",
    "rst_ni",
    "clk_i",
];

/// The KMTS two-player verdict must never contradict the exact one: a definite `True`/`False` implies
/// the exact `Holds`/`Violated`; `⊥` is always sound.
fn assert_sound(kmts: Trit, exact: ExactVerdict, ctx: &str) {
    match kmts {
        Trit::True => assert_eq!(
            exact,
            ExactVerdict::Holds,
            "{ctx}: KMTS True but exact != Holds"
        ),
        Trit::False => {
            assert_eq!(
                exact,
                ExactVerdict::Violated,
                "{ctx}: KMTS False but exact != Violated"
            )
        }
        Trit::Unknown => {}
    }
}

/// Industrial: on the real OpenTitan edn / otbn fixtures, over both input partitions (all-controllable
/// and mode-controllable), the KMTS two-player verdict is SOUND w.r.t. the exact two-player oracle. With
/// a single target predicate the KMTS abstains (`⊥`) on these multi-step reachability games — sound;
/// definite industrial verdicts need game-CEGAR (the Stage-4 follow-up). The point here is that the new
/// engine never fabricates a verdict the exact oracle contradicts, on real designs.
#[test]
#[allow(clippy::type_complexity)] // the case row is local + self-documenting
fn kmts_two_player_is_sound_on_real_rtl() {
    let cases: &[(&str, &str, &str, &[&str], &[&str])] = &[
        (
            "edn",
            EDN,
            "state_q == 44",
            EDN_ALL,
            &["boot_req_mode_i", "edn_enable_i"],
        ),
        ("otbn", OTBN, "state_q == 119", OTBN_ALL, &["start_i"]),
    ];
    for (name, btor, target, all, mode) in cases {
        let f = parser::parse(&format!("mu X. (({target}) || <(ctrl = controllable)> X)")).unwrap();
        let p = preds(&[(*target, *target)]);
        for (label, ctrl) in [("all", *all), ("mode", *mode)] {
            let kmts =
                kmts_two_player_verdict(btor, &f, &p, ctrl, MustSemantics::ForallExists).unwrap();
            let exact = exact_two_player_verdict(btor, &f, ctrl).unwrap();
            assert_sound(kmts, exact, &format!("{name}/{label}"));
        }
    }
}
