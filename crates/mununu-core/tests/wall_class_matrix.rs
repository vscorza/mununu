//! Track R — the **wall-class lever-evaluation matrix** (`.claude/plans/wall-class-lever-evaluation.md`).
//!
//! A FIXED set of class-representative recoverability cases (§ `CASES`) × the mechanisms with a
//! corpus claim — the SINGLETONS (`default` = exact-first+WP · `cube-wp` · `cube-craig`) AND the
//! `combined` COMPOSED plan (config-pin + exact + cube + ranking + guard + Craig, via
//! [`solve_recoverability_combined`]) — run as ONE matrix so a lever is judged on the class it
//! TARGETS, not on whatever design happened to be handy. Crucially, no single mechanism is
//! expected to flip a ⊥ alone: the `combined` column measures whether COMPOSITION decides what
//! every singleton leaves ⊥ (the planner's raison d'être). The run prints a `class × lever →
//! verdict` table and asserts the SOUNDNESS invariant (no lever contradicts the oracle) + that
//! each decidable case is decided by at least one lever. A `*` on a `combined` verdict = the
//! decision is config-SCOPED (pins were applied).
//!
//! `#[ignore]` — a validation HARNESS (some cones are wide + the Craig lever needs MathSAT), run on
//! demand: `cargo test -p mununu-core --test wall_class_matrix -- --ignored --nocapture`
//! (in `mununu-sva-pono` with `MUNUNU_MATHSAT_PATH` set, the `cube-craig` column comes alive).

use mununu_core::adapter::btor2::cegar::PredicateSource;
use mununu_core::adapter::btor2::pin::pin_inputs_to_constants;
use mununu_core::adapter::recoverability::{
    verify_recoverability, verify_recoverability_scalable,
    verify_recoverability_scalable_with_source,
};
use mununu_core::planner::solve_recoverability_combined;
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
    /// Provenance: `"RTL:<design>"` for a lifted design, `"synthetic"` for a hand-written
    /// class fixture. The requirement (2026-07-26) is an RTL representative for every class;
    /// synthetic fixtures are labelled "awaiting RTL" so the gap is explicit + trackable.
    source: &'static str,
    /// Config/reset inputs baked to constants BEFORE the levers run (e.g. `rst_ni=1` to pin an
    /// active-low reset inactive → the operational, no-reset recoverability question). A pinned
    /// verdict is config-SCOPED but sound (the exact engine on the concrete pinned model is
    /// 2-valued sound); used to source the RTL VIOLATED class from a reset-recovering FSM.
    pin: &'static [(&'static str, u64)],
}

