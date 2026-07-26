//! Track R — the **wall-class lever-evaluation matrix** (`.claude/plans/wall-class-lever-evaluation.md`).
//!
//! A FIXED set of class-representative recoverability cases (§ `CASES`) × the mechanisms with a
//! corpus claim (`default` = exact-first+WP · `cube-wp` · `cube-craig`), run as ONE matrix so a
//! lever is judged on the class it TARGETS, not on whatever design happened to be handy. This is
//! the methodology fix for the convenience-biased lever-testing (2026-07-26): the run prints a
//! `class × lever → verdict` table and asserts the SOUNDNESS invariant (no lever contradicts the
//! oracle) + that each decidable case is decided by at least one lever.
//!
//! `#[ignore]` — a validation HARNESS (some cones are wide + the Craig lever needs MathSAT), run on
//! demand: `cargo test -p mununu-core --test wall_class_matrix -- --ignored --nocapture`
//! (in `mununu-sva-pono` with `MUNUNU_MATHSAT_PATH` set, the `cube-craig` column comes alive).

use mununu_core::adapter::btor2::cegar::PredicateSource;
use mununu_core::adapter::recoverability::{
    verify_recoverability, verify_recoverability_scalable,
    verify_recoverability_scalable_with_source,
};
use mununu_core::verdict::PropertyVerdict;

/// The sound oracle verdict for a case — what a COMPLETE analysis reaches.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Oracle {
    Holds,
    Violated,
    /// A trap the abstraction can't prove — the sound requirement is only "never fabricate Holds".
    NeverHolds,
    /// No current lever decides it (register-dominated / research-⊥) — documented, not asserted.
    HardUnknown,
}

struct Case {
    name: &'static str,
    class: &'static str,
    btor2: &'static str,
    target: &'static str,
    oracle: Oracle,
}

// ---- The fixed class-representative set --------------------------------------------------------
// Synthetic fixtures mirror the recoverability/cegar test corpus (kept in sync by class, not
// copied blindly); the RTL case is a checked-in lifted BTOR2.

const RESPONDER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 eq 2 3 4
10 eq 2 3 7
11 ite 1 6 7 4
12 ite 1 10 8 4
13 ite 1 9 11 12
14 next 1 3 13
";

const STALLER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 constd 1 3
10 eq 2 3 4
11 eq 2 3 7
12 ite 1 6 7 4
13 ite 1 11 9 3
14 ite 1 10 12 13
15 next 1 3 14
";

const WIDE_RECOVERABLE: &str = "\
1 sort bitvec 80
2 sort bitvec 2
3 sort bitvec 1
4 state 1 cnt
5 zero 1
6 init 1 4 5
7 inc 1 4
8 next 1 4 7
9 state 2 st
10 zero 2
11 init 2 9 10
12 one 2
13 eq 3 9 10
14 eq 3 4 5
15 ite 2 14 12 10
16 ite 2 13 15 10
17 next 2 9 16
";

const WIDE_TRAP: &str = "\
1 sort bitvec 80
2 sort bitvec 2
3 sort bitvec 1
4 state 1 cnt
5 zero 1
6 init 1 4 5
7 inc 1 4
8 next 1 4 7
9 state 2 st
10 zero 2
11 init 2 9 10
12 constd 2 2
13 eq 3 4 5
14 ite 2 13 12 12
15 next 2 9 14
";

const TRAP_UF_W48: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
8 zero 3
13 one 3
30 constd 3 2
9 init 3 7 13
10 input 2 start
11 one 1
12 constd 1 2
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 8
17 ite 1 10 11 5
25 ite 1 23 12 11
18 ite 1 15 25 5
19 ite 1 14 17 18
20 next 1 4 19
31 add 3 7 30
22 next 3 7 31
";

const POS_W48: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
8 zero 3
9 init 3 7 8
10 input 2 start
11 one 1
12 constd 1 2
13 one 3
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 8
24 and 2 10 23
17 ite 1 24 11 5
18 ite 1 15 12 5
19 ite 1 14 17 18
20 next 1 4 19
21 add 3 7 13
22 next 3 7 21
";

