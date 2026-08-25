module count_top #(parameter int Width = 48) (
  input clk_i, input rst_ni,
  input incr_en_i, input decr_en_i, input commit_i,
  output logic [Width-1:0] cnt_o,
  output logic [Width-1:0] cac_o,
  output logic err_o
);
  prim_count #(.Width(Width), .ResetValue('0)) u_cnt (
    .clk_i, .rst_ni, .clr_i(1'b0), .set_i(1'b0), .set_cnt_i('0),
    .incr_en_i, .decr_en_i, .step_i(Width'(1)), .commit_i,
    .cnt_o, .cnt_after_commit_o(cac_o), .err_o(err_o)
  );
endmodule
