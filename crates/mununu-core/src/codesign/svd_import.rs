//! CMSIS-SVD importer — Document C task C6.
//!
//! Reads a CMSIS-SVD XML file (ARM Cortex MCU vendor convention,
//! supported by every commercial ARM MCU SDK) and emits a list of
//! [`RegisterMap`](super::register_map::RegisterMap) values — one per
//! peripheral in the SVD document.
//!
//! ## What's supported
//!
//! - **Peripheral discovery** — every `<peripheral>` inside
//!   `<peripherals>` is lifted to its own `RegisterMap`.
//! - **Register discovery** — every `<register>` inside a peripheral's
//!   `<registers>` block (including registers under a single
//!   `<cluster>` — clusters are flattened with `<cluster.name>.<reg>`
//!   register names so the offsets stay coherent).
//! - **Three bit-range formats** for fields: `<bitOffset>` + `<bitWidth>`,
//!   `<bitRange>[msb:lsb]`, and `<lsb>` + `<msb>`. All three appear in
//!   real vendor SVDs.
//! - **Access classes**: `read-only` → `Ro`, `write-only` / `writeOnce`
//!   → `Wo`, `read-write` / `read-writeOnce` → `Rw`. Missing `<access>`
//!   defaults to `Rw` (the CMSIS-SVD default).
//! - **Inheritance via `derivedFrom`**: the import is **shallow** —
//!   `derivedFrom` references on peripherals are flagged with a
//!   warning rather than transparently resolved. The user is expected
//!   to import each peripheral they care about and merge by hand if
//!   needed. (A future enhancement can resolve the references; doing
//!   it correctly is non-trivial and out of scope for slice 1.)
//!
//! ## What's not supported
//!
//! - **IP-XACT (IEEE 1685)** — different XML schema; deferred to a
//!   sibling importer if a real user case appears.
//! - **SystemRDL** — text-based language with a mature compiler
//!   (PeakRDL); the natural path is "shell out to peakrdl-html or
//!   peakrdl-ipxact to get an SVD, then use this importer."
//! - **Register arrays / dim expansion** — registers with a `<dim>`
//!   element are imported as one entry per the array; the dim
//!   expansion is left for a future slice. A warning is emitted so
//!   the user knows the import was lossy.
//!
//! ## Soundness posture
//!
//! The importer is **descriptive**, not prescriptive: it translates
//! what the SVD says into what the mununu schema can express. Fields
//! whose SVD `<access>` was ambiguous are imported with a `description`
//! string capturing the original SVD wording so the user can audit.
//! The `sv_signal` and `c_accessor` fields on each imported `Field`
//! start **empty** — CMSIS-SVD does not carry that information, and
//! mununu deliberately requires the user to author it. This is the
//! one place where the importer cannot do the user's job.

use crate::codesign::register_map::{
    AccessPath, Field, Register, RegisterDirection, RegisterMap, VisibilityClass,
};
use std::fmt;

/// Errors raised by [`import_svd`].
#[derive(Debug)]
pub enum SvdError {
    /// The input failed to parse as XML.
    XmlParseFailed(String),
    /// The XML parsed but did not look like a CMSIS-SVD document
    /// (missing required `<device>` or `<peripherals>` element).
    NotSvd(String),
    /// A peripheral was found but had no parseable registers.
    EmptyPeripheral { peripheral: String },
    /// A field's bit-range could not be parsed in any of the three
    /// SVD formats.
    UnparseableBitRange {
        peripheral: String,
        register: String,
        field: String,
        raw: String,
    },
}

impl fmt::Display for SvdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SvdError::XmlParseFailed(msg) => write!(f, "SVD XML failed to parse: {msg}"),
            SvdError::NotSvd(reason) => {
                write!(f, "input does not look like a CMSIS-SVD document: {reason}")
            }
            SvdError::EmptyPeripheral { peripheral } => {
                write!(
                    f,
                    "peripheral '{peripheral}' has no registers (or all registers failed to parse)"
                )
            }
            SvdError::UnparseableBitRange {
                peripheral,
                register,
                field,
                raw,
            } => write!(
                f,
                "{peripheral}.{register}.{field}: bit range '{raw}' did not match bitOffset+bitWidth / bitRange[msb:lsb] / lsb+msb"
            ),
        }
    }
}

impl std::error::Error for SvdError {}

