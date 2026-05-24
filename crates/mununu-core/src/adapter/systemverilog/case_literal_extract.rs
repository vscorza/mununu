//! R-S3 (§Phase 9 §9.1) — case-literal extraction from SV source.
//!
//! Scans SV `case (<signal>) <LITERAL>: ... [<LITERAL>: ...] endcase`
//! blocks and harvests the numeric-literal labels per switched-on
//! signal. Used as discriminator hints for sidecar widening (the
//! signal's `value_map` gets the extracted literals added).
//!
//! Complementary to R-S5 (typedef widening — handles enum-typed
//! signals) and R-S7 (property-syntactic — handles literals
//! referenced in the property formula). R-S3 covers the gap: signals
//! used as case selectors with numeric literal labels but no typedef
//! declared and not yet referenced by a property.
//!
//! ## Scope (MVP)
//!
//! Handles the common shape:
//!
//! ```sv
//! case (opcode)
//!     3'b001: state <= S1;
//!     3'b010: state <= S2;
//!     3'b101: state <= S3;
//! endcase
//! ```
//!
//! ### Supported patterns
//!
//! - `case`, `casez`, `casex` keywords.
//! - Numeric-literal labels in any SV radix (`3'b001`, `8'd42`, `4'hF`,
//!   plain decimal). Underscores within literals are stripped.
//! - Comma-separated labels: `3'b001, 3'b010:` → both extracted.
//!
//! ### Not yet handled (deferred)
//!
//! - Wildcard literals (`?` / `x` in `casez`/`casex`): the wildcard
//!   does not parse as a single integer; a bit-pattern expansion is
//!   needed. R-S3 silently skips wildcard labels.
//! - Identifier labels (typedef variants like `BOOT_IDLE`): handled by
//!   R-S5 via the typedef walk; R-S3 silently skips them.
//! - `default` branches: handled by R-S5 / Path 3; R-S3 silently
//!   skips them.
//! - Nested case statements inside a case body: the outer regex's
//!   lazy `.*?endcase` matches the first `endcase`, which is correct
//!   for nested cases. Extracted literals from the outer case may
//!   include literals from the inner case's body (over-approximation;
//!   harmless — extra discriminators never reduce verdict precision).
//! - Range labels (`[3:5]`): rare in real RTL; not parsed.
//!
//! ## Native SV parser independence
//!
//! Like [`super::typedef_extract`], implemented as a stand-alone
//! scanner over raw SV text — does NOT depend on the hand-rolled
//! `parser.rs` AST. Survives the §Phase 5 Tier B / Tier C native-
//! parser removal.

use super::typedef_extract::{parse_sv_literal, strip_all_comments};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Extract every `case (<signal>) … endcase` block from SV source
/// text and harvest the numeric literal labels per switched-on
/// signal.
///
/// Returns a map keyed by signal name; each entry is a deduped,
/// sorted vector of literal values that appeared as case-branch
/// labels for that signal across the entire source.
///
/// Returns an empty map when no case blocks are found. Silently
/// skips malformed blocks (best-effort scanner).
pub fn extract_case_literals(source: &str) -> HashMap<String, Vec<u64>> {
    let cleaned = strip_all_comments(source);
    let mut out: HashMap<String, std::collections::BTreeSet<u64>> = HashMap::new();
    for cap in CASE_BLOCK_RE.captures_iter(&cleaned) {
        let signal = cap.name("signal").unwrap().as_str().to_string();
        let body = cap.name("body").unwrap().as_str();
        let entry = out.entry(signal).or_default();
        for label in scan_case_labels(body) {
            entry.insert(label);
        }
    }
    out.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

// Outer regex: `case[zx]? (signal_name) <body> endcase`.
// Body match is lazy (`.*?`) so we stop at the first `endcase`, which
// is correct for nested cases (the inner endcase closes the inner
// scope; outer regex picks up the first one but its body still
// contains the inner labels, which is harmless over-approximation).
static CASE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xs)
            \bcase[zx]?
            \s*\(\s*
            (?P<signal>[A-Za-z_]\w*)
            \s*\)
            (?P<body>.*?)
            \bendcase\b
        ",
    )
    .expect("static case-block regex compiles")
});

// Inner regex: a label-list ending in `:`. Each item in the list is
// either a numeric literal or an identifier (skipped); items are
// comma-separated. The `regex` crate has no lookahead support, so
// matches like `<=` / `>=` / `::` slip through and are filtered in
// the post-processing step (`scan_case_labels` drops items that
// don't parse as a clean numeric literal).
//
// We match label lines with `(?m)` so `^` anchors at line start;
// each line's labels are collected, then the per-line item parser
// in `scan_case_labels` splits comma-lists and filters identifiers
// + wildcards.
static CASE_LABEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?P<labels>[A-Za-z0-9_'?,xzXZ][A-Za-z0-9_'?,xzXZ \t]*?)[ \t]*:")
        .expect("static case-label regex compiles")
});

