// Phase 1 example: 2-client round-robin arbiter, safety-only encoding.
//
// Demonstrates a mid-size FSM with multi-bit ports through the
// SV → Yosys → BTOR2 → CLTS pipeline: 1-bit round-robin pointer +
// 2-bit `req` and 2-bit `gnt` vectors. Together with the clk2fflogic
// edge-detect latches, the elaborated BTOR2 carries 6–8 state bits.
//
// Liveness on this design (each requester eventually granted) requires
// SVA's `s_eventually`, which Yosys 0.59's `read_verilog -formal -sv`
// does not parse — see the README §"Yosys SVA support" for the gap and
// the workaround paths (sv2v preprocessing, hand-authored .btor with
// `justice` lines, etc.). The mutual-exclusion safety property below
// is the part Yosys actually accepts; it is exact for this design.
//
// SOUNDNESS: small finite state space, fully bit-blasted.
// Mutual exclusion is verified end-to-end. The per-client liveness
// claim ("each client is eventually granted") is NOT verified here.

module fair_arbiter (
    input  wire       clk,
    input  wire       rst,
    input  wire [1:0] req,
    output reg  [1:0] gnt
);
  // 1-bit round-robin pointer: 0 = client 0 has priority, 1 = client 1.
  reg priority_ptr;

  always @(posedge clk) begin
    if (rst) begin
      gnt          <= 2'b00;
      priority_ptr <= 1'b0;
    end else begin
      gnt <= 2'b00;
      // Round-robin: prefer the priority client when both ask.
      if (req[priority_ptr]) begin
        gnt[priority_ptr] <= 1'b1;
        priority_ptr      <= ~priority_ptr;
      end else if (req[~priority_ptr]) begin
        gnt[~priority_ptr] <= 1'b1;
        priority_ptr       <= priority_ptr;  // pointer unchanged
      end
    end
  end

  // Safety: at most one grant at a time (mutual exclusion).
  // Pure Boolean immediate assertion — Yosys lowers it to a BTOR2 `bad`
  // line via clk2fflogic + chformal -lower.
  always @(posedge clk) begin
    assert (!(gnt[0] && gnt[1]));
  end
endmodule