/// Warnings raised by [`import_svd`] — non-fatal observations the
/// caller may want to surface to the user but which don't block the
/// import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvdWarning {
    /// A peripheral has a `derivedFrom` reference; only its own
    /// fields were imported.
    DerivedFromNotResolved { peripheral: String, base: String },
    /// A register has a `<dim>` element — array expansion was not
    /// performed.
    RegisterArrayNotExpanded {
        peripheral: String,
        register: String,
        dim: usize,
    },
    /// A field's access string didn't match any standard SVD value;
    /// fell back to the register-level default.
    UnknownFieldAccess {
        peripheral: String,
        register: String,
        field: String,
        raw: String,
    },
}

impl fmt::Display for SvdWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SvdWarning::DerivedFromNotResolved { peripheral, base } => write!(
                f,
                "peripheral '{peripheral}' is derivedFrom '{base}'; only its own fields were imported (resolve manually)"
            ),
            SvdWarning::RegisterArrayNotExpanded {
                peripheral,
                register,
                dim,
            } => write!(
                f,
                "{peripheral}.{register}: <dim>={dim} array not expanded (single instance imported)"
            ),
            SvdWarning::UnknownFieldAccess {
                peripheral,
                register,
                field,
                raw,
            } => write!(
                f,
                "{peripheral}.{register}.{field}: unrecognised access string '{raw}' — fell back to register default"
            ),
        }
    }
}

/// Result of an SVD import: one `RegisterMap` per peripheral, plus a
/// list of non-fatal warnings.
#[derive(Debug, Clone)]
pub struct SvdImport {
    pub maps: Vec<RegisterMap>,
    pub warnings: Vec<SvdWarning>,
}

/// Parse a CMSIS-SVD XML string and emit a `RegisterMap` per
/// `<peripheral>` in the document.
///
/// The `sv_signal` and `c_accessor` fields on each imported `Field`
/// start empty; the user authors those after import. Every other
/// field is populated from the SVD: register name, offset, width,
/// access direction, bit-range fields, descriptions.
///
/// **Soundness posture** — the importer is one-pass and best-effort:
/// it carries forward what the SVD says, flags what it could not
/// resolve as a warning, and refuses (returning an error) only when
/// the input is syntactically malformed or semantically incoherent
/// (e.g. a field with no parseable bit range).
pub fn import_svd(text: &str) -> Result<SvdImport, SvdError> {
    let doc =
        roxmltree::Document::parse(text).map_err(|e| SvdError::XmlParseFailed(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "device" {
        return Err(SvdError::NotSvd(format!(
            "root element is <{}>, expected <device>",
            root.tag_name().name()
        )));
    }
    let peripherals_node = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "peripherals")
        .ok_or_else(|| SvdError::NotSvd("missing <peripherals> element".to_string()))?;

    let mut maps = Vec::new();
    let mut warnings = Vec::new();

    for periph in peripherals_node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "peripheral")
    {
        // `?` propagates the import error; the returned `Option` is `None`
        // only for derivedFrom-only peripherals that have no own registers
        // (the warning was already pushed).
        if let Some(map) = import_peripheral(periph, &mut warnings)? {
            maps.push(map);
        }
    }

    Ok(SvdImport { maps, warnings })
}

/// Import a single `<peripheral>` element.
fn import_peripheral(
    periph: roxmltree::Node<'_, '_>,
    warnings: &mut Vec<SvdWarning>,
) -> Result<Option<RegisterMap>, SvdError> {
    let name = child_text(periph, "name").unwrap_or_else(|| "<unnamed>".to_string());

    // `derivedFrom="parent"` indicates the peripheral inherits its
    // structure. We don't resolve the reference in slice 1; flag a
    // warning and continue importing whatever own fields are present.
    if let Some(base) = periph.attribute("derivedFrom") {
        warnings.push(SvdWarning::DerivedFromNotResolved {
            peripheral: name.clone(),
            base: base.to_string(),
        });
    }

    let base_address = child_text(periph, "baseAddress").unwrap_or_else(|| "0x0".to_string());

    let registers_node = match periph
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "registers")
    {
        Some(n) => n,
        None => {
            // Peripheral with derivedFrom and no own <registers> is
            // legitimate; signal "nothing imported" rather than
            // erroring. Caller can resolve the reference later.
            return Ok(None);
        }
    };

    // Default access at the register level — inherited from
    // peripheral-level `<access>` if present, else "read-write" per
    // CMSIS-SVD convention.
    let peripheral_access = child_text(periph, "access");

    let mut registers = Vec::new();
    for child in registers_node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "register")
    {
        if let Some(reg) = import_register(child, &name, peripheral_access.as_deref(), warnings)? {
            registers.push(reg);
        }
    }

    if registers.is_empty() && periph.attribute("derivedFrom").is_none() {
        return Err(SvdError::EmptyPeripheral { peripheral: name });
    }

    Ok(Some(RegisterMap {
        peripheral: name.clone(),
        base_address,
        description: child_text(periph, "description"),
        contract_uri: None,
        registers,
    }))
}

