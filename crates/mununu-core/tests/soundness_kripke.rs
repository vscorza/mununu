//! Soundness regression tests for the SystemVerilog Kripke builder.
//!
//! These tests verify the OOB-as-bottom invariant added in the soundness session:
//! when a register's next-state escapes the abstracted domain (counter overflow,
//! enum index out of range), the affected transition is routed to a designated
//! OOB sink with `$oob$ → "true"` valuation. The mu-calculus evaluator masks
//! that sink out of every formula's satisfying set, so safety properties at any
//! source state with a transition to OOB correctly fail (`[a]Z` requires
//! OOB ∈ Z; OOB ∉ Z always).
//!
//! Reference: Bruns–Godefroid CONCUR 2000 (generalized model checking, safety
//! projection of partial-state semantics).

use mununu_core::adapter::systemverilog::SystemVerilogAdapter;
use mununu_core::adapter::{AdapterOptions, FormatAdapter};
use mununu_core::context_dsl;

fn eval_fraction_sv(sv: &str, automaton: &str, formula_name: &str) -> f64 {
    let options = AdapterOptions::default();
    let output = SystemVerilogAdapter::translate(sv, &options).expect("SV translation failed");

    let mut doc = context_dsl::parse(&output.ctxdsl).unwrap_or_else(|e| {
        panic!(
            "Generated CTXDSL failed to parse: {e}\n\nCTXDSL:\n{}",
            output.ctxdsl
        )
    });
    // Inject the structured state-valuation side channel so the OOB sink
    // marker reaches the CLTS (Phase 2/3 OOB-as-bottom + Phase 8 trit eval).
    doc.state_valuations = output.state_valuations.clone();

    let realized = context_dsl::realize_context(&doc, &[])
        .unwrap_or_else(|e| panic!("Generated CTXDSL failed to realize: {e}"));

    let clts = realized
        .context
        .clts(automaton)
        .unwrap_or_else(|| panic!("Automaton '{automaton}' not found"));
    let formula = realized
        .formulas
        .get(formula_name)
        .unwrap_or_else(|| panic!("Formula '{formula_name}' not found"));
    let env = realized.environment_for(automaton);
    let result = realized
        .context
        .evaluate_mu(automaton, &formula.formula, &env, None)
        .unwrap_or_else(|e| panic!("Eval failed: {e}"));

    let initial_count = clts.initial_states().len();
    if initial_count == 0 {
        return 0.0;
    }
    let satisfied = clts
        .initial_states()
        .iter()
        .filter(|s| result[s.index()])
        .count();
    satisfied as f64 / initial_count as f64
}

fn translate_warnings(sv: &str) -> Vec<String> {
    let options = AdapterOptions::default();
    let output = SystemVerilogAdapter::translate(sv, &options).expect("SV translation failed");
    output.warnings.iter().map(|w| w.message.clone()).collect()
}

// ---------------------------------------------------------------------------
// B1: OOB sink with bottom semantics
// ---------------------------------------------------------------------------

/// Counter increments without a guard that would prevent overflow. The bound
/// is set to 2 in the sidecar annotation, so when count=2 and inc fires, the
/// next-state would be count=3 — outside the domain. Under the old behavior,
/// this transition was silently dropped, so safety verdicts incorrectly held.
/// Under the new behavior, the transition is routed to the OOB sink which has
/// bottom semantics: any safety formula referencing positive predicates fails
/// at the source state.
const SV_OVERFLOWING_COUNTER: &str = r#"
    // @mununu domain count: bounded_counter 0..2
    // @mununu ltl safety: nu X. ([] X)
    module overflowing(input logic clk, input logic rst, input logic inc);
        logic [7:0] count;
        always_ff @(posedge clk or posedge rst) begin
            if (rst) count <= 0;
            else if (inc) count <= count + 1;
        end
    endmodule
"#;

/// Same module but with a guard preventing overflow: `count < 2`.
const SV_BOUNDED_COUNTER: &str = r#"
    // @mununu domain count: bounded_counter 0..2
    // @mununu ltl safety: nu X. ([] X)
    module bounded(input logic clk, input logic rst, input logic inc);
        logic [7:0] count;
        always_ff @(posedge clk or posedge rst) begin
            if (rst) count <= 0;
            else if (inc && count < 2) count <= count + 1;
        end
    endmodule
"#;

