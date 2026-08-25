// Minimal prim_flop matching OpenTitan's abstract-prim register interface (resolves to a plain
// reset flop, identical to prim_generic_flop). Stands in for the prim-abstraction selector.
module prim_flop #(
  parameter int Width = 1,
  parameter logic [Width-1:0] ResetValue = '0
) (
  input clk_i,
  input rst_ni,
  input [Width-1:0] d_i,
  output logic [Width-1:0] q_o
);
  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) q_o <= ResetValue;
    else q_o <= d_i;
  end
endmodule
