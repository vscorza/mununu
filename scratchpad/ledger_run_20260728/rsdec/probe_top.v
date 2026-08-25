// P1.step1 probe wrapper: give RS_dec a `bad` property so pono can run.
// bad = Valid_out reachable (a reachability sub-question of the recovery).
module RS_dec_probe (
    input        clk,
    input        reset,
    input        CE,
    input  [7:0] input_byte
);
    wire [7:0] Out_byte;
    wire       CEO;
    wire       Valid_out;

    RS_dec dut (
        .clk(clk),
        .reset(reset),
        .CE(CE),
        .input_byte(input_byte),
        .Out_byte(Out_byte),
        .CEO(CEO),
        .Valid_out(Valid_out)
    );

    // bad = Valid_out can be 1 (the active/output-valid state is reachable)
    always @(posedge clk)
        assert (!Valid_out);
endmodule
