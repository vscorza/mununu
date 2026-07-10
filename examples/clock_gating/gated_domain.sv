// The clocked datapath the kernel gates (the "LLM-authored" side of the loop).
// It does a unit of work when clocked, freezes while its clock is gated, and
// asserts `activity` while it has work in flight. Its assertion is the safety
// obligation the synthesized kernel must respect.
module gated_domain (input logic clk, input logic gate, input logic start, output logic activity);
  logic busy = 1'b0;
  always_ff @(posedge clk) begin
    if (gate)       busy <= busy;      // clock gated: frozen, no progress
    else if (busy)  busy <= 1'b0;      // one clocked cycle completes the work
    else if (start) busy <= 1'b1;      // new work arrives
  end
  assign activity = busy;
  // The domain must never be mid-work while its clock is gated — corrupting a
  // live computation. The kernel's interlock is exactly what guarantees this.
  a_no_work_while_gated: assert property (@(posedge clk) !(activity && gate));
endmodule
