// V.6 — Tiny AMBA-style arbiter for R.6.7 controllability-aware
// KMTS demonstration.
//
// Per R.6.7 / V.6 (proof-of-fire industrial milestone):
//   - The R.6 plan §2 primary candidate was the AMBA AHB arbiter
//     from the public SYNTCOMP/TLSF corpus. As reported in the
//     2026-06-09 R.6.7 fixture-path analysis, mununu's TLSF
//     adapter goes directly TLSF→CTXDSL (Sharp-only); the path
//     to KMTS with predicate-abstraction-induced MayOnly edges
//     requires BTOR2 input. This file is the Option B fallback:
//     a hand-authored synthesisable Verilog implementation of an
//     AMBA-style arbiter with a small predicate-abstractable
//     burst counter, runnable through sv2v + Yosys + mununu's
//     existing predicate_cube_lift.
//
// The CLAIM Integrity framing: this is a hand-authored Verilog
// fixture, NOT the public AMBA AHB IP. It demonstrates the
// controllability-aware verdict-divergence pattern that the
// R.6.3/4/5/6 evaluators are designed to produce, on a real
// RTL pipeline (sv2v → Yosys → BTOR2 → predicate_cube_lift).
//
// CONTROLLABILITY SPLIT (per R.6.6 controllability-aware lifter):
//   - Uncontrollable (environment-driven):
//       req_0, req_1            -- client requests
//       clk, rstn               -- clock + reset
//   - Controllable (controller-driven):
//       ctrl_g0, ctrl_g1        -- per-cycle controller grant choices
//
// PREDICATE-ABSTRACTION SURFACE:
//   - burst (2-bit register) — the natural predicate-abstraction
//     target. With predicate set {burst==0}, the abstraction
//     produces MayOnly edges on transitions from the {¬burst==0}
//     cube (which collapses burst ∈ {1, 2, 3} into one abstract
//     state whose successor is non-deterministic under the
//     abstraction).
//
// PROPERTIES TO VERIFY (over the abstracted KMTS):
//   - Safety:   mutual exclusion — never grant both clients
//   - Liveness: every request eventually granted (GR(1) response)
//
// VERDICT-DIVERGENCE PATTERN:
//   - Pre-R.6.3 (modality-blind): the safety verdict could
//     over-claim True by accepting a MayOnly controller move as a
//     definite witness, losing soundness.
//   - Post-R.6.3 (modality-aware): the safety verdict correctly
//     returns Unknown (KleeneBot) when the abstracted burst
//     counter's MayOnly transitions matter for the property.
//
// The unit test
// `r6_3_evaluate_tri_mayonly_diamond_is_unknown_at_source` in
// `crates/mununu-core/src/mu_calculus/evaluator.rs` already
// proves the divergence on a synthetic fixture; this V.6 demo
// shows it on a real-RTL-derived KMTS.

module amba_arbiter (
    input  logic clk,
    input  logic rstn,
    // Environment inputs (uncontrollable; declared via
    // `AdapterOptions::controllable_inputs` exclusion).
    input  logic req_0,
    input  logic req_1,
    // Controller inputs (controllable; named with `ctrl_` prefix
    // so the sidecar's controllable_inputs list is unambiguous).
    input  logic ctrl_g0,
    input  logic ctrl_g1,
    // Outputs — the arbiter's per-cycle grants.
    output logic grant_0,
    output logic grant_1
);

    // 2-bit burst counter — predicate-abstraction target.
    // Predicate set {burst==0} yields a 2-cube abstraction whose
    // {¬burst==0} cube has non-deterministic successors (depends
    // on the concrete burst value within {1, 2, 3}), producing
    // the MayOnly edges this V.6 demo relies on.
    logic [1:0] burst;

    always_ff @(posedge clk or negedge rstn) begin
        if (!rstn) begin
            burst   <= 2'd0;
            grant_0 <= 1'b0;
            grant_1 <= 1'b0;
        end else begin
            // Pass-through: per-cycle controller grant decisions
            // become the next-cycle output.
            grant_0 <= ctrl_g0;
            grant_1 <= ctrl_g1;
            // Burst counter ticks only when a grant is active.
            // From burst==0 + grant: re-arm to 3. From burst!=0:
            // decrement. This is the abstraction target.
            if (ctrl_g0 || ctrl_g1) begin
                if (burst == 2'd0) begin
                    burst <= 2'd3;
                end else begin
                    burst <= burst - 2'd1;
                end
            end
        end
    end

endmodule
