module tier1 (input logic clk, input logic rst_n, input logic a, input logic b);
  ap_bool:  assert property (@(posedge clk) a);
  ap_impl:  assert property (@(posedge clk) disable iff (!rst_n) a |-> b);
  ap_nimpl: assert property (@(posedge clk) a |=> b);
  ap_assume: assume property (@(posedge clk) a);
  cp_cover: cover property (@(posedge clk) a && b);
endmodule
