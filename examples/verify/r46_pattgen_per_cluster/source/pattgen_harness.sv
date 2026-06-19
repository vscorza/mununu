// HAND-WRITTEN harness — NOT vendored OpenTitan RTL.
//
// `pattgen_chan` takes its entire configuration as one 116-bit packed
// `pattgen_chan_ctrl_t ctrl_i` struct (enable + polarity + 2 inactive
// levels + prediv[32] + data[64] + len[6] + reps[10]). That single bus
// busts MAX_INPUT_BITS, and bundles `enable` together with the wide
// config, so the vendored module cannot be driven through a narrow
// verification interface on its own.
//
// This harness gives each channel a sane narrow interface: a free 1-bit
// `enXX_i` enable and a SMALL constant configuration (prediv=1, a 1-bit
// pattern, 2 reps). With that config each channel runs a short, bounded
// pattern and asserts `event_doneXX_o` on completion — the property the
// fixture verifies. The vendored `pattgen_chan` logic is unchanged; the
// harness only narrows + concretizes the configuration interface.
//
// Two INDEPENDENT channels (share only clk/rst) → disjoint cones →
// cluster K=2. Each channel's cone still carries the channel's wide state
// (prediv_q[32], data_q[64], clk_cnt_q[32], reps_q/rep_cnt_q[10], …), so
// per-cluster slicing alone cannot fit a cluster — the sidecar's
// param-concretization of those wide cells is what makes each cluster fit.
module pattgen_harness
  import pattgen_ctrl_pkg::*;
(
  input  logic clk_i,
  input  logic rst_ni,
  input  logic en0_i,
  input  logic en1_i,
  output logic done0_o,
  output logic done1_o
);
  // Small constant configuration: a 1-bit pattern (len=0 → one data bit),
  // prediv=1 (the predivider counts 0,1), repeated twice (reps=1).
  function automatic pattgen_chan_ctrl_t mk_ctrl(logic en);
    pattgen_chan_ctrl_t c;
    c.enable             = en;
    c.polarity           = 1'b0;
    c.inactive_level_pcl = 1'b0;
    c.inactive_level_pda = 1'b0;
    c.prediv             = 32'd1;
    c.data               = 64'd1;
    c.len                = 6'd0;
    c.reps               = 10'd1;
    return c;
  endfunction

  logic pda0, pcl0, pda1, pcl1;

  pattgen_chan u_chan0 (
    .clk_i(clk_i), .rst_ni(rst_ni),
    .ctrl_i(mk_ctrl(en0_i)),
    .pda_o(pda0), .pcl_o(pcl0), .event_done_o(done0_o)
  );

  pattgen_chan u_chan1 (
    .clk_i(clk_i), .rst_ni(rst_ni),
    .ctrl_i(mk_ctrl(en1_i)),
    .pda_o(pda1), .pcl_o(pcl1), .event_done_o(done1_o)
  );
endmodule : pattgen_harness
