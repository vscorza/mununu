module tier2 (input logic clk, input logic rst_n, input logic [5:0] state_q, input logic v);
  ap_stable:  assert property (@(posedge clk) v |=> $stable(state_q));
  ap_changed: assert property (@(posedge clk) v |-> $changed(state_q));
  ap_rose:    assert property (@(posedge clk) $rose(v) |-> v);
  ap_fell:    assert property (@(posedge clk) $fell(v));
  ap_past:    assert property (@(posedge clk) v |-> state_q == $past(state_q));
endmodule
