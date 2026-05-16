//! Register-map → SV-side renaming derivation.
//!
//! When the verify framework's [`AlphabetBinding::RegisterMap`]
//! strategy pairs a firmware source with a `sv-rtl` peripheral source,
//! the firmware-side automaton (synthesised by `c-codesign`) emits
//! rendezvous labels via [`crate::codesign::coupling::rendezvous_label_name`]
//! (`wr_<reg>_<field>` / `rd_<reg>_<field>`), while the SV adapter
//! emits its own canonical `<signal>_<value>` labels (see
//! `adapter::systemverilog::kripke::make_input_labels`). Without
//! reconciliation the two sides talk past each other.
//!
//! This module derives a per-field renaming map keyed by the SV-side
//! label pattern and pointing at the firmware-side rendezvous name.
//! The orchestrator applies the map via
//! [`crate::verify::binding::apply_renamings_to_ctxdsl`] after the SV
//! source's CTXDSL emission.
//!
//! ## Scope and limitations
//!
//! Today the rewriter handles only the **single-bit field** case where
//! `Field.sv_signal` is set and the SV adapter exposes that bit as its
//! own signal (i.e. the `sv_signal` value strips to a basename matching
//! the SV adapter's signal name). For each such field we emit two
//! renamings:
//!
//! - `<basename>_0` → rendezvous name (the "de-asserted" SV transition)
//! - `<basename>_1` → rendezvous name (the "asserted" SV transition)
//!
//! Both SV transitions collapse onto the same firmware-side
//! rendezvous because the firmware writes a *single event* per
//! `wr_<reg>_<field>`. This is the common pattern for one-shot
//! control bits and status flags; multi-bit fields, packed
//! `register[bit]` indexing, and protocol-specific behavior (e.g.,
//! "rising edge only counts") are queued as a follow-up — for those
//! the user can fall back to explicit `renamings = [...]` entries.
//!
//! ## Soundness
//!
//! The collapse `signal_0 ∪ signal_1 → wr_<reg>_<field>` is an
//! **over-approximation** when only one SV transition should
//! synchronize with the firmware write. Both SV-side directions
//! become "the firmware wrote this register" from the composition's
//! perspective, so safety verdicts remain sound (any real violation
//! reachable through either SV transition is still observed); liveness
//! verdicts may be optimistic (the SV side can fire the rendezvous on
//! a transition the firmware can't actually drive). Users who need
//! tighter coupling should switch to explicit `renamings = [...]`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::codesign::coupling::{AccessKind, rendezvous_label_name};
use crate::codesign::register_map::{Field, Register, RegisterDirection, RegisterMap};

/// Strip the hierarchical path from a `Field.sv_signal` value.
///
/// `"uart_inst.ctrl_reg[0]"` → `"ctrl_reg[0]"` (the trailing bit
/// index is preserved so the caller can decide whether to skip it).
/// `"uart_inst.tx_busy"` → `"tx_busy"`. A bare signal with no `.` is
/// returned verbatim.
fn sv_signal_basename(sv_signal: &str) -> &str {
    match sv_signal.rsplit_once('.') {
        Some((_, tail)) => tail,
        None => sv_signal,
    }
}

/// Whether the SV-signal basename looks like a plain identifier
/// (no bit-indexing, no path separators) the SV adapter would emit
/// as one of its `<signal>_<value>` labels.
fn is_simple_signal_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Emit the access kinds firmware drives on this register direction.
fn access_kinds_for(direction: RegisterDirection) -> &'static [AccessKind] {
    match direction {
        RegisterDirection::Rw => &[AccessKind::Write, AccessKind::Read],
        RegisterDirection::Wo => &[AccessKind::Write],
        RegisterDirection::Ro => &[AccessKind::Read],
    }
}

