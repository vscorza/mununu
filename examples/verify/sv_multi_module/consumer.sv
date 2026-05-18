// Consumer module: reacts to a valid input signal.
// When valid is asserted, transitions from IDLE to BUSY, then back.
module consumer(
    input logic clk, input logic rst,
    input logic valid
);
    typedef enum logic [1:0] {IDLE, BUSY, ACK} state_t;
    state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: if (valid) state <= BUSY;
            BUSY: state <= ACK;
            ACK: state <= IDLE;
        endcase
    end
endmodule
