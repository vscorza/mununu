// Synchronous FIFO Controller
// Demonstrates: mixed enum FSM + bounded counter via Kripke construction
//
// The FIFO tracks a fill level (bounded counter 0..4) and a state machine
// for the control interface. The data path (data_out_r) is excluded from
// the state space since it doesn't affect control flow.
//
// Properties verified:
//   safety: the automaton is well-formed (no deadlock)
//
// @mununu mode kripke
// @mununu domain fill: bounded_counter 0..4
// @mununu domain data_out_r: ignored
// @mununu input wr_en, rd_en
// @mununu ltl safety: nu X. ([] X)
module fifo(
    input  logic       clk,
    input  logic       rst,
    input  logic       wr_en,
    input  logic       rd_en,
    input  logic [7:0] data_in,
    output logic [7:0] data_out,
    output logic       full,
    output logic       empty
);

    typedef enum logic [1:0] {IDLE, WRITING, READING, RDWR} state_t;
    state_t state;

    logic [2:0] fill;       // 0..4 fill level (3 bits needed for 0..4)
    logic [7:0] data_out_r;

    localparam DEPTH = 4;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            state      <= IDLE;
            fill       <= 0;
            data_out_r <= 0;
        end else begin
            case (state)
                IDLE: begin
                    if (wr_en && rd_en)      state <= RDWR;
                    else if (wr_en)          state <= WRITING;
                    else if (rd_en)          state <= READING;
                end
                WRITING: begin
                    if (fill < DEPTH)
                        fill <= fill + 1;
                    state <= IDLE;
                end
                READING: begin
                    if (fill > 0) begin
                        fill <= fill - 1;
                        data_out_r <= data_in;
                    end
                    state <= IDLE;
                end
                RDWR: begin
                    if (fill == 0)
                        fill <= fill + 1;
                    state <= IDLE;
                end
            endcase
        end
    end

    assign full  = (fill == DEPTH);
    assign empty = (fill == 0);
    assign data_out = data_out_r;

endmodule
