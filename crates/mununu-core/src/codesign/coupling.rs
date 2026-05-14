//! Coupling synthesis — Document C task C2, slice 1.
//!
//! Reads a [`RegisterMap`](super::register_map::RegisterMap) and emits
//! the **CTXDSL connecting tissue** that lets a user verify
//! cross-boundary HW/SW properties today, with hand-authored firmware
//! and a hand-authored peripheral RTL stub.
//!
//! ## Slice 1 scope
//!
//! Per Document C §C.7's "ship the connecting tissue first"
//! recommendation, slice 1 deliberately does *not* parse the user's
//! firmware CTXDSL or merge two `ContextDoc`s into one. Instead it
//! emits **CTXDSL text fragments** the user splices into a single
//! `context { … }` block they author by hand:
//!
//! 1. A canonical **list of rendezvous labels** ([`RendezvousLabel`])
//!    derived from the register map. One label per register-field
//!    bit-read / -write, plus one whole-register read label per
//!    register. Controllability is classified per Document A §4: a
//!    *read* of a peripheral output is `Uncontrollable` (the
//!    peripheral drives it), a *write* by firmware is `Controllable`
//!    (firmware drives it), etc. Direction-derived per register.
//! 2. A **peripheral chaotic-stub automaton** emitted as a CTXDSL
//!    `automaton { … }` block. States: `Idle` + one transient state
//!    per register access. Transitions on the rendezvous labels.
//! 3. A **coupling fragment** that bundles the alphabet declarations,
//!    the peripheral stub, and a synchronous `composition { … }` block
//!    linking the peripheral stub with one or more firmware automaton
//!    names the user supplies.
//!
//! ## What slice 1 leaves for later
//!
//! - **Slice 2 (post-PR):** ingest the firmware `ContextDoc` directly
//!   and produce a fully merged `ContextDoc` (no string splicing). The
//!   `parity-check` skill applies here for the CLI / HTTP / UI surface
//!   coming in §C.9.4.
//! - **Slice 3 (Task C5):** auto-generate the firmware automaton from
//!   C source via libclang.
//! - **Slice 4 (Task C6):** import the register map from IP-XACT /
//!   CMSIS-SVD.

use crate::clts::LabelControllability;
use crate::codesign::register_map::{
    AccessPath, Field, Register, RegisterDirection, RegisterMap, VisibilityClass,
};
use std::fmt::Write;

/// One rendezvous label both sides of the coupling synchronise on.
///
/// Slice 1 derives these by walking the register map:
///   - For each field on a register that firmware can *read*, one
///     `rd_<reg>_<field>` label classified by the register's direction
///     (firmware reading a peripheral-driven STATUS field → label is
///     `Uncontrollable` from firmware's perspective; the peripheral
///     drove it).
///   - For each field on a register that firmware can *write*, one
///     `wr_<reg>_<field>` label, similarly classified.
///   - One whole-register read label (`rd_<reg>`) when no fields are
///     declared (data-payload registers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendezvousLabel {
    /// Canonical label name (sanitised — only ASCII alnum + `_`).
    pub name: String,
    /// Which kind of access this label represents.
    pub kind: AccessKind,
    /// Register the label belongs to.
    pub register: String,
    /// Field the label belongs to. `None` for whole-register
    /// access (data-payload registers without per-bit fields).
    pub field: Option<String>,
    /// Controllability from firmware's perspective:
    ///   - Firmware *writing* a register → `Controllable`
    ///   - Firmware *reading* a register the peripheral drives →
    ///     `Uncontrollable`
    ///   - Firmware *reading* a register firmware itself last wrote
    ///     (RW direction) → `Controllable` (firmware drove the value)
    pub controllability: LabelControllability,
}

/// Kind of register access a `RendezvousLabel` represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessKind {
    /// Firmware reads (peripheral may have written; firmware observes).
    Read,
    /// Firmware writes (peripheral observes the write).
    Write,
}

impl AccessKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessKind::Read => "rd",
            AccessKind::Write => "wr",
        }
    }
}

