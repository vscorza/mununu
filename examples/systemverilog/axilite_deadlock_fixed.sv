// AXI-Lite Write Channel — Overlapping Transaction Fix
//
// FIX: The slave only accepts new transactions in IDLE when bvalid is clear
// (the previous response has been acknowledged by the master).
//
// Reference: zipcpu.com/formal/2019/04/16/axi-mistakes.html

module axilite_deadlock_fixed(
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
                    // FIX: only accept when response channel is clear
                    if (!bvalid_r) begin
                        if (awvalid && wvalid)
                            state <= RESPOND;
                        else if (awvalid)
                            state <= ADDR_WAIT;
                        else if (wvalid)
                            state <= DATA_WAIT;
                    end
                end
                ADDR_WAIT: begin
                    if (wvalid) state <= RESPOND;
                end
                DATA_WAIT: begin
                    if (awvalid) state <= RESPOND;
                end
                RESPOND: begin
                    bvalid_r <= 1;
                    state <= IDLE;
                end
            endcase

            if (bvalid_r && bready)
                bvalid_r <= 0;
        end
    end

    assign awready = (state == IDLE) && !bvalid_r;
    assign wready  = (state == IDLE) && !bvalid_r;
    assign bvalid  = bvalid_r;

endmodule
