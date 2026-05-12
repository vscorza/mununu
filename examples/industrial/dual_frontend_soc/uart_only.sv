// Standalone UART module, for the custom-SV cross-check path.
// (The yosys path reads its own copy inside `soc.sv` alongside the
// DDR3 PHY blackbox declaration.)
//
// Uses `always_ff @(posedge clk or posedge rst)` syntax because that
// is what the custom-SV parser supports today.
module uart_only(
    input        clk,
    input        rst,
    input        tx_start,
    output reg   tx_done,
    output reg   tx_active
);
    always_ff @(posedge clk or posedge rst) begin
        if (rst) begin
            tx_done   <= 1'b0;
            tx_active <= 1'b0;
        end else if (tx_start) begin
            tx_done   <= 1'b0;
            tx_active <= 1'b1;
        end else if (tx_active) begin
            tx_active <= 1'b0;
            tx_done   <= 1'b1;
        end
    end
endmodule
