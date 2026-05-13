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
//! What ships today:
//!
//! - **C1 — register-map sidecar schema** in [`register_map`]. JSON
//!   schema + serde types + structural validation.
//! - **C2 — coupling synthesis (slice 1)** in [`coupling`]. Emits the
//!   CTXDSL fragment (alphabet + chaotic peripheral stub + async
//!   composition) the user splices into a hand-authored context.
//! - **C3 — interleaved trace origin classifier (slice 1)** in
//!   [`trace`]. Tags trace steps as `[SW]` / `[HW]` / `[BUS]` based on
//!   a caller-supplied label partition.
//!
//! Follow-up slices in §C.9 of the design doc: C4 (`mununu codesign
//! verify` three-surface entry point that consumes the C3 classifier),
//! C5 (libclang C extraction + `@mununu_*` C wrappers), C6 (IP-XACT /
//! CMSIS-SVD importer).

pub mod coupling;
pub mod register_map;
pub mod trace;
