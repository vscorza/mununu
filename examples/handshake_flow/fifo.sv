// A small bounded FIFO (the "LLM-authored" datapath the kernel drives).
// Tracks occupancy and exposes full/empty. Its assertions are the protocol
// obligations on whoever drives enq/deq — obligations the synthesized
// flow-control kernel guarantees.
module fifo #(parameter int DEPTH = 2) (
    input  logic clk, input logic enq, input logic deq,
    output logic full, output logic empty);
  logic [2:0] cnt = 3'd0;
  assign full  = (cnt >= DEPTH[2:0]);
  assign empty = (cnt == 3'd0);
  // Legal transfers only change occupancy; illegal ones are caught by the SVA.
  wire do_enq = enq & ~full;
  wire do_deq = deq & ~empty;
  always_ff @(posedge clk)
    cnt <= cnt + (do_enq ? 3'd1 : 3'd0) - (do_deq ? 3'd1 : 3'd0);
  // The driver must never enqueue into a full FIFO or dequeue an empty one.
  a_no_overflow:  assert property (@(posedge clk) !(enq && full));
  a_no_underflow: assert property (@(posedge clk) !(deq && empty));
endmodule
