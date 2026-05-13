//! Source-comment annotation grammar — Document D §D.5.
//!
//! A single tag vocabulary recognised across SystemVerilog,
//! C/C++, TypeScript, Rust, and Python (Document D §D.5.2). The
//! parsers in this module extract `MununuAnnotation` records from
//! either raw SV source text or a yosys `write_json` attribute map;
//! both producers feed the same downstream consumer (the discovery
//! pipeline in Document A §A6).
//!
//! Today the parser supports the SV-side wrappers — `(* mununu_xxx
//! [= "value"] *)` attributes and `// @mununu_xxx value` line
//! comments — plus the attribute-map shape yosys emits. C / TS /
//! Rust / Python wrappers are deliberate follow-up scope per
//! Document D §D.5.4 (ship one language first, then add more).
//!
//! # Tag table (Document D §D.5.1, SV-relevant subset)
//!
//! | Tag                       | Meaning                                                |
//! |---------------------------|--------------------------------------------------------|
//! | `@mununu_blackbox`        | Declare module as black box (no value)                 |
//! | `@mununu_assume <body>`   | Environment assumption                                 |
//! | `@mununu_guarantee <body>`| Guarantee the module provides                          |
//! | `@mununu_interface <uri>` | Reference a stored contract (sidecar path or `contract://`) |
//! | `@mununu_controllable <label>`   | Override default controllability             |
//! | `@mununu_uncontrollable <label>` | Override default controllability             |
//!
//! `@mununu_register` and `@mununu_behavior` are reserved by D.5.1 but
//! out of scope for this module today — they need Document C
//! (codesign) and the template registry, respectively.

use serde::{Deserialize, Serialize};

/// Recognised tag from the §D.5 annotation grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MununuTag {
    /// `@mununu_blackbox` — module is a closed-IP boundary.
    Blackbox,
    /// `@mununu_assume <body>` — environment assumption.
    Assume,
    /// `@mununu_guarantee <body>` — module guarantee.
    Guarantee,
    /// `@mununu_interface <uri>` — corpus / sidecar reference.
    Interface,
    /// `@mununu_controllable <label>` — controllability override.
    Controllable,
    /// `@mununu_uncontrollable <label>` — controllability override.
    Uncontrollable,
}

impl MununuTag {
    /// Parse a tag name (without the `@mununu_` prefix) into a
    /// `MununuTag`. Returns `None` for unrecognised tags so the caller
    /// can decide whether to warn or silently skip.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "blackbox" => Some(MununuTag::Blackbox),
            "assume" => Some(MununuTag::Assume),
            "guarantee" => Some(MununuTag::Guarantee),
            "interface" => Some(MununuTag::Interface),
            "controllable" => Some(MununuTag::Controllable),
            "uncontrollable" => Some(MununuTag::Uncontrollable),
            _ => None,
        }
    }

    /// Display name without the `@mununu_` prefix.
    pub fn name(self) -> &'static str {
        match self {
            MununuTag::Blackbox => "blackbox",
            MununuTag::Assume => "assume",
            MununuTag::Guarantee => "guarantee",
            MununuTag::Interface => "interface",
            MununuTag::Controllable => "controllable",
            MununuTag::Uncontrollable => "uncontrollable",
        }
    }
}

/// A single annotation extracted from source. The `value` is the
/// raw payload — for `Blackbox` this is typically empty; for
/// `Assume` / `Guarantee` / `Interface` / `Controllable` /
/// `Uncontrollable` it is the rest of the tag's body. No further
/// parsing is performed here; downstream consumers (discovery
/// pipeline, HITL UX) interpret it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MununuAnnotation {
    pub tag: MununuTag,
    /// Free-form payload following the tag. Empty for tags like
    /// `Blackbox` that take no value.
    pub value: String,
    /// 1-based source line number when known. Always set by the
    /// SV-source parser; absent when the annotation came from
    /// yosys's attribute map (yosys's `src` attribute is parsed
    /// elsewhere into the enclosing module's source line).
    #[serde(default)]
    pub source_line: Option<u32>,
}

impl MununuAnnotation {
    pub fn new(tag: MununuTag, value: impl Into<String>) -> Self {
        Self {
            tag,
            value: value.into(),
            source_line: None,
        }
    }

    pub fn with_line(mut self, line: u32) -> Self {
        self.source_line = Some(line);
        self
    }
}

