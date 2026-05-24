//! R-S5 (§Phase 9 §9.1) — Type-driven valuation extraction from SV typedef enums.
//!
//! Walks SV source text for `typedef enum logic [W:0] { … } TYPE_NAME;`
//! declarations, extracting per-variant value bindings and the encoded
//! bit-width. The §Phase 9 §9.5 critical-path follow-up to R-Y2: the
//! per-signal anyconst init policy needs the abstraction set to include
//! *all* encodings the type's bit-width admits (named + unmatched), or
//! the abstraction layer drops bug-bearing transitions before R-Y2's
//! init nondeterminism can influence the verdict (the post-R-Y2 §Phase 8
//! §8.2 bottleneck Path 1 closed manually by hand-widening the Caliptra
//! sidecar's `boot_fsm_ns` from `discover` to an explicit 8-variant enum).
//!
//! This module automates the same widening: given the SV typedef
//! declaring 5 named variants in a 3-bit width, it emits the 5 named
//! variants *plus* the 3 unmatched encodings `{UNMATCHED_5,
//! UNMATCHED_6, UNMATCHED_7}` so the abstraction layer keeps them
//! in the abstract relation.
//!
//! ## Scope
//!
//! MVP for §Phase 9 §9.5. Handles the typedef-enum extraction itself
//! (regex-based scanner over SV source text); does NOT yet integrate
//! with the loader to auto-fill sidecar `signals[].abstraction` from
//! the extraction. The integration point is a follow-up (depends on a
//! signal-type discovery mechanism — sidecar field, Yosys
//! `hierarchy.json` walk, or SV source scan for `<TypeName> <signal>;`
//! declarations).
//!
//! ## Native SV parser independence
//!
//! Deliberately implemented as a stand-alone scanner rather than as an
//! extension of [`super::ast::Declaration::Enum`] so R-S5 survives the
//! native-SV parser's Tier B / Tier C removal (per the plan's
//! singular-pipeline commitment). The scanner reads raw SV text; it
//! doesn't depend on the hand-rolled `parser.rs` AST.

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// A typedef-enum declaration extracted from SV source.
///
/// Carries the type name, encoded bit-width, named variants with
/// explicit numeric values, and the unmatched encodings the bit-width
/// admits beyond the named set. The unmatched encodings are
/// load-bearing for CWE-1245-style detection: they represent the
/// FSM state encodings that designers did NOT enumerate in the case
/// statement, which is exactly where `unique casez` without a default
/// branch can fire on undefined behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedefEnum {
    /// The typedef name (e.g. `boot_fsm_state_e`).
    pub type_name: String,
    /// The bit-width from the `logic [W:0]` modifier. `W+1` bits → 2^(W+1) admissible values.
    pub width: u32,
    /// Named variants with their explicit numeric values, in declaration order.
    /// E.g. `[("BOOT_IDLE", 0), ("BOOT_FUSE", 1), ("BOOT_FW_RST", 2), ("BOOT_WAIT", 3), ("BOOT_DONE", 4)]`.
    pub variants: Vec<(String, u64)>,
    /// Encodings admissible by the bit-width but NOT in the named-variant set.
    /// E.g. for `boot_fsm_state_e` (width=3, named={0,1,2,3,4}): `[5, 6, 7]`.
    pub unmatched_encodings: Vec<u64>,
}

impl TypedefEnum {
    /// Total number of admissible encodings under the bit-width (2^width).
    pub fn total_encodings(&self) -> u64 {
        1u64 << self.width
    }

