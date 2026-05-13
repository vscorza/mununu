//! Register-map sidecar — Document C task C1.
//!
//! A small JSON file describing a memory-mapped peripheral's register
//! layout, with enough information to drive *both* sides of the
//! HW/SW codesign coupling:
//!
//! - SV side: each field knows the signal path inside the peripheral
//!   RTL it maps onto (e.g. `uart_inst.ctrl_reg[0]`).
//! - C side: each field knows the C accessor expression a firmware
//!   engineer would write (e.g. `UART->CTRL.bit.tx_start`).
//!
//! Coupling synthesis (Task C2) reads this sidecar plus the two
//! extracted automata and produces a unified `AdapterIR` where each
//! register access is a rendezvous label both sides synchronise on.
//!
//! ## File layout
//!
//! Per Doc D §D.3 (post-M3 scope-down), the recommended location is
//! `.mununu/coupling/register_maps/<peripheral>.json`. The exact path
//! is not enforced — the [`RegisterMap`] type is JSON-serialisable and
//! can be loaded from anywhere via `serde_json`.
//!
//! ## Soundness posture
//!
//! The schema is **descriptive**, not prescriptive: the sidecar
//! describes what *is*, not what mununu *enforces*. Properties about
//! the coupling (e.g. "writes to a RO register are illegal") are
//! authored separately as assume/guarantee clauses against the
//! resulting `AdapterIR`, not inferred from the sidecar. This keeps
//! the schema free of implicit semantics that would surprise a reader.
//!
//! ## Why no IP-XACT / CMSIS-SVD adoption today?
//!
//! Doc C §C.8 names IP-XACT (IEEE 1685-2022), SystemRDL, and
//! CMSIS-SVD as the established candidates. The schema here is
//! intentionally a *superset* of the register subset those formats
//! share — same register/field/bit-range vocabulary, plus the
//! mununu-specific `sv_signal` + `c_accessor` + `access_path` fields
//! the codesign coupling needs. A one-pass importer (Task C6) can
//! ingest any of those formats and emit a [`RegisterMap`].

use serde::{Deserialize, Serialize};
use std::fmt;

/// Top-level register-map sidecar.
///
/// Names a single peripheral, its base address, and every register
/// firmware sees through that base address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterMap {
    /// Logical name of the peripheral, e.g. `"UART_LITE"`.
    pub peripheral: String,
    /// Base address as a string so it round-trips hex literals
    /// exactly. Parse via [`RegisterMap::base_address_value`] when an
    /// integer is needed.
    pub base_address: String,
    /// Ordered list of register definitions. Order is preserved on
    /// disk for human readability; lookups go through
    /// [`RegisterMap::register`].
    pub registers: Vec<Register>,
    /// Optional human-readable description of the peripheral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional reference to a corpus contract describing this
    /// peripheral's external interface — e.g.
    /// `"contract://rtl_protocol/axi4_lite_slave@1.0.0"`. The codesign
    /// pipeline forwards this URI to the contract subsystem at
    /// extraction time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_uri: Option<String>,
}

impl RegisterMap {
    /// Look up a register by its name. Linear scan — register maps
    /// are small (single-digit to low-double-digit register counts).
    pub fn register(&self, name: &str) -> Option<&Register> {
        self.registers.iter().find(|r| r.name == name)
    }

    /// Parse [`Self::base_address`] as a `u64`. Accepts `"0xNNNN"`,
    /// `"0b…"`, or plain decimal. Returns `None` on parse failure.
    pub fn base_address_value(&self) -> Option<u64> {
        parse_address(&self.base_address)
    }

    /// Absolute address of a register's first byte, when both the
    /// peripheral base and the register offset are parseable.
    pub fn register_absolute_address(&self, name: &str) -> Option<u64> {
        let base = self.base_address_value()?;
        let reg = self.register(name)?;
        Some(base.wrapping_add(reg.offset))
    }

