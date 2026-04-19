// FIFO with overflow fix
// FIX: write is guarded by `fill < DEPTH`, preventing overflow.
//
// Expected: property `no_overflow` should PASS (realizable).

module fifo_overflow_fixed(
    input  logic       clk,
    input  logic       rst,
    input  logic       wr_en,
    input  logic       rd_en,
    output logic       full,
    output logic       empty
);

    typedef enum logic [1:0] {IDLE, WRITING, READING} state_t;
    state_t state;

    logic [2:0] fill;
    localparam DEPTH = 4;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state <= IDLE;
            fill  <= 0;
        end else begin
            case (state)
                IDLE: begin
                    if (wr_en)      state <= WRITING;
                    else if (rd_en) state <= READING;
                end
                WRITING: begin
                    // FIX: guard prevents write when full
                    if (fill < DEPTH)
                        fill <= fill + 1;
                    state <= IDLE;
                end
                READING: begin
                    if (fill > 0)
                        fill <= fill - 1;
                    state <= IDLE;
                end
            endcase
        end
    end

    assign full  = (fill == DEPTH);
    assign empty = (fill == 0);

endmodule