fn scan_case_labels(body: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for cap in CASE_LABEL_RE.captures_iter(body) {
        let labels_text = cap.name("labels").unwrap().as_str();
        for item in labels_text.split(',') {
            let trimmed = item.trim();
            // Skip identifier labels (typedef variants) and `default`.
            if trimmed.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                continue;
            }
            // Skip labels containing wildcards (`?`, `x`, `z`); they
            // are bit-patterns, not single integers — a follow-up
            // could enumerate the matching values.
            if trimmed.contains('?')
                || trimmed.contains('x')
                || trimmed.contains('z')
                || trimmed.contains('X')
                || trimmed.contains('Z')
            {
                continue;
            }
            if let Some(v) = parse_sv_literal(trimmed) {
                out.push(v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_case_block_binary_labels() {
        let source = r#"
            module m;
                always_comb case (opcode)
                    3'b001: state <= S1;
                    3'b010: state <= S2;
                    3'b101: state <= S3;
                endcase
            endmodule
        "#;
        let map = extract_case_literals(source);
        let opcode = map.get("opcode").expect("opcode should be extracted");
        assert_eq!(opcode, &vec![1, 2, 5]);
    }

    #[test]
    fn extract_decimal_and_hex_labels() {
        let source = r#"
            case (mode)
                4'd1: x <= 1;
                4'd7: x <= 2;
                4'hF: x <= 3;
            endcase
        "#;
        let map = extract_case_literals(source);
        let mode = map.get("mode").unwrap();
        assert_eq!(mode, &vec![1, 7, 15]);
    }

    #[test]
    fn extract_plain_decimal_labels() {
        let source = r#"
            case (n)
                0: f <= 0;
                1: f <= 1;
                2: f <= 1;
                3: f <= 2;
            endcase
        "#;
        let map = extract_case_literals(source);
        assert_eq!(map.get("n"), Some(&vec![0, 1, 2, 3]));
    }

    #[test]
    fn comma_separated_labels_are_extracted() {
        let source = r#"
            case (opcode)
                3'b001, 3'b010: state <= S1;
                3'b101: state <= S2;
            endcase
        "#;
        let map = extract_case_literals(source);
        assert_eq!(map.get("opcode"), Some(&vec![1, 2, 5]));
    }

    #[test]
    fn identifier_labels_are_skipped_typedef_handled_by_r_s5() {
        let source = r#"
            case (boot_fsm_ps)
                BOOT_IDLE: arc_to_fuse <= 1;
                BOOT_FUSE: arc_to_done <= 1;
                BOOT_DONE: arc_to_idle <= 1;
                default: ;
            endcase
        "#;
        let map = extract_case_literals(source);
        // Identifier labels are typedef variants; R-S5 handles them.
        // R-S3 returns no literals because no numeric labels appeared.
        assert!(map.get("boot_fsm_ps").map(|v| v.is_empty()).unwrap_or(true));
    }

    #[test]
    fn mixed_numeric_and_identifier_labels() {
        let source = r#"
            case (sel)
                3'b001: x <= A;
                MAGIC: x <= B;
                3'b111: x <= C;
            endcase
        "#;
        let map = extract_case_literals(source);
        assert_eq!(map.get("sel"), Some(&vec![1, 7]));
    }

    #[test]
    fn casez_and_casex_keywords_are_extracted() {
        let source = r#"
            casez (req)
                3'b001: gnt <= 1;
            endcase
            casex (priority_in)
                3'b010: priority_out <= 0;
            endcase
        "#;
        let map = extract_case_literals(source);
        assert_eq!(map.get("req"), Some(&vec![1]));
        assert_eq!(map.get("priority_in"), Some(&vec![2]));
    }

    #[test]
    fn wildcard_labels_are_skipped() {
        let source = r#"
            casez (req)
                3'b1??: any_high <= 1;
                3'b001: only_one <= 1;
            endcase
        "#;
        let map = extract_case_literals(source);
        // Wildcards are skipped; only the concrete literal 3'b001 → 1.
        assert_eq!(map.get("req"), Some(&vec![1]));
    }

    #[test]
    fn multiple_case_blocks_merge_per_signal() {
        let source = r#"
            always_comb case (opcode)
                3'b001: x <= 1;
            endcase
            always_comb case (opcode)
                3'b010: y <= 2;
            endcase
        "#;
        let map = extract_case_literals(source);
        assert_eq!(map.get("opcode"), Some(&vec![1, 2]));
    }

    #[test]
    fn no_case_block_returns_empty_map() {
        let source = "module m; always_comb x <= y; endmodule";
        let map = extract_case_literals(source);
        assert!(map.is_empty());
    }

    #[test]
    fn comments_inside_case_body_do_not_break_scan() {
        let source = r#"
            case (opcode)
                3'b001: x <= 1;  // first
                3'b010: x <= 2;  /* second */
                3'b011: x <= 3;
            endcase
        "#;
        let map = extract_case_literals(source);
        assert_eq!(map.get("opcode"), Some(&vec![1, 2, 3]));
    }
}
