// producer_consumer_system_top.sv — KMTS top-module form of the
// sv_multi_module fixture.
//
// Migrated from the native `mununu_sv_multi_v1` sidecar form (S.2b,
// native-parser deletion): the native multi-module path composed the
// standalone `producer.sv` + `consumer.sv` via the sidecar's
// `connections` list (producer.valid -> consumer.valid). The KMTS
// `sv-yosys` multi-module path instead composes a TOP module that
// structurally instantiates the submodules; instance connectivity is
// discovered from the Yosys netlist. This wrapper makes the same
// `valid` net explicit as a structural wire.
module producer_consumer_system_top(
    input logic clk,
    input logic rst,
    input logic enable   // environment trigger for the producer
);

    logic valid;  // producer -> consumer (the shared handshake net)

    producer u_producer(
        .clk(clk),
        .rst(rst),
        .enable(enable),
        .valid(valid)
    );

    consumer u_consumer(
        .clk(clk),
        .rst(rst),
        .valid(valid)
    );

endmodule
