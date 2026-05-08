// Phase 1 example: 3-bit counter with an environment assumption,
// expressed in plain Verilog so Yosys 0.59's `read_verilog -formal -sv`
// accepts it (the built-in parser does not handle temporal SVA — see
// README §"Yosys SVA support" for the full story).
//
// The intent is "reset is at most a 1-cycle pulse": we materialize the
// constraint via an `rst_held` shadow register and `assume(!rst_held)`,
// which lowers to a BTOR2 `constraint` line; mununu's bit-blaster
// filters out (state, input) pairs whose constraint signal is false.
// Under the assume, the counter never sits at saturation forever — and
// the invariant `cnt == 0 || cnt == 1` holds for the cycle directly
// after a reset, captured by the immediate `assert`.
//
// The whole behavior fits in **one** `always @(posedge clk)` block,
// keeping the clk2fflogic edge-detect overhead small enough for the
// elaborated BTOR2 to land under `MAX_STATE_BITS = 16`.
//
// SOUNDNESS: 3-bit counter is exact. The shadow register is a sound
// finite encoding of "rst held no longer than 1 cycle" — same predicate
// as the SVA `rst |-> ##1 !rst`, just spelled with a register and a
// Boolean instead of a sequence operator.

module bounded_counter_with_assume (
    input  wire clk,
    input  wire rst,
    output wire [2:0] cnt_out,
    output wire       saturated
);
  reg [2:0] cnt;
  reg       rst_held;   // shadow: "rst was high last cycle"
  assign cnt_out   = cnt;
  assign saturated = (cnt == 3'b111);

  always @(posedge clk) begin
    // Counter update: clear on rst, count up to saturation, no wrap.
    if (rst)                       cnt <= 3'b000;
    else if (cnt != 3'b111)        cnt <= cnt + 3'b001;

    // Shadow register: was rst high on the previous cycle?
    rst_held <= rst;

    // Environment assume: rst is not held for two consecutive cycles.
    // Lowers to a BTOR2 `constraint` line — pairs with rst_held=1 ∧ rst=1
    // are filtered out before evaluation.
    assume (!(rst_held && rst));

    // Safety: if we're one cycle past a reset, cnt must be 0 or 1.
    // Lowers to a BTOR2 `bad` line. Expected verdict under the assume:
    // unreachable (assertion holds).
    assert (!rst_held || cnt == 3'b000 || cnt == 3'b001);
  end
endmodule
