// Buffer-Overflow Top Module — Buggy (R-MM-6 multi-module parity fixture)
//
// The top-module (netlist) form of the `multi_buffer_overflow_bug` design
// that the `multi_buffer_overflow_bug.mununu.json` sidecar composes
// logically. This structural form is what the KMTS `sv-yosys` multi-module
// path consumes (it discovers instance connectivity from the netlist),
// whereas the native `sv-rtl` path composes the same two modules via the
// sidecar's `connections` list. The R-MM-6 verdict-parity gate evaluates
// the `no_overflow` safety property on BOTH and requires they agree.
//
// Wiring (mirrors the sidecar's two connections):
//   - producer.push  -> buffer.push   (driver -> reader)
//   - buffer.full    -> producer.full (driver -> reader, backpressure)
//
// BUG: the producer pushes regardless of `full`, so the buffer overflows
// (count reaches 3). `no_overflow` is FALSE.

module buffer_overflow_bug_top(
    input logic clk,
    input logic rst,
    input logic send,   // environment trigger for the producer
    input logic pop     // environment drains the buffer
);

    logic push;  // producer -> buffer
    logic full;  // buffer -> producer (backpressure, ignored by the bug)

    buffer_producer_bug u_producer(
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
