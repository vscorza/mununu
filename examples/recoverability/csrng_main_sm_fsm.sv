// csrng_main_sm_fsm.sv — the OpenTitan CSRNG app-command state machine, extracted for isolated
// FSM verification. It is the MONOTONE A/B CONTROL for the edn boot handshake (see edn_boot_sm.sv).
//
// PROVENANCE. Verbatim transition logic from OpenTitan `csrng_main_sm.sv`
// (examples/verify/dc_opentitan_csrng_main_sm/source/csrng_main_sm.sv, lowRISC, Apache-2.0). The full
// module needs `csrng_pkg` (which pulls entropy_src_pkg) + the prim_assert macros, so this fixture
// inlines the `csrng_pkg::main_sm_state_e` enum (VERBATIM 6-bit sparse encodings) and the `acmd_e`
// command enum (VERBATIM 3-bit), replaces the `PRIM_FLOP_SPARSE_FSM` macro with a plain async-reset
// flop, and drops the (state-irrelevant) SVA assertions. The `always_comb` FSM — every transition — is
// UNCHANGED.
//
// WHY THIS DESIGN — THE MONOTONE CONTROL. Unlike edn's boot handshake, csrng's command flow is a
// MONOTONE assert-to-progress pipeline: Idle -> ParseCmd -> {EntropyReq|CmdPrep} -> CmdVld -> ... .
// Every gating input advances the flow when ASSERTED and never needs the opposite value later:
//   acmd_avail_i=1 to leave Idle, acmd_eop_i=1 to leave ParseCmd, flag0_i=1 (or cmd_entropy_avail_i=1)
//   to leave EntropyReq, cmd_rdy_i=1 to leave CmdVld — with the safety inputs held safe
//   (local_escalate_i=0, enable_i=1). So a single CONSTANT input vector drives the command to
//   validation and cycles indefinitely; NO input needs a state-dependent (positional) value.
//
//   Property: AG EF (state_q == MainSmCmdVld(16)) — "a command can always be driven to validation."
//   Free-input HOLDS, AND a CONSTANT hold (the vector above) already HOLDS. Therefore the environment
//   -strategy lever has ZERO MARGINAL REACH here: the shipped constant-hold (Phase-2a slice 1) already
//   suffices. Contrast edn_boot_sm, where BOTH constant boot_req_mode_i holds are VIOLATED and only a
//   positional strategy completes the boot. This pair is the "when does strategy synthesis earn its
//   keep" A/B: it earns its keep on level-request/mode-hold handshakes (edn), not on monotone command
//   pipelines (csrng) — both real OpenTitan CSRNG-family FSMs.

module csrng_main_sm_fsm (
    input  logic       clk_i,
    input  logic       rst_ni,
    input  logic       enable_i,
    input  logic       acmd_avail_i,
    input  logic [2:0] acmd_i,
    input  logic       acmd_eop_i,
    input  logic       flag0_i,
    input  logic       cmd_entropy_avail_i,
    input  logic       cmd_rdy_i,
    input  logic       cmd_complete_i,
    input  logic       local_escalate_i,
    output logic [5:0] main_sm_state_o
);

  localparam int MainSmStateWidth = 6;
  typedef enum logic [MainSmStateWidth-1:0] {
    MainSmIdle        = 6'b110111,
    MainSmParseCmd    = 6'b011101,
    MainSmEntropyReq  = 6'b001110,
    MainSmCmdPrep     = 6'b000011,
    MainSmCmdVld      = 6'b010000,
    MainSmClrAData    = 6'b111010,
    MainSmCmdCompWait = 6'b100100,
    MainSmError       = 6'b101001
  } main_sm_state_e;

  // acmd_e command encodings (csrng_pkg, VERBATIM 3-bit).
  localparam logic [2:0] INS = 3'h1;
  localparam logic [2:0] RES = 3'h2;
  localparam logic [2:0] GEN = 3'h3;
  localparam logic [2:0] UPD = 3'h4;
  localparam logic [2:0] UNI = 3'h5;

  main_sm_state_e state_d, state_q;

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) state_q <= MainSmIdle;
    else state_q <= state_d;
  end

  assign main_sm_state_o = state_q;

  always_comb begin
    state_d = state_q;

    if (state_q == MainSmError) begin
      // In the Error state we ignore local escalate and enable.
    end else if (local_escalate_i) begin
      state_d = MainSmError;
    end else if (!enable_i && state_q inside {MainSmIdle, MainSmParseCmd, MainSmEntropyReq,
                                              MainSmCmdPrep, MainSmCmdVld,
                                              MainSmClrAData, MainSmCmdCompWait}) begin
      state_d = MainSmIdle;
    end else begin
      unique case (state_q)
        MainSmIdle: begin
          if (acmd_avail_i) begin
            state_d = MainSmParseCmd;
          end
        end
        MainSmParseCmd: begin
          if (acmd_eop_i) begin
            unique case (acmd_i)
              INS, RES:      state_d = MainSmEntropyReq;
              GEN, UPD, UNI: state_d = MainSmCmdPrep;
              default:       state_d = MainSmIdle;
            endcase
          end
        end
        MainSmEntropyReq: begin
          if (flag0_i) begin
            state_d = MainSmCmdVld;
          end else begin
            if (cmd_entropy_avail_i) begin
              state_d = MainSmCmdVld;
            end
          end
        end
        MainSmCmdPrep: begin
          state_d = MainSmCmdVld;
        end
        MainSmCmdVld: begin
          if (cmd_rdy_i) begin
            if (cmd_complete_i) begin
              state_d = MainSmIdle;
            end else begin
              state_d = MainSmClrAData;
            end
          end
        end
        MainSmClrAData: begin
          if (cmd_complete_i) begin
            state_d = MainSmIdle;
          end else begin
            state_d = MainSmCmdCompWait;
          end
        end
        MainSmCmdCompWait: begin
          if (cmd_complete_i) begin
            state_d = MainSmIdle;
          end
        end
        default: begin
          state_d = MainSmError;
        end
      endcase
    end
  end

endmodule
