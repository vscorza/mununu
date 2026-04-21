// AXI-Lite Write System Top Module
//
// Instantiates the write master and buggy write slave,
// connecting them via AXI-Lite write channel signals.

module axilite_system_top(
    input  logic clk,
    input  logic rst,
    input  logic cmd_write,
    input  logic cmd_slow_resp
);

    wire awvalid, awready;
    wire wvalid, wready;
    wire bvalid, bready;

    axilite_write_master master_inst(
        .clk(clk),
        .rst(rst),
        .cmd_write(cmd_write),
        .cmd_slow_resp(cmd_slow_resp),
        .m_axi_awvalid(awvalid),
        .m_axi_awready(awready),
        .m_axi_wvalid(wvalid),
        .m_axi_wready(wready),
        .m_axi_bvalid(bvalid),
        .m_axi_bready(bready)
    );

    axilite_write_slave_xilinx_bug slave_inst(
        .clk(clk),
        .rst(rst),
        .s_axi_awvalid(awvalid),
        .s_axi_awready(awready),
        .s_axi_wvalid(wvalid),
        .s_axi_wready(wready),
        .s_axi_bvalid(bvalid),
        .s_axi_bready(bready)
    );

endmodule
