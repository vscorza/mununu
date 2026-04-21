// NoC System Top Module
//
// Instantiates the memory engine and NoC buffer, connecting them
// via push and credit_avail wires.

module noc_system_top(
    input  logic clk,
    input  logic rst,
    input  logic start,
    input  logic pop
);

    wire push_wire;
    wire credit_wire;

    mem_engine_bug engine_inst(
        .clk(clk),
        .rst(rst),
        .start(start),
        .credit_avail(credit_wire),
        .push(push_wire)
    );

    noc_buffer #(.DEPTH(4)) buffer_inst(
        .clk(clk),
        .rst(rst),
        .push(push_wire),
        .pop(pop),
        .credit_avail(credit_wire),
        .full(),
        .empty()
    );

endmodule