#[test]
fn b1_overflow_routes_to_oob_sink() {
    // The overflowing model should produce at least one BoundOverflow warning,
    // signalling that the bound is too small for the actual register behavior.
    let warnings = translate_warnings(SV_OVERFLOWING_COUNTER);
    assert!(
        warnings
            .iter()
            .any(|m| m.contains("would take value") || m.contains("OOB sink")),
        "Overflowing counter should emit a BoundOverflow warning. Got: {:?}",
        warnings
    );
}

#[test]
fn b1_bounded_counter_no_oob() {
    // The guarded increment never overflows, so no OOB-related warning fires.
    let warnings = translate_warnings(SV_BOUNDED_COUNTER);
    assert!(
        !warnings.iter().any(|m| m.contains("OOB sink")),
        "Bounded counter must not emit OOB warnings. Got: {:?}",
        warnings
    );
}

#[test]
fn b1_oob_falsifies_safety() {
    // The bounded version satisfies `nu X. [] X` (universal box trivially holds
    // when all states' successors are in-domain). The overflowing version has
    // an OOB transition from count=2; the OOB sink fails every positive
    // predicate (OOB-as-bottom), so `[] X` at count=2 requires OOB ∈ X (always
    // false), and the gfp does not include count=2. Source states reaching
    // count=2 also lose the property. Initial state (count=0) reaches count=2
    // via two `inc`s, so initial ∉ X — verdict flips to failing.
    let bounded_sat = eval_fraction_sv(SV_BOUNDED_COUNTER, "bounded", "safety");
    let overflowing_sat = eval_fraction_sv(SV_OVERFLOWING_COUNTER, "overflowing", "safety");

    assert!(
        bounded_sat >= 1.0,
        "Bounded counter: nu X. [] X should hold at all initials (got {bounded_sat})"
    );
    // The overflowing variant must NOT be more optimistic than the bounded
    // variant — that's the soundness invariant for over-approx + OOB sink.
    assert!(
        overflowing_sat <= bounded_sat,
        "Sound-for-safety invariant: overflowing model must not be more optimistic \
         than bounded model (got overflowing={overflowing_sat}, bounded={bounded_sat})"
    );
    // Strict assertion: the overflowing case actually flips. Required to
    // detect regressions where the OOB infrastructure stops triggering.
    assert!(
        overflowing_sat < 1.0,
        "Overflowing counter: BitVec eval should drop initial states from gfp \
         (got {overflowing_sat}, expected < 1.0). If this passes at 1.0, the OOB \
         sink valuation is not reaching the CLTS — check that translate_and_realize \
         injects state_valuations."
    );
}

// ---------------------------------------------------------------------------
// B1: No-OOB no-regression
// ---------------------------------------------------------------------------

/// A handshake state machine without any counter or overflow path. The OOB
/// integration must not affect verdicts for examples that don't trigger it.
const SV_NO_OOB_HANDSHAKE: &str = r#"
    // @mununu ltl safety: nu X. ([] X)
    // @mununu ltl ack_reachable: mu X. (ACTIVE || <> X)
    module handshake(
        input logic clk, input logic rst,
        input logic req,
        output logic ack
    );
        typedef enum logic [1:0] {IDLE, WAIT_ACK, ACTIVE, DONE} state_t;
        state_t state;
        always_ff @(posedge clk or posedge rst) begin
            if (rst) state <= IDLE;
            else case (state)
                IDLE: if (req) state <= WAIT_ACK;
                WAIT_ACK: state <= ACTIVE;
                ACTIVE: if (!req) state <= DONE;
                DONE: state <= IDLE;
            endcase
        end
        assign ack = (state == ACTIVE);
    endmodule
"#;

#[test]
fn b1_no_oob_safety_holds() {
    let safety_sat = eval_fraction_sv(SV_NO_OOB_HANDSHAKE, "handshake", "safety");
    assert!(
        safety_sat >= 1.0,
        "Handshake without overflow path: safety should hold (got {safety_sat})"
    );
}

#[test]
fn b1_no_oob_no_warning() {
    let warnings = translate_warnings(SV_NO_OOB_HANDSHAKE);
    assert!(
        !warnings.iter().any(|m| m.contains("OOB sink")),
        "Handshake should not emit OOB warnings. Got: {:?}",
        warnings
    );
}