/// Derive `<sv_label> → <rendezvous_label>` renamings from a parsed
/// register map.
///
/// Returns a sorted-key map suitable for
/// [`crate::verify::binding::apply_renamings_to_ctxdsl`]. Empty when
/// no field carries an `sv_signal` value or no SV bindings resolve to
/// simple-identifier basenames.
///
/// **Scope**: single-bit fields only. Wider fields and bit-indexed
/// `sv_signal` values (e.g. `"ctrl_reg[0]"`) are skipped — the user
/// can layer explicit `renamings = [...]` entries on top for those.
pub fn derive_sv_renamings_from_register_map(map: &RegisterMap) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for register in &map.registers {
        let kinds = access_kinds_for(register.direction);
        if register.fields.is_empty() {
            // Whole-register access — only meaningful when the
            // register itself maps to one SV signal. We skip these
            // for now (no `sv_signal` on the register; lives on the
            // field).
            continue;
        }
        for field in &register.fields {
            insert_renamings_for_field(register, field, kinds, &mut out);
        }
    }
    out
}

fn insert_renamings_for_field(
    register: &Register,
    field: &Field,
    kinds: &[AccessKind],
    out: &mut BTreeMap<String, String>,
) {
    if !field.has_sv_binding() {
        return;
    }
    if field.width_bits() != 1 {
        // Multi-bit fields skipped — the SV adapter's value-suffix
        // shape doesn't map 1:1 onto a single rendezvous event for
        // these.
        return;
    }
    let raw = field
        .sv_signal
        .as_deref()
        .expect("has_sv_binding ⇒ sv_signal is Some");
    let basename = sv_signal_basename(raw);
    if !is_simple_signal_name(basename) {
        // Bit-indexed (`ctrl_reg[0]`) or otherwise non-identifier-shaped
        // — skip; the user falls back to explicit renamings.
        return;
    }
    for &kind in kinds {
        let rendezvous = rendezvous_label_name(&register.name, Some(&field.name), kind);
        // Both `_0` and `_1` SV transitions collapse onto the same
        // rendezvous event — see module-level soundness note.
        for value in ["0", "1"] {
            let sv_label = format!("{basename}_{value}");
            // The first inserted renaming wins. For an RW field this
            // means the write-direction rendezvous is preferred over
            // the read-direction; that matches the "firmware writes
            // RW fields" convention from Doc A §4.
            out.entry(sv_label).or_insert_with(|| rendezvous.clone());
        }
    }
}

