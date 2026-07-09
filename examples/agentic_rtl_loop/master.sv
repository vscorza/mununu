// A bus master (the "LLM-authored" datapath side of the loop).
// Latches a request when it wants the bus and holds it until granted — the
// protocol the synthesized arbiter assumes of its environment.
module master (input logic clk, input logic want, input logic grant, output logic req);
  logic pending = 1'b0;
  always_ff @(posedge clk) begin
    if (grant)     pending <= 1'b0;   // request served
    else if (want) pending <= 1'b1;   // new request latched
  end
  assign req = pending | want;
endmodule
