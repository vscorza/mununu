// AXI4-Lite Slave Interface
// 6-state FSM handling write (AW/W/B) and read (AR/R) channels
// @mununu ltl safety: nu X. ([] X)
// @mununu ltl wr_response: mu X. (WR_RESP || <> X)
module axi4lite_slave(
    input logic clk, input logic rst,
    input logic awvalid, input logic wvalid, input logic bready,
    input logic arvalid, input logic rready,
    output logic awready, output logic wready, output logic bvalid,
    output logic arready, output logic rvalid
);
    typedef enum logic [2:0] {
        IDLE, WR_ADDR, WR_DATA, WR_RESP, RD_ADDR, RD_DATA
    } state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: begin
                if (awvalid) state <= WR_ADDR;
                else if (arvalid) state <= RD_ADDR;
            end
            WR_ADDR: if (wvalid) state <= WR_DATA;
            WR_DATA: state <= WR_RESP;
            WR_RESP: if (bready) state <= IDLE;
            RD_ADDR: state <= RD_DATA;
            RD_DATA: if (rready) state <= IDLE;
        endcase
    end
endmodule