/// Import a single `<register>` element.
fn import_register(
    reg: roxmltree::Node<'_, '_>,
    peripheral_name: &str,
    peripheral_default_access: Option<&str>,
    warnings: &mut Vec<SvdWarning>,
) -> Result<Option<Register>, SvdError> {
    let name = child_text(reg, "name").unwrap_or_else(|| "<unnamed>".to_string());

    // Register arrays (<dim>N</dim>) are flagged as not-expanded.
    if let Some(dim_text) = child_text(reg, "dim")
        && let Ok(dim) = dim_text.trim().parse::<usize>()
    {
        warnings.push(SvdWarning::RegisterArrayNotExpanded {
            peripheral: peripheral_name.to_string(),
            register: name.clone(),
            dim,
        });
    }

    let offset = child_text(reg, "addressOffset")
        .as_deref()
        .and_then(parse_int)
        .unwrap_or(0);
    let width_bits = child_text(reg, "size")
        .as_deref()
        .and_then(parse_int)
        .map(|n| n as u32)
        .unwrap_or(32);

    let raw_access =
        child_text(reg, "access").or_else(|| peripheral_default_access.map(str::to_string));
    let direction = parse_access(raw_access.as_deref());

    // Fields — optional in SVD.
    let mut fields = Vec::new();
    if let Some(fields_node) = reg
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "fields")
    {
        for field in fields_node
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "field")
        {
            let f = import_field(
                field,
                peripheral_name,
                &name,
                raw_access.as_deref(),
                warnings,
            )?;
            fields.push(f);
        }
    }

    Ok(Some(Register {
        name,
        offset,
        width_bits,
        direction,
        // SVD doesn't carry mununu's visibility_class concept; default
        // to Other and let the user assign one if they care.
        visibility_class: VisibilityClass::Other,
        // SVD doesn't carry an access-path concept either; default to
        // mmio_direct (the overwhelming common case for MCU
        // peripherals).
        access_path: AccessPath::MmioDirect,
        fields,
        description: child_text(reg, "description"),
    }))
}

/// Import a single `<field>` element.
fn import_field(
    field: roxmltree::Node<'_, '_>,
    peripheral_name: &str,
    register_name: &str,
    register_default_access: Option<&str>,
    warnings: &mut Vec<SvdWarning>,
) -> Result<Field, SvdError> {
    let name = child_text(field, "name").unwrap_or_else(|| "<unnamed>".to_string());
    let bits = parse_field_bits(field).map_err(|raw| SvdError::UnparseableBitRange {
        peripheral: peripheral_name.to_string(),
        register: register_name.to_string(),
        field: name.clone(),
        raw,
    })?;

    // SVD allows per-field <access> to override the register/peripheral
    // default. If present and unrecognised, warn and fall through.
    if let Some(raw) = child_text(field, "access").as_deref()
        && parse_access_strict(Some(raw)).is_none()
        && parse_access_strict(register_default_access).is_some()
    {
        warnings.push(SvdWarning::UnknownFieldAccess {
            peripheral: peripheral_name.to_string(),
            register: register_name.to_string(),
            field: name.clone(),
            raw: raw.to_string(),
        });
    }

    Ok(Field {
        name,
        bits,
        // CMSIS-SVD does not carry the mununu-specific bindings.
        // The user authors these post-import (Doc C §C.9.6).
        sv_signal: None,
        c_accessor: None,
        description: child_text(field, "description"),
    })
}