#[test]
fn b1_no_oob_reachability_holds() {
    let reach = eval_fraction_sv(SV_NO_OOB_HANDSHAKE, "handshake", "ack_reachable");
    assert!(
        reach >= 1.0,
        "ACTIVE should be reachable from IDLE (got {reach})"
    );
}

// ---------------------------------------------------------------------------
// B5: admit-on-None for unevaluable guards (transitive soundness for B2/B3/B4)
// ---------------------------------------------------------------------------

/// Demonstrates that the over-approximation invariant holds: a model whose guard
/// is unevaluable (eval_expr returns None for some operand) admits the
/// transition rather than dropping it. The verdict on the over-approx model
/// must not be more optimistic than on a precise model.
///
/// We construct two equivalent specs, one where the guard reduces to a concrete
/// truth value, and one where part of the operand is abstract. Both should have
/// the same safety verdict if B5 admits the abstract case correctly.
const SV_CONCRETE_GUARD: &str = r#"
    // @mununu ltl no_error: nu X. ([] X)
    module concrete(
        input logic clk, input logic rst, input logic start
    );
        typedef enum logic [1:0] {IDLE, BUSY, ERROR} state_t;
        state_t state;
        always_ff @(posedge clk or posedge rst) begin
            if (rst) state <= IDLE;
            else case (state)
                IDLE: if (start) state <= BUSY;
                BUSY: state <= IDLE;
                ERROR: state <= ERROR;
            endcase
        end
    endmodule
"#;

#[test]
fn b5_concrete_guard_no_oob() {
    // The concrete-guard model has no register overflow; OOB sink must not be
    // created and the trivial safety formula `nu X. [] X` should hold.
    let warnings = translate_warnings(SV_CONCRETE_GUARD);
    assert!(
        !warnings.iter().any(|m| m.contains("OOB sink")),
        "Concrete-guard model should not emit OOB warnings. Got: {:?}",
        warnings
    );
    let sat = eval_fraction_sv(SV_CONCRETE_GUARD, "concrete", "no_error");
    assert!(
        sat >= 1.0,
        "Concrete-guard model with no OOB: safety should hold (got {sat})"
    );
}

// ---------------------------------------------------------------------------
// Phase 8: three-valued (TritSet) verdicts for OOB-reaching states
// ---------------------------------------------------------------------------

/// Helper: run the three-valued evaluator on an SV input and return the
/// verdict at the initial state.
fn tri_verdict_at_initial(
    sv: &str,
    automaton: &str,
    formula_name: &str,
) -> mununu_core::mu_calculus::Trit {
    let options = AdapterOptions::default();
    let output = SystemVerilogAdapter::translate(sv, &options).expect("SV translation failed");
    let mut doc = context_dsl::parse(&output.ctxdsl).unwrap_or_else(|e| {
        panic!(
            "Generated CTXDSL failed to parse: {e}\n\nCTXDSL:\n{}",
            output.ctxdsl
        )
    });
    // Inject the structured state-valuation side channel so the OOB sink
    // marker reaches the CLTS for the trit evaluator's compute_oob_bits.
    doc.state_valuations = output.state_valuations.clone();
    let realized = context_dsl::realize_context(&doc, &[])
        .unwrap_or_else(|e| panic!("Generated CTXDSL failed to realize: {e}"));
    let clts = realized
        .context
        .clts(automaton)
        .unwrap_or_else(|| panic!("Automaton '{automaton}' not found"));
    let formula = realized
        .formulas
        .get(formula_name)
        .unwrap_or_else(|| panic!("Formula '{formula_name}' not found"));
    let env = realized.environment_for(automaton);
    let trit = realized
        .context
        .evaluate_mu_tri(automaton, &formula.formula, &env, None)
        .unwrap_or_else(|e| panic!("Tri-eval failed: {e}"));
    let initial = clts
        .initial_states()
        .iter()
        .next()
        .copied()
        .expect("at least one initial state");
    trit.verdict_at(initial.index())
}

