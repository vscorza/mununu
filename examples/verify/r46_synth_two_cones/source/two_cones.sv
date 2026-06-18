// SYNTHETIC fixture (hand-written, NOT vendored) — R46-5 / R.4.6.
//
// Two INDEPENDENT 11-bit counters sharing only clk/rst. Joint state =
// 22 bits (> MAX_STATE_BITS = 20, so the joint bit-blast busts the cap);
// each counter's cone is 11 bits (2048 states, which fits). The two
// counters use different increments (+1 / +3) so Yosys `opt` cannot merge
// the registers into one.
//
// This is the minimal faithful "joint busts cap, clusters fit" shape: the
// single-module verify route FLATTENS it into one BTOR2 whose dep graph
// has two disjoint register cones, the two reachability properties
// cluster K=2, and per-cluster verification (R46-2/R46-3) slices each
// cluster down to its own 11-bit cone and verifies it.
module two_cones (
  input  logic        clk_i,
  input  logic        rst_ni,
  input  logic        a_en_i,
  input  logic        b_en_i,
  output logic        a_max_o,
  output logic        b_max_o
);
  logic [10:0] cnt_a, cnt_b;
  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) begin
      cnt_a <= '0;
      cnt_b <= '0;
    end else begin
      if (a_en_i) cnt_a <= cnt_a + 11'd1;
      if (b_en_i) cnt_b <= cnt_b + 11'd3;
    end
  end
  assign a_max_o = (cnt_a == 11'd2047);
  assign b_max_o = (cnt_b == 11'd2046);
endmodule