/// Generate the canonical list of rendezvous labels for a register map.
///
/// Order: registers in declaration order, then within each register:
/// per-field writes (if firmware can write), per-field reads (if
/// firmware can read), then the whole-register read for data registers
/// with no fields.
pub fn register_map_labels(rm: &RegisterMap) -> Vec<RendezvousLabel> {
    let mut out = Vec::new();
    for register in &rm.registers {
        let can_write = matches!(
            register.direction,
            RegisterDirection::Rw | RegisterDirection::Wo
        );
        let can_read = matches!(
            register.direction,
            RegisterDirection::Rw | RegisterDirection::Ro
        );

        if register.fields.is_empty() {
            // Data-payload register with no declared fields. Emit a
            // whole-register access label per allowed direction.
            if can_write {
                out.push(register_label(register, None, AccessKind::Write));
            }
            if can_read {
                out.push(register_label(register, None, AccessKind::Read));
            }
            continue;
        }

        for field in &register.fields {
            if can_write {
                out.push(register_label(register, Some(field), AccessKind::Write));
            }
        }
        for field in &register.fields {
            if can_read {
                out.push(register_label(register, Some(field), AccessKind::Read));
            }
        }
    }
    out
}

fn register_label(register: &Register, field: Option<&Field>, kind: AccessKind) -> RendezvousLabel {
    let name = rendezvous_label_name(&register.name, field.map(|f| f.name.as_str()), kind);
    RendezvousLabel {
        name,
        kind,
        register: register.name.clone(),
        field: field.map(|f| f.name.clone()),
        controllability: classify_access(register, kind),
    }
}

/// Canonical rendezvous-label name for a register / field / access
/// kind, matching the convention used by [`register_map_labels`].
///
/// Exposed so [`crate::codesign::c_extract`] (slice 2.b) can emit
/// firmware-side transitions that synchronise with the peripheral
/// stub on the exact same label spelling.
pub fn rendezvous_label_name(register: &str, field: Option<&str>, kind: AccessKind) -> String {
    match field {
        Some(f) => format!(
            "{kind}_{reg}_{field}",
            kind = kind.as_str(),
            reg = sanitise_ident(register),
            field = sanitise_ident(f)
        ),
        None => format!(
            "{kind}_{reg}",
            kind = kind.as_str(),
            reg = sanitise_ident(register)
        ),
    }
}

/// Document A §4 controllability rule applied at the register
/// boundary.
///
/// - **Firmware writes** are *always* `Controllable` from firmware's
///   perspective: the firmware drives the value.
/// - **Firmware reads** depend on who *produced* the value being read:
///   - On a `WO` (write-only) register, reads don't apply.
///   - On an `RO` register, the peripheral wrote it →
///     `Uncontrollable`.
///   - On a `RW` register, firmware wrote it last in the typical
///     flow → `Controllable`, but with the soundness caveat that
///     peripherals may also drive RW registers (e.g. status bits in
///     a control register). The conservative choice is
///     `Uncontrollable` for safety verdicts; we pick that here.
fn classify_access(register: &Register, kind: AccessKind) -> LabelControllability {
    match kind {
        AccessKind::Write => LabelControllability::Controllable,
        AccessKind::Read => match register.direction {
            // A WO register has no reads — this branch isn't reached
            // because `register_map_labels` doesn't emit a read label
            // for WO; the match is total for completeness.
            RegisterDirection::Wo => LabelControllability::Internal,
            RegisterDirection::Ro => LabelControllability::Uncontrollable,
            // Conservative: a RW status bit might be peripheral-driven.
            // Treat as Uncontrollable for soundness.
            RegisterDirection::Rw => LabelControllability::Uncontrollable,
        },
    }
}

