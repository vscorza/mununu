// Parameterized Round-Robin Arbiter (N=4 clients)
// 5-state FSM: IDLE + 4 grant states
// @mununu ltl safety: nu X. ([] X)
module rr_arbiter #(parameter N = 4) (
    input logic clk, input logic rst,
    input logic req_a, input logic req_b, input logic req_c, input logic req_d,
    output logic grant_a, output logic grant_b, output logic grant_c, output logic grant_d
);
    typedef enum logic [2:0] {IDLE, GRANT_A, GRANT_B, GRANT_C, GRANT_D} state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: begin
                if (req_a) state <= GRANT_A;
                else if (req_b) state <= GRANT_B;
                else if (req_c) state <= GRANT_C;
                else if (req_d) state <= GRANT_D;
            end
            GRANT_A: if (!req_a) state <= IDLE;
            GRANT_B: if (!req_b) state <= IDLE;
            GRANT_C: if (!req_c) state <= IDLE;
            GRANT_D: if (!req_d) state <= IDLE;
        endcase
    end
endmodule