    /// Validate the structural invariants of a freshly-parsed
    /// register map. Returns the list of issues (empty for a
    /// well-formed map).
    ///
    /// Checked invariants:
    ///   - The base address is parseable.
    ///   - Register offsets fit in the addressable space implied by
    ///     `base_address`'s parsed width (informational — we just
    ///     check the offset itself parses sensibly).
    ///   - No two registers share a name.
    ///   - No two fields *within the same register* share a name.
    ///   - Each field's bit-range fits inside the register width.
    ///   - Field bit-ranges do not overlap within a register.
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        if self.base_address_value().is_none() {
            issues.push(ValidationIssue::UnparseableBaseAddress {
                peripheral: self.peripheral.clone(),
                value: self.base_address.clone(),
            });
        }
        let mut seen = std::collections::HashSet::new();
        for register in &self.registers {
            if !seen.insert(register.name.as_str()) {
                issues.push(ValidationIssue::DuplicateRegisterName {
                    peripheral: self.peripheral.clone(),
                    name: register.name.clone(),
                });
            }
            issues.extend(register.validate(&self.peripheral));
        }
        issues
    }
}

/// A single memory-mapped register.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Register {
    /// Register name as it appears in the datasheet, e.g. `"CTRL"`.
    pub name: String,
    /// Byte offset from the peripheral base.
    pub offset: u64,
    /// Register width in bits — typically 8, 16, 32, or 64.
    pub width_bits: u32,
    /// Direction relative to the firmware's perspective.
    pub direction: RegisterDirection,
    /// Visibility class — captures the standard concurrency
    /// semantics of common register patterns. Defaults to
    /// [`VisibilityClass::Other`] for registers that don't fit a
    /// standard class.
    #[serde(default)]
    pub visibility_class: VisibilityClass,
    /// How firmware reaches the register. Defaults to
    /// [`AccessPath::MmioDirect`] — the common case.
    #[serde(default)]
    pub access_path: AccessPath,
    /// Ordered list of fields inside the register. May be empty for
    /// data-only registers (e.g. UART_DATA where the whole register
    /// is one byte of payload).
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Register {
    /// Look up a field by its name within this register.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    fn validate(&self, peripheral: &str) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for field in &self.fields {
            if !seen.insert(field.name.as_str()) {
                issues.push(ValidationIssue::DuplicateFieldName {
                    peripheral: peripheral.to_string(),
                    register: self.name.clone(),
                    field: field.name.clone(),
                });
            }
            if field.bits[1] >= self.width_bits {
                issues.push(ValidationIssue::FieldOutOfRange {
                    peripheral: peripheral.to_string(),
                    register: self.name.clone(),
                    field: field.name.clone(),
                    high: field.bits[1],
                    width: self.width_bits,
                });
            }
            if field.bits[0] > field.bits[1] {
                issues.push(ValidationIssue::InvertedBitRange {
                    peripheral: peripheral.to_string(),
                    register: self.name.clone(),
                    field: field.name.clone(),
                    bits: field.bits,
                });
            }
        }
        // Overlap detection — O(n²) over the (small) field list.
        for (i, a) in self.fields.iter().enumerate() {
            for b in self.fields.iter().skip(i + 1) {
                if a.bits[0] <= b.bits[1] && b.bits[0] <= a.bits[1] {
                    issues.push(ValidationIssue::FieldOverlap {
                        peripheral: peripheral.to_string(),
                        register: self.name.clone(),
                        a: a.name.clone(),
                        b: b.name.clone(),
                    });
                }
            }
        }
        issues
    }
}

/// A single bit-field inside a [`Register`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// Field name, e.g. `"tx_start"`.
    pub name: String,
    /// Bit range as `[low, high]` (inclusive). The pair shape
    /// matches the convention used in IP-XACT / SystemRDL / CMSIS-SVD.
    /// `[0, 0]` is a single bit at position 0.
    pub bits: [u32; 2],
    /// Optional path to the SV signal this field maps onto, e.g.
    /// `"uart_inst.ctrl_reg[0]"`. Empty when the SV mapping has not
    /// been authored yet (Task C6 leaves these blank when importing
    /// from CMSIS-SVD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sv_signal: Option<String>,
    /// Optional C accessor expression a firmware engineer would write,
    /// e.g. `"UART->CTRL.bit.tx_start"`. Empty until the C mapping has
    /// been authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_accessor: Option<String>,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Field {
    /// Width of the field in bits.
    pub fn width_bits(&self) -> u32 {
        self.bits[1].saturating_sub(self.bits[0]).saturating_add(1)
    }

    /// Whether the SV mapping is authored — i.e. `sv_signal` is set.
    pub fn has_sv_binding(&self) -> bool {
        self.sv_signal.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Whether the C mapping is authored — i.e. `c_accessor` is set.
    pub fn has_c_binding(&self) -> bool {
        self.c_accessor.as_ref().is_some_and(|s| !s.is_empty())
    }
}

