// handshake_peripheral.sv — req/ack handshake peripheral.
//
// Same module the existing systemverilog adapter test
// fixture (`examples/systemverilog/handshake.sv`) exercises. The
// verify framework's `sv-rtl` adapter parses this through
// `SystemVerilogAdapter::translate` and produces a 4-state
// automaton (IDLE / WAIT_ACK / ACTIVE / DONE).
//
// Annotations on the verify side: this fixture has no `.mununu.json`
// sidecar — the adapter infers controllability from input direction
// (req = environment / uncontrollable). For a richer pairing where
// the microprogram and peripheral interact over named events,
// future fixtures would author a sidecar declaring which
// transitions correspond to which microprogram instructions.
module handshake_peripheral(
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