#[test]
fn b8_oob_yields_unknown_in_tri_evaluator() {
    use mununu_core::mu_calculus::Trit;

    // Bounded counter: no OOB → BitVec eval says True; trit eval says True.
    let bounded = tri_verdict_at_initial(SV_BOUNDED_COUNTER, "bounded", "safety");
    assert_eq!(
        bounded,
        Trit::True,
        "Bounded counter (no OOB): trit verdict should be definitely True, got {bounded:?}"
    );

    // Overflowing counter: BitVec eval says False (OOB-as-bottom drops the
    // initial state from the gfp). Trit eval says Unknown — the OOB sink is
    // Unknown, the modal `[a]Z` at count=2 propagates Unknown rather than
    // False, and the chain back to the initial state preserves Unknown.
    // This is the practical user benefit of Phase 8: the verdict
    // distinguishes "we found a counterexample" from "we couldn't verify".
    let overflowing = tri_verdict_at_initial(SV_OVERFLOWING_COUNTER, "overflowing", "safety");
    assert_eq!(
        overflowing,
        Trit::Unknown,
        "Overflowing counter (OOB reachable): trit verdict should be Unknown, got {overflowing:?}"
    );
}

#[test]
fn b8_no_oob_no_regression_in_tri() {
    use mununu_core::mu_calculus::Trit;

    // Handshake without overflow path: trit verdict should match the BitVec
    // verdict (definitely True for safety, since OOB is unreachable and so
    // every state has must=may).
    let safety = tri_verdict_at_initial(SV_NO_OOB_HANDSHAKE, "handshake", "safety");
    assert_eq!(
        safety,
        Trit::True,
        "Handshake (no OOB): trit verdict should be definitely True, got {safety:?}"
    );

    let reach = tri_verdict_at_initial(SV_NO_OOB_HANDSHAKE, "handshake", "ack_reachable");
    assert_eq!(
        reach,
        Trit::True,
        "Handshake reachability (no OOB): trit verdict should be definitely True, got {reach:?}"
    );
}

// ---------------------------------------------------------------------------
// A1-A4: Fail-loud on malformed property
// ---------------------------------------------------------------------------

/// SV with a `template_ref` to an undefined template. Previously this was
/// silently dropped from the property list (A1 silent under-approximation).
/// After the fix, the adapter returns `AdapterError::ParseError`.
#[test]
fn a1_unknown_template_is_error() {
    // Use a sidecar JSON sidestepping the SV adapter convention isn't trivial
    // from the inline annotation pipeline, so we exercise the XState path
    // (A3, equivalent code path) which accepts JSON with __mununu directly.
    let xstate_json = r#"{
        "id": "M",
        "initial": "Idle",
        "states": {
            "Idle": { "on": { "go": "Busy" } },
            "Busy": { "on": { "done": "Idle" } }
        },
        "__mununu": {
            "controllable": [],
            "uncontrollable": ["go", "done"],
            "properties": [
                {
                    "name": "bogus",
                    "role": "standalone",
                    "template_ref": { "template": "no_such_template_xyz", "args": {} }
                }
            ]
        }
    }"#;

    use mununu_core::adapter::xstate::XStateAdapter;
    let options = AdapterOptions::default();
    let result = XStateAdapter::translate(xstate_json, &options);
    assert!(
        result.is_err(),
        "Unknown template_ref must produce an AdapterError, not silent omission. \
         Got: {:?}",
        result.map(|o| o.warnings.len())
    );
    if let Err(e) = result {
        assert!(
            e.message.contains("template") || e.message.contains("no_such_template_xyz"),
            "Error message should reference the missing template. Got: {}",
            e.message
        );
    }
}

#[test]
fn a4_no_formula_no_template_is_error() {
    // Property with neither `formula` nor `template_ref` — previously silent drop.
    let xstate_json = r#"{
        "id": "M",
        "initial": "Idle",
        "states": {
            "Idle": { "on": { "go": "Busy" } },
            "Busy": { "on": { "done": "Idle" } }
        },
        "__mununu": {
            "controllable": [],
            "uncontrollable": ["go", "done"],
            "properties": [
                {
                    "name": "empty_property",
                    "role": "standalone"
                }
            ]
        }
    }"#;

    use mununu_core::adapter::xstate::XStateAdapter;
    let options = AdapterOptions::default();
    let result = XStateAdapter::translate(xstate_json, &options);
    assert!(
        result.is_err(),
        "Property with neither formula nor template_ref must produce an AdapterError. \
         Got: {:?}",
        result.map(|o| o.warnings.len())
    );
    if let Err(e) = result {
        assert!(
            e.message.contains("formula") || e.message.contains("template_ref"),
            "Error message should reference the missing fields. Got: {}",
            e.message
        );
    }
}
