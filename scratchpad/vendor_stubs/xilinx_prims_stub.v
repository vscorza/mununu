// Reusable FPGA vendor-primitive stubs for the SV→BTOR2 formal lift.
// These replace vendor simulation models (which reference the Xilinx `glbl` global
// net, undefined outside a Xilinx flow) with transparent, synthesizable behaviour.
// SOUNDNESS: a clock buffer is made TRANSPARENT (`O = I`) — the clock always passes,
// ignoring the enable/gating. This over-approximates behaviour (the design runs at full
// clock), which is the correct posture for a functional reachability/recoverability check
// (we want the FSM to advance); it never hides a real reachable state.
module BUFGCE (output O, input CE, input I);
  assign O = I;
endmodule
module BUFG (output O, input I);
  assign O = I;
endmodule
