// Buffer-Overflow Top Module — Fixed (R-MM-6 multi-module parity fixture)
//
// The top-module (netlist) form of the `multi_buffer_overflow_fixed`
// design that the `multi_buffer_overflow_fixed.mununu.json` sidecar
// composes logically. See the bug variant's header for why both forms
// exist (KMTS netlist path vs native sidecar path) and the R-MM-6 gate.
//
// Wiring (mirrors the sidecar's two connections):
//   - producer.push  -> buffer.push   (driver -> reader)
//   - buffer.full    -> producer.full (driver -> reader, backpressure)
//
// FIX: the producer gates `push` on `!full` (push is Mealy — a function of
// both its state AND the buffer's full input), so the buffer never
// overflows (count maxes at 2). `no_overflow` is TRUE. This exercises a
// Mealy-output rendezvous across the composition (the producer's push
// depends on the buffer's combinational full output).

module buffer_overflow_fixed_top(
    input logic clk,
    input logic rst,
    input logic send,   // environment trigger for the producer
    input logic pop     // environment drains the buffer
);

    logic push;  // producer -> buffer (gated on !full)
    logic full;  // buffer -> producer (backpressure, honoured by the fix)

    buffer_producer_fixed u_producer(
        .clk(clk),
        .rst(rst),
        .send(send),
        .full(full),
        .push(push)
    );

    bounded_buffer u_buffer(
        .clk(clk),
        .rst(rst),
        .push(push),
        .pop(pop),
        .full(full)
    );

endmodule
