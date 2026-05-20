// SVA-macro stubs. Yosys (Phase 1) does not support concurrent SVA;
// the macros expand to no-ops so the surrounding module parses.
`define CALIPTRA_ASSERT_KNOWN(ID, SIG, CLK, RST_B)
`define CALIPTRA_ASSERT_NEVER(ID, EXPR, CLK, RST_B)
`define CALIPTRA_ASSERT(ID, EXPR, CLK, RST_B)
`define CALIPTRA_ASSERT_INIT(ID, EXPR)
`define CALIPTRA_ASSUME(ID, EXPR, CLK, RST_B)
`define CALIPTRA_COVER(ID, EXPR, CLK, RST_B)
