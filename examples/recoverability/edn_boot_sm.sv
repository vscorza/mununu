// edn_boot_sm.sv — the OpenTitan EDN main state machine, extracted for isolated FSM verification.
//
// PROVENANCE. Verbatim transition logic from OpenTitan `edn_main_sm.sv`
// (examples/verify/dc_opentitan_edn_main_sm/source/edn_main_sm.sv, lowRISC, Apache-2.0). The full SoC
// module needs a deep package chain (prim_util_pkg → entropy_src_pkg → csrng_pkg → edn_pkg) + the
// prim_assert macros, so this fixture inlines the `edn_pkg::state_e` enum (VERBATIM 9-bit sparse
// encodings), replaces the `PRIM_FLOP_SPARSE_FSM` macro with a plain async-reset flop, and drops the
// (state-irrelevant) SVA assertions. The `always_comb` FSM — every transition — is UNCHANGED.
//
// WHY THIS DESIGN. It is the corpus's genuine POSITIONAL environment-strategy case: the boot handshake
// needs `boot_req_mode_i = 1` in `Idle` (to start: line "Idle: if (boot_req_mode_i && edn_enable_i)")
// AND `boot_req_mode_i = 0` in `BootDone` (to progress: "BootDone: if (!boot_req_mode_i)"). So NO
// constant hold drives a full boot; the recovering discipline is STATE-DEPENDENT. And the `!edn_enable_i`
// abort only routes to `Idle` (never to `BootLoadUni`), so it cannot collapse the strategy to a constant.
//
//   Property: AG EF (state_q == BootUniAckWait(44)) — "a full boot instantiate→generate→uninstantiate
//   handshake can always be driven to completion." Free-input HOLDS (∃ the positional path); every
//   CONSTANT boot_req_mode_i hold is VIOLATED (const-1 hangs in BootDone; const-0 never starts).

module edn_boot_sm (
    input  logic       clk_i,
    input  logic       rst_ni,
    input  logic       edn_enable_i,
    input  logic       boot_req_mode_i,
    input  logic       auto_req_mode_i,
    input  logic       sw_cmd_req_load_i,
    input  logic       csrng_cmd_ack_i,
    input  logic       max_reqs_cnt_zero_i,
    input  logic       cmd_sent_i,
    input  logic       csrng_ack_err_i,
    input  logic       local_escalate_i,
    output logic [8:0] main_sm_state_o
);

  localparam int StateWidth = 9;
  typedef enum logic [StateWidth-1:0] {
    Idle               = 9'b011000001,
    BootLoadIns        = 9'b111000111,
    BootInsAckWait     = 9'b001111001,
    BootLoadGen        = 9'b000000011,
    BootGenAckWait     = 9'b001110111,
    BootPulse          = 9'b010101001,
    BootDone           = 9'b011110000,
    BootLoadUni        = 9'b100110101,
    BootUniAckWait     = 9'b000101100,
    AutoLoadIns        = 9'b110111100,
    AutoFirstAckWait   = 9'b110100011,
    AutoAckWait        = 9'b010010010,
    AutoDispatch       = 9'b101100001,
    AutoCaptGenCnt     = 9'b100001110,
    AutoSendGenCmd     = 9'b111011101,
    AutoCaptReseedCnt  = 9'b010111111,
    AutoSendReseedCmd  = 9'b001101010,
    SWPortMode         = 9'b010010101,
    RejectCsrngEntropy = 9'b000011000,
    Error              = 9'b101111110
  } state_e;

  state_e state_d, state_q;

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) state_q <= Idle;
    else state_q <= state_d;
  end

  assign main_sm_state_o = state_q;

  always_comb begin
    state_d = state_q;
    unique case (state_q)
      Idle: begin
        if (boot_req_mode_i && edn_enable_i) begin
          state_d = BootLoadIns;
        end else if (auto_req_mode_i && edn_enable_i) begin
          state_d = AutoLoadIns;
        end else if (edn_enable_i) begin
          state_d = SWPortMode;
        end
      end
      BootLoadIns:    state_d = BootInsAckWait;
      BootInsAckWait: if (csrng_cmd_ack_i) state_d = BootLoadGen;
      BootLoadGen:    state_d = BootGenAckWait;
      BootGenAckWait: if (csrng_cmd_ack_i) state_d = BootPulse;
      BootPulse:      state_d = BootDone;
      BootDone:       if (!boot_req_mode_i) state_d = BootLoadUni;
      BootLoadUni:    state_d = BootUniAckWait;
      BootUniAckWait: if (csrng_cmd_ack_i) state_d = Idle;
      AutoLoadIns:      if (sw_cmd_req_load_i) state_d = AutoFirstAckWait;
      AutoFirstAckWait: if (csrng_cmd_ack_i) state_d = AutoDispatch;
      AutoAckWait:      if (csrng_cmd_ack_i) state_d = AutoDispatch;
      AutoDispatch: begin
        if (!auto_req_mode_i) begin
          state_d = Idle;
        end else if (max_reqs_cnt_zero_i) begin
          state_d = AutoCaptReseedCnt;
        end else begin
          state_d = AutoCaptGenCnt;
        end
      end
      AutoCaptGenCnt:    state_d = AutoSendGenCmd;
      AutoSendGenCmd:    if (cmd_sent_i) state_d = AutoAckWait;
      AutoCaptReseedCnt: state_d = AutoSendReseedCmd;
      AutoSendReseedCmd: if (cmd_sent_i) state_d = AutoAckWait;
      SWPortMode:         state_d = SWPortMode;
      RejectCsrngEntropy: state_d = RejectCsrngEntropy;
      Error:              state_d = Error;
      default:            state_d = Error;
    endcase

    if (local_escalate_i || csrng_ack_err_i) begin
      state_d = local_escalate_i    ? Error :
                state_q == Error    ? Error : RejectCsrngEntropy;
    end else if (!edn_enable_i && state_q inside {BootLoadIns, BootInsAckWait, BootLoadGen,
                                                  BootGenAckWait, BootLoadUni, BootUniAckWait,
                                                  BootPulse, BootDone,
                                                  AutoLoadIns, AutoFirstAckWait, AutoAckWait,
                                                  AutoDispatch, AutoCaptGenCnt, AutoSendGenCmd,
                                                  AutoCaptReseedCnt, AutoSendReseedCmd,
                                                  SWPortMode, RejectCsrngEntropy}) begin
      state_d = Idle;
    end
  end

endmodule