fn is_rtl(c: &Case) -> bool {
    c.source.starts_with("RTL")
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

// RTL — lifted OpenTitan aes_cipher_control_fsm (5-state sparse FSM), via the Auto→slang
// fallback (#396; read_verilog rejects its `module … import aes_pkg::*;`). Two classes from one
// design: free-reset recoverability HOLDS (reset clears any state); reset PINNED (`rst_ni=1`,
// the standard operational reset-gating) → VIOLATED (the FSM traps in its absorbing error state,
// only reset recovers). Reproduce: examples/verify/dc_opentitan_aes_cipher_control_fsm/source.
const AES_CIPHER: &str = include_str!("fixtures/wall_classes/aes_cipher_control_fsm.btor");
// More RTL FSMs of the SAME two classes (breadth across designs, via the #396 Auto→slang
// fallback): OpenTitan csrng_main_sm (single 6-bit sparse FSM, idle MainSmIdle=55) + aes_ctr_fsm
// (idle CTR_IDLE=14). Same pattern: free-reset HOLDS, reset-pinned `rst_ni=1` VIOLATED.
const CSRNG: &str = include_str!("fixtures/wall_classes/csrng_main_sm.btor");
const AES_CTR: &str = include_str!("fixtures/wall_classes/aes_ctr_fsm.btor");

const NO_PIN: &[(&str, u64)] = &[];
const PIN_RST: &[(&str, u64)] = &[("rst_ni", 1)];

fn cases() -> Vec<Case> {
    vec![
        // ---- synthetic class fixtures (mechanism checks; awaiting RTL representatives) ----
        Case {
            name: "responder",
            class: "decidable-exact/HOLDS",
            btor2: RESPONDER,
            target: "st == 0",
            oracle: Oracle::Holds,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "staller",
            class: "decidable-exact/VIOLATED",
            btor2: STALLER,
            target: "st == 0",
            oracle: Oracle::Violated,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "wide_recoverable",
            class: "wide-cube/HOLDS",
            btor2: WIDE_RECOVERABLE,
            target: "st == 0",
            oracle: Oracle::Holds,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "wide_trap",
            class: "wide-cube/VIOLATED",
            btor2: WIDE_TRAP,
            target: "st == 0",
            oracle: Oracle::Violated,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "trap_uf_w48",
            class: "uf-wrap/soundness",
            btor2: TRAP_UF_W48,
            target: "ctrl == 0",
            oracle: Oracle::NeverHolds,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "pos_w48",
            class: "wide-datapath-return/HOLDS",
            btor2: POS_W48,
            target: "ctrl == 0",
            oracle: Oracle::Holds,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "class2_datadep",
            class: "guard-atom/HOLDS",
            btor2: CLASS2_DATADEP,
            target: "ctrl == 0",
            oracle: Oracle::Holds,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "craig_emergent",
            class: "invariant-class/HOLDS",
            btor2: CRAIG_EMERGENT,
            target: "busy == 0",
            oracle: Oracle::Holds,
            source: "synthetic",
            pin: NO_PIN,
        },
        // ---- RTL class-representatives (lifted designs — the curation target) ----
        Case {
            name: "i2c_scl_padoen",
            class: "register-dominated/⊥",
            btor2: I2C_SCL_PADOEN,
            target: "scl_padoen_o == 1",
            oracle: Oracle::HardUnknown,
            source: "RTL:i2c",
            pin: NO_PIN,
        },
        Case {
            name: "aes_cipher (free)",
            class: "decidable/HOLDS",
            btor2: AES_CIPHER,
            target: "aes_cipher_ctrl_cs == 9",
            oracle: Oracle::Holds,
            source: "RTL:opentitan-aes",
            pin: NO_PIN,
        },
        Case {
            name: "aes_cipher (rst pinned)",
            class: "operational/VIOLATED",
            btor2: AES_CIPHER,
            target: "aes_cipher_ctrl_cs == 9",
            oracle: Oracle::Violated,
            source: "RTL:opentitan-aes",
            pin: PIN_RST,
        },
        Case {
            name: "csrng (free)",
            class: "decidable/HOLDS",
            btor2: CSRNG,
            target: "state_q == 55",
            oracle: Oracle::Holds,
            source: "RTL:opentitan-csrng",
            pin: NO_PIN,
        },
        Case {
            name: "csrng (rst pinned)",
            class: "operational/VIOLATED",
            btor2: CSRNG,
            target: "state_q == 55",
            oracle: Oracle::Violated,
            source: "RTL:opentitan-csrng",
            pin: PIN_RST,
        },
        Case {
            name: "aes_ctr (free)",
            class: "decidable/HOLDS",
            btor2: AES_CTR,
            target: "aes_ctr_cs == 14",
            oracle: Oracle::Holds,
            source: "RTL:opentitan-aes",
            pin: NO_PIN,
        },
        Case {
            name: "aes_ctr (rst pinned)",
            class: "operational/VIOLATED",
            btor2: AES_CTR,
            target: "aes_ctr_cs == 14",
            oracle: Oracle::Violated,
            source: "RTL:opentitan-aes",
            pin: PIN_RST,
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
        "\n{:32} {:26} {:>6} {:>6} {:>6} {:>7}",
        "case", "class", "dflt", "wp", "craig", "comb"
    );
    println!("{}", "-".repeat(92));

    let mut soundness_violations = Vec::new();
    let mut craig_unique = Vec::new();
    let mut combined_fullspace = Vec::new(); // ⊥-singletons → combined decides with NO pins (transfers)
    let mut combined_scoped = Vec::new(); // ⊥-singletons → combined decides but pins applied (scoped)

    let mut rtl_classes = std::collections::BTreeSet::new();
    let mut synthetic_classes = std::collections::BTreeSet::new();

    for c in cases() {
        // Bake the case's reset/config pins (e.g. `rst_ni=1` — the standard operational
        // reset-gating) into the model BEFORE the levers. Sound: the exact engine on the
        // concrete pinned model is 2-valued sound (the operational, out-of-reset question).
        let model = if c.pin.is_empty() {
            c.btor2.to_string()
        } else {
            let pins: Vec<(String, u64)> = c.pin.iter().map(|(n, v)| (n.to_string(), *v)).collect();
            pin_inputs_to_constants(c.btor2, &pins).0
        };
        if is_rtl(&c) {
            rtl_classes.insert(c.class);
        } else {
            synthetic_classes.insert(c.class);
        }
        let dflt = verify_recoverability(&model, c.target);
        let wp = verify_recoverability_scalable(&model, c.target, &[]);
        let craig = verify_recoverability_scalable_with_source(
            &model,
            c.target,
            &[],
            PredicateSource::CraigInterpolation,
        );
        // The COMPOSED plan (config-pin + exact + cube + ranking + guard + Craig) — the whole
        // point of a planner: a lever that is 0 alone can contribute in combination. A HOLDS with
        // pins applied is SCOPED to that configuration (marked `*`).
        let (combined, pins) = solve_recoverability_combined(&model, c.target);
        let comb_str = if pins.is_empty() {
            v(&combined).to_string()
        } else {
            format!("{}*", v(&combined).trim())
        };
        println!(
            "{:32} {:26} {:>6} {:>6} {:>6} {:>7}",
            c.name,
            c.class,
            v(&dflt),
            v(&wp),
            v(&craig),
            comb_str
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

        // The COMBINE-MECHANISMS hypothesis: a case EVERY singleton leaves ⊥ that the composed
        // plan DECIDES. Split by soundness — a decision reached with NO pins is a genuine
        // FULL-SPACE win (transfers); one reached WITH config-pins is SCOPED: recoverability
        // (AG EF) is not monotone under input-restriction, so a config-pinned verdict answers only
        // "recoverable under held inputs" and does NOT transfer to the full space.
        let all_singletons_bottom = [&dflt, &wp, &craig]
            .iter()
            .all(|r| matches!(r, Ok(PropertyVerdict::Unknown)));
        let combined_decides = matches!(
            combined,
            Ok(PropertyVerdict::Holds | PropertyVerdict::Violated)
        );
        if all_singletons_bottom && combined_decides {
            if pins.is_empty() {
                combined_fullspace.push(c.name);
            } else {
                combined_scoped.push(c.name);
            }
        }
    }

    println!("{}", "-".repeat(92));
    assert!(
        soundness_violations.is_empty(),
        "SOUNDNESS VIOLATIONS (a lever contradicted the oracle): {soundness_violations:?}"
    );

    // The COMBINE-MECHANISMS hypothesis (the planner's raison d'être) — measured, not assumed,
    // split by what actually TRANSFERS. A `*` in the table = the combined verdict is config-scoped.
    if combined_fullspace.is_empty() {
        println!(
            "MEASURED: no genuine FULL-SPACE combination win on this set. The composed plan's only \
             ⊥→decided cases are config-SCOPED (pins applied): {combined_scoped:?} — a valid but \
             weaker 'recoverable under held inputs' sub-question that does NOT transfer (AG EF is \
             not monotone under input-restriction; pinning `go`/`start`=0 hides the trap). The \
             sound verdict-PRESERVING shrinkers (F1/F2/COI) save ~0 on the data-dependent ⊥, so \
             composition adds no full-space decision here. Combination stays the right DEFAULT; \
             this set simply has no combination-only full-space-decidable case yet."
        );
    } else {
        println!(
            "COMBINATION WIN (full-space, transfers — decided with no pins where every singleton \
             was ⊥): {combined_fullspace:?}   [config-scoped-only: {combined_scoped:?}]"
        );
    }

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

    // RTL-COVERAGE tracker (requirement 2026-07-26: an RTL representative for every class). The
    // synthetic fixtures are valid MECHANISM checks; RTL adds realism. This prints the classes
    // that HAVE a lifted-RTL representative vs those still synthetic-only, so the gap is explicit
    // and shrinks visibly as P1.1 lifts more designs (OpenTitan via the #396 Auto→slang fallback).
    println!("{}", "-".repeat(92));
    println!("RTL-backed classes: {rtl_classes:?}");
    let synthetic_only: Vec<_> = synthetic_classes.difference(&rtl_classes).collect();
    println!("synthetic-only (awaiting RTL representative): {synthetic_only:?}");
}
