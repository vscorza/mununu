//! P2.5-F — the exact-symbolic engine EXTRACTING the two-player game strategy via
//! `exact_two_player_strategy`. Where `exact_two_player_verdict` answers realizable? (yes/no), this
//! recovers the WINNER's positional strategy: the CONTROLLER's forced controllable inputs when the
//! reachability game is realizable, or the ENVIRONMENT's counterstrategy (forced environment inputs
//! witnessing why no controller wins) when it is not.
//!
//! Validation is by KNOWN ANSWER on the same minimal 1-bit games as `exact_two_player_game.rs`
//! (`st' = c`, `st' = c & ¬e`), plus a REDUCTION check + an unrealizable finding on the real OpenTitan
//! `edn_main_sm` fixture:
//!
//! - `CTRL`  (`st' = c`)       : realizable — controller forces `c = 1` to reach `st = 1`.
//! - `ENVBLK`(`st' = c & ¬e`)  : unrealizable — the environment counterstrategy forces `e = 1` to keep
//!   `st = 0` against every controllable move.
//! - `edn` all-controllable    : the two-player strategy must EQUAL the 1-player positional strategy
//!   (`∀∅ ∃all` reduces `cpre_controllable`→`diamond_pre`, `ctrl_forcing_moves`→`move_into`).
//! - `edn` mode-controllable   : unrealizable — the counterstrategy forces only ENVIRONMENT inputs
//!   (the acks/escalation), never the controllable mode signals.

use mununu_core::adapter::btor2::symbolic_bitblast::{
    PositionalStrategy, exact_env_positional_strategy, exact_two_player_strategy,
};

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

const EDN: &str = include_str!("fixtures/wall_classes/edn_boot_sm.btor");
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

/// The value the strategy forces `input` to at control-state `state_value`, or `None` if free/absent.
fn forced(s: &PositionalStrategy, state_value: u128, input: &str) -> Option<u128> {
    s.entries
        .iter()
        .find(|e| e.state_value == state_value)?
        .forced_inputs
        .get(input)
        .copied()
}

/// CTRL (st'=c), `good = st==1`, `c` controllable: the game is realizable (the controller forces it in
/// one step). The controller strategy forces `c = 1` at the rank-1 state `st=0`, and forces nothing at
/// the already-good `st=1` (rank 0). It must NOT force the environment input `e`.
#[test]
fn controller_strategy_forces_the_winning_move() {
    let strat = exact_two_player_strategy(CTRL_INIT0, "st == 1", &["c"]).unwrap();
    assert!(strat.realizable, "st'=c: the controller wins");
    let s = &strat.strategy;
    assert_eq!(s.state_register, "st");
    // st=0 is rank 1 (one controllable step to good) and forces c=1.
    assert_eq!(
        forced(s, 0, "c"),
        Some(1),
        "at st=0 the controller sets c=1"
    );
    // The controller never forces the environment's input.
    assert_eq!(
        forced(s, 0, "e"),
        None,
        "e is the environment's, not forced"
    );
    // The already-good state has rank 0 and needs no forcing move.
    let good_entry = s.entries.iter().find(|e| e.state_value == 1).unwrap();
    assert_eq!(good_entry.rank, 0);
    assert!(good_entry.forced_inputs.is_empty());
}

/// ENVBLK (st'=c&¬e), `good = st==1`, `c` controllable: UNREALIZABLE — for `e=1` no `c` reaches good, so
/// `∀e` fails and the controller cannot win. The extractor returns the ENVIRONMENT's counterstrategy:
/// at `st=0` it forces `e = 1` (the unique move keeping the play out of `{st==1}` against every `c`), and
/// forces nothing on the controllable input `c`.
#[test]
fn environment_counterstrategy_forces_the_blocking_move() {
    let strat = exact_two_player_strategy(ENVBLK_INIT0, "st == 1", &["c"]).unwrap();
    assert!(!strat.realizable, "st'=c&¬e: the environment wins");
    let s = &strat.strategy;
    // The environment holds e=1 at st=0 to keep the controller out of st=1.
    assert_eq!(
        forced(s, 0, "e"),
        Some(1),
        "the environment blocks with e=1"
    );
    // It does not (cannot) force the controller's input.
    assert_eq!(forced(s, 0, "c"), None, "c is the controller's, not forced");
}

/// Reduction on real RTL: with EVERY input controllable, the controllable predecessor collapses to the
/// single-agent diamond and the forcing move to `move_into`, so the two-player strategy must be EXACTLY
/// the 1-player positional strategy (`exact_env_positional_strategy`). Cross-checks the game extractor
/// against the already-validated 1-player extractor on the OpenTitan boot FSM.
#[test]
fn edn_all_controllable_strategy_equals_one_player() {
    let good = "state_q == 44";
    let one = exact_env_positional_strategy(EDN, good).expect("1-player strategy");
    let two = exact_two_player_strategy(EDN, good, EDN_ALL).expect("2-player strategy");
    assert!(
        two.realizable,
        "all-controllable: the handshake is reachable"
    );
    assert_eq!(
        two.strategy, one,
        "all-controllable two-player strategy == one-player positional strategy"
    );
}

/// The genuine two-player finding on real RTL: with only the mode signals controllable, the boot
/// handshake is UNREALIZABLE (the environment withholds the ack / escalates). The extractor returns the
/// environment's counterstrategy — and it forces ONLY environment inputs, never the controllable mode
/// signals. This is the witness behind the assume-guarantee reading (the handshake needs an environment
/// assumption).
#[test]
fn edn_mode_controllable_counterstrategy_is_environment_only() {
    let mode: &[&str] = &["boot_req_mode_i", "edn_enable_i"];
    let strat = exact_two_player_strategy(EDN, "state_q == 44", mode).expect("2-player strategy");
    assert!(
        !strat.realizable,
        "mode-controllable, acks environment: unrealizable ⇒ a counterstrategy"
    );
    assert!(
        !strat.strategy.entries.is_empty(),
        "the initial state is in the environment's winning region ⇒ at least one row"
    );
    for e in &strat.strategy.entries {
        for k in e.forced_inputs.keys() {
            assert!(
                k != "boot_req_mode_i" && k != "edn_enable_i",
                "the counterstrategy forces environment inputs only, got controllable {k}"
            );
        }
    }
}
