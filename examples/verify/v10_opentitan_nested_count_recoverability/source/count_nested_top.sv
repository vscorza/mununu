// Nested-counter usage of real prim_count (the prescaler+counter pattern): the inner counter
// down-counts on each tick and reloads to INNER_MAX at zero; when it reloads, the outer counter
// decrements. AG EF (outer_o == 0): outer alone is not a ranking (it holds while inner runs down),
// but the lexicographic (outer, inner) decreases every tick.
module count_nested_top #(
  parameter int OW = 48,
  parameter int IW = 8,
  parameter logic [IW-1:0] INNER_MAX = 8'd100,
  parameter logic [OW-1:0] OUTER_INIT = 48'd1099511627776  // 2^40
) (
  input clk_i, input rst_ni,
  input tick_i, input commit_i,
  output logic [OW-1:0] outer_o,
  output logic [IW-1:0] inner_o_p, output logic [OW-1:0] ic_p, oc_p, output logic ie_p, oe_p
);
  logic [IW-1:0] inner_o; assign inner_o_p = inner_o;
  logic inner_zero; assign inner_zero = (inner_o == '0);
  logic outer_zero; assign outer_zero = (outer_o == '0);
    prim_count #(.Width(IW), .ResetValue(INNER_MAX)) u_inner (
    .clk_i, .rst_ni, .clr_i(1'b0),
    .set_i(tick_i & inner_zero), .set_cnt_i(INNER_MAX),
    .incr_en_i(1'b0), .decr_en_i(tick_i & ~inner_zero), .step_i(IW'(1)),
    .commit_i, .cnt_o(inner_o), .cnt_after_commit_o(ic_p), .err_o(ie_p)
  );
  prim_count #(.Width(OW), .ResetValue(OUTER_INIT)) u_outer (
    .clk_i, .rst_ni, .clr_i(1'b0), .set_i(1'b0), .set_cnt_i('0),
    .incr_en_i(1'b0), .decr_en_i(tick_i & inner_zero & ~outer_zero), .step_i(OW'(1)),
    .commit_i, .cnt_o(outer_o), .cnt_after_commit_o(oc_p), .err_o(oe_p)
  );
endmodule
