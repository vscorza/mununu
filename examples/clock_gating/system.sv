// The assembled clock-gating system: the gated datapath + the mununu-synthesized
// clock-gating kernel (`gr1_controller`, clock_gate.sv), plus the whole-system
// verification monitors btormc checks. `start` (work arrival) and `sleep_req`
// (power-policy request) are the free environment.
module top (input logic clk, input logic start, input logic sleep_req,
            output logic bad_interlock, output logic bad_sleep_stall,
            output logic bad_assume_idle);
  logic activity, gate;
  gated_domain dom  (.clk(clk), .gate(gate), .start(start), .activity(activity));
  gr1_controller ctrl (.clk(clk), .activity(activity), .sleep_req(sleep_req), .gate(gate));

  // (1) Interlock (safety) — the kernel's guarantee: never gate an active domain.
  assign bad_interlock = gate & activity;

  // (2) Liveness — the kernel eagerly honors a sleep request once the domain is
  //     idle (never leaves a grantable sleep waiting more than one cycle).
  logic ps1 = 1'b0;
  wire pend_sleep = sleep_req & ~activity & ~gate;   // could gate for power, didn't
  assign bad_sleep_stall = pend_sleep & ps1;

  // (3) Assume-guarantee discharge — the domain supplies G F !activity (it idles).
  logic [2:0] ti = 3'd0;
  assign bad_assume_idle = (ti >= 3'd3);             // idle at least every other cycle

  always_ff @(posedge clk) begin
    ps1 <= pend_sleep;
    ti  <= activity ? (ti + 3'd1) : 3'd0;
  end
endmodule
