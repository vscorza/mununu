// Memory Engine — Fixed
//
// The fix: check credit_avail before advancing to the next beat.
// If the buffer is full (credit_avail low), stall in the current beat
// state until space is available.
//
// This is the AutoSVA-recommended fix: gate the push/advance with
// a "credit available" condition from the buffer.

module mem_engine_fixed(
    input  logic        clk,
    input  logic        rst,
    input  logic        start,
    input  logic        credit_avail,  // buffer has space (CHECKED in fixed version)
    output logic        push
);

    typedef enum logic [2:0] {
        IDLE,
        BEAT_1,
        BEAT_2,
        BEAT_3,
        DONE
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
                    // FIX: only advance if buffer has space
                    if (credit_avail) state <= BEAT_2;
                end
                BEAT_2: begin
                    if (credit_avail) state <= BEAT_3;
                end
                BEAT_3: begin
                    if (credit_avail) state <= DONE;
                end
                DONE: begin
                    state <= IDLE;
                end
            endcase
        end
    end

    // FIX: push only when in a BEAT state AND buffer has credit
    assign push = ((state == BEAT_1) || (state == BEAT_2) || (state == BEAT_3)) && credit_avail;

endmodule
