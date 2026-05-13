//! HW/SW codesign — formal verification across the boundary.
//!
//! Implements Document C of mununu's four-document arc — see
//! [`docs/design/hw-sw-codesign-extraction.md`](../../../../docs/design/hw-sw-codesign-extraction.md).
//!
//! The conceptual model: peripheral RTL + firmware C, modelled as two
//! reactive modules (Alur & Henzinger, *Reactive Modules*, FMSD 1999)
//! that share a set of *coupled variables*. Each memory-mapped register
//! is a coupled variable; the SV side and the C side both have
//! read/write access governed by the register's direction, visibility
//! class, and access path. The coupling spec lives in a JSON
//! "register-map sidecar" that names each register, its bit-layout, and
//! how each side accesses it.
//!
//! Today only **Task C1** ships: the [`register_map`] module spells out
//! the canonical JSON schema, serde types, and the schema-validation
//! plumbing. Coupling synthesis (C2), interleaved counterexample
//! reporting (C3), the `mununu codesign verify` three-surface entry
//! point (C4), libclang-backed C extraction with `@mununu_*` C
//! wrappers (C5), and the IP-XACT / CMSIS-SVD importer (C6) are
//! follow-up slices in §C.9 of the design doc.

pub mod coupling;
pub mod register_map;
