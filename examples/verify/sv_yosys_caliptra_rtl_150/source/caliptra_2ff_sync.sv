// NOT vendored — minimal stub of Caliptra's caliptra_2ff_sync two-flop
// synchroniser, sufficient for boot-FSM extraction. Models the two-stage
// register delay with async active-low reset to RST_VAL. The exact
// metastability-hardening behaviour is irrelevant to the boot-FSM
// state-reachability property; a registered two-stage delay is a sound
// stand-in.
module caliptra_2ff_sync #(
    parameter int unsigned     WIDTH   = 1,
    parameter logic [WIDTH-1:0] RST_VAL = '0
) (
    input  logic             clk,
    input  logic             rst_b,
    input  logic [WIDTH-1:0] din,
    output logic [WIDTH-1:0] dout
);
    logic [WIDTH-1:0] ff1, ff2;
    always_ff @(posedge clk or negedge rst_b) begin
        if (!rst_b) begin
            ff1 <= RST_VAL;
            ff2 <= RST_VAL;
        end else begin
            ff1 <= din;
            ff2 <= ff1;
        end
    end
    assign dout = ff2;
endmodule
