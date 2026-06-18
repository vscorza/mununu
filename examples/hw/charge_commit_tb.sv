// charge_commit_tb.sv — Verilator testbench for charge_commit.sv.
//
// Drives randomized stimulus for a bounded number of cycles and relies on the
// concurrent assertion `safety_charged` inside the DUT (active under --assert).
// A bounded simulation does not *prove* the property — it is a sanity check that
// the RTL exhibits no violation under random stimulus, complementing the
// exhaustive verdict mununu produces over the predicate abstraction. Run:
//
//   $ verilator --binary --assert --timing -Wall -Wno-DECLFILENAME \
//       charge_commit.sv charge_commit_tb.sv --top-module charge_commit_tb \
//       -o charge_commit_sim
//   $ ./obj_dir/charge_commit_sim
module charge_commit_tb;
    logic clk = 0;
    logic rst, req, pulse, fire, clr;
    logic commit_en;

    charge_commit dut (
        .clk(clk), .rst(rst), .req(req), .pulse(pulse),
        .fire(fire), .clr(clr), .commit_en(commit_en)
    );

    always #5 clk = ~clk;

    int unsigned lfsr = 32'hC0FFEE01;
    function automatic logic next_bit();
        // xorshift PRNG — deterministic, no $random dependency
        lfsr ^= lfsr << 13;
        lfsr ^= lfsr >> 17;
        lfsr ^= lfsr << 5;
        return lfsr[0];
    endfunction

    int unsigned cycles = 0;
    int unsigned commit_seen = 0;

    initial begin
        rst = 1; req = 0; pulse = 0; fire = 0; clr = 0;
        repeat (2) @(posedge clk);
        rst = 0;
        repeat (20000) begin
            @(negedge clk);
            req   = next_bit();
            pulse = next_bit() | next_bit();   // bias pulse high so charging happens
            fire  = next_bit();
            clr   = next_bit() & next_bit();   // bias clr low so COMMIT lingers
            @(posedge clk);
            cycles++;
            if (commit_en) commit_seen++;
        end
        $display("charge_commit_tb: %0d cycles, commit_en asserted in %0d, no assertion failures",
                 cycles, commit_seen);
        $finish;
    end
endmodule
