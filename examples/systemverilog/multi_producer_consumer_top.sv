// Producer-Consumer Top Module
//
// Wires a producer and consumer through a bounded buffer.
// The producer pushes data when it produces (valid signal),
// and the consumer pops data when it acknowledges (ack wire).
//
// Property of interest: the buffer never overflows (count < 3).
//
// Usage (CLI):
//   mununu sv init --multi multi_producer_consumer_top.sv
//   mununu context eval <sidecar>.mununu.json \
//     --adapter sv --formula no_overflow --automaton system
//
// Usage (UI — Extraction tab):
//   1. Load this file as primary source (SystemVerilog RTL domain)
//   2. Click "+ Add Files" and add: multi_producer.sv, multi_consumer.sv, multi_buffer.sv
//   3. Run "Initialize Sidecar" (generates multi-module .mununu.json)
//   4. Edit sidecar: add the no_overflow property (see guide)
//   5. Click "Continue to Translate"
//   6. Click "Run Translate" to generate CTXDSL
//   7. Switch to Verification tab to check properties

module producer_consumer_top(
    input logic clk,
    input logic rst,
    input logic enable  // environment controls when producer starts
);

    // Internal wires
    logic valid;     // producer -> consumer (data available)
    logic push_sig;  // producer -> buffer (push request)
    logic pop_sig;   // consumer -> buffer (pop request)
    logic full;      // buffer -> (observable output)

    // Producer: IDLE -> PRODUCING -> DONE -> IDLE
    // Outputs valid=1 when PRODUCING
    producer u_producer(
        .clk(clk),
        .rst(rst),
        .enable(enable),
        .valid(valid)
    );

    // Buffer: 2-entry bounded buffer
    // push increments count, pop decrements, full when count >= 2
    bounded_buffer u_buffer(
        .clk(clk),
        .rst(rst),
        .push(valid),
        .pop(pop_sig),
        .full(full)
    );

    // Consumer: IDLE -> BUSY -> ACK -> IDLE
    // Reacts when valid is asserted
    consumer u_consumer(
        .clk(clk),
        .rst(rst),
        .valid(valid)
    );

endmodule
