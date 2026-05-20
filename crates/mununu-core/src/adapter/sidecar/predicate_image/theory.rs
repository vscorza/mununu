//! SMT theory selector for predicate-image discovery.
//!
//! Each variant declares both the **logic** Z3 should use to encode
//! the transition relation and the **soundness contract** the caller
//! is signing up for.

/// SMT theory used to encode the design's transition relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theory {
    /// `QF_BV` — pure bit-vector logic. Suitable for the BTOR2 and
    /// SystemVerilog adapters: every storage element is a bit-vector,
    /// every operator is bit-precise.
    ///
    /// **Soundness contract.** The BTOR2 / SV → SMT encoder must be
    /// faithful to the bit-vector semantics of the source. The Phase
    /// A.4 encoder in [`super::btor2_encode`] is exact for the Phase 1
    /// supported operator set; operators outside that set cause the
    /// encoder to refuse the design (hard error, not silent
    /// approximation).
    BvOnly,
    /// `QF_BV + QF_UF + arrays` — bit-vectors plus uninterpreted
    /// functions plus extensional arrays. Required by the C-extraction
    /// path: pointer dereferences encode as UF symbols (sound
    /// over-approximation of any concrete address resolution); memory
    /// regions encode as arrays indexed by address.
    ///
    /// **Soundness contract.** UF over-approximates pointer aliasing
    /// (every pointer read may return any prior write to a UF-aliased
    /// location). This preserves safety under universal-modality
    /// (`[]` / `nu`) verdicts but admits more behaviours than the
    /// concrete program. The
    /// [`phase-a3-followup-indirect-references.md`](../../../../../../.claude/plans/phase-a3-followup-indirect-references.md)
    /// follow-up plan still owns the dep-graph alias-tracking half;
    /// `BvUfArray` here is only the SMT-theory layer beneath it.
    BvUfArray,
}

impl Theory {
    /// Z3 logic identifier for [`z3::Config::set_logic`]. Returned as
    /// a `&'static str` so callers can plug it straight into the
    /// solver configuration without allocation.
    pub fn z3_logic(&self) -> &'static str {
        match self {
            Theory::BvOnly => "QF_BV",
            Theory::BvUfArray => "QF_AUFBV",
        }
    }

    /// `true` when the theory permits encoding pointer-aliased writes
    /// (and other indirect references) via UF / array primitives.
    /// Used by the extraction adapter's gate at step 4.5.
    pub fn supports_indirect_references(&self) -> bool {
        matches!(self, Theory::BvUfArray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z3_logic_returns_canonical_strings() {
        assert_eq!(Theory::BvOnly.z3_logic(), "QF_BV");
        assert_eq!(Theory::BvUfArray.z3_logic(), "QF_AUFBV");
    }

    #[test]
    fn indirect_references_gate() {
        assert!(!Theory::BvOnly.supports_indirect_references());
        assert!(Theory::BvUfArray.supports_indirect_references());
    }
}
