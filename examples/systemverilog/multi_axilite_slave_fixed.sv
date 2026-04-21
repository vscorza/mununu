// AXI-Lite Write Slave — Fixed
//
// The fix: IDLE state only accepts new transactions when no response
// is pending (!pending). This prevents the slave from overwriting a
// response that hasn't been acknowledged yet.
//
// Reference: ZipCPU's easyaxil.v pattern:
//   assign axil_write_ready = awskd_valid && wskd_valid
//                             && (!S_AXI_BVALID || S_AXI_BREADY);

module axilite_slave_fixed(
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
    logic pending;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state    <= IDLE;
            bvalid_r <= 0;
            pending  <= 0;
        end else begin
            case (state)
                IDLE: begin
                    // FIX: only accept new write if no response is pending
                    if (!pending && awvalid && wvalid)
                        state <= RESPOND;
                    else if (!pending && (awvalid || wvalid))
                        state <= ACTIVE;
                end
                ACTIVE: begin
                    if (awvalid || wvalid)
                        state <= RESPOND;
                end
                RESPOND: begin
                    bvalid_r <= 1;
                    pending  <= 1;
                    // FIX: wait for acknowledgment before returning to IDLE
                    if (bvalid_r && bready) begin
                        bvalid_r <= 0;
                        pending  <= 0;
                        state    <= IDLE;
                    end
                end
            endcase

            // bready clears bvalid independently (also handles IDLE/ACTIVE)
            if (bvalid_r && bready) begin
                bvalid_r <= 0;
                pending  <= 0;
            end
        end
    end

    assign bvalid = bvalid_r;

endmodule
