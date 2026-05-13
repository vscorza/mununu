// Small SoC fragment for Document B §B.8 industrial example.
// Top module: a tiny host that drives a closed-IP DDR3 PHY (declared as
// `(* blackbox *)`) and a tiny open UART peripheral.
//
// Running `mununu` against this file with the yosys frontend triggers
// the §B.7.3 sidecar emission: the driver detects `(* blackbox *)` on
// `ddr3_phy_v2` and auto-emits `ddr3_phy_v2.interface.json` +
// `ddr3_phy_v2.gap_report.json` alongside the BTOR2 output.

// Closed-IP DDR3 PHY wrapper. The example uses 1-bit signals so the
// bit-blaster fits in the default state-space budget; real silicon
// would have 32-bit address/data buses. The (* blackbox *) attribute is
// what matters here — the yosys driver detects it and auto-emits the
// sidecar JSONs alongside the BTOR2 output.
(* blackbox *)
module ddr3_phy_v2(
    input         clk,
    input         reset_n,
    input         host_init_burst,
    input         addr,
    input         wdata,
    output        rdata,
    output        ddr_ready,
    output        ddr_busy
);
endmodule

module uart_peripheral(
    input        clk,
    input        reset_n,
    input        tx_start,
    output reg   tx_done,
    output reg   tx_active
);
    always @(posedge clk or negedge reset_n) begin
        if (!reset_n) begin
            tx_done   <= 1'b0;
            tx_active <= 1'b0;
        end else if (tx_start) begin
            tx_done   <= 1'b0;
            tx_active <= 1'b1;
        end else if (tx_active) begin
            tx_active <= 1'b0;
            tx_done   <= 1'b1;
        end
    end
endmodule

module top(
    input         clk,
    input         reset_n,
    input         host_init_burst,
    input         tx_start,
    input         addr,
    input         wdata,
    output        rdata,
    output        ddr_ready,
    output        ddr_busy,
    output        tx_done,
    output        tx_active
);

    ddr3_phy_v2 ddr(
        .clk(clk),
        .reset_n(reset_n),
        .host_init_burst(host_init_burst),
        .addr(addr),
        .wdata(wdata),
        .rdata(rdata),
        .ddr_ready(ddr_ready),
        .ddr_busy(ddr_busy)
    );

    uart_peripheral uart(
        .clk(clk),
        .reset_n(reset_n),
        .tx_start(tx_start),
        .tx_done(tx_done),
        .tx_active(tx_active)
    );

endmodule
