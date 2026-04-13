// Traffic Light Controller
// 4-state timer-driven FSM with sensor input
// Cross-validates with examples/hw/traffic_light.ctxdsl
// @mununu ltl safety: nu X. ([] X)
module traffic_light(
    input logic clk, input logic rst,
    input logic sensor
);
    typedef enum logic [1:0] {GREEN, YELLOW, RED, RED_WAIT} state_t;
    state_t state;
    always_ff @(posedge clk or posedge rst) begin
        if (rst) state <= GREEN;
        else case (state)
            GREEN: state <= YELLOW;
            YELLOW: state <= RED;
            RED: state <= RED_WAIT;
            RED_WAIT: if (sensor) state <= GREEN;
        endcase
    end
endmodule