/// Direction of the register relative to firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RegisterDirection {
    /// Read–write: firmware can both read and write. Peripheral may
    /// also update on its own (e.g. STATUS bits that the peripheral
    /// drives but firmware can mask via writes).
    Rw,
    /// Read-only: firmware reads; peripheral writes.
    Ro,
    /// Write-only: firmware writes; peripheral reacts but does not
    /// return data through this register.
    Wo,
}

impl fmt::Display for RegisterDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RegisterDirection::Rw => "RW",
            RegisterDirection::Ro => "RO",
            RegisterDirection::Wo => "WO",
        };
        f.write_str(s)
    }
}

/// Visibility class — captures the standard concurrency semantics of
/// common register patterns. Doc C §C.2 names these as the second
/// orthogonal axis of register characterisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityClass {
    /// Writes trigger peripheral behaviour. Mutually exclusive at the
    /// register granularity.
    Control,
    /// Reads observe peripheral state. Concurrent reads are safe.
    Status,
    /// FIFO ingress or egress. Each access advances the FIFO pointer
    /// by one element.
    Data,
    /// Sticky bit set by the peripheral, cleared by firmware writing
    /// 1. Common interrupt-flag pattern.
    InterruptFlag,
    /// Read has a side-effect — the act of reading clears the
    /// register's state.
    ClearOnRead,
    /// Anything that does not fit a standard class. Default for
    /// freshly-imported register maps until the author assigns one.
    #[default]
    Other,
}

/// How firmware reaches the register. Doc C §C.2 names this as the
/// third orthogonal axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPath {
    /// Direct memory-mapped load/store. The most common case; default.
    #[default]
    MmioDirect,
    /// Memory-mapped through a bridge (e.g. AHB-Lite, AXI-Lite).
    /// Multi-beat with potential back-pressure.
    MmioBridge,
    /// DMA-mediated — the access is asynchronous to firmware
    /// execution.
    Dma,
}

/// Errors / warnings produced by [`RegisterMap::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationIssue {
    UnparseableBaseAddress {
        peripheral: String,
        value: String,
    },
    DuplicateRegisterName {
        peripheral: String,
        name: String,
    },
    DuplicateFieldName {
        peripheral: String,
        register: String,
        field: String,
    },
    FieldOutOfRange {
        peripheral: String,
        register: String,
        field: String,
        high: u32,
        width: u32,
    },
    InvertedBitRange {
        peripheral: String,
        register: String,
        field: String,
        bits: [u32; 2],
    },
    FieldOverlap {
        peripheral: String,
        register: String,
        a: String,
        b: String,
    },
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationIssue::UnparseableBaseAddress { peripheral, value } => write!(
                f,
                "{peripheral}: base_address '{value}' is not a valid address (expected 0x… / 0b… / decimal)"
            ),
            ValidationIssue::DuplicateRegisterName { peripheral, name } => {
                write!(
                    f,
                    "{peripheral}: register '{name}' is defined more than once"
                )
            }
            ValidationIssue::DuplicateFieldName {
                peripheral,
                register,
                field,
            } => write!(
                f,
                "{peripheral}.{register}: field '{field}' is defined more than once"
            ),
            ValidationIssue::FieldOutOfRange {
                peripheral,
                register,
                field,
                high,
                width,
            } => write!(
                f,
                "{peripheral}.{register}.{field}: bit {high} is outside the register's {width}-bit width"
            ),
            ValidationIssue::InvertedBitRange {
                peripheral,
                register,
                field,
                bits,
            } => write!(
                f,
                "{peripheral}.{register}.{field}: bit range [{}, {}] has low > high (use [low, high])",
                bits[0], bits[1]
            ),
            ValidationIssue::FieldOverlap {
                peripheral,
                register,
                a,
                b,
            } => write!(f, "{peripheral}.{register}: fields '{a}' and '{b}' overlap"),
        }
    }
}

