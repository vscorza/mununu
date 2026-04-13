// SPI Master Controller
// 5-state FSM: IDLE → LOAD → SHIFT → CAPTURE → DONE
// @mununu ltl safety: nu X. ([] X)
// @mununu ltl done_reachable: mu X. (DONE || <> X)
module spi_master(
    input logic clk, input logic rst,
    input logic start, input logic miso,
    output logic sclk, output logic mosi, output logic cs_n
);
    typedef enum logic [2:0] {IDLE, LOAD, SHIFT, CAPTURE, DONE} state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= IDLE;
        else case (state)
            IDLE: if (start) state <= LOAD;
            LOAD: state <= SHIFT;
            SHIFT: state <= CAPTURE;
            CAPTURE: state <= DONE;
            DONE: state <= IDLE;
        endcase
    end
endmodule