    /// Emit `(variant_name, value)` pairs for *all* admissible encodings,
    /// including unmatched ones (named with the `UNMATCHED_<n>` convention).
    /// Used to construct a complete abstraction set for the sidecar.
    ///
    /// Note: leading underscores were intentionally dropped from the
    /// synthetic variant name — predicate-name resolution at the
    /// mu-calculus parser layer composes the predicate as
    /// `<signal>_<variant>`, which produces *triple* underscores if
    /// the variant starts with `__` (e.g. `boot_fsm_ns_UNMATCHED_5`).
    /// Those names fail to resolve in the realizer (silent default-to-
    /// false), causing properties referencing them to evaluate
    /// vacuously. `UNMATCHED_<n>` produces clean `<signal>_UNMATCHED_<n>`
    /// predicates that resolve.
    pub fn all_encodings(&self) -> Vec<(String, u64)> {
        let mut out = self.variants.clone();
        for &v in &self.unmatched_encodings {
            out.push((format!("UNMATCHED_{v}"), v));
        }
        out.sort_by_key(|(_, v)| *v);
        out
    }
}

/// Extract every typedef-enum declaration from SV source text.
///
/// Returns a map keyed by typedef name. Handles:
///
/// - Explicit width: `typedef enum logic [2:0] { … } name;`
/// - Implicit width (defaults to 32): `typedef enum { … } name;` — emitted with `width=32`.
/// - Per-variant explicit values: `BOOT_IDLE = 3'b000` / `= 3'd0` / `= 3'h0` / `= 0`.
/// - Implicit per-variant values: variants without `=` get the next sequential index.
///
/// Returns an empty map if no typedef enums are found. Silently skips
/// malformed declarations (best-effort scanner).
pub fn extract_typedef_enums(source: &str) -> HashMap<String, TypedefEnum> {
    let mut out = HashMap::new();
    for td in find_typedef_enum_blocks(source) {
        out.insert(td.type_name.clone(), td);
    }
    out
}

// The regex matches `typedef enum [optional `logic`/`bit`] [optional [W:0]]
// { body } name;`. The body and name are captured for further parsing.
static TYPEDEF_ENUM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xs)
            typedef\s+enum
            (?:\s+(?:logic|bit|reg))?           # optional base type
            (?:\s*\[\s*(?P<msb>\d+)\s*:\s*(?P<lsb>\d+)\s*\])?   # optional width
            \s*\{
                (?P<body>[^}]*)
            \}
            \s*(?P<name>[A-Za-z_]\w*)
            \s*;
        ",
    )
    .expect("static typedef-enum regex compiles")
});

fn find_typedef_enum_blocks(source: &str) -> Vec<TypedefEnum> {
    // Strip comments from the whole source FIRST so commas inside comments
    // don't break the comma-split in `parse_variant_body`. Block comments
    // are stripped too — SV allows `/* ... */` anywhere whitespace is
    // legal, including inside enum bodies.
    let cleaned = strip_all_comments(source);
    let mut out = Vec::new();
    for cap in TYPEDEF_ENUM_RE.captures_iter(&cleaned) {
        let name = cap.name("name").unwrap().as_str().to_string();
        let body = cap.name("body").unwrap().as_str();
        let width = match (cap.name("msb"), cap.name("lsb")) {
            (Some(msb), Some(lsb)) => {
                let m: u32 = msb.as_str().parse().unwrap_or(0);
                let l: u32 = lsb.as_str().parse().unwrap_or(0);
                m.max(l) - m.min(l) + 1
            }
            _ => 32, // default SV int width when none declared
        };
        let variants = parse_variant_body(body);
        let unmatched = compute_unmatched(&variants, width);
        out.push(TypedefEnum {
            type_name: name,
            width,
            variants,
            unmatched_encodings: unmatched,
        });
    }
    out
}

