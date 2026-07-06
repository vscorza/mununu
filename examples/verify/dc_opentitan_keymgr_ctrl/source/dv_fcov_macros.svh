// Minimal synthesis stub for OpenTitan/ibex `dv_fcov_macros.svh` (functional-coverage macros).
//
// mununu verifies the design's FUNCTIONAL behaviour, not its DV functional coverage. The real
// header defines covergroup/coverpoint helpers that a synthesis flow (sv2v + Yosys) cannot parse;
// this stub expands the only macro ibex_controller uses, `DV_FCOV_SIGNAL`, to the signal
// declaration + continuous assignment ALONE — no coverage attributes. That is functionally
// equivalent for the model (Yosys prunes the coverage-only signal if it is never read, and
// preserves it correctly if it is), so it changes no verification verdict. Mirrors the role of the
// vendored `prim_assert.sv` synthesis shim. Each macro is `ifndef`-guarded so a real header wins.
`ifndef DV_FCOV_MACROS_SVH
`define DV_FCOV_MACROS_SVH

`ifndef DV_FCOV_SIGNAL
  `define DV_FCOV_SIGNAL(TYPE_T, NAME, SEL) TYPE_T NAME; assign NAME = (SEL);
`endif

`ifndef DV_FCOV_SIGNAL_GEN_IF
  `define DV_FCOV_SIGNAL_GEN_IF(TYPE_T, NAME, SEL, COND) TYPE_T NAME; assign NAME = (COND) ? (SEL) : '0;
`endif

`ifndef DV_FCOV_EXPR_SEEN
  `define DV_FCOV_EXPR_SEEN(NAME, SEL)
`endif

`endif // DV_FCOV_MACROS_SVH
