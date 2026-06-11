//! R-S6 (Phase 9 §9.1) sub-item R-S6.1 (2026-06-11) — VCD
//! (Value Change Dump) trace parser skeleton + header extraction.
//!
//! # What this module does (R-S6.1 scope)
//!
//! Parses the **header** of an IEEE-1364 VCD file and returns a
//! structured `VcdHeader` listing the scope hierarchy + every
//! declared signal (name, width, signal-id). Wraps the
//! third-party [`vcd`] crate (canonical Rust VCD parser per
//! CLAUDE.md "Code Reuse"); the wrapper enforces mununu's
//! `AdapterError` convention so callers don't need to depend on
//! `vcd::Error` directly.
//!
//! # What this module does NOT do yet
//!
//! R-S6 is a multi-session arc; R-S6.1 ships ONLY the header
//! layer. Subsequent sub-items will add:
//!
//! - R-S6.2 — value-change parsing over time. Walks `#<time>`
//!   markers + value-change records; emits a `Vec<VcdChange>`
//!   the miner can consume.
//! - R-S6.3 — value-frequency miner (heavy-hitters per signal,
//!   boundary values, reserve set). Pure helper.
//! - R-S6.4 — sidecar `vcd_traces: Option<Vec<VcdTraceConfig>>`
//!   field. Lists trace-file paths + per-signal sampling
//!   policy.
//! - R-S6.5 — bit-blaster seeding helper
//!   `apply_vcd_seeding(valuations, …)` analogous to R-S2b.4's
//!   `apply_reset_simulation_seeding`. Feeds mined values into
//!   `EnumValues` discriminator lists.
//! - R-S6.6 — CLI flag + `translate()` orchestration site
//!   (mirrors R-S2b.6). Closes the R-S6 arc.
//!
//! # Why VCD
//!
//! Industrial design teams already maintain regression suites
//! that produce VCD traces routinely (Verilator's `--trace`,
//! VCS's `+vcs+dumpvars`, Icarus's `$dumpvars`). R-S6 mines those
//! traces for free — no new simulation pass, no extra tooling
//! beyond the parser. When the trace covers reset + idle +
//! steady-state behaviour, the heavy-hitter values become
//! sensible predicate-cube discriminators.
//!
//! # VCD format primer
//!
//! ```text
//! $date Sat Jan 1 00:00:00 2000 $end
//! $version Verilator 5.018 $end
//! $timescale 1ps $end
//! $scope module top $end
//! $var wire 4 ! count $end
//! $var wire 1 " enable $end
//! $upscope $end
//! $enddefinitions $end
//! #0
//! b0000 !
//! 0 "
//! #1
//! b0001 !
//! ```
//!
//! The header runs from the start of file to `$enddefinitions`;
//! after that come time markers (`#<time>`) and value changes
//! (`<value> <signal-id>` or `b<bits> <signal-id>`).

use crate::adapter::{AdapterError, AdapterErrorKind};

/// A signal declared in a VCD header. Stable across the entire
/// trace — value-change records reference the `id` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcdSignal {
    /// Signal name as declared in `$var wire <width> <id> <name> $end`.
    /// The fully-qualified hierarchical path (e.g.
    /// `top.uart_tx.count`) is in `path`; this field carries only
    /// the leaf name.
    pub name: String,
    /// Full hierarchical path from the top scope down to this
    /// signal, joined with `.`. Example: `top.uart_tx.count`.
    pub path: String,
    /// Bit-width as declared. Wires of width `> 1` carry binary
    /// or signed-bin or hex value strings in the change records;
    /// width-1 wires carry a single `0`/`1`/`x`/`z` character.
    pub width: u32,
    /// VCD signal-id (the second token in the `$var` line). This
    /// is what value-change records reference. The id is an
    /// arbitrary printable ASCII string the simulator chose for
    /// compactness — not the signal name.
    pub id: String,
}

