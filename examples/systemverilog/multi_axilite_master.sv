// AXI-Lite Write Master
//
// Simple master that issues write requests and accepts responses.
// The master has an internal pending counter: it can issue a new write
// while a previous response is still outstanding, and may delay
// asserting bready (backpressure on the response channel).
//
// States:
//   IDLE     — no pending write, ready to issue
//   ISSUED   — write request sent (awvalid+wvalid), waiting for response
//   BACKPRESSURE — response available (bvalid high) but master not ready
//
// The master's bready behavior is input-driven: an external signal
// `ready_delay` controls whether the master asserts bready immediately
// or applies one cycle of backpressure.

module axilite_master(
    input  logic clk,
    input  logic rst,
    input  logic start,         // external trigger to issue a write
    input  logic ready_delay,   // when high, master delays bready by one cycle
    input  logic bvalid,        // response valid from slave
    output logic awvalid,
    output logic wvalid,
    output logic bready
);

    typedef enum logic [1:0] {IDLE, ISSUED, BACKPRESSURE} state_t;
    state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
        end else begin
            case (state)
                IDLE: begin
                    if (start) state <= ISSUED;
                end
                ISSUED: begin
                    if (bvalid) begin
                        if (ready_delay)
                            state <= BACKPRESSURE;
                        else
                            state <= IDLE;
                    end
                end
                BACKPRESSURE: begin
                    // One cycle of backpressure, then accept
                    state <= IDLE;
                end
            endcase
        end
    end

    assign awvalid = (state == IDLE) && start;
    assign wvalid  = (state == IDLE) && start;
    assign bready  = (state == ISSUED && !ready_delay) || (state == BACKPRESSURE);

endmodule
