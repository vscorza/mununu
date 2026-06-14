// Design-pattern DEMONSTRATION — NOT a real-system finding.
//
// CWE-1260 (improper isolation of address regions) class, reduced to the
// smallest shape that exercises **full per-value state-splitting** of
// combinational outputs in the KMTS pipeline. Two input-dependent
// combinational region-selects (`sel_a`, `sel_b`) derived from a 4-bit
// `addr`, plus a tiny FSM so the model has register state alongside the
// combinational signals (the non-degenerate case).
//
// The joint-mutex property "never both selects high simultaneously"
// (see joint_mutex_demo_fixed.mununu.json `no_double_sel`) is the
// consumer for state-splitting: the KMTS ∃-priority combinational
// labeling reports a SPURIOUS violation here (sel_a can be high for one
// addr, sel_b for a different addr — "both can be high" is not "both high
// together"), while splitting each register-state by the JOINT
// (sel_a, sel_b) assignment per input is correct.
//
// FIXED variant: regions are DISJOINT — A = [4, 8), B = [8, 12) — so the
// two selects are never simultaneously high → `no_double_sel` HOLDS.
module joint_mutex_demo_fixed(
    input  logic       clk,
    input  logic       rst,
    input  logic [3:0] addr,
    input  logic       req,
    output logic       busy
);
    typedef enum logic [1:0] {IDLE, BUSY_S} state_t;
    state_t state;

    logic sel_a, sel_b;
    // Disjoint regions: never both high for any single addr.
    assign sel_a = (addr >= 4'd4) && (addr < 4'd8);
    assign sel_b = (addr >= 4'd8) && (addr < 4'd12);

    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE:   if (req) state <= BUSY_S;
            BUSY_S: if (!req) state <= IDLE;
        endcase
    end

    assign busy = (state == BUSY_S);
endmodule
