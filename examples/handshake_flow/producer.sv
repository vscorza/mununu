// The upstream producer. Latches an item to offer (from a free `gen` input)
// and holds the offer until the kernel accepts it (enq).
module producer (input logic clk, input logic gen, input logic enq, output logic p_valid);
  logic has = 1'b0;
  always_ff @(posedge clk) begin
    if (enq)      has <= 1'b0;   // accepted
    else if (gen) has <= 1'b1;   // new item offered
  end
  assign p_valid = has | gen;
endmodule
