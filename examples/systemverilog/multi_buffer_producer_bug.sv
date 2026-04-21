// Buffer Producer — Buggy (OpenPiton-inspired)
//
// Based on the OpenPiton Mem Engine / NoC buffer deadlock found by AutoSVA
// (DAC 2021). The producer assumes the buffer always has space, but the
// buffer has finite capacity. When the consumer is slow, the producer
// overflows the buffer.
//
// BUG: The producer issues requests (push=1) without checking the buffer's
// full signal. In the real OpenPiton bug, the Mem Engine reused a buffer
// designed for the L1.5 cache which naturally limited request rate, but
// the Mem Engine could burst faster than the buffer could drain.

module buffer_producer_bug(
    input  logic clk,
    input  logic rst,
    input  logic send,     // external trigger
    input  logic full,     // buffer full signal (from buffer)
    output logic push      // push data into buffer
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
                    // BUG: no check on 'full' — pushes regardless of buffer state
                    state <= WAIT_ACK;
                end
                WAIT_ACK: begin
                    // Return to idle, ready for next send
                    state <= IDLE;
                end
            endcase
        end
    end

    // Push is asserted for one cycle in SENDING state
    // BUG: does not gate on !full
    assign push = (state == SENDING);

endmodule
