// AXI-Lite Write Slave — Xilinx Vivado Pattern (Buggy)
//
// Faithful to the Xilinx Vivado 2018.3 AXI4 slave template write channel.
// Adapted from the AXI4-Full pattern to AXI-Lite (single-beat, no WLAST).
//
// Reference: https://zipcpu.com/formal/2019/05/13/axifull.html
//
// BUG: The write-in-progress flag (aw_flag) clears on data acceptance
// (awready && awvalid && wready && wvalid), NOT on response acknowledgment
// (bvalid && bready). This creates a window where:
//   1. First write completes data phase → flag clears, bvalid asserts
//   2. Master holds bready low (backpressure)
//   3. New awvalid arrives → accepted (flag is 0) → flag sets again
//   4. Second write completes → bvalid re-asserted (but was already high!)
//   5. First response is DROPPED — master waits forever
//
// In AXI-Lite, every transfer is single-beat, so the "WLAST" condition
// is implicit in every WVALID && WREADY handshake.

module axilite_write_slave_xilinx_bug(
    input  logic        clk,
    input  logic        rst,
    input  logic        s_axi_awvalid,
    output logic        s_axi_awready,
    input  logic        s_axi_wvalid,
    output logic        s_axi_wready,
    output logic        s_axi_bvalid,
    input  logic        s_axi_bready
);

    logic axi_awready;
    logic axi_wready;
    logic axi_bvalid;
    logic aw_flag;  // write-in-progress flag (axi_awv_awr_flag in Xilinx code)

    // AWREADY + flag control
    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            axi_awready <= 0;
            aw_flag     <= 0;
        end else begin
            if (~axi_awready && s_axi_awvalid && ~aw_flag) begin
                // Accept new write address when no write is in progress
                axi_awready <= 1;
                aw_flag     <= 1;
            end else if (axi_wready && s_axi_wvalid) begin
                // BUG: flag clears on DATA acceptance, not RESPONSE acceptance
                // This is the exact Xilinx pattern: WLAST && WREADY clears the flag
                aw_flag <= 0;
                axi_awready <= 0;
            end else begin
                axi_awready <= 0;
            end
        end
    end

    // WREADY control
    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            axi_wready <= 0;
        end else begin
            if (~axi_wready && s_axi_wvalid && aw_flag)
                axi_wready <= 1;
            else
                axi_wready <= 0;
        end
    end

    // BVALID control
    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            axi_bvalid <= 0;
        end else begin
            if (axi_wready && s_axi_wvalid && ~axi_bvalid)
                // Assert bvalid when data is accepted
                axi_bvalid <= 1;
            else if (s_axi_bready && axi_bvalid)
                // Clear bvalid when master acknowledges
                axi_bvalid <= 0;
        end
    end

    assign s_axi_awready = axi_awready;
    assign s_axi_wready  = axi_wready;
    assign s_axi_bvalid  = axi_bvalid;

endmodule