const CLASS2_DATADEP: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 sort bitvec 48
4 state 1 ctrl
5 zero 1
6 init 1 4 5
7 state 3 data
30 constd 3 7
9 init 3 7 30
10 input 2 start
11 one 1
14 eq 2 4 5
15 eq 2 4 11
23 eq 2 7 30
17 ite 1 10 11 5
25 ite 1 23 5 11
18 ite 1 15 25 5
19 ite 1 14 17 18
20 next 1 4 19
22 next 3 7 7
";

// The Craig-DISCRIMINATING case: exact decides it (small), but the CUBE path needs the emergent
// relational invariant `data == target` that only Craig discovers — cube-wp ⊥, cube-craig HOLDS.
const CRAIG_EMERGENT: &str = "\
1 sort bitvec 1
2 sort bitvec 3
3 state 1 busy
4 state 2 data
5 state 2 target
6 one 1
7 zero 1
8 zero 2
9 one 2
10 init 1 3 6
11 init 2 4 8
12 init 2 5 8
13 add 2 4 9
14 next 2 4 13
15 add 2 5 9
16 next 2 5 15
17 eq 1 4 5
18 ite 1 17 7 3
19 next 1 3 18
";

// RTL — lifted i2c (44 registers); the register-dominated cone (86% data-dependent, per the
// register-width probe) that NO current lever decides. Reproduce: lift examples/… i2c with
// `sv verify-auto --engine exact-symbolic` + MUNUNU_KEEP_YOSYS_TMP=1.
const I2C_SCL_PADOEN: &str = include_str!("fixtures/wall_classes/i2c_scl_padoen.btor");

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "responder",
            class: "decidable-exact/HOLDS",
            btor2: RESPONDER,
            target: "st == 0",
            oracle: Oracle::Holds,
        },
        Case {
            name: "staller",
            class: "decidable-exact/VIOLATED",
            btor2: STALLER,
            target: "st == 0",
            oracle: Oracle::Violated,
        },
        Case {
            name: "wide_recoverable",
            class: "wide-cube/HOLDS",
            btor2: WIDE_RECOVERABLE,
            target: "st == 0",
            oracle: Oracle::Holds,
        },
        Case {
            name: "wide_trap",
            class: "wide-cube/VIOLATED",
            btor2: WIDE_TRAP,
            target: "st == 0",
            oracle: Oracle::Violated,
        },
        Case {
            name: "trap_uf_w48",
            class: "uf-wrap/soundness",
            btor2: TRAP_UF_W48,
            target: "ctrl == 0",
            oracle: Oracle::NeverHolds,
        },
        Case {
            name: "pos_w48",
            class: "wide-datapath-return/HOLDS",
            btor2: POS_W48,
            target: "ctrl == 0",
            oracle: Oracle::Holds,
        },
        Case {
            name: "class2_datadep",
            class: "guard-atom/HOLDS",
            btor2: CLASS2_DATADEP,
            target: "ctrl == 0",
            oracle: Oracle::Holds,
        },
        Case {
            name: "craig_emergent",
            class: "invariant-class/HOLDS (Craig)",
            btor2: CRAIG_EMERGENT,
            target: "busy == 0",
            oracle: Oracle::Holds,
        },
        Case {
            name: "i2c_scl_padoen (RTL)",
            class: "register-dominated/⊥",
            btor2: I2C_SCL_PADOEN,
            target: "scl_padoen_o == 1",
            oracle: Oracle::HardUnknown,
        },
    ]
}

fn v(r: &Result<PropertyVerdict, String>) -> &'static str {
    match r {
        Ok(PropertyVerdict::Holds) => "HOLDS",
        Ok(PropertyVerdict::Violated) => "VIOL ",
        Ok(PropertyVerdict::Unknown) => "  ⊥  ",
        Ok(PropertyVerdict::Skipped) => "SKIP ",
        Err(_) => "ERR  ",
    }
}