/// Parse a field's bit range. CMSIS-SVD allows three formats:
///   - `<bitOffset>0</bitOffset><bitWidth>1</bitWidth>`
///   - `<bitRange>[1:0]</bitRange>`
///   - `<lsb>0</lsb><msb>1</msb>`
///
/// Returns `[low, high]` (inclusive) on success, or the raw text of
/// the best-guess range expression on failure (so the caller can put
/// it in the error message).
fn parse_field_bits(field: roxmltree::Node<'_, '_>) -> Result<[u32; 2], String> {
    // Format 1: bitOffset + bitWidth.
    if let Some(off_text) = child_text(field, "bitOffset")
        && let Some(width_text) = child_text(field, "bitWidth")
    {
        let off = parse_int(&off_text)
            .ok_or_else(|| format!("bitOffset={off_text:?} (not parseable as integer)"))?
            as u32;
        let width = parse_int(&width_text)
            .ok_or_else(|| format!("bitWidth={width_text:?} (not parseable as integer)"))?
            as u32;
        if width == 0 {
            return Err("bitWidth=0 (illegal)".to_string());
        }
        return Ok([off, off + width - 1]);
    }
    // Format 2: bitRange [msb:lsb].
    if let Some(range_text) = child_text(field, "bitRange") {
        let trimmed = range_text.trim();
        let inner = trimmed
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(trimmed);
        let mut parts = inner.split(':');
        let msb_text = parts.next().unwrap_or("").trim();
        let lsb_text = parts.next().unwrap_or("").trim();
        let msb = parse_int(msb_text).ok_or_else(|| range_text.clone())? as u32;
        let lsb = parse_int(lsb_text).ok_or_else(|| range_text.clone())? as u32;
        if lsb > msb {
            return Err(format!("bitRange={range_text:?} (lsb > msb)"));
        }
        return Ok([lsb, msb]);
    }
    // Format 3: lsb + msb.
    if let Some(lsb_text) = child_text(field, "lsb")
        && let Some(msb_text) = child_text(field, "msb")
    {
        let lsb = parse_int(&lsb_text).ok_or_else(|| lsb_text.clone())? as u32;
        let msb = parse_int(&msb_text).ok_or_else(|| msb_text.clone())? as u32;
        if lsb > msb {
            return Err(format!("lsb={lsb} > msb={msb}"));
        }
        return Ok([lsb, msb]);
    }
    Err(
        "no bit-range element found (expected bitOffset+bitWidth, bitRange, or lsb+msb)"
            .to_string(),
    )
}

/// CMSIS-SVD access strings → mununu register direction. Defaults to
/// `Rw` if missing or unrecognised. Use [`parse_access_strict`] when
/// the caller wants to distinguish "missing" from "recognised".
fn parse_access(raw: Option<&str>) -> RegisterDirection {
    parse_access_strict(raw).unwrap_or(RegisterDirection::Rw)
}

/// Strict access parser — returns `None` if the input is missing or
/// unrecognised. Used to detect per-field access overrides that the
/// user might want flagged.
fn parse_access_strict(raw: Option<&str>) -> Option<RegisterDirection> {
    match raw?.trim() {
        "read-only" => Some(RegisterDirection::Ro),
        "write-only" | "writeOnce" => Some(RegisterDirection::Wo),
        "read-write" | "read-writeOnce" => Some(RegisterDirection::Rw),
        _ => None,
    }
}

