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
    verify_recoverability_scalable_with_source, verify_recoverability_spcr_only,
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
1 sort bitvec 300
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
1 sort bitvec 300
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

// ARRAY-CONTENT-GATED νµ recoverability (the SPCR class, PR #410). `AG EF(busy==0)` where recovery
// routes through ARRAY CONTENT read at a latched index: `busy` clears only when `mem[key]==all-ones`.
// exact-symbolic SKIPs the in-cone `$mem`; the plain cube's must-edge is an AUFBV ∀-over-array query
// → Unknown → the νµ abstains (⊥). SPCR registerizes the accessed cell + drops the array → QF_BV →
// exact decides. TWO sibling cases mark the FRAGMENT BOUNDARY (the whole point of the class):
//   (decides) `key` latches `waddr` (the write address) → the index only ever moves TO the written
//             cell → `mem'[key']=mem'[waddr]=wdata` exact → SPCR applies → HOLDS.
//   (abstains) `key` latches a free input `sel` INDEPENDENT of `waddr` → the index jumps to an
//             arbitrary past-written cell no finite prophecy-set tracks → SPCR soundly abstains
//             (`spcr` col = SKIP) → exact SKIP + ∀-array cube ⊥ → HardUnknown.
// synthetic: the class is measured-ABSENT from real RTL corpora (0/135 AssertLLM2, 0/315 HWMCC20,
// full OpenTitan hw/ip sweep — the only near-miss, the OTBN loop stack, recovers via a down-counter
// + PC-match, not a stored-content value-test), so there is no RTL representative to lift.
const ARRAY_CONTENT_SPCR: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 state 1 busy
8 state 2 key
9 state 3 mem
10 const 1 0
11 init 1 7 10
12 const 2 00
13 init 2 8 12
14 const 2 11
15 read 2 9 8
16 eq 1 15 14
17 not 1 7
18 and 1 4 17
19 const 1 1
20 and 1 7 16
21 ite 1 20 10 7
22 ite 1 18 19 21
23 next 1 7 22
24 ite 2 18 5 8
25 next 2 8 24
26 write 3 9 5 6
27 next 3 9 26
";

