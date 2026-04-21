// Buffer Producer — Fixed
//
// The fix: check the 'full' signal before pushing. If the buffer is full,
// stay in SENDING state until space is available (backpressure).
//
// This is the AutoSVA-recommended fix: gate the acknowledge/push with
// a "not-full" condition.

module buffer_producer_fixed(
    input  logic clk,
    input  logic rst,
    input  logic send,
    input  logic full,
    output logic push
);

    typedef enum logic [1:0] {IDLE, SENDING, WAIT_ACK} state_t;
    state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
        end else begin
            case (state)
                IDLE: begin
                    if (send) state <= SENDING;
                end
                SENDING: begin
                    // FIX: only proceed if buffer is not full
                    if (!full) state <= WAIT_ACK;
                    // else: stay in SENDING (backpressure)
                end
                WAIT_ACK: begin
                    state <= IDLE;
                end
            endcase
        end
    end

    // FIX: push only when in SENDING AND buffer not full
    assign push = (state == SENDING) && !full;

endmodule