fn is_holds(r: &Result<PropertyVerdict, String>) -> bool {
    matches!(r, Ok(PropertyVerdict::Holds))
}
fn is_viol(r: &Result<PropertyVerdict, String>) -> bool {
    matches!(r, Ok(PropertyVerdict::Violated))
}
fn decides(r: &Result<PropertyVerdict, String>, want: PropertyVerdict) -> bool {
    matches!(r, Ok(x) if *x == want)
}

#[test]
#[ignore = "validation harness: wide cones + cube-craig needs MathSAT (mununu-sva-pono)"]
fn wall_class_matrix() {
    let mathsat = std::env::var_os("MUNUNU_MATHSAT_PATH").is_some();
    println!(
        "\n{:32} {:30} {:>6} {:>6} {:>6}",
        "case", "class", "dflt", "wp", "craig"
    );
    println!("{}", "-".repeat(88));

    let mut soundness_violations = Vec::new();
    let mut craig_unique = Vec::new();

    for c in cases() {
        let dflt = verify_recoverability(c.btor2, c.target);
        let wp = verify_recoverability_scalable(c.btor2, c.target, &[]);
        let craig = verify_recoverability_scalable_with_source(
            c.btor2,
            c.target,
            &[],
            PredicateSource::CraigInterpolation,
        );
        println!(
            "{:32} {:30} {:>6} {:>6} {:>6}",
            c.name,
            c.class,
            v(&dflt),
            v(&wp),
            v(&craig)
        );

        // SOUNDNESS invariant — no lever may contradict the oracle.
        for (lever, r) in [("default", &dflt), ("cube-wp", &wp), ("cube-craig", &craig)] {
            let bad = match c.oracle {
                Oracle::Holds => is_viol(r),
                Oracle::Violated => is_holds(r),
                Oracle::NeverHolds => is_holds(r),
                Oracle::HardUnknown => false,
            };
            if bad {
                soundness_violations.push(format!("{}::{} = {}", c.name, lever, v(r)));
            }
        }

        // COMPLETENESS — a decidable case is decided by at least one lever.
        match c.oracle {
            Oracle::Holds => assert!(
                decides(&dflt, PropertyVerdict::Holds)
                    || decides(&wp, PropertyVerdict::Holds)
                    || decides(&craig, PropertyVerdict::Holds),
                "{} (HOLDS) is decided by no lever",
                c.name
            ),
            Oracle::Violated => assert!(
                decides(&dflt, PropertyVerdict::Violated)
                    || decides(&wp, PropertyVerdict::Violated),
                "{} (VIOLATED) is decided by no lever",
                c.name
            ),
            _ => {}
        }

        // Craig's UNIQUE reach: a case cube-wp leaves ⊥ that cube-craig decides.
        if matches!(wp, Ok(PropertyVerdict::Unknown)) && is_holds(&craig) {
            craig_unique.push(c.name);
        }
    }

    println!("{}", "-".repeat(88));
    assert!(
        soundness_violations.is_empty(),
        "SOUNDNESS VIOLATIONS (a lever contradicted the oracle): {soundness_violations:?}"
    );

    // Craig's MARGINAL reach is MEASURED, not presupposed. Through the production pipeline the
    // WP path already runs guard-atom extraction (reads a syntactic invariant like `data==target`
    // off the design's own `eq` node), so it subsumes the invariant-class cases whose invariant is
    // syntactically present. Craig's unique value is only a NON-syntactic emergent invariant — the
    // research frontier. An empty `craig_unique` here is a real finding: on this set the shipped
    // guard-atom lever covers what Craig would, so a Craig planner-operator adds no marginal reach.
    if mathsat {
        if craig_unique.is_empty() {
            println!(
                "MEASURED: cube-craig has NO unique reach on this set (WP+guard-atoms subsume it). \
                 A Craig operator adds no marginal decision here; its value needs a NON-syntactic \
                 emergent-invariant case (research frontier — not yet in the set)."
            );
        } else {
            println!("cube-craig UNIQUELY decided (wp ⊥ → craig HOLDS): {craig_unique:?}");
        }
    } else {
        println!(
            "MathSAT absent (MUNUNU_MATHSAT_PATH unset): cube-craig falls back to WP; \
             run in mununu-sva-pono to exercise + measure the Craig column."
        );
    }
}
