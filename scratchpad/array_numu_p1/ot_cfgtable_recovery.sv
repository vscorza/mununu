// ot_cfgtable_recovery.sv — OpenTitan-SHAPED SPCR test cases (synthetic; a demo,
// not a finding extracted from real OT RTL — see claims-integrity §"planted/
// synthetic = demo"). Two variants that together characterize the BOUNDARY of
// SPCR's decidable fragment on a per-channel config-register-file idiom.
//
// Both model an OT-idiomatic controller (async active-low reset) with a per-channel
// CONFIGURATION register file `chan_cfg[N]` (the shape used by dma / edn / spi_host
// per-channel config), and a status FSM that enters STALLED on a request and can
// return to IDLE (recover) ONLY when the config word of the selected channel is
// fully enabled (all-ones). Recovery routes through ARRAY CONTENT read at a
// registered index — the array-νμ wall (exact-symbolic SKIPs the in-cone $mem; the
// cube's must-edge is an AUFBV ∀-over-array query → Unknown → the νμ abstains ⊥).
//
// The two variants differ ONLY in WHICH signal the recovery index latches, and that
// difference decides whether SPCR applies:
//
//   (A) ot_cfgtable_recovery_*  — the recovery index latches `req_chan_i`, an input
//       INDEPENDENT of the write address `cfg_addr_i`. On a new request the index
//       jumps to an arbitrary, freely-chosen cell whose content was set by past
//       writes. No finite set of prophecy registers can track a cell selected by a
//       free input independent of the write port, so SPCR SOUNDLY ABSTAINS (falls to
//       the cube → ⊥). This is the OUTSIDE-the-fragment boundary case.
//
//   (B) ot_lastcfg_recovery_*   — the recovery index latches `cfg_addr_i`, i.e. the
//       controller processes the JUST-CONFIGURED channel (the P-B.1 "latch the last-
//       written address" pattern, idx' = ite(latch, waddr, idx)). Now the index only
//       ever moves TO the write address, so mem'[idx'] = mem'[waddr] = wdata is exact
//       — SPCR registerizes the accessed cell, drops the array → QF_BV → exact-
//       symbolic decides HOLDS, at O(#accessed-cells), array-size-independent.
//
// SPCR is a verdict-preserving reformulation (not an approximation); the free
// unconditional SW write port is a documented over-approximation of the real
// cfg_we-gated register write — for an AG EF recoverability demo it only makes
// recovery more reachable.

