// AXI-Lite Write Slave — Fixed (ZipCPU pattern)
//
// The fix: aw_flag clears only when the response handshake completes
// (bvalid && bready), NOT on data acceptance (wready && wvalid).
// This prevents new write acceptance while a response is pending.
//
// Reference: ZipCPU's easyaxil.v / demoaxi.v corrected pattern:
//   https://github.com/ZipCPU/wb2axip
//
// The key change from the buggy Xilinx template:
//   BUGGY:  aw_flag clears on (axi_wready && s_axi_wvalid)
//   FIXED:  aw_flag clears on (axi_bvalid && s_axi_bready)

module axilite_write_slave_xilinx_fixed(
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
    logic aw_flag;

    // AWREADY + flag control
    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            axi_awready <= 0;
            aw_flag     <= 0;
        end else begin
            if (~axi_awready && s_axi_awvalid && ~aw_flag) begin
                axi_awready <= 1;
                aw_flag     <= 1;
            end else if (axi_bvalid && s_axi_bready) begin
                // FIX: flag clears on RESPONSE acknowledgment, not data acceptance
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
                axi_bvalid <= 1;
            else if (s_axi_bready && axi_bvalid)
                axi_bvalid <= 0;
        end
    end

    assign s_axi_awready = axi_awready;
    assign s_axi_wready  = axi_wready;
    assign s_axi_bvalid  = axi_bvalid;

endmodule
