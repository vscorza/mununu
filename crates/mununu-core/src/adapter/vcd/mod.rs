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

// ─────────────────────────────────────────────────────────────────────
// R-S6.2 (2026-06-11) — value-change parsing over time
// ─────────────────────────────────────────────────────────────────────

/// A single sampled value at one VCD time marker.
///
/// `Determinate` carries the bit-pattern as `u64` (clipped to the
/// low 64 bits when the signal is wider — the R-S2b
/// `RegisterValuation` contract uses the same shape, so downstream
/// R-S6.5 seeding can lift values one-for-one). `Indeterminate`
/// fires when any bit of the change is `x`/`X`/`z`/`Z` — these
/// values do NOT belong in EnumValues discriminator lists (a
/// predicate cube can't meaningfully refer to an X/Z bit-pattern),
/// so R-S6.3's frequency miner discards them silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcdSampleValue {
    /// All bits are `0` or `1`; bit-pattern encoded as `u64`
    /// (clipped at 64 bits for wider signals).
    Determinate(u64),
    /// At least one bit is `x` / `X` / `z` / `Z`.
    Indeterminate,
}

/// A single value-change record from a VCD trace.
///
/// `id` matches [`VcdSignal::id`] (the VCD signal-id, NOT the
/// signal name); the caller correlates by id to map a change
/// back to the declaration. The R-S6.3 miner builds a
/// `HashMap<id, Vec<u64>>` of per-signal observed values from
/// the change stream this function produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcdChange {
    /// VCD time stamp at which this change happened. The unit is
    /// whatever [`VcdHeader::timescale`] declares.
    pub time: u64,
    /// VCD signal-id (matches [`VcdSignal::id`]).
    pub id: String,
    /// The sampled value.
    pub value: VcdSampleValue,
}

/// R-S6.2 (2026-06-11) — parse the value-change section of a VCD
/// file. Reads the header internally (re-using the underlying
/// [`vcd::Parser`]) and walks every Command until EOF, emitting
/// one `VcdChange` per scalar / vector value change.
///
/// Maintains the current time stamp across `Command::Timestamp`
/// records; every change inherits the most recent stamp. Initial
/// changes that appear before the first `#<time>` marker carry
/// `time = 0` (matches the VCD spec's "initial value" convention).
///
/// **Skipped commands** (irrelevant for bit-vector predicate
/// seeding):
/// - `Command::ChangeReal` — predicate-cube discriminators are
///   bit-vector values, not floats. Real signals are typically
///   `$display` / `$monitor` artefacts.
/// - `Command::ChangeString` — same rationale.
/// - `Command::Begin` / `Command::End` — simulation-command
///   bracketing (`$dumpall`, `$dumpvars`); the contained value
///   changes are parsed individually.
///
/// **Width handling**: vectors wider than 64 bits clip to the
/// low 64 bits silently. The R-S5 / R-S2b / R-Y? family of
/// seeding strategies all use `u64` values, so a wider signal's
/// upper bits can't be carried into the bit-blaster anyway; the
/// clip is information-lossy but downstream-honest. R-S6.3's
/// miner adds a per-signal width annotation so the consumer
/// knows the clip happened.
///
/// # Errors
///
/// Returns `AdapterError { kind: ParseError, ... }` when the
/// underlying [`vcd`] crate rejects the input as malformed
/// (header parse error, mid-trace `Command` decode error).
pub fn parse_vcd_changes(content: &[u8]) -> Result<Vec<VcdChange>, AdapterError> {
    let mut parser = vcd::Parser::new(content);
    parser.parse_header().map_err(|e| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: format!("adapter/vcd: failed to parse VCD header: {e}"),
        location: None,
    })?;

    let mut changes = Vec::new();
    let mut current_time: u64 = 0;

    for cmd in parser {
        let cmd = cmd.map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("adapter/vcd: failed to parse value-change record: {e}"),
            location: None,
        })?;
        match cmd {
            vcd::Command::Timestamp(t) => {
                current_time = t;
            }
            vcd::Command::ChangeScalar(id, value) => {
                changes.push(VcdChange {
                    time: current_time,
                    id: id.to_string(),
                    value: scalar_to_sample_value(value),
                });
            }
            vcd::Command::ChangeVector(id, vector) => {
                changes.push(VcdChange {
                    time: current_time,
                    id: id.to_string(),
                    value: vector_to_sample_value(&vector),
                });
            }
            // ChangeReal / ChangeString / Begin / End — silently
            // skipped. Bit-vector predicate seeding has no use for
            // float / string values, and the Begin/End brackets
            // don't carry value information of their own (the
            // contained changes parse as their own commands).
            _ => {}
        }
    }

    Ok(changes)
}

