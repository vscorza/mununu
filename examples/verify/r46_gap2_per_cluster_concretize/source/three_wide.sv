// R46-6 / GAP-2 — per-cluster slicing × param-concretization, composed.
//
// Three INDEPENDENT 24-bit counters (share only clk/rst). Each output
// reads only one counter, so the three cones are disjoint and cluster K=3.
//
// Why this is the GAP-2 composition demo (not just R46-5's "joint busts,
// clusters fit", and not just r46_gap2_wide_concretize's single wide cell):
//
//   * Joint raw width = 3 × 24 = 72 state bits — far past MAX_STATE_BITS=20.
//   * Each cluster's cone, after SLICING, is still one 24-bit counter = 24
//     raw bits > 20. So per-cluster SLICING ALONE cannot rescue this design.
//   * The sidecar param-concretizes each counter to `bounded_counter
//     bound=127` → 7 effective bits. Joint effective = 3 × 7 = 21 > 20
//     (still busts → per-cluster fires), but each cluster = 7 ≤ 20 (fits).
//
// So the rescue requires BOTH primitives stacked: per-cluster slicing to
// isolate each cone AND param-concretization to shrink the wide cell inside
// each cone. Each counter escapes its declared set at 128 → OOB sink, the
// shape that the realize numericity-gate fix (PR #94) makes verify soundly.
module three_wide (
  input  logic clk_i, rst_ni, a_en_i, b_en_i, c_en_i,
  output logic a7_o, b7_o, c7_o);
  logic [23:0] cnt_a, cnt_b, cnt_c;
  always_ff @(posedge clk_i or negedge rst_ni)
    if (!rst_ni) begin cnt_a <= '0; cnt_b <= '0; cnt_c <= '0; end
    else begin
      if (a_en_i) cnt_a <= cnt_a + 24'd1;
      if (b_en_i) cnt_b <= cnt_b + 24'd1;
      if (c_en_i) cnt_c <= cnt_c + 24'd1;
    end
  assign a7_o = (cnt_a == 24'd7);
  assign b7_o = (cnt_b == 24'd7);
  assign c7_o = (cnt_c == 24'd7);
endmodule