/// Extract all mununu annotations from raw SystemVerilog source text.
///
/// Recognises:
///   - `(* mununu_<tag> [= "value"] *)` attribute syntax. The `value`
///     part is optional (e.g. `(* mununu_blackbox *)`).
///   - `// @mununu_<tag> [value]` line-comment syntax. The value is
///     the rest of the line.
///   - `/* @mununu_<tag> [value] */` block-comment syntax — single-
///     line only; multi-line block comments are tolerated but only
///     the first line's content is captured.
///
/// Both forms can coexist in the same file. Unknown tags are
/// silently skipped (they may belong to another tool's
/// `(* synthesis_attribute *)` namespace).
pub fn extract_from_sv_source(text: &str) -> Vec<MununuAnnotation> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        // First, scan for `(* mununu_xxx [= "value"] *)` attributes.
        let mut cursor = 0usize;
        while let Some(start) = line[cursor..].find("(*") {
            let abs_start = cursor + start + 2;
            let Some(end_rel) = line[abs_start..].find("*)") else {
                break;
            };
            let body = &line[abs_start..abs_start + end_rel];
            for ann in parse_attribute_body(body) {
                out.push(ann.with_line(line_no));
            }
            cursor = abs_start + end_rel + 2;
        }
        // Then, scan for `// @mununu_xxx ...` line comments. The `//`
        // form must NOT be inside a `(* ... *)` block we already
        // processed — we approximate by requiring the `//` to start
        // before the first `(*`.
        if let Some(comment_start) = line.find("//")
            && let Some(annot) = parse_line_comment_body(&line[comment_start + 2..])
        {
            out.push(annot.with_line(line_no));
        }
        // Block comments: `/* @mununu_xxx ... */` on a single line.
        if let Some(b_start) = line.find("/*")
            && let Some(b_end_rel) = line[b_start + 2..].find("*/")
        {
            let body = &line[b_start + 2..b_start + 2 + b_end_rel];
            if let Some(annot) = parse_line_comment_body(body) {
                out.push(annot.with_line(line_no));
            }
        }
    }
    out
}

/// Parse the body of a `(* ... *)` attribute block. The body may
/// contain comma-separated attributes (e.g.
/// `(* mununu_blackbox, mununu_assume = "G(req → ack)" *)`).
fn parse_attribute_body(body: &str) -> Vec<MununuAnnotation> {
    let mut out = Vec::new();
    for raw in body.split(',') {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("mununu_") {
            // Two shapes: `mununu_<tag>` or `mununu_<tag> = "value"`.
            let (tag_name, value) = match rest.find('=') {
                None => (rest.trim(), String::new()),
                Some(eq) => {
                    let tag_name = rest[..eq].trim();
                    let raw_value = rest[eq + 1..].trim();
                    let v = unquote(raw_value).to_string();
                    (tag_name, v)
                }
            };
            if let Some(tag) = MununuTag::from_name(tag_name) {
                out.push(MununuAnnotation::new(tag, value));
            }
        }
    }
    out
}

/// Parse the body of a `// @mununu_xxx ...` line comment. Returns
/// `None` if the body doesn't start with an `@mununu_` tag.
fn parse_line_comment_body(body: &str) -> Option<MununuAnnotation> {
    let trimmed = body.trim_start();
    let rest = trimmed.strip_prefix("@mununu_")?;
    // Either `<tag>` alone, or `<tag> <value>`.
    let (tag_name, value) = match rest.find(|c: char| c.is_whitespace()) {
        None => (rest.trim_end(), String::new()),
        Some(idx) => {
            let tag_name = &rest[..idx];
            let raw_value = rest[idx..].trim();
            (tag_name, unquote(raw_value).to_string())
        }
    };
    MununuTag::from_name(tag_name).map(|tag| MununuAnnotation::new(tag, value))
}

