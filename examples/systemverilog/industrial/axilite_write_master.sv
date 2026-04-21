// AXI-Lite Write Master — Realistic Pattern
//
// Implements a minimal AXI-Lite write master that can:
// 1. Issue write transactions (awvalid + wvalid)
// 2. Accept write responses (bvalid/bready handshake)
// 3. Apply backpressure on the response channel (bready held low)
//
// The master can issue a new write while a previous response is outstanding.
// This is legal per AXI spec (pipelined writes) and is the scenario that
// triggers the Xilinx slave bug.

module axilite_write_master(
    input  logic        clk,
    input  logic        rst,
    // Control inputs (from testbench/environment)
    input  logic        cmd_write,     // trigger a write transaction
    input  logic        cmd_slow_resp, // hold bready low for one extra cycle
    // Write address channel
    output logic        m_axi_awvalid,
    input  logic        m_axi_awready,
    // Write data channel
    output logic        m_axi_wvalid,
    input  logic        m_axi_wready,
    // Write response channel
    input  logic        m_axi_bvalid,
    output logic        m_axi_bready
);

    // Master state machine
    typedef enum logic [2:0] {
        M_IDLE,         // No pending write
        M_ADDR_DATA,    // Asserting awvalid + wvalid, waiting for ready
        M_WAIT_RESP,    // Write accepted, waiting for bvalid
        M_RESP_DELAY,   // bvalid received but holding bready low (backpressure)
        M_RESP_ACK      // Acknowledging response (bready high)
    } master_state_t;

    master_state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= M_IDLE;
        end else begin
            case (state)
                M_IDLE: begin
                    if (cmd_write)
                        state <= M_ADDR_DATA;
                end
                M_ADDR_DATA: begin
                    // Wait for both awready and wready
                    if (m_axi_awready && m_axi_wready)
                        state <= M_WAIT_RESP;
                end
                M_WAIT_RESP: begin
                    if (m_axi_bvalid) begin
                        if (cmd_slow_resp)
                            state <= M_RESP_DELAY; // Apply backpressure
                        else
                            state <= M_RESP_ACK;   // Immediate accept
                    end
                end
                M_RESP_DELAY: begin
                    // One cycle of backpressure, then acknowledge
                    state <= M_RESP_ACK;
                end
                M_RESP_ACK: begin
                    state <= M_IDLE;
                end
            endcase
        end
    end

    // Output assignments
    assign m_axi_awvalid = (state == M_ADDR_DATA);
    assign m_axi_wvalid  = (state == M_ADDR_DATA);
    assign m_axi_bready  = (state == M_RESP_ACK);

endmodule
