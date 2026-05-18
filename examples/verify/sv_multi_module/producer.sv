// Producer module: generates a valid signal based on FSM state.
// When enabled, cycles through IDLE -> PRODUCING -> DONE -> IDLE.
module producer(
    input logic clk, input logic rst,
    input logic enable,
    output logic valid
);
    typedef enum logic [1:0] {IDLE, PRODUCING, DONE} state_t;
    state_t state;

    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: if (enable) state <= PRODUCING;
            PRODUCING: state <= DONE;
            DONE: state <= IDLE;
        endcase
    end

    assign valid = (state == PRODUCING);
endmodule