/// Sanitise a register / field name into a CTXDSL identifier:
/// ASCII alnum + `_` only, anything else collapses to `_`.
fn sanitise_ident(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Options for [`emit_coupling_fragment`].
#[derive(Debug, Clone, Default)]
pub struct CouplingOptions<'a> {
    /// The CTXDSL identifier for the peripheral automaton. Default
    /// derives from the peripheral name in the register map.
    pub peripheral_automaton: Option<&'a str>,
    /// Composition name. Default `<Peripheral>System`.
    pub composition_name: Option<&'a str>,
    /// Names of firmware automata that should be composed with the
    /// peripheral. The user is responsible for ensuring those
    /// automata are defined elsewhere in the same `context { … }`
    /// block and that they use the rendezvous label names this
    /// module produces.
    pub firmware_members: &'a [&'a str],
}

/// Emit the **peripheral chaotic-stub** automaton as a CTXDSL
/// `automaton { … }` block.
///
/// The emitted automaton has:
///   - One `Idle` initial state.
///   - One transient `Busy_<reg>` state per register, entered on a
///     firmware-initiated access and left on the corresponding
///     peripheral response (read with output) or immediately for
///     writes.
///   - One self-loop on `Idle` for every read of a peripheral-driven
///     register (the peripheral may signal at any time).
pub fn emit_peripheral_stub_ctxdsl(rm: &RegisterMap, options: &CouplingOptions<'_>) -> String {
    let automaton_name = options
        .peripheral_automaton
        .map(str::to_string)
        .unwrap_or_else(|| sanitise_ident(&rm.peripheral).to_ascii_uppercase());

    let labels = register_map_labels(rm);
    let mut buf = String::new();
    // CTXDSL groups automata under an `automata { … }` section. The
    // parser merges multiple such sections, so the user's firmware
    // automaton can live in a separate `automata { … }` block they
    // author themselves.
    let _ = writeln!(buf, "    automata {{");
    let _ = writeln!(buf, "        automaton {automaton_name} {{");

    // Controllable / internal sets must list every label that is
    // not the default (Uncontrollable in CTXDSL).
    // The peripheral stub MUST NOT claim any rendezvous label as
    // `controllable`. Firmware drives the `wr_*` labels (the firmware
    // automaton declares them in its own `controllable { … }` block);
    // the peripheral merely observes/responds. `rd_*` labels are
    // chaotic outputs neither side controls. Declaring write labels
    // as controllable here would conflict with the firmware's
    // declaration at realise time. The label classifications on
    // `RendezvousLabel` describe the labels from *firmware's*
    // perspective and are surfaced for downstream consumers (e.g.
    // the trace classifier in `codesign::trace`); they do not
    // determine what the peripheral stub itself declares.
    //
    // We deliberately suppress unused-variable lints rather than
    // dropping the call to `labels`, because `labels` documents
    // intent and is consumed by the per-register loops below.
    let _labels = &labels;

    let _ = writeln!(buf, "        states {{");
    let _ = writeln!(buf, "            state Idle initial;");
    for reg in &rm.registers {
        let _ = writeln!(buf, "            state Busy_{};", sanitise_ident(&reg.name));
    }
    let _ = writeln!(buf, "        }}");
    let _ = writeln!(buf);

    let _ = writeln!(buf, "        transitions {{");
    for reg in &rm.registers {
        let busy = format!("Busy_{}", sanitise_ident(&reg.name));
        let can_write = matches!(reg.direction, RegisterDirection::Rw | RegisterDirection::Wo);
        let can_read = matches!(reg.direction, RegisterDirection::Rw | RegisterDirection::Ro);

        // Writes — firmware drives Idle → Busy_<reg> and the peripheral
        // returns to Idle immediately (no value to return).
        if can_write {
            let write_labels: Vec<_> = labels
                .iter()
                .filter(|l| l.kind == AccessKind::Write && l.register == reg.name)
                .collect();
            for l in &write_labels {
                let _ = writeln!(
                    buf,
                    "            transition Idle -> {busy} on label {};",
                    l.name
                );
                let _ = writeln!(
                    buf,
                    "            transition {busy} -> Idle on label {};",
                    l.name
                );
            }
        }

        // Reads — peripheral signals (Uncontrollable) when firmware
        // reads. Self-loop on Idle expresses "peripheral may update
        // the value at any time"; transient Busy_<reg> covers a
        // multi-cycle response if the firmware models one.
        if can_read {
            let read_labels: Vec<_> = labels
                .iter()
                .filter(|l| l.kind == AccessKind::Read && l.register == reg.name)
                .collect();
            for l in &read_labels {
                let _ = writeln!(
                    buf,
                    "            transition Idle -> Idle on label {};",
                    l.name
                );
                let _ = writeln!(
                    buf,
                    "            transition Idle -> {busy} on label {};",
                    l.name
                );
                let _ = writeln!(
                    buf,
                    "            transition {busy} -> Idle on label {};",
                    l.name
                );
            }
        }

        // Annotate with the register's metadata so a reader can see
        // why the transitions are shaped this way.
        let _ = writeln!(
            buf,
            "            // {} ({}, {})",
            reg.name,
            reg.direction,
            visibility_label(reg.visibility_class)
        );
        // Access-path annotation is informational; reflected in
        // diagnostics but does not change the stub shape.
        if !matches!(reg.access_path, AccessPath::MmioDirect) {
            let _ = writeln!(buf, "            // access_path: {:?}", reg.access_path);
        }
    }
    let _ = writeln!(buf, "        }}");
    // Close the `automaton { … }` body.
    let _ = writeln!(buf, "        }}");
    // Close the wrapping `automata { … }` section.
    let _ = writeln!(buf, "    }}");
    buf
}

