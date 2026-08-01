// otbn_start_stop_fsm.sv — the OpenTitan OTBN (big-number crypto co-processor) start/stop controller
// state machine, extracted for isolated FSM verification. It is the SECOND real POSITIONAL
// environment-strategy case (a different module family from edn_boot_sm.sv — a compute-engine
// start/execute/secure-wipe handshake, not a boot/command flow).
//
// PROVENANCE. Verbatim transition logic from OpenTitan `otbn_start_stop_control.sv`
// (examples/verify/dc_opentitan_otbn_start_stop_control/source/otbn_start_stop_control.sv, lowRISC,
// Apache-2.0). The full module needs otbn_pkg + prim_mubi_pkg + lc_ctrl/otp packages + prim_assert +
// the prim_mubi4_sender flops, so this fixture:
//   - inlines the `otbn_start_stop_state_e` enum (VERBATIM 8-bit sparse encodings from otbn_pkg),
//   - replaces the `PRIM_FLOP_SPARSE_FSM` macro + prim_mubi4_sender flops with plain async-reset flops,
//   - DE-MUBI's the two life-cycle control inputs `escalate_en_i` / `rma_req_i` (mubi4_t, 4-bit) to
//     plain 1-bit `escalate_i` / `rma_req_i` — the multi-bit-bool ENCODING is a fault-hardening detail
//     irrelevant to the FSM's state reachability; the control ROLE (esc_request / rma_request, and hence
//     `stop`) is preserved exactly. `wipe_after_urnd_refresh` likewise becomes a plain 1-bit register
//     (0 = MuBi4False init, 1 = MuBi4True), matching the prim_mubi4_sender ResetValue.
//   - drops the state-irrelevant secure-wipe DATAPATH outputs (sec_wipe_*), the error/status registers
//     (state_error, mubi_err, ...), the mubi-INVALID→Locked checks (unreachable with 1-bit control), and
//     SecSkipUrndReseedAtStart (=0 ⇒ skip_reseed_q=0).
// The `always_comb` FSM — every state transition, including the spurious-URND-ACK trap and the addr_cnt
// secure-wipe phase counter — is UNCHANGED.
//
// WHY THIS DESIGN — A SECOND POSITIONAL CASE. Two primary inputs need STATE-DEPENDENT (positional)
// values to drive a full start→execute→secure-wipe cycle, so NO constant hold works:
//   - `secure_wipe_req_i` (= `stop` when escalation/RMA are inactive) must be 0 at Halt and UrndRefresh
//     (to reach Running) AND 1 at Running (to leave into the secure wipe). Const-0 never leaves Running;
//     const-1 at Halt goes straight to Locked (a trap).
//   - `urnd_reseed_ack_i` must be 1 at UrndRefresh (to reach Running) AND 0 elsewhere — a stray ACK
//     outside {Initial, UrndRefresh} hits the spurious-ACK trap → Locked.
//
//   Property (with escalation + RMA inactive — normal operation, escalate_i=0, rma_req_i=0):
//     AG EF (state_q == OtbnStartStopSecureWipeWdrUrnd(144)) — "the design can always be driven through
//     a full start → run → secure-wipe cycle." Free-input HOLDS (∃ the positional path; the free reset
//     provides the AG-escape from traps); every CONSTANT secure_wipe_req_i hold is VIOLATED. The witness
//     exhibits secure_wipe_req_i=0 at Halt/UrndRefresh and =1 at Running — the positional strategy.

