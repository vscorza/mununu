// Free-running up-counter built from real OpenTitan prim_count: incr_en/commit tied high, decr tied
// low. Every tick advances cnt (until it saturates at MAX), so `AG EF (cnt == MAX)` holds on EVERY
// path — the ALL-PATH ranking, not just some-path. No free input touches cnt's evolution.
module count_freerun_top #(parameter int Width = 48) (
  input clk_i, input rst_ni,
  output logic [Width-1:0] cnt_o,
  output logic [Width-1:0] cac_o,
  output logic err_o
);
  prim_count #(.Width(Width), .ResetValue('0)) u_cnt (
    .clk_i, .rst_ni, .clr_i(1'b0), .set_i(1'b0), .set_cnt_i('0),
    .incr_en_i(1'b1), .decr_en_i(1'b0), .step_i(Width'(1)), .commit_i(1'b1),
    .cnt_o, .cnt_after_commit_o(cac_o), .err_o(err_o)
  );
endmodule
