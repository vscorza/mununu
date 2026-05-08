// Phase 1 demo: 2-bit free-running counter with a deliberately violatable
// safety assertion. Used to exercise the SV→Yosys→BTOR2→CLTS pipeline added
// in Phase 1 of the RTL roadmap (`adapter::yosys` + `adapter::btor2`).
//
// The counter wraps modulo 4 — `cnt` reaches 2'b11 every 4 cycles whenever
// `rst` is held low. The assertion `cnt != 2'b11` is therefore violated by
// design; mununu's evaluator must surface it as `bad`-reachable.

module safety_demo (input wire clk, input wire rst, output wire warn);
  reg [1:0] cnt;
  assign warn = (cnt == 2'b11);

  always @(posedge clk) begin
    if (rst) cnt <= 2'b00;
    else     cnt <= cnt + 1;
  end

  // Yosys's `chformal -lower` translates this assertion into a BTOR2 `bad`
  // line — the bit-blaster surfaces it as a safety obligation.
  always @(posedge clk) begin
    assert (cnt != 2'b11);
  end
endmodule
