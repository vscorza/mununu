// Wrapper — instantiates the upstream prim_onehot_check with SMALL parameters so
// the combinational error-detection assertions stay within the predicate-cube +
// cone-input abstraction's reach (a narrow one-hot vector keeps each combinational
// signal's input cone small).
//
// OneHotWidth = 4 (AddrWidth = 2). Only the onehot0 check is enabled
// (EnableCheck = AddrCheck = 0): its assertion `Onehot0Check_A, !$onehot0(oh_i) |->
// err_o` is the one that exercises the `$onehot0` translator support and reaches a
// clean HOLDS. EnableCheck (`(|oh_i) != en_i |-> err_o`) DOES translate now
// (reduction-OR in a 1-bit comparison → XOR), but reaches ⊥: with it on, `err_o`'s
// combinational cone spans BOTH `onehot0(oh_i)` and the enable-mismatch, and the
// property's own atoms don't pin all of it (an abstraction-completeness limit, the
// cnt_clr-cone class). The reduction-OR translation is validated by the redor
// demonstrator + unit test instead. The upstream `prim_onehot_check.sv` is used
// UNCHANGED via parameter binding — this wrapper is test scaffolding.

module prim_onehot_check_wrapper (
  input  logic       clk_i,
  input  logic       rst_ni,
  input  logic [3:0] oh_i,
  output logic       err_o
);

  prim_onehot_check #(
    .AddrWidth   (2),
    .OneHotWidth (4),
    .AddrCheck   (0),
    .EnableCheck (0),
    .StrictCheck (1)
  ) u_chk (
    .clk_i  (clk_i),
    .rst_ni (rst_ni),
    .oh_i   (oh_i),
    .addr_i ('0),
    .en_i   (1'b0),
    .err_o  (err_o)
  );

endmodule
