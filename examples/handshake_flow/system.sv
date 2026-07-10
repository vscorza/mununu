// The assembled flow-control system: producer -> FIFO -> consumer, with the
// mununu-synthesized flow-control kernel (`gr1_controller`, flow_ctrl.sv)
// driving enq/deq, plus the whole-system verification monitors btormc checks.
module top (input logic clk, input logic gen,
            output logic bad_overflow, output logic bad_underflow,
            output logic bad_drop,     output logic bad_phantom,
            output logic bad_stall_enq, output logic bad_stall_deq,
            output logic bad_assume_cready);
  logic p_valid, c_ready, full, empty, enq, deq;
  producer prod (.clk(clk), .gen(gen), .enq(enq), .p_valid(p_valid));
  consumer cons (.clk(clk), .c_ready(c_ready));
  fifo #(.DEPTH(2)) buf0 (.clk(clk), .enq(enq), .deq(deq), .full(full), .empty(empty));
  gr1_controller ctrl (.clk(clk), .p_valid(p_valid), .c_ready(c_ready),
                       .full(full), .empty(empty), .enq(enq), .deq(deq));

  // (1) Safety — the kernel's guarantees, in the assembled system.
  assign bad_overflow  = enq & full;        // no overflow (never enq into a full FIFO)
  assign bad_underflow = deq & empty;       // no underflow (never deq an empty FIFO)
  assign bad_drop      = deq & ~c_ready;    // no drop (never deq unless the consumer takes it)
  assign bad_phantom   = enq & ~p_valid;    // no phantom (never accept a non-offered item)

  // (2) Liveness — eager transfer: the kernel never leaves an accept-able item
  //     (or a drainable one) waiting more than a single cycle. `pend_*` marks a
  //     cycle where the kernel could act but didn't; the monitor fires only if
  //     that persists two cycles in a row (a >1-cycle stall).
  logic pe1 = 1'b0, pd1 = 1'b0;
  wire pend_enq = p_valid & ~full & ~enq;            // could accept, didn't
  wire pend_deq = ~empty & c_ready & ~deq;           // could deliver (data + consumer ready), didn't
  assign bad_stall_enq = pend_enq & pe1;
  assign bad_stall_deq = pend_deq & pd1;

  // (3) Assume-guarantee discharge — the consumer supplies G F c_ready.
  logic [2:0] tc = 3'd0;
  assign bad_assume_cready = (tc >= 3'd3);   // consumer ready at least every 2 cycles

  always_ff @(posedge clk) begin
    pe1 <= pend_enq;
    pd1 <= pend_deq;
    tc  <= c_ready ? 3'd0 : (tc + 3'd1);
  end
endmodule
