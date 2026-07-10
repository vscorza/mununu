// Same assembled system as system.sv, but the arbiter's mutual-exclusion
// guarantee is written as an SVA over registered (state) grants so the
// productized verb `mununu sv verify-auto` binds and checks it. Uses `master`
// (master.sv) and `gr1_controller` (synthesized).
module system_checked (input logic clk, input logic want_0, input logic want_1,
                       output logic grant_0_q, output logic grant_1_q);
  logic req_0, req_1, grant_0, grant_1;
  master m0 (.clk(clk), .want(want_0), .grant(grant_0), .req(req_0));
  master m1 (.clk(clk), .want(want_1), .grant(grant_1), .req(req_1));
  gr1_controller arb (.clk(clk), .req_0(req_0), .req_1(req_1),
                      .grant_0(grant_0), .grant_1(grant_1));
  always_ff @(posedge clk) begin
    grant_0_q <= grant_0;
    grant_1_q <= grant_1;
  end
  p_mutex: assert property (@(posedge clk) !(grant_0_q && grant_1_q));
endmodule
