// Phase 1 example: 3-state traffic-light FSM (RED → GREEN → YELLOW → RED).
// Pure-FSM safety only. Exercises the simplest "real-shaped" SV idiom —
// `enum`, `case` block, `always_ff` — through the Yosys → BTOR2 path.
//
// SOUNDNESS: deterministic, single-clock; no abstraction beyond what
// `chformal -lower` introduces (edge-detect latches via clk2fflogic).
// Both safety assertions are exact.

module traffic_light (
    input  wire clk,
    input  wire rst,
    output wire is_red,
    output wire is_green,
    output wire is_yellow
);
  typedef enum logic [1:0] {
    RED    = 2'b00,
    GREEN  = 2'b01,
    YELLOW = 2'b10
  } state_t;

  state_t state, next_state;

  always @(posedge clk) begin
    if (rst) state <= RED;
    else     state <= next_state;
  end

  always @* begin
    case (state)
      RED:    next_state = GREEN;
      GREEN:  next_state = YELLOW;
      YELLOW: next_state = RED;
      default: next_state = RED;
    endcase
  end

  assign is_red    = (state == RED);
  assign is_green  = (state == GREEN);
  assign is_yellow = (state == YELLOW);

  // Safety: at most one light on at any instant.
  always @(posedge clk) begin
    assert (!(is_red && is_green));
    assert (!(is_green && is_yellow));
    assert (!(is_red && is_yellow));
  end
endmodule
