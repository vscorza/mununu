// Declaration-only copy of the DDR3 PHY blackbox interface, for the
// Document B § B.8 side-by-side cross-check.
//
// The yosys pipeline reads the same module from `soc.sv` (which has
// `(* blackbox *)` on this declaration plus the rest of the SoC).
// The custom-SV pipeline reads *this* file alone, because its
// multi-module sidecar points each module to its own source file.
//
// Both pipelines extract the port list and emit a BlackBoxInterface
// sidecar. The validate.sh cross-check confirms the port structures
// agree (the source_file metadata naturally differs).
module ddr3_phy_v2(
    input  clk,
    input  reset_n,
    input  host_init_burst,
    input  addr,
    input  wdata,
    output rdata,
    output ddr_ready,
    output ddr_busy
);
endmodule
