// The assembled system: two masters + the mununu-synthesized arbiter
// (`gr1_controller`, in gr1_controller.sv), plus the whole-system verification
// monitors that btormc checks.
module top (input logic clk, input logic want_0, input logic want_1,
            output logic bad_mutex,
            output logic bad_starve_0, output logic bad_starve_1,
            output logic bad_proto_0,  output logic bad_proto_1);
  logic req_0, req_1, grant_0, grant_1;
  master m0 (.clk(clk), .want(want_0), .grant(grant_0), .req(req_0));
  master m1 (.clk(clk), .want(want_1), .grant(grant_1), .req(req_1));
  gr1_controller arb (.clk(clk), .req_0(req_0), .req_1(req_1),
                      .grant_0(grant_0), .grant_1(grant_1));

  // (1) mutual exclusion — the arbiter guarantee, in the assembled system.
  assign bad_mutex = grant_0 & grant_1;

  // (2) no-starvation per client (bounded): a pending request never waits too long.
  logic [2:0] t0 = 3'd0, t1 = 3'd0;
  wire pend0 = req_0 & ~grant_0;
  wire pend1 = req_1 & ~grant_1;
  assign bad_starve_0 = (t0 >= 3'd6);
  assign bad_starve_1 = (t1 >= 3'd6);

  // (3) assume-guarantee: the masters discharge the arbiter's "hold req until
  //     grant" assumption. bad_proto fires iff a master drops req before a grant.
  logic req_0_p = 1'b0, grant_0_p = 1'b0, req_1_p = 1'b0, grant_1_p = 1'b0;
  assign bad_proto_0 = req_0_p & ~grant_0_p & ~req_0;
  assign bad_proto_1 = req_1_p & ~grant_1_p & ~req_1;

  always_ff @(posedge clk) begin
    t0 <= pend0 ? (t0 + 3'd1) : 3'd0;
    t1 <= pend1 ? (t1 + 3'd1) : 3'd0;
    req_0_p <= req_0; grant_0_p <= grant_0;
    req_1_p <= req_1; grant_1_p <= grant_1;
  end
endmodule
