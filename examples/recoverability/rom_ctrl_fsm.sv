// rom_ctrl_fsm.sv — the OpenTitan ROM controller check FSM, extracted for isolated FSM verification.
// The THIRD real POSITIONAL environment-strategy case (a secure-boot ROM-integrity controller — a
// different security-critical function than edn's boot flow, otbn's compute start/stop, or csrng's RNG
// command). It confirms the "acknowledge only when expected" positional pattern (first seen on otbn) is a
// RECURRING OpenTitan security-FSM discipline, not a one-off.
//
// PROVENANCE. Verbatim transition logic from OpenTitan `rom_ctrl_fsm.sv`
// (examples/verify/dc_opentitan_rom_ctrl_fsm/source/rom_ctrl_fsm.sv, lowRISC, Apache-2.0). This fixture:
//   - inlines the `rom_ctrl_pkg::fsm_state_e` enum (VERBATIM 10-bit encodings: a 6-bit sparse code
//     concatenated with a 4-bit mubi4 that flags "== Done"; MuBi4True=4'h6, MuBi4False=4'h9),
//   - replaces the `PRIM_FLOP_SPARSE_FSM` macro with a plain async-reset flop,
//   - BLACK-BOXES the two instantiated submodules (`rom_ctrl_counter`, `rom_ctrl_compare`, not part of
//     this module's own logic) by exposing their FSM-relevant outputs as free primary inputs:
//     `counter_lnt`, `counter_done`, `checker_done`, `checker_alert`. This is the standard chaotic-stub
//     over-approximation (the environment controls them) and is exactly what makes the ROM/KMAC datapath
//     drop out of the state cone.
//   - drops the state-irrelevant 256-bit digest datapath (`digest_i`/`exp_digest_i`/`kmac_digest_i`),
//     the KMAC/ROM data-plane outputs, and the pwrmgr/keymgr output routing.
// The `always_comb` FSM — every state transition, the `{kmac_done_i, counter_done}` race branch, the
// SEC_CM consistency-check trap, and the alert→Invalid escalation — is UNCHANGED.
//
// WHY THIS DESIGN — a THIRD positional case (ack-when-expected). `kmac_done_i` (the "KMAC digest is
// ready" ACK) must take OPPOSITE values in two states to reach the terminal Done:
//   - `= 0` at ReadingLow — a stray ACK outside {ReadingHigh, RomAhead} trips the consistency check
//     (SEC_CM: CHECKER.CTRL_FLOW.CONSISTENCY) → Invalid (raises an alert next cycle).
//   - `= 1` at ReadingHigh / RomAhead — to hand off the completed digest and progress to Checking → Done.
// No constant ACK reaches Done (const-1 → Invalid at ReadingLow; const-0 → stalls at RomAhead), yet a
// POSITIONAL strategy does. The FSM is LINEAR (upstream `ASSERT_FPV_LINEAR_FSM`): Done ← Checking ←
// {RomAhead | KmacAhead} ← ReadingHigh ← ReadingLow, so Done has a UNIQUE path (no alternate entry).
//
//   Property: AG EF (state_q == Done (518)) — "the ROM check can always be driven to completion."
//   Free-input HOLDS (∃ the positional path; the free reset provides the AG-escape from Invalid); every
//   CONSTANT kmac_done_i hold is VIOLATED.

module rom_ctrl_fsm (
    input  logic       clk_i,
    input  logic       rst_ni,
    input  logic       kmac_rom_rdy_i,
    input  logic       kmac_done_i,
    input  logic       kmac_err_i,
    // Black-boxed submodule outputs (rom_ctrl_counter / rom_ctrl_compare) — free environment inputs.
    input  logic       counter_lnt,
    input  logic       counter_done,
    input  logic       checker_done,
    input  logic       checker_alert,
    output logic [9:0] main_fsm_state_o,
    output logic       alert_o
);

  localparam int StateWidth = 10;
  typedef enum logic [StateWidth-1:0] {
    ReadingLow  = 10'b0011001001, // {6'b001100, MuBi4False(4'h9)}
    ReadingHigh = 10'b0010111001, // {6'b001011, MuBi4False}
    RomAhead    = 10'b1110011001, // {6'b111001, MuBi4False}
    KmacAhead   = 10'b1001111001, // {6'b100111, MuBi4False}
    Checking    = 10'b0101011001, // {6'b010101, MuBi4False}
    Done        = 10'b1000000110, // {6'b100000, MuBi4True(4'h6)} == 518
    Invalid     = 10'b0100101001  // {6'b010010, MuBi4False}
  } fsm_state_e;

  fsm_state_e state_d, state_q;
  logic       fsm_alert;
  logic       unexpected_counter_change;

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) state_q <= ReadingLow;
    else state_q <= state_d;
  end

  assign main_fsm_state_o = state_q;

  always_comb begin
    state_d = state_q;
    fsm_alert = 1'b0;

    unique case (state_q)
      ReadingLow: begin
        if (counter_lnt && kmac_rom_rdy_i) begin
          state_d = ReadingHigh;
        end
      end

      ReadingHigh: begin
        unique case ({kmac_done_i, counter_done})
          2'b01: state_d = RomAhead;
          2'b10: state_d = kmac_err_i ? Invalid : KmacAhead;
          2'b11: state_d = kmac_err_i ? Invalid : Checking;
          default: ; // No change
        endcase
      end

      RomAhead: begin
        if (kmac_done_i) state_d = kmac_err_i ? Invalid : Checking;
      end

      KmacAhead: begin
        if (counter_done) state_d = Checking;
      end

      Checking: begin
        if (checker_done) state_d = Done;
      end

      Done: begin
        // Final state
      end

      default: begin
        fsm_alert = 1'b1;
        state_d = Invalid;
      end
    endcase

    // SEC_CM: CHECKER.CTRL_FLOW.CONSISTENCY — the "signal high only when expected" trap.
    if ((checker_done && !(state_q inside {Checking, Done})) ||
        (counter_done && state_q == ReadingLow) ||
        (kmac_done_i && !(state_q inside {ReadingHigh, RomAhead}))) begin
      state_d = Invalid;
    end

    // SEC_CM: CHECKER.FSM.LOCAL_ESC — any alert forces Invalid.
    if (alert_o) begin
      state_d = Invalid;
    end
  end

  // unexpected_counter_change = mubi4_test_true_loose(in_state_done) & !counter_done, and only Done
  // carries the MuBi4True bottom bits, so this reduces to (state_q == Done) & ~counter_done.
  assign unexpected_counter_change = (state_q == Done) & ~counter_done;
  assign alert_o = fsm_alert | checker_alert | unexpected_counter_change;

endmodule
