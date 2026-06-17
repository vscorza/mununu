// NOT vendored — R4W-4 / M.3 harness (clustered cone-of-influence).
// Two independent real csrng_main_sm instances. To keep the explicit
// bit-blast tiny, every input except `local_escalate_i` is tied to a
// constant inside the harness; each instance keeps exactly one free
// primary input, and the two instances share no free input. The
// verified behaviour (the csrng_main_sm sparse FSM) is real upstream
// RTL; the harness only fixes the I/O boundary.
module csrng_main_sm_pair
  import csrng_pkg::*;
(
  input  logic   clk_i,
  input  logic   rst_ni,
  input  logic   u0_local_escalate_i,
  input  logic   u1_local_escalate_i,

  output logic [MainSmStateWidth-1:0] u0_main_sm_state_o,
  output logic                        u0_main_sm_err_o,
  output logic [MainSmStateWidth-1:0] u1_main_sm_state_o,
  output logic                        u1_main_sm_err_o
);

  csrng_main_sm u0 (
    .clk_i,
    .rst_ni,
    .enable_i            (1'b1),
    .acmd_avail_i        (1'b1),
    .acmd_accept_o       (),
    .acmd_i              (GEN),
    .acmd_eop_i          (1'b1),
    .flag0_i             (1'b0),
    .cmd_entropy_req_o   (),
    .cmd_entropy_avail_i (1'b1),
    .cmd_vld_o           (),
    .cmd_rdy_i           (1'b1),
    .clr_adata_packer_o  (),
    .cmd_complete_i      (1'b1),
    .local_escalate_i    (u0_local_escalate_i),
    .main_sm_state_o     (u0_main_sm_state_o),
    .main_sm_err_o       (u0_main_sm_err_o)
  );

  csrng_main_sm u1 (
    .clk_i,
    .rst_ni,
    .enable_i            (1'b1),
    .acmd_avail_i        (1'b1),
    .acmd_accept_o       (),
    .acmd_i              (GEN),
    .acmd_eop_i          (1'b1),
    .flag0_i             (1'b0),
    .cmd_entropy_req_o   (),
    .cmd_entropy_avail_i (1'b1),
    .cmd_vld_o           (),
    .cmd_rdy_i           (1'b1),
    .clr_adata_packer_o  (),
    .cmd_complete_i      (1'b1),
    .local_escalate_i    (u1_local_escalate_i),
    .main_sm_state_o     (u1_main_sm_state_o),
    .main_sm_err_o       (u1_main_sm_err_o)
  );

endmodule
