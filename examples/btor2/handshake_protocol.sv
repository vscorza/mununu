// Phase 1 example: AMBA-style valid/ready handshake (master side only).
//
// Models a small FSM that asserts `valid` once a `request` arrives, holds
// it until the slave acknowledges with `ready`, then returns to idle.
// Encodes Phase 5A's H1 (handshake stability) property from §7.4 of the
// RTL roadmap, but using a **shadow register** instead of SVA `|=>` —
// Yosys 0.59's `read_verilog -formal -sv` does not parse temporal SVA.
// See README §"Yosys SVA support".
//
// Phase 5A's S1 (payload stability via `$stable`) is NOT verified here:
// adding the payload register and a `prev_payload` shadow pushes the
// clk2fflogic-elaborated state space past `MAX_STATE_BITS = 16`. S1 is
// the natural follow-up once compositional decomposition lands (Phase 3)
// or sv2v preprocessing is wired into the Yosys driver.
//
// SOUNDNESS: state, valid, and the shadow register are exact. The
// shadow `held_valid_no_ready` is an exact finite encoding of "valid
// was asserted last cycle without ready," matching the SVA `(valid &&
// !ready) |=> valid` semantics under the default clocking convention.

module handshake_protocol (
    input  wire clk,
    input  wire rst,
    input  wire request,
    input  wire ready,
    output reg  valid
);
  typedef enum logic [0:0] {
    IDLE      = 1'b0,
    SENDING   = 1'b1
  } state_t;

  state_t state;

  // Shadow: "valid was asserted last cycle without ready". Used to
  // express the H1 obligation as a Boolean assertion over current +
  // shadow signals.
  reg held_valid_no_ready;

  always @(posedge clk) begin
    if (rst) begin
      state               <= IDLE;
      valid               <= 1'b0;
      held_valid_no_ready <= 1'b0;
    end else begin
      // Handshake FSM
      case (state)
        IDLE: begin
          if (request) begin
            state <= SENDING;
            valid <= 1'b1;
          end
        end
        SENDING: begin
          if (ready) begin
            state <= IDLE;
            valid <= 1'b0;
          end
          // While waiting for `ready`, hold valid stable.
        end
      endcase
      // Shadow update
      held_valid_no_ready <= (valid && !ready);
    end

    // H1 — VALID stable until READY. If last cycle valid was high and
    // ready was low, valid must still be high this cycle.
    assert (!held_valid_no_ready || valid);
  end
endmodule
