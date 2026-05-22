// M.0 Fix B wrapper — instantiates the upstream prim_arbiter_fixed with
// small parameters so the explicit-state BTOR2 bit-blaster's input-cap
// (MAX_INPUT_BITS = 10) admits the design.
//
// Per M-0-blocker-2026-05-21.md Fix B (user-arbitrated): the upstream
// module is used UNCHANGED at N=2 channels × DW=2-bit data. Total
// input bits = 1 (clk) + 1 (rst_ni) + 2 (req_i) + 4 (data_i) + 1
// (ready_i) = 9 bits → 2^9 = 512 enumerated input combinations, under
// the BTOR2 reader's MAX_INPUT_BITS = 10 (2^10 = 1024) cap.
//
// This wrapper is test scaffolding, NOT a translation: the upstream
// SV file [`prim_arbiter_fixed.sv`](prim_arbiter_fixed.sv) is the
// real fixture, instantiated as-is via parameter binding. The
// property the wrapper would carry — "at most one gnt_o[i] is high
// per cycle" — is not added at M.0 (verification is deferred to M.1+
// once the KMTS lifter / KleeneDomain evaluator ship).
//
// **Scale-to-N=8 commitment.** The user-arbitrated decision was to
// promote this fixture to N=8, DW=32 (production parameters) once R.2
// (BTOR2 → KMTS lifter) is on. The KMTS lifter abstracts the wide
// `data_i` port via predicate / UF wrapping; the bit-blaster's
// input-cap stops being load-bearing. M.1+ re-uses this same
// upstream `prim_arbiter_fixed.sv` with a different wrapper.

module prim_arbiter_fixed_m0_wrapper(
    input  logic       clk_i,
    input  logic       rst_ni,
    input  logic [1:0] req_i,
    input  logic [3:0] data_i,
    output logic [1:0] gnt_o,
    output logic       idx_o,
    output logic       valid_o,
    output logic [1:0] data_o,
    input  logic       ready_i
);

    prim_arbiter_fixed #(
        .N(2),
        .DW(4)
    ) u_arb (
        .clk_i  (clk_i),
        .rst_ni (rst_ni),
        .req_i  (req_i),
        .data_i (data_i),
        .gnt_o  (gnt_o),
        .idx_o  (idx_o),
        .valid_o(valid_o),
        .data_o (data_o),
        .ready_i(ready_i)
    );

endmodule