/// Parsed VCD header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VcdHeader {
    /// Optional `$timescale` value (e.g. `"1ps"`, `"100ns"`). May
    /// be absent in malformed / hand-edited traces.
    pub timescale: Option<String>,
    /// Optional `$version` string from the source simulator (e.g.
    /// `"Verilator 5.018"`).
    pub version: Option<String>,
    /// Optional `$date` string. Useful for diagnostic logging; not
    /// load-bearing for any analysis.
    pub date: Option<String>,
    /// Every signal declared in the trace, in declaration order.
    /// The R-S6.3 miner uses this list to drive per-signal value
    /// extraction; R-S6.5 uses the leaf `name` to match against
    /// BTOR2 state cells (mirrors R-S2b.4's name-based lookup).
    pub signals: Vec<VcdSignal>,
}

/// R-S6.1 (2026-06-11) — parse the header of a VCD file from a
/// byte slice. Reads up to `$enddefinitions $end` and returns
/// the populated `VcdHeader`. Value-change records (R-S6.2 scope)
/// are NOT consumed by this function.
///
/// # Errors
///
/// Returns `AdapterError { kind: ParseError, ... }` when the
/// underlying [`vcd`] crate rejects the input as malformed (e.g.
/// missing `$enddefinitions`, unbalanced `$scope`/`$upscope`,
/// non-decimal width in `$var`, EOF inside a command).
///
/// # Signal naming convention
///
/// VCD declares signals nested under `$scope module <name>`
/// blocks. The hierarchical path stored in `VcdSignal::path` is
/// built by joining every enclosing scope name with `.`, in
/// outer-to-inner order. For example a `count` signal declared
/// under `$scope module top` → `$scope module uart_tx` resolves
/// to `top.uart_tx.count`. The leaf `name` field stays the bare
/// `count` (matches the SV declaration).
///
/// # Implementation note
///
/// Uses [`vcd::Parser::new`] + a single pass through
/// `parse_header`. The crate exposes the `Scope` + `Var` + `Header`
/// AST nodes directly; this wrapper flattens them into mununu's
/// preferred `Vec<VcdSignal>` shape.
pub fn parse_vcd_header(content: &[u8]) -> Result<VcdHeader, AdapterError> {
    let mut parser = vcd::Parser::new(content);
    let header = parser.parse_header().map_err(|e| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: format!("adapter/vcd: failed to parse VCD header: {e}"),
        location: None,
    })?;

    let mut signals = Vec::new();
    collect_signals(&header.items, &mut Vec::new(), &mut signals);

    let timescale = header
        .timescale
        .map(|(value, unit)| format!("{value}{unit}"));
    let version = header.version;
    let date = header.date;

    Ok(VcdHeader {
        timescale,
        version,
        date,
        signals,
    })
}

