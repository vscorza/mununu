//! P2.5-F — the exact-symbolic engine solving TWO-PLAYER μ-calculus games via
//! `exact_two_player_verdict` and `ExactModel::cpre_controllable` / `cpre_environment`. The first step
//! of the unified two-player game verifier (`.claude/plans/unified-two-player-mu-calculus-verifier.md`):
//! the exact ROBDD engine now honours the controllability axis (`<(ctrl = controllable)>`).
//!
//! Validation is by KNOWN ANSWER on two minimal 1-bit games whose winning regions are computed by hand.
//! Both share the state `st` and inputs `c` (controllable) + `e` (environment); they differ only in the
//! transition, which is exactly whether the environment can block the controller:
//!
//! - `CTRL`  : `st' = c`        — the controller sets the next state directly (environment powerless).
//! - `ENVBLK`: `st' = c & ¬e`   — the environment can force `st' = 0` by asserting `e` (blocking).
//!
//! Across the two fixpoint directions (μ = reach, ν = maintain) and both players, the pair pins down
//! `cpre_controllable` and `cpre_environment`. (Cross-validation against the explicit `gr1.rs` /
//! `parity_game.rs` solver on a shared CLTS is a follow-up; here the hand-computed winning regions are
//! the oracle.)

use mununu_core::adapter::btor2::symbolic_bitblast::{ExactVerdict, exact_two_player_verdict};
use mununu_core::mu_calculus::parser;

// st' = c : the controller sets the next state.
const CTRL_INIT0: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 st
5 zero 1
6 init 1 4 5
7 next 1 4 2
";
const CTRL_INIT1: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 st
5 one 1
6 init 1 4 5
7 next 1 4 2
";

// st' = c & ¬e : the environment can force st' = 0 by asserting e.
const ENVBLK_INIT0: &str = "\
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
const ENVBLK_INIT1: &str = "\
1 sort bitvec 1
2 input 1 c
3 input 1 e
4 state 1 st
5 one 1
6 init 1 4 5
7 not 1 3
8 and 1 2 7
9 next 1 4 8
";

fn verdict(btor2: &str, formula: &str) -> ExactVerdict {
    let f = parser::parse(formula).expect("formula parses");
    exact_two_player_verdict(btor2, &f, &["c"]).expect("exact two-player verdict")
}

/// Controllable reachability (`μ`): can the CONTROLLER force reaching `good = (st == 1)`?
/// `CTRL` (st'=c): from st=0, `∀e ∃c: c==1` — yes ⇒ HOLDS. `ENVBLK` (st'=c&¬e): for e=1 no c reaches
/// good ⇒ the `∀e` fails ⇒ VIOLATED. Same formula, differing only by whether the environment can block.
#[test]
fn controllable_reachability_holds_iff_environment_cannot_block() {
    let reach_good = "mu X. ((st == 1) || <(ctrl = controllable)> X)";
    assert_eq!(
        verdict(CTRL_INIT0, reach_good),
        ExactVerdict::Holds,
        "st'=c: the controller forces st=1 in one step"
    );
    assert_eq!(
        verdict(ENVBLK_INIT0, reach_good),
        ExactVerdict::Violated,
        "st'=c&¬e: the environment blocks with e=1, so the controller cannot force st=1"
    );
}

/// Controllable safety (`ν`): can the CONTROLLER MAINTAIN `good = (st == 1)` forever (init st=1)?
/// `CTRL`: from st=1, `∀e ∃c: c==1` keeps st=1 ⇒ HOLDS. `ENVBLK`: e=1 knocks st to 0 against any c ⇒
/// the greatest fixpoint collapses to ∅ ⇒ VIOLATED.
#[test]
fn controllable_safety_holds_iff_environment_cannot_knock_out() {
    let maintain_good = "nu X. ((st == 1) && <(ctrl = controllable)> X)";
    assert_eq!(
        verdict(CTRL_INIT1, maintain_good),
        ExactVerdict::Holds,
        "st'=c: the controller maintains st=1"
    );
    assert_eq!(
        verdict(ENVBLK_INIT1, maintain_good),
        ExactVerdict::Violated,
        "st'=c&¬e: the environment can knock st out of 1"
    );
}

/// The DUAL — environment predecessor (`cpre_environment`). Can the ENVIRONMENT force reaching
/// `st == 0` (init st=1)? `ENVBLK`: `∃e ∀c: (c&¬e)==0` holds for e=1 ⇒ HOLDS. `CTRL`: the environment
/// is powerless (`∀c: c==0` fails, c=1 keeps st=1) ⇒ VIOLATED. The mirror image of the controllable
/// results — confirming `cpre_environment` is the genuine dual, not a copy of `cpre_controllable`.
#[test]
fn environment_predecessor_is_the_genuine_dual() {
    let env_forces_zero = "mu X. ((st == 0) || <(ctrl = environment)> X)";
    assert_eq!(
        verdict(ENVBLK_INIT1, env_forces_zero),
        ExactVerdict::Holds,
        "st'=c&¬e: the environment forces st=0 with e=1"
    );
    assert_eq!(
        verdict(CTRL_INIT1, env_forces_zero),
        ExactVerdict::Violated,
        "st'=c: the environment cannot force st=0 (the controller holds st=1)"
    );
}

/// Regression: with NO controllable inputs declared, `<(ctrl = controllable)>` degenerates to the box
/// pre-image (the environment owns everything) — the controller can force `good` only if EVERY move
/// already leads there. On `ENVBLK` reaching `st==1` is then unforceable ⇒ VIOLATED, confirming the
/// empty-partition degeneration is the sound "controller cannot influence" reading.
#[test]
fn empty_partition_degenerates_to_box() {
    let f = parser::parse("mu X. ((st == 1) || <(ctrl = controllable)> X)").unwrap();
    // No controllable inputs: c is (implicitly) environment too.
    assert_eq!(
        exact_two_player_verdict(ENVBLK_INIT0, &f, &[]).unwrap(),
        ExactVerdict::Violated,
    );
}
