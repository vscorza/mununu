// Round-Robin 2-Client Arbiter
// 3-state FSM with mutual exclusion
// Cross-validates with examples/hw/arbiter.ctxdsl
// @mununu ltl safety: nu X. ([] X)
module arbiter(
    input logic clk, input logic rst,
    input logic req_a, input logic req_b,
    output logic grant_a, output logic grant_b
);
    typedef enum logic [1:0] {IDLE, GRANT_A, GRANT_B} state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: begin
                if (req_a) state <= GRANT_A;
                else if (req_b) state <= GRANT_B;
            end
            GRANT_A: if (!req_a) state <= IDLE;
            GRANT_B: if (!req_b) state <= IDLE;
        endcase
    end
endmodule