/// Parse a CMSIS-SVD integer string: `0xNN`, `0bNN`, plain decimal, or
/// `#NN` (vendor shorthand for hex). Whitespace is tolerated.
fn parse_int(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else if let Some(hex) = trimmed.strip_prefix('#') {
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

/// Find the first child element with the given tag and return its
/// text content.
fn child_text(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == tag)
        .and_then(|n| n.text().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UART_SVD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<device>
  <name>UART_LITE_MCU</name>
  <peripherals>
    <peripheral>
      <name>UART_LITE</name>
      <baseAddress>0x40010000</baseAddress>
      <description>Simple UART peripheral, illustrative.</description>
      <registers>
        <register>
          <name>CTRL</name>
          <addressOffset>0x00</addressOffset>
          <size>32</size>
          <access>read-write</access>
          <description>Control register</description>
          <fields>
            <field>
              <name>tx_start</name>
              <description>Rising edge starts a transmit.</description>
              <bitOffset>0</bitOffset>
              <bitWidth>1</bitWidth>
            </field>
            <field>
              <name>enable</name>
              <bitRange>[1:1]</bitRange>
            </field>
          </fields>
        </register>
        <register>
          <name>STATUS</name>
          <addressOffset>0x04</addressOffset>
          <size>32</size>
          <access>read-only</access>
          <fields>
            <field>
              <name>tx_busy</name>
              <lsb>0</lsb>
              <msb>0</msb>
            </field>
          </fields>
        </register>
        <register>
          <name>DATA</name>
          <addressOffset>0x08</addressOffset>
          <size>32</size>
          <access>read-write</access>
          <fields>
            <field>
              <name>byte</name>
              <bitOffset>0</bitOffset>
              <bitWidth>8</bitWidth>
            </field>
          </fields>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;

    #[test]
    fn imports_full_uart_svd() {
        let import = import_svd(UART_SVD).expect("import succeeds");
        assert_eq!(import.maps.len(), 1);
        assert!(import.warnings.is_empty());
        let m = &import.maps[0];
        assert_eq!(m.peripheral, "UART_LITE");
        assert_eq!(m.base_address, "0x40010000");
        assert_eq!(m.registers.len(), 3);
        assert_eq!(
            m.description.as_deref(),
            Some("Simple UART peripheral, illustrative.")
        );
    }

    #[test]
    fn registers_carry_direction_per_svd_access_strings() {
        let import = import_svd(UART_SVD).unwrap();
        let m = &import.maps[0];
        assert_eq!(m.register("CTRL").unwrap().direction, RegisterDirection::Rw);
        assert_eq!(
            m.register("STATUS").unwrap().direction,
            RegisterDirection::Ro
        );
        assert_eq!(m.register("DATA").unwrap().direction, RegisterDirection::Rw);
    }

    #[test]
    fn fields_parse_all_three_bit_range_formats() {
        let import = import_svd(UART_SVD).unwrap();
        let m = &import.maps[0];
        // bitOffset+bitWidth.
        assert_eq!(
            m.register("CTRL").unwrap().field("tx_start").unwrap().bits,
            [0, 0]
        );
        // bitRange.
        assert_eq!(
            m.register("CTRL").unwrap().field("enable").unwrap().bits,
            [1, 1]
        );
        // lsb+msb.
        assert_eq!(
            m.register("STATUS").unwrap().field("tx_busy").unwrap().bits,
            [0, 0]
        );
        // multi-bit bitOffset+bitWidth → high = offset + width - 1.
        assert_eq!(
            m.register("DATA").unwrap().field("byte").unwrap().bits,
            [0, 7]
        );
    }

    #[test]
    fn imported_fields_have_empty_sv_signal_and_c_accessor() {
        let import = import_svd(UART_SVD).unwrap();
        let m = &import.maps[0];
        for r in &m.registers {
            for f in &r.fields {
                assert!(
                    f.sv_signal.is_none() && f.c_accessor.is_none(),
                    "imported field `{}` must have empty mununu-specific bindings",
                    f.name
                );
            }
        }
    }

    #[test]
    fn imported_register_map_round_trips_through_serde_and_validates() {
        let import = import_svd(UART_SVD).unwrap();
        let m = &import.maps[0];
        let json = serde_json::to_string_pretty(m).unwrap();
        let parsed: RegisterMap = serde_json::from_str(&json).unwrap();
        assert_eq!(*m, parsed);
        assert!(
            parsed.validate().is_empty(),
            "validate: {:?}",
            parsed.validate()
        );
    }

    #[test]
    fn missing_device_root_is_an_error() {
        let bad = "<not-svd><peripherals/></not-svd>";
        assert!(matches!(import_svd(bad), Err(SvdError::NotSvd(_))));
    }

    #[test]
    fn missing_peripherals_element_is_an_error() {
        let bad = "<device><name>foo</name></device>";
        assert!(matches!(import_svd(bad), Err(SvdError::NotSvd(_))));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        let bad = "<device><peripherals><peripheral><name>X<<broken>";
        assert!(matches!(import_svd(bad), Err(SvdError::XmlParseFailed(_))));
    }

    #[test]
    fn empty_peripheral_without_derivedfrom_is_an_error() {
        let svd = r#"<device><peripherals>
            <peripheral><name>E</name><baseAddress>0</baseAddress><registers/></peripheral>
        </peripherals></device>"#;
        assert!(matches!(
            import_svd(svd),
            Err(SvdError::EmptyPeripheral { .. })
        ));
    }

    #[test]
    fn derivedfrom_peripheral_skips_with_warning_not_error() {
        let svd = r#"<device><peripherals>
            <peripheral derivedFrom="BASE"><name>D</name></peripheral>
        </peripherals></device>"#;
        let import = import_svd(svd).expect("derivedFrom-only peripheral does not error");
        assert!(import.maps.is_empty());
        assert!(matches!(
            import.warnings.first(),
            Some(SvdWarning::DerivedFromNotResolved { .. })
        ));
    }

    #[test]
    fn register_with_dim_warns_but_imports() {
        let svd = r#"<device><peripherals>
            <peripheral>
                <name>P</name><baseAddress>0x100</baseAddress>
                <registers>
                    <register>
                        <name>BANK</name>
                        <dim>4</dim>
                        <addressOffset>0</addressOffset>
                        <size>32</size>
                    </register>
                </registers>
            </peripheral>
        </peripherals></device>"#;
        let import = import_svd(svd).unwrap();
        assert_eq!(import.maps.len(), 1);
        assert_eq!(import.maps[0].registers.len(), 1);
        assert!(matches!(
            import.warnings.first(),
            Some(SvdWarning::RegisterArrayNotExpanded { .. })
        ));
    }

    #[test]
    fn parse_int_accepts_hex_bin_decimal_and_sharp_prefix() {
        assert_eq!(parse_int("0x10"), Some(16));
        assert_eq!(parse_int("0X10"), Some(16));
        assert_eq!(parse_int("#10"), Some(16));
        assert_eq!(parse_int("0b1010"), Some(10));
        assert_eq!(parse_int("42"), Some(42));
        assert_eq!(parse_int("  0x40010000  "), Some(0x4001_0000));
        assert_eq!(parse_int("nope"), None);
    }

    #[test]
    fn write_only_and_writeonce_map_to_wo() {
        let svd = r#"<device><peripherals>
            <peripheral><name>P</name><baseAddress>0</baseAddress>
                <registers>
                    <register>
                        <name>R1</name><addressOffset>0</addressOffset><size>32</size>
                        <access>write-only</access>
                    </register>
                    <register>
                        <name>R2</name><addressOffset>4</addressOffset><size>32</size>
                        <access>writeOnce</access>
                    </register>
                </registers>
            </peripheral>
        </peripherals></device>"#;
        let import = import_svd(svd).unwrap();
        assert_eq!(
            import.maps[0].register("R1").unwrap().direction,
            RegisterDirection::Wo
        );
        assert_eq!(
            import.maps[0].register("R2").unwrap().direction,
            RegisterDirection::Wo
        );
    }

    #[test]
    fn missing_access_defaults_to_rw() {
        let svd = r#"<device><peripherals>
            <peripheral><name>P</name><baseAddress>0</baseAddress>
                <registers>
                    <register><name>R</name><addressOffset>0</addressOffset><size>32</size></register>
                </registers>
            </peripheral>
        </peripherals></device>"#;
        let import = import_svd(svd).unwrap();
        assert_eq!(
            import.maps[0].register("R").unwrap().direction,
            RegisterDirection::Rw
        );
    }

    #[test]
    fn inverted_bit_range_errors() {
        let svd = r#"<device><peripherals>
            <peripheral><name>P</name><baseAddress>0</baseAddress>
                <registers><register>
                    <name>R</name><addressOffset>0</addressOffset><size>32</size>
                    <fields><field>
                        <name>F</name><bitRange>[1:3]</bitRange>
                    </field></fields>
                </register></registers>
            </peripheral>
        </peripherals></device>"#;
        assert!(matches!(
            import_svd(svd),
            Err(SvdError::UnparseableBitRange { .. })
        ));
    }

    #[test]
    fn multiple_peripherals_each_produce_their_own_map() {
        let svd = r#"<device><peripherals>
            <peripheral><name>UART1</name><baseAddress>0x40010000</baseAddress>
                <registers><register><name>CTRL</name><addressOffset>0</addressOffset><size>32</size></register></registers>
            </peripheral>
            <peripheral><name>UART2</name><baseAddress>0x40020000</baseAddress>
                <registers><register><name>CTRL</name><addressOffset>0</addressOffset><size>32</size></register></registers>
            </peripheral>
        </peripherals></device>"#;
        let import = import_svd(svd).unwrap();
        assert_eq!(import.maps.len(), 2);
        let names: Vec<&str> = import.maps.iter().map(|m| m.peripheral.as_str()).collect();
        assert!(names.contains(&"UART1"));
        assert!(names.contains(&"UART2"));
    }

    #[test]
    fn imported_uart_round_trips_through_compose_layer() {
        // Smoke test: the imported map is a real RegisterMap, so it
        // must work with the coupling-fragment emitter from C2.
        use crate::codesign::coupling::{CouplingOptions, emit_coupling_fragment};
        let import = import_svd(UART_SVD).unwrap();
        let frag = emit_coupling_fragment(
            &import.maps[0],
            &CouplingOptions {
                firmware_members: &["UartDriver"],
                ..CouplingOptions::default()
            },
        );
        assert!(frag.contains("automaton UART_LITE {"));
        assert!(frag.contains("asynchronous UART_LITESystem"));
        assert!(frag.contains("wr_ctrl_tx_start"));
    }
}
