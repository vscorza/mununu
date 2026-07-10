// The downstream consumer. It applies backpressure — it is NOT always ready —
// but it is fair: ready every other cycle. This discharges the kernel's
// `G F c_ready` assumption inside the system.
module consumer (input logic clk, output logic c_ready);
  logic tog = 1'b0;
  always_ff @(posedge clk) tog <= ~tog;
  assign c_ready = tog;
endmodule
