// R46-6 / GAP-2 — wide field concretized to a small value set.
//
// A 24-bit counter: at raw width the design has 24 state bits, past the
// bit-blast cap (MAX_STATE_BITS = 20), so it is REJECTED without an
// abstraction. The sidecar's `bounded_counter bound=7` concretizes `cnt`
// to the value set {0..7} = 3 effective bits, which fits the cap (the
// GAP-2 effective-bits accounting). The counter escapes its declared set
// at 7→8, producing the absorbing OOB sink — the exact shape that, before
// the realize numericity-gate fix, disabled atom binding and reported a
// spurious VIOLATED for the in-set, genuinely-reachable targets below.
module wide7 (
  input  logic clk_i, rst_ni, en_i,
  output logic at1_o, at5_o, at7_o);
  logic [23:0] cnt;
  always_ff @(posedge clk_i or negedge rst_ni)
    if (!rst_ni) cnt <= '0;
    else if (en_i) cnt <= cnt + 24'd1;
  assign at1_o = (cnt == 24'd1);
  assign at5_o = (cnt == 24'd5);
  assign at7_o = (cnt == 24'd7);
endmodule