/// Walk every `vcd::ScopeItem` recursively + flatten declared
/// variables into `VcdSignal`s, joining the enclosing scope
/// path. Pure helper.
fn collect_signals(items: &[vcd::ScopeItem], path: &mut Vec<String>, out: &mut Vec<VcdSignal>) {
    for item in items {
        match item {
            vcd::ScopeItem::Scope(scope) => {
                path.push(scope.identifier.clone());
                collect_signals(&scope.items, path, out);
                path.pop();
            }
            vcd::ScopeItem::Var(var) => {
                let leaf = var.reference.clone();
                let mut full = path.clone();
                full.push(leaf.clone());
                let path_str = full.join(".");
                out.push(VcdSignal {
                    name: leaf,
                    path: path_str,
                    width: var.size,
                    id: var.code.to_string(),
                });
            }
            _ => {
                // Future VCD extensions (e.g. `$comment` blocks,
                // typedef-aware scopes from SystemVerilog tools)
                // are skipped silently; R-S6.1 does not depend on
                // them.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vcd() -> &'static [u8] {
        b"$date Sat Jan 1 00:00:00 2000 $end\n\
          $version Verilator 5.018 $end\n\
          $timescale 1ps $end\n\
          $scope module top $end\n\
          $var wire 4 ! count $end\n\
          $var wire 1 \" enable $end\n\
          $upscope $end\n\
          $enddefinitions $end\n\
          #0\n\
          b0000 !\n\
          0 \"\n\
          #1\n\
          b0001 !\n"
    }

    #[test]
    fn parse_vcd_header_extracts_two_signals() {
        let h = parse_vcd_header(sample_vcd()).expect("parse ok");
        assert_eq!(h.signals.len(), 2);
        assert_eq!(h.signals[0].name, "count");
        assert_eq!(h.signals[0].path, "top.count");
        assert_eq!(h.signals[0].width, 4);
        assert_eq!(h.signals[1].name, "enable");
        assert_eq!(h.signals[1].path, "top.enable");
        assert_eq!(h.signals[1].width, 1);
    }

    #[test]
    fn parse_vcd_header_extracts_signal_ids() {
        // IDs are arbitrary printable ASCII; the parser must
        // surface them so R-S6.2's change parser can correlate.
        let h = parse_vcd_header(sample_vcd()).expect("parse ok");
        assert_eq!(h.signals[0].id, "!");
        assert_eq!(h.signals[1].id, "\"");
    }

    #[test]
    fn parse_vcd_header_captures_timescale() {
        let h = parse_vcd_header(sample_vcd()).expect("parse ok");
        // The `vcd` crate formats the unit lowercase.
        assert_eq!(h.timescale.as_deref(), Some("1ps"));
    }

    #[test]
    fn parse_vcd_header_captures_version_and_date() {
        let h = parse_vcd_header(sample_vcd()).expect("parse ok");
        assert!(
            h.version.as_deref().unwrap().contains("Verilator"),
            "expected version to mention Verilator, got {:?}",
            h.version
        );
        assert!(
            h.date.as_deref().unwrap().contains("2000"),
            "expected date to mention 2000, got {:?}",
            h.date
        );
    }

    #[test]
    fn parse_vcd_header_handles_nested_scope_hierarchy() {
        let nested = b"$timescale 1ns $end\n\
                       $scope module top $end\n\
                       $scope module uart_tx $end\n\
                       $var wire 4 ! count $end\n\
                       $upscope $end\n\
                       $upscope $end\n\
                       $enddefinitions $end\n";
        let h = parse_vcd_header(nested).expect("parse ok");
        assert_eq!(h.signals.len(), 1);
        // Hierarchical path includes both scope names.
        assert_eq!(h.signals[0].path, "top.uart_tx.count");
        // Leaf name remains the bare signal name.
        assert_eq!(h.signals[0].name, "count");
    }

    #[test]
    fn parse_vcd_header_returns_error_on_malformed_input() {
        // Missing `$enddefinitions` — the parser will surface a
        // structured error, not panic.
        let bad = b"$scope module top $end\n$var wire 4 ! count $end\n";
        let err = parse_vcd_header(bad).expect_err("malformed input must error");
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
        assert!(
            err.message.contains("vcd") || err.message.contains("VCD"),
            "error must mention vcd; got: {}",
            err.message
        );
    }

    #[test]
    fn parse_vcd_header_preserves_signal_declaration_order() {
        let three_signals = b"$timescale 1ns $end\n\
                              $scope module top $end\n\
                              $var wire 8 a zzz $end\n\
                              $var wire 4 b aaa $end\n\
                              $var wire 2 c mmm $end\n\
                              $upscope $end\n\
                              $enddefinitions $end\n";
        let h = parse_vcd_header(three_signals).expect("parse ok");
        assert_eq!(h.signals.len(), 3);
        // Declaration order preserved — important because R-S6.3's
        // miner correlates per-signal frequency lists with this
        // index.
        assert_eq!(h.signals[0].name, "zzz");
        assert_eq!(h.signals[1].name, "aaa");
        assert_eq!(h.signals[2].name, "mmm");
    }
}