/// Strip SV line comments (`// …`) and block comments (`/* … */`) from
/// source text. Preserves whitespace and newlines so line/column
/// positions stay roughly aligned for error messages. Naive scanner —
/// does NOT account for `//` or `/*` inside string literals (rare in
/// typedef contexts).
///
/// Visibility: `pub(super)` so R-S3's `case_literal_extract` module
/// reuses this helper rather than duplicating the byte-walk.
pub(super) fn strip_all_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Line comment — skip to end of line, keep the newline.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Block comment — skip to closing `*/`.
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2; // skip the closing `*/`
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Parse the variant body of a typedef enum (the text between `{` and `}`).
/// Splits on commas, then for each item extracts `NAME` and optional `= VALUE`.
/// Variants without `= VALUE` get the sequential index (starting from the
/// last explicit value + 1, or 0 if none have been seen yet).
fn parse_variant_body(body: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut next_implicit: u64 = 0;

    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // Split on '=' to find the explicit value
        let (name_part, value_part) = match item.find('=') {
            Some(idx) => (item[..idx].trim(), Some(item[idx + 1..].trim())),
            None => (item, None),
        };
        // The name is the first identifier token in name_part
        let name = name_part
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let value = match value_part {
            Some(v) => parse_sv_literal(v).unwrap_or(next_implicit),
            None => next_implicit,
        };
        out.push((name, value));
        next_implicit = value + 1;
    }
    out
}

/// Parse a SystemVerilog integer literal:
///
/// - `<width>'b<binary>` — binary, e.g. `3'b101` → 5
/// - `<width>'d<decimal>` — decimal, e.g. `8'd42` → 42
/// - `<width>'h<hex>` — hex, e.g. `4'hF` → 15
/// - `<width>'o<octal>` — octal, e.g. `3'o7` → 7
/// - `<decimal>` — plain decimal, e.g. `42` → 42
///
/// Underscores within numeric literals are stripped (SV allows
/// `8'b1010_0101` for readability).
///
/// Visibility: `pub(super)` so R-S3's `case_literal_extract` module
/// reuses this parser rather than duplicating the radix-handling.
pub(super) fn parse_sv_literal(s: &str) -> Option<u64> {
    let s = s.trim().replace('_', "");
    // Split on apostrophe to find base specifier
    if let Some(apos_idx) = s.find('\'') {
        let after_apos = &s[apos_idx + 1..];
        if after_apos.is_empty() {
            return None;
        }
        let base_char = after_apos.chars().next().unwrap().to_ascii_lowercase();
        let digits = &after_apos[1..];
        match base_char {
            'b' => u64::from_str_radix(digits, 2).ok(),
            'd' => digits.parse::<u64>().ok(),
            'h' => u64::from_str_radix(digits, 16).ok(),
            'o' => u64::from_str_radix(digits, 8).ok(),
            _ => None,
        }
    } else {
        s.parse::<u64>().ok()
    }
}