fn visibility_label(v: VisibilityClass) -> &'static str {
    match v {
        VisibilityClass::Control => "control",
        VisibilityClass::Status => "status",
        VisibilityClass::Data => "data",
        VisibilityClass::InterruptFlag => "interrupt_flag",
        VisibilityClass::ClearOnRead => "clear_on_read",
        VisibilityClass::Other => "other",
    }
}

/// Emit the **full coupling CTXDSL fragment**: alphabet declarations,
/// the peripheral chaotic-stub automaton, and an asynchronous
/// composition block tying the peripheral to the named firmware
/// members.
///
/// The output is a string the user pastes inside a hand-authored
/// `context <name> { … }` block, alongside the firmware automaton(s)
/// they have authored separately. The user is responsible for using
/// the rendezvous label names this module produces in their firmware
/// transitions.
///
/// Per Document C §C.5, the composition uses
/// [`CompositionKind::Asynchronous`](crate::context_dsl::ast::CompositionKind::Asynchronous)
/// — bus arbitration is a real source of non-determinism, and
/// modelling firmware ↔ peripheral as synchronous (one-step rendezvous)
/// is unsound for properties about racy access.
pub fn emit_coupling_fragment(rm: &RegisterMap, options: &CouplingOptions<'_>) -> String {
    let composition_name = options
        .composition_name
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "{}System",
                sanitise_ident(&rm.peripheral).to_ascii_uppercase()
            )
        });
    let peripheral_automaton = options
        .peripheral_automaton
        .map(str::to_string)
        .unwrap_or_else(|| sanitise_ident(&rm.peripheral).to_ascii_uppercase());

    let labels = register_map_labels(rm);
    let mut buf = String::new();

    // ---------------- header banner --------------------------------
    let _ = writeln!(
        buf,
        "    // ----------------------------------------------------------"
    );
    let _ = writeln!(
        buf,
        "    // Coupling fragment emitted by mununu codesign couple"
    );
    let _ = writeln!(buf, "    // Peripheral: {}", rm.peripheral);
    let _ = writeln!(buf, "    // Base address: {}", rm.base_address);
    if let Some(uri) = &rm.contract_uri {
        let _ = writeln!(buf, "    // Contract URI: {uri}");
    }
    let _ = writeln!(
        buf,
        "    // Coupling is asynchronous — Document C §C.5 soundness rule."
    );
    let _ = writeln!(
        buf,
        "    // ----------------------------------------------------------"
    );
    let _ = writeln!(buf);

    // ---------------- alphabet block -------------------------------
    let _ = writeln!(buf, "    alphabet {{");
    for l in &labels {
        let location = match &l.field {
            Some(f) => format!("{}.{}", l.register, f),
            None => l.register.clone(),
        };
        let _ = writeln!(
            buf,
            "        label {};   // {} {} — {:?}",
            l.name,
            l.kind.as_str(),
            location,
            l.controllability
        );
    }
    let _ = writeln!(buf, "    }}");
    let _ = writeln!(buf);

    // ---------------- peripheral stub ------------------------------
    buf.push_str(&emit_peripheral_stub_ctxdsl(rm, options));
    let _ = writeln!(buf);

    // ---------------- composition block ----------------------------
    let _ = writeln!(buf, "    composition {{");
    let _ = writeln!(buf, "        asynchronous {composition_name} {{");
    let mut members = vec![peripheral_automaton.as_str()];
    members.extend(options.firmware_members.iter().copied());
    let _ = writeln!(buf, "            members [{}];", members.join(", "));
    let _ = writeln!(buf, "        }}");
    let _ = writeln!(buf, "    }}");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codesign::register_map::{Field as RmField, Register as RmRegister};

    fn uart_map() -> RegisterMap {
        RegisterMap {
            peripheral: "UART_LITE".to_string(),
            base_address: "0x40010000".to_string(),
            description: None,
            contract_uri: None,
            registers: vec![
                RmRegister {
                    name: "CTRL".to_string(),
                    offset: 0,
                    width_bits: 32,
                    direction: RegisterDirection::Rw,
                    visibility_class: VisibilityClass::Control,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![
                        RmField {
                            name: "tx_start".to_string(),
                            bits: [0, 0],
                            sv_signal: None,
                            c_accessor: None,
                            description: None,
                        },
                        RmField {
                            name: "enable".to_string(),
                            bits: [1, 1],
                            sv_signal: None,
                            c_accessor: None,
                            description: None,
                        },
                    ],
                },
                RmRegister {
                    name: "STATUS".to_string(),
                    offset: 4,
                    width_bits: 32,
                    direction: RegisterDirection::Ro,
                    visibility_class: VisibilityClass::Status,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![RmField {
                        name: "tx_busy".to_string(),
                        bits: [0, 0],
                        sv_signal: None,
                        c_accessor: None,
                        description: None,
                    }],
                },
                RmRegister {
                    name: "DATA".to_string(),
                    offset: 8,
                    width_bits: 32,
                    direction: RegisterDirection::Rw,
                    visibility_class: VisibilityClass::Data,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![],
                },
            ],
        }
    }

    #[test]
    fn labels_per_field_for_rw_register() {
        let m = uart_map();
        let labels = register_map_labels(&m);
        let names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
        // CTRL is RW with two fields → 2 writes + 2 reads = 4 labels.
        assert!(names.contains(&"wr_ctrl_tx_start"));
        assert!(names.contains(&"wr_ctrl_enable"));
        assert!(names.contains(&"rd_ctrl_tx_start"));
        assert!(names.contains(&"rd_ctrl_enable"));
    }

    #[test]
    fn labels_for_ro_register_are_read_only() {
        let m = uart_map();
        let labels = register_map_labels(&m);
        // STATUS is RO → 1 read for tx_busy, no writes.
        assert!(
            labels
                .iter()
                .any(|l| l.name == "rd_status_tx_busy" && l.kind == AccessKind::Read)
        );
        assert!(!labels.iter().any(|l| l.name == "wr_status_tx_busy"));
    }

    #[test]
    fn data_register_without_fields_gets_whole_register_labels() {
        let m = uart_map();
        let labels = register_map_labels(&m);
        // DATA has no declared fields → whole-register `rd_data` + `wr_data`.
        let names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
        assert!(names.contains(&"rd_data"));
        assert!(names.contains(&"wr_data"));
    }

    #[test]
    fn write_labels_are_controllable() {
        let m = uart_map();
        let labels = register_map_labels(&m);
        for l in &labels {
            if l.kind == AccessKind::Write {
                assert_eq!(
                    l.controllability,
                    LabelControllability::Controllable,
                    "write label `{}` should be Controllable",
                    l.name
                );
            }
        }
    }

    #[test]
    fn read_of_ro_register_is_uncontrollable() {
        let m = uart_map();
        let labels = register_map_labels(&m);
        let rd_status = labels
            .iter()
            .find(|l| l.name == "rd_status_tx_busy")
            .expect("rd_status_tx_busy exists");
        assert_eq!(
            rd_status.controllability,
            LabelControllability::Uncontrollable
        );
    }

    #[test]
    fn read_of_rw_register_is_conservatively_uncontrollable() {
        // CTRL.tx_start is RW; peripherals may drive RW status bits,
        // so reads are conservatively Uncontrollable per Doc C §C.5.
        let m = uart_map();
        let labels = register_map_labels(&m);
        let rd_ctrl = labels
            .iter()
            .find(|l| l.name == "rd_ctrl_tx_start")
            .expect("rd_ctrl_tx_start exists");
        assert_eq!(
            rd_ctrl.controllability,
            LabelControllability::Uncontrollable
        );
    }

    #[test]
    fn label_names_are_lowercase_safe_identifiers() {
        let mut m = uart_map();
        // Introduce a register name with non-identifier characters.
        m.registers[0].name = "Ctrl-Status".to_string();
        let labels = register_map_labels(&m);
        for l in &labels {
            for c in l.name.chars() {
                assert!(
                    c.is_ascii_alphanumeric() || c == '_',
                    "label `{}` contains non-identifier char {:?}",
                    l.name,
                    c
                );
            }
        }
        // Specifically: "Ctrl-Status" → "ctrl_status".
        assert!(labels.iter().any(|l| l.name.starts_with("wr_ctrl_status_")));
    }

    #[test]
    fn peripheral_stub_emits_idle_initial_state() {
        let m = uart_map();
        let stub = emit_peripheral_stub_ctxdsl(&m, &CouplingOptions::default());
        assert!(stub.contains("state Idle initial;"));
    }

    #[test]
    fn peripheral_stub_emits_one_busy_state_per_register() {
        let m = uart_map();
        let stub = emit_peripheral_stub_ctxdsl(&m, &CouplingOptions::default());
        assert!(stub.contains("state Busy_ctrl;"));
        assert!(stub.contains("state Busy_status;"));
        assert!(stub.contains("state Busy_data;"));
    }

    #[test]
    fn peripheral_stub_does_not_emit_a_controllable_block() {
        // The peripheral stub MUST NOT claim any rendezvous label as
        // `controllable`. Firmware drives writes (the firmware
        // automaton declares them); reads are chaotic outputs neither
        // side controls. Claiming writes here would conflict with the
        // firmware's declaration at realise time, which the CTXDSL
        // realiser rejects with a "duplicate controllable label"
        // error. The label classifications on `RendezvousLabel` are
        // surfaced for downstream consumers (e.g. the trace
        // classifier) but do not affect the peripheral stub's own
        // declarations.
        let m = uart_map();
        let stub = emit_peripheral_stub_ctxdsl(&m, &CouplingOptions::default());
        assert!(
            !stub.contains("controllable {"),
            "peripheral stub must not emit a `controllable {{ … }}` block; \
             firmware owns the wr_* labels"
        );
    }

    #[test]
    fn peripheral_stub_has_a_self_loop_on_idle_for_each_read_label() {
        let m = uart_map();
        let stub = emit_peripheral_stub_ctxdsl(&m, &CouplingOptions::default());
        for l in register_map_labels(&m)
            .iter()
            .filter(|l| l.kind == AccessKind::Read)
        {
            assert!(
                stub.contains(&format!("transition Idle -> Idle on label {};", l.name)),
                "missing self-loop on Idle for read label `{}`",
                l.name
            );
        }
    }

    #[test]
    fn coupling_fragment_includes_alphabet_block() {
        let m = uart_map();
        let frag = emit_coupling_fragment(&m, &CouplingOptions::default());
        assert!(frag.contains("alphabet {"));
        // Every generated label name must appear in the alphabet.
        for l in register_map_labels(&m) {
            assert!(
                frag.contains(&format!("label {};", l.name)),
                "fragment missing alphabet declaration for `{}`",
                l.name
            );
        }
    }

    #[test]
    fn coupling_fragment_uses_asynchronous_composition() {
        let m = uart_map();
        let frag = emit_coupling_fragment(
            &m,
            &CouplingOptions {
                firmware_members: &["UartDriver"],
                ..CouplingOptions::default()
            },
        );
        assert!(frag.contains("asynchronous "));
        assert!(frag.contains("members [UART_LITE, UartDriver];"));
    }

    #[test]
    fn coupling_fragment_uses_default_names_from_peripheral() {
        let m = uart_map();
        let frag = emit_coupling_fragment(&m, &CouplingOptions::default());
        // Composition default: <PERIPHERAL>System.
        assert!(frag.contains("UART_LITESystem"));
        // Peripheral automaton default: uppercased peripheral name.
        assert!(frag.contains("automaton UART_LITE {"));
    }

    #[test]
    fn coupling_fragment_respects_user_supplied_names() {
        let m = uart_map();
        let frag = emit_coupling_fragment(
            &m,
            &CouplingOptions {
                peripheral_automaton: Some("UartIp"),
                composition_name: Some("UartTopLevel"),
                firmware_members: &["Driver"],
            },
        );
        assert!(frag.contains("automaton UartIp {"));
        assert!(frag.contains("asynchronous UartTopLevel"));
        assert!(frag.contains("members [UartIp, Driver];"));
    }

    #[test]
    fn fragment_contains_soundness_comment_about_async() {
        let m = uart_map();
        let frag = emit_coupling_fragment(&m, &CouplingOptions::default());
        assert!(frag.contains("asynchronous"));
        // The header banner should remind the reader that the
        // composition is intentionally async per §C.5.
        assert!(frag.contains("Document C §C.5"));
    }

    #[test]
    fn fragment_plus_minimal_firmware_parses_through_ctxdsl_parser() {
        // End-to-end soundness check: the emitted fragment must be
        // valid CTXDSL when spliced into a `context { … }` block
        // alongside a hand-authored firmware automaton. If this
        // breaks, the fragment shape has drifted from CTXDSL syntax.
        use crate::context_dsl::parser::parse;
        let m = uart_map();
        let frag = emit_coupling_fragment(
            &m,
            &CouplingOptions {
                firmware_members: &["UartDriver"],
                ..CouplingOptions::default()
            },
        );

        // Tiny firmware automaton that exercises every rendezvous
        // label kind: a write (wr_ctrl_tx_start), a read of a
        // peripheral output (rd_status_tx_busy), and a data write
        // (wr_data_byte). Wrapped in its own `automata { … }`
        // section — the CTXDSL parser merges multiple such sections
        // in one context.
        let firmware = r#"
            automata {
                automaton UartDriver {
                    controllable {
                        label wr_ctrl_tx_start;
                        label wr_data_byte;
                    }

                    states {
                        state Init initial;
                        state Polling;
                        state Sending;
                    }

                    transitions {
                        transition Init -> Polling on label rd_status_tx_busy;
                        transition Polling -> Sending on label wr_data_byte;
                        transition Sending -> Init on label wr_ctrl_tx_start;
                    }
                }
            }
        "#;

        let source = format!("context CoupledUart {{\n{frag}\n{firmware}\n}}\n");
        let doc = parse(&source).expect("emitted CTXDSL fragment must parse");
        // Sanity checks on the parsed AST.
        assert_eq!(doc.name.name, "CoupledUart");
        assert!(
            doc.automata.iter().any(|a| a.name.name == "UART_LITE"),
            "peripheral automaton must be present in parsed AST"
        );
        assert!(
            doc.automata.iter().any(|a| a.name.name == "UartDriver"),
            "firmware automaton must be present in parsed AST"
        );
        assert_eq!(doc.compositions.len(), 1);
        let comp = &doc.compositions[0];
        assert!(matches!(
            comp.kind,
            crate::context_dsl::ast::CompositionKind::Asynchronous
        ));
        assert_eq!(comp.members.len(), 2);
    }
}
