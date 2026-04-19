// AXI-Lite Write Channel — Overlapping Transaction Bug
//
// Based on the Xilinx Vivado AXI-lite slave template bug documented by
// ZipCPU (zipcpu.com/formal/2019/04/16/axi-mistakes.html).
//
// BUG: The slave transitions from RESPOND to IDLE without clearing bvalid
// first. The next cycle, IDLE accepts a new awvalid/wvalid even though
// bvalid is still asserted from the previous transaction.
//
// This models the real Xilinx bug: ready signals are not gated by
// response channel availability.

module axilite_deadlock_bug(
    input  logic clk,
    input  logic rst,
    input  logic awvalid,
    input  logic wvalid,
    input  logic bready,
    output logic awready,
    output logic wready,
    output logic bvalid
);

    typedef enum logic [2:0] {IDLE, ADDR_WAIT, DATA_WAIT, RESPOND} state_t;
    state_t state;

    logic bvalid_r;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state    <= IDLE;
            bvalid_r <= 0;
        end else begin
            case (state)
                IDLE: begin
                    // BUG: no check for bvalid_r — accepts new transaction
                    // even if previous response hasn't been acknowledged
                    if (awvalid && wvalid)
                        state <= RESPOND;
                    else if (awvalid)
                        state <= ADDR_WAIT;
                    else if (wvalid)
                        state <= DATA_WAIT;
                end
                ADDR_WAIT: begin
                    if (wvalid) state <= RESPOND;
                end
                DATA_WAIT: begin
                    if (awvalid) state <= RESPOND;
                end
                RESPOND: begin
                    bvalid_r <= 1;
                    // BUG: returns to IDLE unconditionally — bvalid stays high
                    state <= IDLE;
                end
            endcase

            // bready clears bvalid independently
            if (bvalid_r && bready)
                bvalid_r <= 0;
        end
    end

    assign awready = (state == IDLE);
    assign wready  = (state == IDLE);
    assign bvalid  = bvalid_r;

endmodule
