// Minimal hand-written stub of OpenTitan's `csrng_pkg` for M.2.
//
// The upstream `csrng_pkg.sv` pulls in `csrng_reg_pkg::NumApps` and
// `entropy_src_pkg::FIPS_BUS_WIDTH` (a transitive package chain
// running into prim_secded / lc_ctrl / lifecycle types we don't
// need for the M.2 control-FSM property). This stub defines ONLY
// the two enums `csrng_main_sm` actually consumes:
//   - `acmd_e`         — the 3-bit application-command opcode.
//   - `main_sm_state_e` — the 6-bit sparse FSM state encoding.
//
// Values match the upstream definitions verbatim (verified against
// `hw/ip/csrng/rtl/csrng_pkg.sv` at the same UPSTREAM_COMMIT pin).
// The stub is shipped in this repo (not vendored) because the
// upstream package's transitive dependencies would force vendoring
// of ~10 additional packages whose contents are irrelevant to the
// FSM property the M.2 milestone verifies.
//
// SOUNDNESS: identical enum values + identical type widths ⇒ the
// stub is observationally equivalent to the upstream package for
// every BTOR2 line the FSM emits. The dropped parameters
// (BlkLen, KeyLen, SeedLen, etc.) are never referenced by
// `csrng_main_sm.sv`.

package csrng_pkg;

  typedef enum logic [2:0] {
    INS = 3'h1,
    RES = 3'h2,
    GEN = 3'h3,
    UPD = 3'h4,
    UNI = 3'h5
  } acmd_e;

  parameter int unsigned MainSmStateWidth = 6;
  typedef enum logic [MainSmStateWidth-1:0] {
    MainSmIdle        = 6'b110111,
    MainSmParseCmd    = 6'b011101,
    MainSmEntropyReq  = 6'b001110,
    MainSmCmdPrep     = 6'b000011,
    MainSmCmdVld      = 6'b010000,
    MainSmClrAData    = 6'b111010,
    MainSmCmdCompWait = 6'b100100,
    MainSmError       = 6'b101001
  } main_sm_state_e;

endpackage