// ============================ VARIANT (A) core ============================
// Recovery index latches `req_chan_i` (independent of the write address).
// SPCR SOUNDLY ABSTAINS: the index moves to a free-input-selected cell.
module ot_cfg_recover #(
    parameter  int unsigned NumChan = 4,
    parameter  int unsigned CfgW    = 4,
    localparam int unsigned ChanW   = (NumChan <= 1) ? 1 : $clog2(NumChan)
) (
    input                    clk_i,
    input                    rst_ni,
    input                    req_i,
    input  [ChanW-1:0]       req_chan_i,   // selects the stalled channel
    input  [ChanW-1:0]       cfg_addr_i,   // SW config write address (INDEPENDENT)
    input  [CfgW-1:0]        cfg_wdata_i,
    output                   stalled
);
    logic [CfgW-1:0]  chan_cfg [0:NumChan-1];
    logic [ChanW-1:0] active_chan;
    logic             stalled_q;

    always_ff @(posedge clk_i) begin
        chan_cfg[cfg_addr_i] <= cfg_wdata_i;   // free SW write port
    end

    always_ff @(posedge clk_i or negedge rst_ni) begin
        if (!rst_ni) begin
            stalled_q   <= 1'b0;
            active_chan <= '0;
        end else if (req_i && !stalled_q) begin
            stalled_q   <= 1'b1;
            active_chan <= req_chan_i;          // index INDEPENDENT of write addr
        end else if (stalled_q) begin
            if (chan_cfg[active_chan] == {CfgW{1'b1}})
                stalled_q <= 1'b0;
        end
    end
    assign stalled = stalled_q;
endmodule

// ============================ VARIANT (B) core ============================
// Recovery index latches `cfg_addr_i` (the write address) — process the just-
// configured channel. SPCR DECIDES (index only moves to the write address).
module ot_lastcfg_recover #(
    parameter  int unsigned NumChan = 4,
    parameter  int unsigned CfgW    = 4,
    localparam int unsigned ChanW   = (NumChan <= 1) ? 1 : $clog2(NumChan)
) (
    input                    clk_i,
    input                    rst_ni,
    input                    req_i,
    input  [ChanW-1:0]       cfg_addr_i,   // SW config write address = latched idx
    input  [CfgW-1:0]        cfg_wdata_i,
    output                   stalled
);
    logic [CfgW-1:0]  chan_cfg [0:NumChan-1];
    logic [ChanW-1:0] active_chan;
    logic             stalled_q;

    always_ff @(posedge clk_i) begin
        chan_cfg[cfg_addr_i] <= cfg_wdata_i;   // free SW write port
    end

    always_ff @(posedge clk_i or negedge rst_ni) begin
        if (!rst_ni) begin
            stalled_q   <= 1'b0;
            active_chan <= '0;
        end else if (req_i && !stalled_q) begin
            stalled_q   <= 1'b1;
            active_chan <= cfg_addr_i;          // latch the WRITE address (P-B.1)
        end else if (stalled_q) begin
            if (chan_cfg[active_chan] == {CfgW{1'b1}})
                stalled_q <= 1'b0;
        end
    end
    assign stalled = stalled_q;
endmodule

// ---- VARIANT (A) instances: SPCR SOUNDLY ABSTAINS (index independent of waddr) ----
// @mununu_guarantee nu Y.((mu X.(stalled==0 || <> X)) && [] Y)
module ot_cfgtable_recovery_small (
    input clk_i, input rst_ni, input req_i,
    input [1:0] req_chan_i, input [1:0] cfg_addr_i, input [3:0] cfg_wdata_i,
    output stalled
);
    ot_cfg_recover #(.NumChan(4), .CfgW(4)) u (
        .clk_i(clk_i), .rst_ni(rst_ni), .req_i(req_i),
        .req_chan_i(req_chan_i), .cfg_addr_i(cfg_addr_i),
        .cfg_wdata_i(cfg_wdata_i), .stalled(stalled));
endmodule

module ot_cfgtable_recovery_large (
    input clk_i, input rst_ni, input req_i,
    input [7:0] req_chan_i, input [7:0] cfg_addr_i, input [7:0] cfg_wdata_i,
    output stalled
);
    ot_cfg_recover #(.NumChan(256), .CfgW(8)) u (
        .clk_i(clk_i), .rst_ni(rst_ni), .req_i(req_i),
        .req_chan_i(req_chan_i), .cfg_addr_i(cfg_addr_i),
        .cfg_wdata_i(cfg_wdata_i), .stalled(stalled));
endmodule

// ---- VARIANT (B) instances: SPCR DECIDES (latch the write address) ----
// SMALL (4 chan x 4 bits = 16 array bits): ROBDD-oracle-checkable; SPCR must AGREE.
// LARGE (256 chan x 8 bits = 2048 array bits): defeats registerization; SPCR is the
// ONLY path that decides (one prophecy register, array dropped).
// @mununu_guarantee nu Y.((mu X.(stalled==0 || <> X)) && [] Y)
module ot_lastcfg_recovery_small (
    input clk_i, input rst_ni, input req_i,
    input [1:0] cfg_addr_i, input [3:0] cfg_wdata_i,
    output stalled
);
    ot_lastcfg_recover #(.NumChan(4), .CfgW(4)) u (
        .clk_i(clk_i), .rst_ni(rst_ni), .req_i(req_i),
        .cfg_addr_i(cfg_addr_i), .cfg_wdata_i(cfg_wdata_i), .stalled(stalled));
endmodule

module ot_lastcfg_recovery_large (
    input clk_i, input rst_ni, input req_i,
    input [7:0] cfg_addr_i, input [7:0] cfg_wdata_i,
    output stalled
);
    ot_lastcfg_recover #(.NumChan(256), .CfgW(8)) u (
        .clk_i(clk_i), .rst_ni(rst_ni), .req_i(req_i),
        .cfg_addr_i(cfg_addr_i), .cfg_wdata_i(cfg_wdata_i), .stalled(stalled));
endmodule