// Sibling of ARRAY_CONTENT_SPCR with ONE change: `key` latches the independent input `sel` (nid 28)
// instead of `waddr` (nid 5) — the outside-the-fragment boundary case (SPCR abstains soundly).
const ARRAY_CONTENT_INDEP: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort array 2 2
4 input 1 start
5 input 2 waddr
6 input 2 wdata
7 state 1 busy
8 state 2 key
9 state 3 mem
10 const 1 0
11 init 1 7 10
12 const 2 00
13 init 2 8 12
14 const 2 11
15 read 2 9 8
16 eq 1 15 14
17 not 1 7
18 and 1 4 17
19 const 1 1
20 and 1 7 16
21 ite 1 20 10 7
22 ite 1 18 19 21
23 next 1 7 22
24 ite 2 18 28 8
25 next 2 8 24
26 write 3 9 5 6
27 next 3 9 26
28 input 2 sel
";

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
        // The SPCR class (PR #410) + its fragment boundary. `array_content_spcr` is decided ONLY via
        // the SPCR pre-pass (exact SKIPs the in-cone array; the ∀-array cube leaves ⊥) — so the
        // completeness assertion below is a live regression guard on SPCR's marginal reach. The
        // independent-index sibling is HardUnknown (SPCR soundly abstains → the class's honest ⊥).
        Case {
            name: "array_content_spcr",
            class: "array-content-gated/HOLDS",
            btor2: ARRAY_CONTENT_SPCR,
            target: "busy == 0",
            oracle: Oracle::Holds,
            source: "synthetic",
            pin: NO_PIN,
        },
        Case {
            name: "array_content_indep_idx",
            class: "array-content-indep-idx/⊥",
            btor2: ARRAY_CONTENT_INDEP,
            target: "busy == 0",
            oracle: Oracle::HardUnknown,
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
        "\n{:32} {:26} {:>6} {:>6} {:>6} {:>6} {:>7}",
        "case", "class", "dflt", "wp", "craig", "spcr", "comb"
    );
    println!("{}", "-".repeat(98));

    let mut soundness_violations = Vec::new();
    let mut craig_unique = Vec::new();
    let mut spcr_decided = Vec::new(); // array-content cases the SPCR pre-pass decides (its reach)
    let mut combined_fullspace = Vec::new(); // ⊥-singletons → combined decides with NO pins (transfers)

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
        // The SPCR pre-pass in ISOLATION (its own `spcr` column): Holds/Violated when SPCR applies +
        // exact decides the array-free design, Skipped when SPCR does not apply (no array / outside
        // the fragment → sound abstention). Attributes an array-content decision to SPCR — otherwise
        // invisible because SPCR is embedded inside `dflt`/`wp`.
        let spcr = verify_recoverability_spcr_only(&model, c.target);
        // The COMPOSED plan (exact-first + cube + ranking + guard + Craig) — the whole point of a
        // planner: a lever that is 0 alone can contribute in combination. It runs only SOUND,
        // transferable levers (the earlier unsound auto-input-pinning was removed — see
        // `solve_recoverability_combined`), so its verdict is full-space and oracle-comparable.
        let combined = solve_recoverability_combined(&model, c.target);
        println!(
            "{:32} {:26} {:>6} {:>6} {:>6} {:>6} {:>7}",
            c.name,
            c.class,
            v(&dflt),
            v(&wp),
            v(&craig),
            v(&spcr),
            v(&combined)
        );

        // SOUNDNESS invariant — no lever may contradict the oracle. The `combined` plan is included:
        // it now returns only full-space, transferable verdicts, so a contradiction would be a real
        // composition-soundness bug (the gap the removed input-pinning previously hid).
        for (lever, r) in [
            ("default", &dflt),
            ("cube-wp", &wp),
            ("cube-craig", &craig),
            ("spcr", &spcr),
            ("combined", &combined),
        ] {
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
                    || decides(&craig, PropertyVerdict::Holds)
                    || decides(&spcr, PropertyVerdict::Holds),
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

        // SPCR's reach: an array-content case the SPCR pre-pass decides (Holds/Violated) in
        // isolation. exact-alone SKIPs it (in-cone `$mem`) and the plain ∀-array cube leaves ⊥, so a
        // decision here is SPCR's marginal contribution.
        if is_holds(&spcr) || is_viol(&spcr) {
            spcr_decided.push(c.name);
        }

        // The COMBINE-MECHANISMS hypothesis: a case EVERY singleton leaves ⊥ that the composed
        // plan DECIDES — a genuine FULL-SPACE combination win (transfers). The composed plan now
        // runs only SOUND, transferable levers (no unsound input-pinning), so any decision it
        // reaches is full-space and oracle-comparable.
        let all_singletons_bottom = [&dflt, &wp, &craig]
            .iter()
            .all(|r| matches!(r, Ok(PropertyVerdict::Unknown)));
        let combined_decides = matches!(
            combined,
            Ok(PropertyVerdict::Holds | PropertyVerdict::Violated)
        );
        if all_singletons_bottom && combined_decides {
            combined_fullspace.push(c.name);
        }
    }

    println!("{}", "-".repeat(92));
    assert!(
        soundness_violations.is_empty(),
        "SOUNDNESS VIOLATIONS (a lever contradicted the oracle): {soundness_violations:?}"
    );

    // The COMBINE-MECHANISMS hypothesis (the planner's raison d'être) — measured, not assumed. The
    // composed plan is the union of its SOUND component levers (exact-first ∪ scalable-cube), so a
    // win here is a case every singleton leaves ⊥ that their sound composition decides + TRANSFERS.
    if combined_fullspace.is_empty() {
        println!(
            "MEASURED: no genuine FULL-SPACE combination win on this set. Among the SOUND, \
             transferable levers the composed plan is the union of exact-first and the scalable-cube \
             path (cube+ranking+guard+Craig): every case is decided by a single sound lever, so \
             composition adds only union COVERAGE, no synergy. (The earlier apparent 'win' — \
             `staller` HOLDS* / `trap_uf_w48` HOLDS* — was the removed UNSOUND auto-input-pinning \
             producing non-transferable scoped verdicts; AG EF is not monotone under \
             input-restriction, so those never transferred.) Combination stays the right DEFAULT for \
             COVERAGE + the soundness cross-check (exact corroborating/overturning cube); it is not a \
             new decidability lever on this set."
        );
    } else {
        println!(
            "COMBINATION WIN (full-space, transfers — every singleton was ⊥, the sound composition \
             decided): {combined_fullspace:?}"
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

    // SPCR's MARGINAL reach (PR #410) — MEASURED, not presupposed. The `spcr` column decides the
    // array-content-gated νµ class (`array_content_spcr`) that exact-alone SKIPs (in-cone `$mem`) and
    // the plain ∀-array cube leaves ⊥; on every non-array case it correctly SKIPs (no marginal reach,
    // no harm), and on the independent-index sibling it SKIPs too (the sound fragment boundary). The
    // class is measured-ABSENT from real RTL corpora (2026-07-29), so this is a MECHANISM guard.
    println!("{}", "-".repeat(98));
    if spcr_decided.is_empty() {
        println!(
            "MEASURED: SPCR decides nothing on this set — a REGRESSION (the array-content case must \
             be SPCR-decided; exact SKIPs it and the ∀-array cube is ⊥)."
        );
    } else {
        println!(
            "SPCR uniquely enables the array-content-gated νµ class (exact SKIP + ∀-array cube ⊥ → \
             SPCR HOLDS): {spcr_decided:?}. Fragment boundary: the independent-index sibling → SKIP \
             (sound abstain). Class measured-ABSENT from real RTL — mechanism guard, not corpus-decide."
        );
    }
    // Guard the reach so a silent SPCR regression fails the matrix, not just the printout.
    assert!(
        spcr_decided.contains(&"array_content_spcr"),
        "SPCR must decide the array-content-gated νµ case (its whole reach); exact SKIPs it and the \
         ∀-array cube leaves ⊥, so a non-decision is an SPCR regression"
    );

    // RTL-COVERAGE tracker (requirement 2026-07-26: an RTL representative for every class). The
    // synthetic fixtures are valid MECHANISM checks; RTL adds realism. This prints the classes
    // that HAVE a lifted-RTL representative vs those still synthetic-only, so the gap is explicit
    // and shrinks visibly as P1.1 lifts more designs (OpenTitan via the #396 Auto→slang fallback).
    println!("{}", "-".repeat(92));
    println!("RTL-backed classes: {rtl_classes:?}");
    let synthetic_only: Vec<_> = synthetic_classes.difference(&rtl_classes).collect();
    println!("synthetic-only (awaiting RTL representative): {synthetic_only:?}");
}

/// Config-partition on REAL OpenTitan RTL — the refined-verdicts capability-A industrial validation
/// (A3). The `AG EF(idle)` recoverability of the aes_cipher / csrng control FSMs DEPENDS on `rst_ni`:
/// held in reset (`rst_ni=0`, active-low) the FSM stays idle (HOLDS); operational (`rst_ni=1`) it
/// traps in its absorbing error state (VIOLATED). `config_partition` turns that branching pair — the
/// SVA-inexpressible differentiator (§3.3) — into ONE `ConfigDependent` verdict. Sound per cell (each
/// pinned `rst_ni` is a concrete model the exact engine decides). Host-runnable on the checked-in
/// lifted fixtures (small FSMs), so NOT `#[ignore]` — it gates `make ci`.
#[test]
fn config_partition_over_reset_partitions_opentitan_fsms() {
    use mununu_core::adapter::recoverability::config_partition;

    let aes = config_partition(
        AES_CIPHER,
        "aes_cipher_ctrl_cs == 9",
        &[("rst_ni".to_string(), vec![0, 1])],
    )
    .expect("aes_cipher AG EF(idle) depends on rst_ni ⇒ a ConfigDependent partition");
    assert!(
        aes.violated.contains(&vec![("rst_ni".to_string(), 1)]),
        "operational (rst_ni=1) traps in the absorbing error state: {aes:?}"
    );
    assert!(
        aes.holds.contains(&vec![("rst_ni".to_string(), 0)]),
        "held in reset (rst_ni=0) stays idle: {aes:?}"
    );

    let csrng = config_partition(
        CSRNG,
        "state_q == 55",
        &[("rst_ni".to_string(), vec![0, 1])],
    )
    .expect("csrng_main_sm AG EF(idle) depends on rst_ni ⇒ a ConfigDependent partition");
    assert!(
        csrng.violated.contains(&vec![("rst_ni".to_string(), 1)]),
        "operational csrng traps: {csrng:?}"
    );
    assert!(
        csrng.holds.contains(&vec![("rst_ni".to_string(), 0)]),
        "held-in-reset csrng stays idle: {csrng:?}"
    );
}