/// Convenience: derive + persist a `<base_dir>/<filename>.renamings`
/// log of the rewriting decisions. Returned for telemetry / future
/// surfacing in the `VerifyReport`. Pure function — no side effects;
/// the orchestrator decides whether to write to disk.
pub fn derive_with_report(
    map: &RegisterMap,
    _base_dir: &Path,
) -> (BTreeMap<String, String>, Vec<String>) {
    let renamings = derive_sv_renamings_from_register_map(map);
    let mut log: Vec<String> = Vec::new();
    log.push(format!(
        "register-map rewriter: {} SV-side renamings derived from {} register(s)",
        renamings.len(),
        map.registers.len(),
    ));
    for (k, v) in &renamings {
        log.push(format!("  {k} -> {v}"));
    }
    (renamings, log)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codesign::register_map::{
        AccessPath, Field, Register, RegisterDirection, RegisterMap, VisibilityClass,
    };

    fn rm_single_control_field() -> RegisterMap {
        RegisterMap {
            peripheral: "UART".to_string(),
            base_address: "0x40000000".to_string(),
            description: None,
            contract_uri: None,
            registers: vec![Register {
                name: "ctrl".to_string(),
                offset: 0x00,
                width_bits: 32,
                direction: RegisterDirection::Wo,
                visibility_class: VisibilityClass::Control,
                access_path: AccessPath::default(),
                description: None,
                fields: vec![Field {
                    name: "tx_start".to_string(),
                    bits: [0, 0],
                    sv_signal: Some("dut.tx_start".to_string()),
                    c_accessor: Some("UART->CTRL.bit.tx_start".to_string()),
                    description: None,
                }],
            }],
        }
    }

    #[test]
    fn write_only_field_emits_only_write_renamings() {
        let rm = rm_single_control_field();
        let renamings = derive_sv_renamings_from_register_map(&rm);
        assert_eq!(renamings.len(), 2);
        assert_eq!(
            renamings.get("tx_start_0").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
        assert_eq!(
            renamings.get("tx_start_1").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
    }

    #[test]
    fn rw_field_emits_write_then_read_renamings_with_write_winning() {
        let mut rm = rm_single_control_field();
        rm.registers[0].direction = RegisterDirection::Rw;
        let renamings = derive_sv_renamings_from_register_map(&rm);
        // 2 SV-side labels × 1 field; the first inserted (write) wins
        // for both _0 and _1 per `or_insert_with` semantics.
        assert_eq!(renamings.len(), 2);
        assert_eq!(
            renamings.get("tx_start_0").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
        assert_eq!(
            renamings.get("tx_start_1").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
    }

    #[test]
    fn read_only_field_emits_read_renamings() {
        let mut rm = rm_single_control_field();
        rm.registers[0].direction = RegisterDirection::Ro;
        rm.registers[0].name = "status".to_string();
        rm.registers[0].fields[0].name = "tx_busy".to_string();
        rm.registers[0].fields[0].sv_signal = Some("dut.tx_busy".to_string());
        let renamings = derive_sv_renamings_from_register_map(&rm);
        assert_eq!(renamings.len(), 2);
        assert_eq!(
            renamings.get("tx_busy_0").map(String::as_str),
            Some("rd_status_tx_busy")
        );
        assert_eq!(
            renamings.get("tx_busy_1").map(String::as_str),
            Some("rd_status_tx_busy")
        );
    }

    #[test]
    fn bit_indexed_sv_signal_is_skipped() {
        let mut rm = rm_single_control_field();
        rm.registers[0].fields[0].sv_signal = Some("dut.ctrl_reg[0]".to_string());
        let renamings = derive_sv_renamings_from_register_map(&rm);
        // Bit-indexed sv_signal → not a simple identifier after
        // basename stripping → skipped.
        assert!(renamings.is_empty(), "got renamings: {renamings:?}");
    }

    #[test]
    fn multi_bit_field_is_skipped() {
        let mut rm = rm_single_control_field();
        rm.registers[0].fields[0].bits = [0, 7];
        let renamings = derive_sv_renamings_from_register_map(&rm);
        assert!(renamings.is_empty(), "got renamings: {renamings:?}");
    }

    #[test]
    fn missing_sv_binding_is_skipped() {
        let mut rm = rm_single_control_field();
        rm.registers[0].fields[0].sv_signal = None;
        let renamings = derive_sv_renamings_from_register_map(&rm);
        assert!(renamings.is_empty(), "got renamings: {renamings:?}");
    }

    #[test]
    fn bare_signal_name_with_no_dot_works() {
        let mut rm = rm_single_control_field();
        rm.registers[0].fields[0].sv_signal = Some("tx_start".to_string());
        let renamings = derive_sv_renamings_from_register_map(&rm);
        assert_eq!(renamings.len(), 2);
        assert_eq!(
            renamings.get("tx_start_1").map(String::as_str),
            Some("wr_ctrl_tx_start")
        );
    }

    #[test]
    fn empty_register_map_produces_no_renamings() {
        let rm = RegisterMap {
            peripheral: "Empty".to_string(),
            base_address: "0x0".to_string(),
            description: None,
            contract_uri: None,
            registers: Vec::new(),
        };
        assert!(derive_sv_renamings_from_register_map(&rm).is_empty());
    }

    #[test]
    fn report_log_includes_each_renaming() {
        let rm = rm_single_control_field();
        let (_renamings, log) = derive_with_report(&rm, Path::new("."));
        assert!(log[0].contains("2 SV-side renamings"));
        // 1 header line + 2 renaming lines.
        assert_eq!(log.len(), 3);
        assert!(
            log.iter()
                .any(|l| l.contains("tx_start_0 -> wr_ctrl_tx_start"))
        );
        assert!(
            log.iter()
                .any(|l| l.contains("tx_start_1 -> wr_ctrl_tx_start"))
        );
    }
}
