// Req/Ack Handshake Protocol
// 4-state FSM: IDLE → WAIT → ACTIVE → DONE → IDLE
// Cross-validates with examples/hw/handshake.ctxdsl
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