impl std::error::Error for ValidationIssue {}

/// Parse an address literal: `"0xNNNN"` / `"0b…"` / plain decimal.
/// Returns `None` on parse failure or overflow.
fn parse_address(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else if let Some(bin) = trimmed
        .strip_prefix("0b")
        .or_else(|| trimmed.strip_prefix("0B"))
    {
        u64::from_str_radix(bin, 2).ok()
    } else {
        trimmed.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uart_map() -> RegisterMap {
        RegisterMap {
            peripheral: "UART_LITE".to_string(),
            base_address: "0x40010000".to_string(),
            description: Some("UART driver illustrative test fixture".to_string()),
            contract_uri: None,
            registers: vec![
                Register {
                    name: "CTRL".to_string(),
                    offset: 0,
                    width_bits: 32,
                    direction: RegisterDirection::Rw,
                    visibility_class: VisibilityClass::Control,
                    access_path: AccessPath::MmioDirect,
                    description: Some("control register".to_string()),
                    fields: vec![
                        Field {
                            name: "tx_start".to_string(),
                            bits: [0, 0],
                            sv_signal: Some("uart_inst.ctrl_reg[0]".to_string()),
                            c_accessor: Some("UART->CTRL.bit.tx_start".to_string()),
                            description: None,
                        },
                        Field {
                            name: "enable".to_string(),
                            bits: [1, 1],
                            sv_signal: Some("uart_inst.ctrl_reg[1]".to_string()),
                            c_accessor: Some("UART->CTRL.bit.enable".to_string()),
                            description: None,
                        },
                    ],
                },
                Register {
                    name: "STATUS".to_string(),
                    offset: 4,
                    width_bits: 32,
                    direction: RegisterDirection::Ro,
                    visibility_class: VisibilityClass::Status,
                    access_path: AccessPath::MmioDirect,
                    description: None,
                    fields: vec![Field {
                        name: "tx_busy".to_string(),
                        bits: [0, 0],
                        sv_signal: Some("uart_inst.tx_busy".to_string()),
                        c_accessor: Some("UART->STATUS.bit.tx_busy".to_string()),
                        description: None,
                    }],
                },
            ],
        }
    }

    #[test]
    fn round_trips_through_serde() {
        let original = uart_map();
        let json = serde_json::to_string_pretty(&original).expect("serialise");
        let parsed: RegisterMap = serde_json::from_str(&json).expect("parse");
        assert_eq!(original, parsed);
    }

    #[test]
    fn round_trip_uses_uppercase_direction_strings() {
        let original = uart_map();
        let json = serde_json::to_string(&original).expect("serialise");
        // The wire format must use UPPERCASE for directions so it
        // matches the datasheet convention.
        assert!(json.contains("\"direction\":\"RW\""));
        assert!(json.contains("\"direction\":\"RO\""));
    }

    #[test]
    fn base_address_value_parses_hex_bin_decimal() {
        let mut m = uart_map();
        assert_eq!(m.base_address_value(), Some(0x4001_0000));
        m.base_address = "0b1000".to_string();
        assert_eq!(m.base_address_value(), Some(0b1000));
        m.base_address = "1024".to_string();
        assert_eq!(m.base_address_value(), Some(1024));
        m.base_address = "not_a_number".to_string();
        assert_eq!(m.base_address_value(), None);
    }

    #[test]
    fn register_absolute_address_adds_offset_to_base() {
        let m = uart_map();
        assert_eq!(m.register_absolute_address("CTRL"), Some(0x4001_0000));
        assert_eq!(m.register_absolute_address("STATUS"), Some(0x4001_0004));
        assert_eq!(m.register_absolute_address("MISSING"), None);
    }

    #[test]
    fn register_lookup_and_field_lookup() {
        let m = uart_map();
        let ctrl = m.register("CTRL").expect("CTRL exists");
        assert_eq!(ctrl.direction, RegisterDirection::Rw);
        assert_eq!(ctrl.fields.len(), 2);
        let tx_start = ctrl.field("tx_start").expect("tx_start field exists");
        assert_eq!(tx_start.bits, [0, 0]);
        assert!(tx_start.has_sv_binding());
        assert!(tx_start.has_c_binding());
    }

    #[test]
    fn validate_well_formed_map_returns_no_issues() {
        let m = uart_map();
        assert!(m.validate().is_empty());
    }

    #[test]
    fn validate_unparseable_base_address() {
        let mut m = uart_map();
        m.base_address = "garbage".to_string();
        let issues = m.validate();
        assert!(matches!(
            issues.first(),
            Some(ValidationIssue::UnparseableBaseAddress { .. })
        ));
    }

    #[test]
    fn validate_duplicate_register_name() {
        let mut m = uart_map();
        let dup = m.registers[0].clone();
        m.registers.push(dup);
        let issues = m.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ValidationIssue::DuplicateRegisterName { .. }))
        );
    }

    #[test]
    fn validate_field_out_of_range() {
        let mut m = uart_map();
        m.registers[0].fields.push(Field {
            name: "bogus".to_string(),
            // 32-bit register, bit 32 is out of range.
            bits: [32, 32],
            sv_signal: None,
            c_accessor: None,
            description: None,
        });
        let issues = m.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ValidationIssue::FieldOutOfRange { .. }))
        );
    }

    #[test]
    fn validate_inverted_bit_range() {
        let mut m = uart_map();
        m.registers[0].fields.push(Field {
            name: "inverted".to_string(),
            bits: [5, 2],
            sv_signal: None,
            c_accessor: None,
            description: None,
        });
        let issues = m.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ValidationIssue::InvertedBitRange { .. }))
        );
    }

    #[test]
    fn validate_field_overlap() {
        let mut m = uart_map();
        m.registers[0].fields.push(Field {
            name: "overlaps_tx_start".to_string(),
            // CTRL.tx_start is [0, 0]; this overlaps.
            bits: [0, 3],
            sv_signal: None,
            c_accessor: None,
            description: None,
        });
        let issues = m.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ValidationIssue::FieldOverlap { .. }))
        );
    }

    #[test]
    fn field_width_bits_handles_inclusive_range() {
        let f = Field {
            name: "x".to_string(),
            bits: [4, 7],
            sv_signal: None,
            c_accessor: None,
            description: None,
        };
        assert_eq!(f.width_bits(), 4);
    }

    #[test]
    fn has_sv_binding_false_when_empty_string() {
        let f = Field {
            name: "x".to_string(),
            bits: [0, 0],
            sv_signal: Some(String::new()),
            c_accessor: None,
            description: None,
        };
        assert!(!f.has_sv_binding());
    }

    #[test]
    fn defaults_collapse_to_other_and_mmio_direct() {
        let json = r#"{
            "peripheral": "MINI",
            "base_address": "0x1000",
            "registers": [
                { "name": "R", "offset": 0, "width_bits": 32, "direction": "RW",
                  "fields": [{ "name": "f", "bits": [0, 0] }] }
            ]
        }"#;
        let m: RegisterMap = serde_json::from_str(json).expect("parse");
        let r = m.register("R").expect("R");
        assert_eq!(r.visibility_class, VisibilityClass::Other);
        assert_eq!(r.access_path, AccessPath::MmioDirect);
    }

    #[test]
    fn missing_optional_strings_round_trip_cleanly() {
        let m = RegisterMap {
            peripheral: "MIN".to_string(),
            base_address: "0x0".to_string(),
            description: None,
            contract_uri: None,
            registers: vec![],
        };
        let json = serde_json::to_string(&m).expect("serialise");
        // Optional fields should be omitted when None.
        assert!(!json.contains("description"));
        assert!(!json.contains("contract_uri"));
    }
}
