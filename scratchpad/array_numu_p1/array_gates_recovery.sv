// array_gates_recovery.sv — P1-a clean single-array-wall synthetic.
//
// Recovery of `busy` is GATED ON ARRAY CONTENT: busy returns to idle only when
// mem[key] is all-ones. There is NO register-dominated datapath and NO wide
// counter — the ONLY hard axis is the in-cone array. This isolates shot ① so the
// three-shot composition can be validated on one wall at a time.
//
// AG EF(busy==0) HOLDS: from any busy state the free write port can set
// mem[key] := all-ones, so idle is always reachable. But the recovery ROUTES
// THROUGH array content, so:
//   - exact-symbolic ROBDD: SKIPs (in-cone $mem is not bit-blastable; mununu keeps $mem).
//   - symbolic predicate-cube KMTS (Z3 QF_AUFBV): ⊥ — with no array-content
//     predicate every array valuation merges into one cube cell, so it cannot
//     tell "mem[key] can become all-ones" from "stuck".
// The SMALL instance is decidable by exact-symbolic ROBDD AFTER registerizing the
// tiny array (= the ground-truth ORACLE); the LARGE instance blows the bit cap
// under registerization, so the symbolic-cube + array-content-predicate / prophecy
// composition is the only path — which is exactly what P1-a measures.

module agr #(parameter AW = 2, parameter DW = 2) (
    input           clk,
    input           rst_n,
    input           start,
    input  [AW-1:0] waddr,
    input  [DW-1:0] wdata,
    output reg      busy
);
    reg [DW-1:0] mem [0:(1<<AW)-1];
    reg [AW-1:0] key;

    // Free write port — the environment can write any cell any cycle.
    always @(posedge clk)
        mem[waddr] <= wdata;

    // Control: busy rises on start, recovers ONLY when the keyed cell is all-ones.
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            busy <= 1'b0;
            key  <= {AW{1'b0}};
        end else if (start && !busy) begin
            busy <= 1'b1;
            key  <= waddr;            // latch the cell that will gate recovery
        end else if (busy) begin
            if (mem[key] == {DW{1'b1}})   // content-gated recovery
                busy <= 1'b0;
        end
    end
endmodule

// SMALL instance (4 entries x 2 bits = 8 array bits) — the ROBDD oracle target.
// @mununu_guarantee nu Y.((mu X.(busy==0 || <> X)) && [] Y)
// @mununu_guarantee mu Z.(busy==1 || <> Z)
module array_gates_recovery_small (
    input clk, input rst_n, input start,
    input [1:0] waddr, input [1:0] wdata, output busy
);
    agr #(.AW(2), .DW(2)) u (
        .clk(clk), .rst_n(rst_n), .start(start),
        .waddr(waddr), .wdata(wdata), .busy(busy));
endmodule

// LARGE instance (256 entries x 8 bits = 2048 array bits) — defeats registerization.
// @mununu_guarantee nu Y.((mu X.(busy==0 || <> X)) && [] Y)
// @mununu_guarantee mu Z.(busy==1 || <> Z)
module array_gates_recovery_large (
    input clk, input rst_n, input start,
    input [7:0] waddr, input [7:0] wdata, output busy
);
    agr #(.AW(8), .DW(8)) u (
        .clk(clk), .rst_n(rst_n), .start(start),
        .waddr(waddr), .wdata(wdata), .busy(busy));
endmodule
