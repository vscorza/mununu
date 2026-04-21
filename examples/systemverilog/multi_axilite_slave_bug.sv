// AXI-Lite Write Slave — Buggy (Xilinx pattern)
//
// Based on the Xilinx Vivado AXI-lite slave template bug documented by
// ZipCPU (zipcpu.com/formal/2019/04/16/axi-mistakes.html).
//
// BUG: The slave's IDLE state accepts a new transaction (awvalid && wvalid)
// without checking whether the previous response has been acknowledged
// (bvalid && bready). When the master applies backpressure (bready low),
// a new transaction overwrites the pending response — the first response
// is DROPPED, and the master waits forever.
//
// This is a two-module interaction bug: the slave works correctly when
// bready is always high, but fails under legitimate backpressure.

module axilite_slave_bug(
    input  logic clk,
    input  logic rst,
    input  logic awvalid,
    input  logic wvalid,
    input  logic bready,
    output logic bvalid
);

    typedef enum logic [1:0] {IDLE, ACTIVE, RESPOND} state_t;
    state_t state;

    logic bvalid_r;
    logic pending;  // tracks whether a response is outstanding

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state    <= IDLE;
            bvalid_r <= 0;
            pending  <= 0;
        end else begin
            case (state)
                IDLE: begin
                    // BUG: no guard on pending/bvalid_r — accepts new write
                    // even if previous response hasn't been acknowledged
                    if (awvalid && wvalid)
                        state <= RESPOND;
                    else if (awvalid || wvalid)
                        state <= ACTIVE;
                end
                ACTIVE: begin
                    // Wait for both address and data (simplified)
                    if (awvalid || wvalid)
                        state <= RESPOND;
                end
                RESPOND: begin
                    bvalid_r <= 1;
                    pending  <= 1;
                    // BUG: returns to IDLE immediately without waiting for bready
                    state <= IDLE;
                end
            endcase

            // bready clears bvalid independently
            if (bvalid_r && bready) begin
                bvalid_r <= 0;
                pending  <= 0;
            end
        end
    end

    assign bvalid = bvalid_r;

endmodule