/// R-S6.2 helper — convert a `vcd::Value` (scalar) to a
/// `VcdSampleValue`. `V0` → 0, `V1` → 1, `X`/`Z` → Indeterminate.
fn scalar_to_sample_value(v: vcd::Value) -> VcdSampleValue {
    match v {
        vcd::Value::V0 => VcdSampleValue::Determinate(0),
        vcd::Value::V1 => VcdSampleValue::Determinate(1),
        vcd::Value::X | vcd::Value::Z => VcdSampleValue::Indeterminate,
    }
}

/// R-S6.2 helper — convert a `vcd::Vector` to a `VcdSampleValue`.
///
/// VCD vectors are stored MSB-first. This helper reads the bits
/// in declaration order (left → right) and shifts them into a
/// `u64` accumulator (MSB-first). Any X/Z bit triggers an early
/// return of `Indeterminate`. Vectors wider than 64 bits clip
/// silently to the low 64 bits.
fn vector_to_sample_value(vector: &vcd::Vector) -> VcdSampleValue {
    let mut accumulator: u64 = 0;
    for bit in vector {
        accumulator = match bit {
            vcd::Value::V0 => accumulator << 1,
            vcd::Value::V1 => (accumulator << 1) | 1,
            vcd::Value::X | vcd::Value::Z => return VcdSampleValue::Indeterminate,
        };
    }
    VcdSampleValue::Determinate(accumulator)
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

    // ─────────────────────────────────────────────────────────────
    // R-S6.2 tests — value-change parsing over time
    // ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_vcd_changes_extracts_two_changes_from_sample() {
        let changes = parse_vcd_changes(sample_vcd()).expect("parse ok");
        // sample_vcd() declares 4 value changes: b0000 ! and 0 " at
        // time 0, then b0001 ! at time 1. (The 0 " is a scalar.)
        // Time 0: 2 changes. Time 1: 1 change. Total 3.
        assert_eq!(changes.len(), 3, "got: {changes:?}");
    }

    #[test]
    fn parse_vcd_changes_preserves_time_progression() {
        let changes = parse_vcd_changes(sample_vcd()).expect("parse ok");
        // The first two changes are at #0; the third is at #1.
        assert_eq!(changes[0].time, 0);
        assert_eq!(changes[1].time, 0);
        assert_eq!(changes[2].time, 1);
    }

    #[test]
    fn parse_vcd_changes_decodes_determinate_vector_value() {
        let changes = parse_vcd_changes(sample_vcd()).expect("parse ok");
        // Time 1, signal !: b0001 → 1.
        let last = &changes[2];
        assert_eq!(last.id, "!");
        assert_eq!(last.value, VcdSampleValue::Determinate(1));
    }

    #[test]
    fn parse_vcd_changes_decodes_scalar_zero_to_determinate() {
        let changes = parse_vcd_changes(sample_vcd()).expect("parse ok");
        // The `0 "` scalar at time 0 must surface as
        // Determinate(0) bound to signal-id `"`.
        let scalar = changes
            .iter()
            .find(|c| c.id == "\"")
            .expect("scalar change for signal \" present");
        assert_eq!(scalar.value, VcdSampleValue::Determinate(0));
    }

    #[test]
    fn parse_vcd_changes_decodes_x_value_to_indeterminate() {
        // A 4-bit vector with `x` in one position must surface as
        // Indeterminate — these are not safe for predicate seeding.
        let trace = b"$timescale 1ns $end\n\
                      $scope module top $end\n\
                      $var wire 4 ! count $end\n\
                      $upscope $end\n\
                      $enddefinitions $end\n\
                      #0\n\
                      b0x10 !\n";
        let changes = parse_vcd_changes(trace).expect("parse ok");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].value, VcdSampleValue::Indeterminate);
    }

    #[test]
    fn parse_vcd_changes_decodes_z_value_to_indeterminate() {
        let trace = b"$timescale 1ns $end\n\
                      $scope module top $end\n\
                      $var wire 4 ! count $end\n\
                      $upscope $end\n\
                      $enddefinitions $end\n\
                      #0\n\
                      b0z10 !\n";
        let changes = parse_vcd_changes(trace).expect("parse ok");
        assert_eq!(changes[0].value, VcdSampleValue::Indeterminate);
    }

    #[test]
    fn parse_vcd_changes_decodes_full_byte_vector() {
        // 8-bit vector b11111111 → 255.
        let trace = b"$timescale 1ns $end\n\
                      $scope module top $end\n\
                      $var wire 8 ! byte $end\n\
                      $upscope $end\n\
                      $enddefinitions $end\n\
                      #0\n\
                      b11111111 !\n";
        let changes = parse_vcd_changes(trace).expect("parse ok");
        assert_eq!(changes[0].value, VcdSampleValue::Determinate(0xFF));
    }

    #[test]
    fn parse_vcd_changes_clips_wide_vector_to_low_64_bits() {
        // 72-bit vector. The clip semantics: as the parser shifts
        // u64 left for each new MSB-first bit, bits beyond position
        // 63 fall off the top. The result is the LAST 64 bits of
        // the original vector — not the first 64.
        //
        // Encoding: 8 leading 1s (which will fall off) followed by
        // 56 zeros and the trailing byte 0x5A. The last 64 bits are
        // "0000...0 (56 zeros) 01011010" = 0x000000000000005A.
        let mut bits = "1".repeat(8); // these 8 bits get shifted off the top
        bits.push_str(&"0".repeat(56));
        bits.push_str("01011010"); // 0x5A
        assert_eq!(bits.len(), 72);
        let trace = format!(
            "$timescale 1ns $end\n\
             $scope module top $end\n\
             $var wire 72 ! wide $end\n\
             $upscope $end\n\
             $enddefinitions $end\n\
             #0\n\
             b{bits} !\n"
        );
        let changes = parse_vcd_changes(trace.as_bytes()).expect("parse ok");
        assert_eq!(
            changes[0].value,
            VcdSampleValue::Determinate(0x5A),
            "wide vector must clip to its low 64 bits (last 64 bits of the vector); got {:?}",
            changes[0].value
        );
    }

    #[test]
    fn parse_vcd_changes_returns_empty_for_no_changes() {
        let header_only = b"$timescale 1ns $end\n\
                            $scope module top $end\n\
                            $var wire 4 ! count $end\n\
                            $upscope $end\n\
                            $enddefinitions $end\n";
        let changes = parse_vcd_changes(header_only).expect("parse ok");
        assert!(changes.is_empty());
    }

    #[test]
    fn parse_vcd_changes_skips_real_and_string_commands() {
        // Mununu does not consume float / string changes — silently
        // skip them. The bit-vector scalar `0 !` must still surface.
        let trace = b"$timescale 1ns $end\n\
                      $scope module top $end\n\
                      $var wire 1 ! enable $end\n\
                      $var real 64 # data_real $end\n\
                      $var string 64 % msg_string $end\n\
                      $upscope $end\n\
                      $enddefinitions $end\n\
                      #0\n\
                      0 !\n\
                      r3.14 #\n\
                      shello %\n";
        let changes = parse_vcd_changes(trace).expect("parse ok");
        // Only the scalar change surfaces.
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].id, "!");
        assert_eq!(changes[0].value, VcdSampleValue::Determinate(0));
    }
}