module otbn_start_stop_fsm (
    input  logic       clk_i,
    input  logic       rst_ni,
    input  logic       start_i,
    input  logic       secure_wipe_req_i,
    input  logic       urnd_reseed_ack_i,
    input  logic       escalate_i,   // de-mubi'd escalate_en_i (mubi4_test_true_loose)
    input  logic       rma_req_i,    // de-mubi'd rma_req_i (mubi4_test_true_strict)
    output logic [7:0] main_fsm_state_o
);

  localparam int StateWidth = 8;
  typedef enum logic [StateWidth-1:0] {
    OtbnStartStopStateInitial             = 8'b10100111,
    OtbnStartStopStateHalt                = 8'b00000001,
    OtbnStartStopStateUrndRefresh         = 8'b11110010,
    OtbnStartStopStateRunning             = 8'b01110111,
    OtbnStartStopSecureWipeWdrUrnd        = 8'b10010000,
    OtbnStartStopSecureWipeAccModBaseUrnd = 8'b10001110,
    OtbnStartStopSecureWipeExtIsprsUrnd   = 8'b00110100,
    OtbnStartStopSecureWipeAllZero        = 8'b01111000,
    OtbnStartStopSecureWipeComplete       = 8'b10101001,
    OtbnStartStopStateLocked              = 8'b01101101
  } otbn_start_stop_state_e;

  otbn_start_stop_state_e state_d, state_q;

  // `stop` and the latched lock, verbatim (esc/rma de-mubi'd to plain 1-bit).
  logic esc_request, rma_request, should_lock_d, should_lock_q, stop;
  assign esc_request   = escalate_i;
  assign rma_request   = rma_req_i;
  assign stop          = esc_request | rma_request | secure_wipe_req_i;
  assign should_lock_d = should_lock_q | esc_request | rma_request;

  // wipe_after_urnd_refresh: plain 1-bit (0 = MuBi4False init, 1 = MuBi4True).
  logic wipe_after_urnd_refresh_d, wipe_after_urnd_refresh_q;

  // Secure-wipe phase counter, verbatim.
  logic addr_cnt_inc;
  logic [5:0] addr_cnt_q, addr_cnt_d;

  assign main_fsm_state_o = state_q;

  always_ff @(posedge clk_i or negedge rst_ni) begin
    if (!rst_ni) begin
      state_q                   <= OtbnStartStopStateInitial;
      should_lock_q             <= 1'b0;
      wipe_after_urnd_refresh_q <= 1'b0; // MuBi4False
      addr_cnt_q                <= 6'd0;
    end else begin
      state_q                   <= state_d;
      should_lock_q             <= should_lock_d;
      wipe_after_urnd_refresh_q <= wipe_after_urnd_refresh_d;
      addr_cnt_q                <= addr_cnt_d;
    end
  end

  always_comb begin
    state_d                   = state_q;
    addr_cnt_inc              = 1'b0;
    wipe_after_urnd_refresh_d = wipe_after_urnd_refresh_q;

    unique case (state_q)
      OtbnStartStopStateInitial: begin
        if (rma_request) begin
          state_d = OtbnStartStopSecureWipeWdrUrnd;
          wipe_after_urnd_refresh_d = 1'b1; // MuBi4True
        end else if (urnd_reseed_ack_i) begin
          state_d = OtbnStartStopSecureWipeWdrUrnd;
        end
      end
      OtbnStartStopStateHalt: begin
        if (stop && !rma_request) begin
          state_d = OtbnStartStopStateLocked;
        end else if (start_i || rma_request) begin
          if (rma_request) begin
            state_d = OtbnStartStopSecureWipeWdrUrnd;
            wipe_after_urnd_refresh_d = 1'b1; // MuBi4True
          end else begin // start_i
            state_d = OtbnStartStopStateUrndRefresh;
          end
        end
      end
      OtbnStartStopStateUrndRefresh: begin
        if (stop) begin
          if (!wipe_after_urnd_refresh_q) begin
            state_d = OtbnStartStopStateLocked;
          end else begin
            if (urnd_reseed_ack_i) begin
              state_d = OtbnStartStopSecureWipeWdrUrnd;
            end
          end
        end else begin
          if (!wipe_after_urnd_refresh_q) begin
            if (urnd_reseed_ack_i) begin
              state_d = OtbnStartStopStateRunning;
            end
          end else begin
            if (urnd_reseed_ack_i) begin
              state_d = OtbnStartStopSecureWipeWdrUrnd;
            end
          end
        end
      end
      OtbnStartStopStateRunning: begin
        if (stop) begin
          state_d = OtbnStartStopSecureWipeWdrUrnd;
        end
      end
      OtbnStartStopSecureWipeWdrUrnd: begin
        addr_cnt_inc = 1'b1;
        if (addr_cnt_q == 6'b100000) begin
          addr_cnt_inc = 1'b0;
          state_d = OtbnStartStopSecureWipeAccModBaseUrnd;
        end
      end
      OtbnStartStopSecureWipeAccModBaseUrnd: begin
        addr_cnt_inc = 1'b1;
        if (addr_cnt_q == 6'b011111) begin
          addr_cnt_inc = 1'b0;
          state_d = OtbnStartStopSecureWipeExtIsprsUrnd;
        end
      end
      OtbnStartStopSecureWipeExtIsprsUrnd: begin
        addr_cnt_inc = 1'b1;
        if (addr_cnt_q == 6'b011111) begin
          addr_cnt_inc = 1'b0;
          state_d = OtbnStartStopSecureWipeAllZero;
        end
      end
      OtbnStartStopSecureWipeAllZero: begin
        if (!wipe_after_urnd_refresh_q) begin
          state_d = OtbnStartStopStateUrndRefresh;
          wipe_after_urnd_refresh_d = 1'b1; // MuBi4True
        end else begin
          state_d = OtbnStartStopSecureWipeComplete;
        end
      end
      OtbnStartStopSecureWipeComplete: begin
        state_d = should_lock_d ? OtbnStartStopStateLocked : OtbnStartStopStateHalt;
        wipe_after_urnd_refresh_d = 1'b0; // MuBi4False
      end
      OtbnStartStopStateLocked: begin
        // Terminal state.
      end
      default: begin
        state_d = OtbnStartStopStateLocked;
      end
    endcase

    // Spurious-URND-ACK trap, verbatim: a stray ACK outside {Initial, UrndRefresh} → Locked.
    if (urnd_reseed_ack_i &&
        !(state_q inside {OtbnStartStopStateInitial, OtbnStartStopStateUrndRefresh})) begin
      state_d = OtbnStartStopStateLocked;
    end
  end

  assign addr_cnt_d = addr_cnt_inc ? (addr_cnt_q + 6'd1) : 6'd0;

endmodule