/// Strip surrounding double quotes from a string, if present.
fn unquote(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Extract mununu annotations from a yosys `write_json`
/// `attributes` map. Each entry whose key starts with `mununu_`
/// produces one `MununuAnnotation`; the value is the bitstring or
/// string yosys serialised.
///
/// Yosys serialises bare-flag attributes (like `(* blackbox *)`) as
/// a bitstring ending in `1`. We detect that pattern and emit an
/// empty `value`. Everything else is treated as a literal string
/// value.
pub fn extract_from_yosys_attributes(attrs: &serde_json::Value) -> Vec<MununuAnnotation> {
    let Some(map) = attrs.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in map {
        let Some(rest) = key.strip_prefix("mununu_") else {
            continue;
        };
        let Some(tag) = MununuTag::from_name(rest) else {
            continue;
        };
        let raw = value.as_str().unwrap_or("");
        // Yosys's bare-flag encoding: bitstring of '0's and '1's
        // (the bit width of the attribute's value). For boolean
        // flags this is `...01`. Treat any string that is exclusively
        // '0' / '1' characters as a flag with empty body.
        let value_text = if raw.chars().all(|c| c == '0' || c == '1') && !raw.is_empty() {
            String::new()
        } else {
            raw.to_string()
        };
        out.push(MununuAnnotation::new(tag, value_text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ----- SV source parser -----

    #[test]
    fn extract_recognises_attribute_blackbox_flag() {
        let sv = "(* mununu_blackbox *) module foo();\nendmodule";
        let anns = extract_from_sv_source(sv);
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].tag, MununuTag::Blackbox);
        assert_eq!(anns[0].value, "");
        assert_eq!(anns[0].source_line, Some(1));
    }

    #[test]
    fn extract_recognises_attribute_with_quoted_value() {
        let sv = r#"(* mununu_guarantee = "G(req -> ack)" *) module foo(); endmodule"#;
        let anns = extract_from_sv_source(sv);
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].tag, MununuTag::Guarantee);
        assert_eq!(anns[0].value, "G(req -> ack)");
    }

    #[test]
    fn extract_handles_multiple_attributes_in_one_block() {
        let sv = r#"(* mununu_blackbox, mununu_interface = "contract://rtl/x@1" *)"#;
        let anns = extract_from_sv_source(sv);
        assert_eq!(anns.len(), 2);
        assert_eq!(anns[0].tag, MununuTag::Blackbox);
        assert_eq!(anns[1].tag, MununuTag::Interface);
        assert_eq!(anns[1].value, "contract://rtl/x@1");
    }

    #[test]
    fn extract_recognises_line_comment() {
        let sv = "// @mununu_assume G(reset -> idle_within_8_cycles)\nmodule foo();";
        let anns = extract_from_sv_source(sv);
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].tag, MununuTag::Assume);
        assert_eq!(anns[0].value, "G(reset -> idle_within_8_cycles)");
        assert_eq!(anns[0].source_line, Some(1));
    }

    #[test]
    fn extract_recognises_block_comment() {
        let sv = "/* @mununu_controllable reset_n */ module bar();";
        let anns = extract_from_sv_source(sv);
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].tag, MununuTag::Controllable);
        assert_eq!(anns[0].value, "reset_n");
    }

    #[test]
    fn extract_ignores_non_mununu_attributes() {
        let sv = r#"(* synthesis, keep="1" *) module foo();"#;
        assert!(extract_from_sv_source(sv).is_empty());
    }

    #[test]
    fn extract_records_line_numbers_per_annotation() {
        let sv =
            "(* mununu_blackbox *)\nmodule foo(input clk);\n// @mununu_guarantee X\nendmodule\n";
        let anns = extract_from_sv_source(sv);
        assert_eq!(anns.len(), 2);
        assert_eq!(anns[0].source_line, Some(1));
        assert_eq!(anns[1].source_line, Some(3));
    }

    #[test]
    fn extract_handles_unknown_tag_gracefully() {
        let sv = "(* mununu_unknownthing = \"x\" *) module foo();";
        // Unknown tag is silently skipped.
        assert!(extract_from_sv_source(sv).is_empty());
    }

    // ----- yosys attribute parser -----

    #[test]
    fn extract_yosys_blackbox_flag() {
        let attrs = json!({
            "blackbox": "00000000000000000000000000000001",
            "mununu_blackbox": "1",
            "mununu_guarantee": "G(awvalid -> awready)"
        });
        let anns = extract_from_yosys_attributes(&attrs);
        assert_eq!(anns.len(), 2);
        let bb = anns.iter().find(|a| a.tag == MununuTag::Blackbox).unwrap();
        assert_eq!(bb.value, "", "bitstring-only attribute → empty value");
        let g = anns.iter().find(|a| a.tag == MununuTag::Guarantee).unwrap();
        assert_eq!(g.value, "G(awvalid -> awready)");
    }

    #[test]
    fn extract_yosys_ignores_non_mununu_keys() {
        let attrs = json!({
            "src": "foo.sv:10",
            "top": "1"
        });
        assert!(extract_from_yosys_attributes(&attrs).is_empty());
    }

    #[test]
    fn extract_yosys_empty_attributes() {
        let attrs = json!({});
        assert!(extract_from_yosys_attributes(&attrs).is_empty());
    }
}
