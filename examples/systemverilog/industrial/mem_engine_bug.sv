// Memory Engine — Buggy (OpenPiton-inspired)
//
// Based on the OpenPiton Mem Engine bug found by AutoSVA (DAC 2021).
// The Mem Engine reuses a NoC buffer originally designed for the L1.5 cache.
// The L1.5 cache naturally limited its request rate (never exceeding buffer
// capacity), but the Mem Engine can burst requests without regard to buffer
// occupancy.
//
// BUG: The engine issues requests (push) based solely on its internal state,
// without checking the buffer's credit_avail signal. When the buffer is full,
// pushes cause overflow (data corruption/loss).
//
// This module represents a simplified request scheduler that can issue
// back-to-back requests when processing a multi-beat operation.

module mem_engine_bug(
    input  logic        clk,
    input  logic        rst,
    input  logic        start,         // trigger a multi-beat operation
    input  logic        credit_avail,  // buffer has space (IGNORED in buggy version)
    output logic        push           // push request to buffer
);

    typedef enum logic [2:0] {
        IDLE,       // waiting for work
        BEAT_1,     // issuing first beat
        BEAT_2,     // issuing second beat
        BEAT_3,     // issuing third beat
        DONE        // operation complete
    } engine_state_t;

    engine_state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
        end else begin
            case (state)
                IDLE: begin
                    if (start) state <= BEAT_1;
                end
                BEAT_1: begin
                    // BUG: advances unconditionally without checking credit_avail
                    state <= BEAT_2;
                end
                BEAT_2: begin
                    state <= BEAT_3;
                end
                BEAT_3: begin
                    state <= DONE;
                end
                DONE: begin
                    state <= IDLE;
                end
            endcase
        end
    end

    // BUG: push is asserted in all BEAT states regardless of credit_avail
    assign push = (state == BEAT_1) || (state == BEAT_2) || (state == BEAT_3);

endmodule
