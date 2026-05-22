//! BTOR2 (word-level bit-vector verification IR) adapter.
//!
//! Reads a BTOR2 file — produced by Yosys (`write_btor`), Pono, AVR, BtorMC,
//! or hand-authored — and translates it into CTXDSL via the shared adapter IR.
//!
//! # Phase 1 scope
//!
//! - **Supported operators** — see [`ast::Op::is_blastable`].
//! - **Bounded state space** — total state-bit width ≤ [`bit_blast::MAX_STATE_BITS`].
//!   Designs above that are rejected; the recommended path is compose-and-decompose
//!   (Phase 3) before BTOR2 hand-off to an external symbolic engine.
//! - **Properties** — `bad`, `constraint`, `fair`, `justice` are translated to
//!   safety / liveness μ-calculus formulas. `output` is informational only.
//!
//! # Out of scope (Phase 1)
//!
//! - Array sorts (`read`, `write`).
//! - Modular / signed division (`sdiv`, `udiv`, `srem`, `smod`, `urem`).
//! - Overflow detectors (`saddo`, `uaddo`, `smulo`, ...).
//! - Multi-clock designs.

pub mod ast;
pub mod bit_blast;
pub mod dep_graph;
pub mod kmts_lift;
pub mod parser;

pub use kmts_lift::{KmtsLiftOptions, KmtsLiftResult, LiftedPredicate, lift_btor2_to_kmts};

use super::{AdapterError, AdapterOptions, AdapterOutput, FormatAdapter};

/// BTOR2 adapter implementing [`FormatAdapter`].
pub struct Btor2Adapter;

impl FormatAdapter for Btor2Adapter {
    fn detect(content: &str) -> bool {
        // BTOR2 has no magic bytes; detect heuristically: at least one line
        // matching `<int> sort bitvec <int>` near the top.
        for line in content.lines().take(64) {
            let trimmed = match line.find(';') {
                Some(p) => &line[..p],
                None => line,
            }
            .trim();
            if trimmed.is_empty() {
                continue;
            }
            let toks: Vec<&str> = trimmed.split_whitespace().collect();
            if toks.len() >= 4
                && toks[0].parse::<i64>().is_ok()
                && toks[1] == "sort"
                && toks[2] == "bitvec"
            {
                return true;
            }
            // First non-empty non-comment line must be a NID; if not, give up.
            if toks[0].parse::<i64>().is_err() {
                return false;
            }
        }
        false
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        bit_blast::translate(content, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_typical_btor2() {
        let src = "; comment\n1 sort bitvec 1\n2 input 1\n";
        assert!(Btor2Adapter::detect(src));
    }

    #[test]
    fn rejects_non_btor2() {
        let src = "module foo;\nendmodule\n";
        assert!(!Btor2Adapter::detect(src));
    }

    #[test]
    fn detects_via_format_adapter_trait() {
        let src = "1 sort bitvec 1\n2 zero 1\n";
        assert!(Btor2Adapter::detect(src));
        let out = Btor2Adapter::translate(src, &AdapterOptions::default()).expect("ok");
        assert_eq!(out.source_info.state_count, 1);
    }
}
