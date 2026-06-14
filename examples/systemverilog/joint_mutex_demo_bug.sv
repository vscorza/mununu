// Design-pattern DEMONSTRATION — NOT a real-system finding. Companion to
// joint_mutex_demo_fixed.sv (see its header for the full rationale).
//
// BUG variant: regions OVERLAP — A = [4, 10), B = [8, 12) — so addr in
// [8, 10) drives BOTH selects high simultaneously → `no_double_sel` FAILS
// (a genuine joint-mutex violation, CWE-1260 address-overlap class). This
// is the contrast that shows state-splitting does not trivially make the
// property hold: it correctly reports false here and true on the disjoint
// (fixed) variant.
module joint_mutex_demo_bug(
    input  logic       clk,
    input  logic       rst,
    input  logic [3:0] addr,
    input  logic       req,
    output logic       busy
);
    typedef enum logic [1:0] {IDLE, BUSY_S} state_t;
    state_t state;

    logic sel_a, sel_b;
    // Overlapping regions: addr in [8, 10) makes both selects high.
    assign sel_a = (addr >= 4'd4) && (addr < 4'd10);
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
