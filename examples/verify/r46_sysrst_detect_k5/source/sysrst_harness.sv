// HAND-WRITTEN harness — NOT vendored OpenTitan RTL.
//
// OpenTitan `sysrst_ctrl` runs K independent debounce/detect sub-blocks
// (autoblock / ulp / keyintr / combo / pin), each an instance of the
// vendored `sysrst_ctrl_detect` parameterized timer block, wired together
// through a TileLink-UL register file. The full top therefore pulls in the
// reg_top + tlul infrastructure, which is irrelevant to the per-cluster
// verification mechanism this fixture exercises.
//
// This harness reproduces sysrst_ctrl's load-bearing shape — K=5
// INDEPENDENT detectors that share only clk/rst, each carrying the
// vendored block's 32-bit detect/debounce timer counter (`cnt_q`) — with
// a narrow interface: a free per-detector `trigKK_i` trigger + `enKK_i`
// enable, and SMALL constant timer thresholds. The wide `cfg_*_timer_i`
// inputs (16b + 32b) and the 32-bit `cnt_q` are exactly the fields the
// de-risk note flagged: the joint cone busts MAX_STATE_BITS via the five
// 32-bit timers, while each detector cluster fits once its timer is
// param-concretized. Five distinct EventTypes mirror the real top's mix of
// level/edge detectors.
module sysrst_harness
  import sysrst_ctrl_pkg::*;
(
  input  logic clk_i,
  input  logic rst_ni,
  input  logic trig0_i, trig1_i, trig2_i, trig3_i, trig4_i,
  input  logic en0_i,   en1_i,   en2_i,   en3_i,   en4_i,
  output logic det0_o,  det1_o,  det2_o,  det3_o,  det4_o
);
  // Small constant timer thresholds: debounce after 1 cycle, detect after 7.
  // The vendored counter (`cnt_q`, 32 bits) only ever reaches the larger
  // threshold (7), so the sidecar concretizes it to {0..7}.
  localparam logic [15:0] DEB = 16'd1;
  localparam logic [31:0] DET = 32'd7;

  sysrst_ctrl_detect #(.EventType(LowLevel)) u_det0 (
    .clk_i, .rst_ni, .trigger_i(trig0_i),
    .cfg_debounce_timer_i(DEB), .cfg_detect_timer_i(DET), .cfg_enable_i(en0_i),
    .event_detected_o(det0_o), .event_detected_pulse_o()
  );
  sysrst_ctrl_detect #(.EventType(HighLevel)) u_det1 (
    .clk_i, .rst_ni, .trigger_i(trig1_i),
    .cfg_debounce_timer_i(DEB), .cfg_detect_timer_i(DET), .cfg_enable_i(en1_i),
    .event_detected_o(det1_o), .event_detected_pulse_o()
  );
  sysrst_ctrl_detect #(.EventType(EdgeToLow)) u_det2 (
    .clk_i, .rst_ni, .trigger_i(trig2_i),
    .cfg_debounce_timer_i(DEB), .cfg_detect_timer_i(DET), .cfg_enable_i(en2_i),
    .event_detected_o(det2_o), .event_detected_pulse_o()
  );
  sysrst_ctrl_detect #(.EventType(EdgeToHigh)) u_det3 (
    .clk_i, .rst_ni, .trigger_i(trig3_i),
    .cfg_debounce_timer_i(DEB), .cfg_detect_timer_i(DET), .cfg_enable_i(en3_i),
    .event_detected_o(det3_o), .event_detected_pulse_o()
  );
  sysrst_ctrl_detect #(.EventType(LowLevel), .Sticky(1)) u_det4 (
    .clk_i, .rst_ni, .trigger_i(trig4_i),
    .cfg_debounce_timer_i(DEB), .cfg_detect_timer_i(DET), .cfg_enable_i(en4_i),
    .event_detected_o(det4_o), .event_detected_pulse_o()
  );
endmodule : sysrst_harness