/// Compute the encodings admissible by `width` but not in the named-variant set.
/// Returns the list in ascending order.
fn compute_unmatched(variants: &[(String, u64)], width: u32) -> Vec<u64> {
    let total = if width >= 64 {
        return Vec::new(); // bail on widths we can't represent
    } else {
        1u64 << width
    };
    let named: std::collections::BTreeSet<u64> = variants.iter().map(|(_, v)| *v).collect();
    let mut out = Vec::new();
    for v in 0..total {
        if !named.contains(&v) {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caliptra_boot_fsm_state_e_extraction() {
        // The actual typedef from examples/verify/sv_yosys_caliptra_rtl_150/source/soc_ifc_pkg.sv
        let source = r#"
            //BOOT FSM
            typedef enum logic [2:0] {
                BOOT_IDLE   = 3'b000,
                BOOT_FUSE   = 3'b001,
                BOOT_FW_RST = 3'b010,
                BOOT_WAIT   = 3'b011,
                BOOT_DONE   = 3'b100
            } boot_fsm_state_e;
        "#;
        let map = extract_typedef_enums(source);
        let td = map
            .get("boot_fsm_state_e")
            .expect("boot_fsm_state_e must be found");
        assert_eq!(td.width, 3);
        assert_eq!(td.total_encodings(), 8);
        assert_eq!(
            td.variants,
            vec![
                ("BOOT_IDLE".to_string(), 0),
                ("BOOT_FUSE".to_string(), 1),
                ("BOOT_FW_RST".to_string(), 2),
                ("BOOT_WAIT".to_string(), 3),
                ("BOOT_DONE".to_string(), 4),
            ]
        );
        // This is the load-bearing assertion: the 3 unmatched encodings {5, 6, 7}
        // are exactly the bug-bearing encodings the Caliptra CWE-1245 fixture
        // exhibits. R-S5 surfaces them automatically; Path 1 surfaced them
        // by hand-editing the sidecar to enum {E0..E7_BUG}.
        assert_eq!(td.unmatched_encodings, vec![5, 6, 7]);
    }

    #[test]
    fn caliptra_boot_fsm_all_encodings_round_trip() {
        let source = r#"
            typedef enum logic [2:0] {
                BOOT_IDLE   = 3'b000,
                BOOT_FUSE   = 3'b001,
                BOOT_FW_RST = 3'b010,
                BOOT_WAIT   = 3'b011,
                BOOT_DONE   = 3'b100
            } boot_fsm_state_e;
        "#;
        let map = extract_typedef_enums(source);
        let td = map.get("boot_fsm_state_e").unwrap();
        let all = td.all_encodings();
        assert_eq!(all.len(), 8);
        // Sorted by value, named variants keep their declared name, unmatched
        // get the synthetic UNMATCHED_<n> name.
        assert_eq!(all[0], ("BOOT_IDLE".to_string(), 0));
        assert_eq!(all[4], ("BOOT_DONE".to_string(), 4));
        assert_eq!(all[5], ("UNMATCHED_5".to_string(), 5));
        assert_eq!(all[6], ("UNMATCHED_6".to_string(), 6));
        assert_eq!(all[7], ("UNMATCHED_7".to_string(), 7));
    }

    #[test]
    fn implicit_variant_values_increment() {
        let source = r#"
            typedef enum logic [1:0] {
                IDLE,
                WAIT,
                DONE
            } simple_state_t;
        "#;
        let map = extract_typedef_enums(source);
        let td = map.get("simple_state_t").unwrap();
        assert_eq!(td.width, 2);
        assert_eq!(td.total_encodings(), 4);
        assert_eq!(
            td.variants,
            vec![
                ("IDLE".to_string(), 0),
                ("WAIT".to_string(), 1),
                ("DONE".to_string(), 2),
            ]
        );
        // One unmatched encoding: {3}
        assert_eq!(td.unmatched_encodings, vec![3]);
    }

    #[test]
    fn mixed_explicit_and_implicit_variant_values() {
        let source = r#"
            typedef enum logic [2:0] {
                A,           // 0
                B,           // 1
                C = 3'd4,    // 4
                D            // 5 (implicit, follows last explicit)
            } mixed_state_t;
        "#;
        let map = extract_typedef_enums(source);
        let td = map.get("mixed_state_t").unwrap();
        assert_eq!(
            td.variants,
            vec![
                ("A".to_string(), 0),
                ("B".to_string(), 1),
                ("C".to_string(), 4),
                ("D".to_string(), 5),
            ]
        );
        // Unmatched: {2, 3, 6, 7}
        assert_eq!(td.unmatched_encodings, vec![2, 3, 6, 7]);
    }

    #[test]
    fn sparse_encoding_unmatched_set() {
        // The Caliptra mbox_fsm_state_e pattern — sparse mapping, both directions of unmatched.
        let source = r#"
            typedef enum logic [2:0] {
                MBOX_IDLE         = 3'b000,
                MBOX_RDY_FOR_CMD  = 3'b001,
                MBOX_RDY_FOR_DLEN = 3'b011,
                MBOX_RDY_FOR_DATA = 3'b010,
                MBOX_EXECUTE_UC   = 3'b110,
                MBOX_EXECUTE_SOC  = 3'b100,
                MBOX_ERROR        = 3'b111
            } mbox_fsm_state_e;
        "#;
        let map = extract_typedef_enums(source);
        let td = map.get("mbox_fsm_state_e").unwrap();
        assert_eq!(td.width, 3);
        // Named values: {0, 1, 2, 3, 4, 6, 7} → unmatched: {5}
        assert_eq!(td.unmatched_encodings, vec![5]);
    }

    #[test]
    fn multiple_typedefs_in_one_source() {
        let source = r#"
            typedef enum logic [1:0] { A, B } first_t;
            typedef enum logic [2:0] { X = 0, Y = 7 } second_t;
        "#;
        let map = extract_typedef_enums(source);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("first_t").unwrap().width, 2);
        assert_eq!(map.get("second_t").unwrap().width, 3);
        assert_eq!(
            map.get("second_t").unwrap().unmatched_encodings,
            vec![1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn hex_and_decimal_literals() {
        let source = r#"
            typedef enum logic [3:0] {
                LOW    = 4'h0,
                MID    = 4'd7,
                HIGH   = 4'hF
            } range_t;
        "#;
        let map = extract_typedef_enums(source);
        let td = map.get("range_t").unwrap();
        assert_eq!(
            td.variants,
            vec![
                ("LOW".to_string(), 0),
                ("MID".to_string(), 7),
                ("HIGH".to_string(), 15),
            ]
        );
    }

    #[test]
    fn underscore_in_numeric_literal() {
        let source = r#"
            typedef enum logic [7:0] {
                SMALL = 8'b0000_0001,
                BIG   = 8'b1010_0101
            } byte_t;
        "#;
        let map = extract_typedef_enums(source);
        let td = map.get("byte_t").unwrap();
        assert_eq!(td.variants[0], ("SMALL".to_string(), 1));
        assert_eq!(td.variants[1], ("BIG".to_string(), 0b1010_0101));
    }

    #[test]
    fn no_typedef_returns_empty_map() {
        let source = "module m; endmodule";
        let map = extract_typedef_enums(source);
        assert!(map.is_empty());
    }

    #[test]
    fn integration_caliptra_pkg_file() {
        // Round-trip the actual Caliptra package file. Loads from disk
        // (the workspace ships the fixture) and verifies all five
        // declared typedef enums are extracted with the expected
        // unmatched-encoding sets. This is the load-bearing
        // integration check for §Phase 9 §9.5 Path 2.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/verify/sv_yosys_caliptra_rtl_150/source/soc_ifc_pkg.sv");
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!(
                    "skipping: Caliptra fixture not present at {}",
                    path.display()
                );
                return;
            }
        };
        let map = extract_typedef_enums(&source);
        // The header section of soc_ifc_pkg.sv declares 5 typedefs:
        // boot_fsm_state_e, mbox_fsm_state_e, sha_fsm_state_e, plus
        // two more 4-bit / 2-bit ones further down. Check at least
        // the load-bearing FSM ones.
        let boot = map
            .get("boot_fsm_state_e")
            .expect("boot_fsm_state_e must be extracted");
        assert_eq!(boot.width, 3);
        assert_eq!(boot.variants.len(), 5);
        assert_eq!(
            boot.unmatched_encodings,
            vec![5, 6, 7],
            "the 3 bug-bearing encodings the CWE-1245 fixture exhibits"
        );
    }

    #[test]
    fn parse_sv_literal_round_trips() {
        assert_eq!(parse_sv_literal("3'b101"), Some(5));
        assert_eq!(parse_sv_literal("8'd42"), Some(42));
        assert_eq!(parse_sv_literal("4'hF"), Some(15));
        assert_eq!(parse_sv_literal("3'o7"), Some(7));
        assert_eq!(parse_sv_literal("42"), Some(42));
        assert_eq!(parse_sv_literal("8'b1010_0101"), Some(0b1010_0101));
        assert_eq!(parse_sv_literal(""), None);
    }
}
