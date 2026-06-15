//! SystemVerilog adapter surface.
//!
//! After the S.2b native-parser excision, this module no longer hosts a
//! SystemVerilog *verification* frontend — the sole SV verification path
//! is the KMTS route (`sv-yosys`: sv2v → Yosys → BTOR2 → bit-blast; see
//! [`crate::adapter::yosys`] / [`crate::adapter::btor2`]). What remains
//! here is pipeline-agnostic SV tooling consumed by the KMTS path and the
//! controller emitter:
//!
//! - [`annotation`] — the `.mununu.json` sidecar schema (`SvAnnotation`,
//!   signal / init / memory abstractions) the BTOR2 bit-blaster and the
//!   sidecar resolver consume.
//! - [`typedef_extract`] / [`case_literal_extract`] — standalone
//!   SV-source scanners for extraction-time predicate seeding (R-S5 /
//!   R-S3); they do not parse the full design, only lift typedef-enum and
//!   `case`-literal constants.
//! - [`emit_controller`] — synthesised-controller → SystemVerilog emitter
//!   (`--output-format systemverilog`), driven from a `ControllerSpec`,
//!   independent of any SV *input* parser.

pub mod annotation;
pub mod case_literal_extract;
pub mod emit_controller;
pub mod typedef_extract;
