// NoC Buffer — 4-entry FIFO (OpenPiton-inspired)
//
// A simple bounded FIFO with credit-based flow control.
// The buffer reports its fill level via `credit_avail`.
// Overflow occurs when a push happens while the buffer is full.
//
// Based on the OpenPiton NoC buffer pattern where the L1.5 cache
// naturally limited request rate, but the Mem Engine could burst
// faster than the buffer's capacity.

module noc_buffer #(parameter DEPTH = 4) (
    input  logic        clk,
    input  logic        rst,
    input  logic        push,          // write request from producer
    input  logic        pop,           // read acknowledge from consumer
    output logic        credit_avail,  // buffer has space (at least 1 free entry)
    output logic        full,          // buffer completely full
    output logic        empty          // buffer empty
);

    logic [2:0] count;  // occupancy counter (0..DEPTH, DEPTH+1 = overflow)

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            count <= 0;
        end else begin
            if (push && !pop) begin
                count <= count + 1;
            end else if (pop && !push) begin
                if (count > 0)
                    count <= count - 1;
            end
        end
    end

    assign credit_avail = (count < DEPTH);
    assign full = (count >= DEPTH);
    assign empty = (count == 0);

endmodule
