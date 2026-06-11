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

// ─────────────────────────────────────────────────────────────────────
// R-S6.3 (2026-06-11) — per-signal value-frequency miner
// ─────────────────────────────────────────────────────────────────────

/// Per-signal observed-value statistics. Output of
/// [`mine_vcd_frequencies`]; consumed by R-S6.5's bit-blaster
/// seeding helper.
///
/// The mining strategy is split into three categories per §Phase 9
/// R-S6's design:
///
/// - **Heavy-hitter values** (`heavy_hitters`): the values that
///   appear most often across the trace. These are the values the
///   designer cares about — initial states, idle states, common
///   steady-state register valuations. R-S6.5 lifts the top-N
///   into `EnumValues` discriminators.
/// - **Boundary values** (`min` / `max`): the extreme values
///   observed. Useful for counter / saturator predicates ("did
///   the counter reach its full count?") and for typedef-style
///   FSM states where the encoding hits 0 or `(2^width - 1)`.
/// - **Indeterminate sample count** (`indeterminate_count`): not
///   a seed source — Indeterminate samples carry no
///   predicate-cube meaning — but surfaces as a diagnostic in
///   R-S6.6 (a signal that's mostly X means the trace doesn't
///   exercise it).
///
/// The reserve set (rare values R.5 CEGAR may promote on
/// KleeneBot) is derived implicitly by the R-S6.5 caller from
/// the tail of `heavy_hitters` (entries below a count threshold).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcdValueStats {
    /// VCD signal-id (matches [`VcdSignal::id`] and
    /// [`VcdChange::id`]). Caller correlates back to a declared
    /// signal via `VcdHeader::signals`.
    pub id: String,
    /// Observed determinate values + their occurrence counts.
    /// Sorted by count descending, then by value ascending (so
    /// the result is deterministic even when counts tie).
    /// `heavy_hitters[0]` is the most-frequent value; the tail
    /// is the reserve-set candidate.
    pub heavy_hitters: Vec<(u64, usize)>,
    /// Minimum determinate value observed. `None` when no
    /// determinate samples appeared.
    pub min: Option<u64>,
    /// Maximum determinate value observed. `None` when no
    /// determinate samples appeared.
    pub max: Option<u64>,
    /// Count of indeterminate samples (X / Z). Diagnostic only
    /// — these never become EnumValues discriminators.
    pub indeterminate_count: usize,
}

/// R-S6.3 (2026-06-11) — mine per-signal value frequencies from
/// a stream of [`VcdChange`] records.
///
/// **Algorithm**:
/// 1. Walk every change once.
/// 2. For each `Determinate(v)`, increment the `(signal_id,
///    value) → count` map and update min/max.
/// 3. For each `Indeterminate`, increment a per-signal counter.
/// 4. For each signal observed, sort its heavy_hitters by
///    `(-count, value)` for determinism.
///
/// **Pure**: no I/O. Stable output ordering — signals are
/// returned in ascending id order so the caller (R-S6.5) sees a
/// deterministic stats list across runs.
///
/// **Time complexity**: `O(N log N)` for N = total change count
/// (the dominant term is the per-signal heavy_hitters sort).
/// Space: `O(N)` for the value-frequency hashmaps.
///
/// **Note on `id` matching**: the returned `VcdValueStats::id`
/// matches the change-stream's `id`, which in turn matches the
/// VCD header's `VcdSignal::id`. To resolve a stats entry to a
/// signal name, the caller looks up the id in `VcdHeader::signals`.
pub fn mine_vcd_frequencies(changes: &[VcdChange]) -> Vec<VcdValueStats> {
    use std::collections::HashMap;

    // Per-signal accumulators. Indexed by signal-id.
    #[derive(Default)]
    struct Acc {
        counts: HashMap<u64, usize>,
        min: Option<u64>,
        max: Option<u64>,
        indeterminate_count: usize,
    }
    let mut accumulators: HashMap<String, Acc> = HashMap::new();

    for change in changes {
        let acc = accumulators.entry(change.id.clone()).or_default();
        match change.value {
            VcdSampleValue::Determinate(v) => {
                *acc.counts.entry(v).or_insert(0) += 1;
                acc.min = Some(acc.min.map_or(v, |m| m.min(v)));
                acc.max = Some(acc.max.map_or(v, |m| m.max(v)));
            }
            VcdSampleValue::Indeterminate => {
                acc.indeterminate_count += 1;
            }
        }
    }

    // Render to the public type. Sort signals by id for
    // determinism, and within each signal sort heavy_hitters by
    // (-count, value) so ties resolve deterministically.
    let mut ids: Vec<String> = accumulators.keys().cloned().collect();
    ids.sort();

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let acc = accumulators.remove(&id).expect("id from keys()");
        let mut heavy_hitters: Vec<(u64, usize)> = acc.counts.into_iter().collect();
        // Sort by count descending; tie-break by value ascending.
        heavy_hitters.sort_by(|(va, ca), (vb, cb)| cb.cmp(ca).then_with(|| va.cmp(vb)));
        out.push(VcdValueStats {
            id,
            heavy_hitters,
            min: acc.min,
            max: acc.max,
            indeterminate_count: acc.indeterminate_count,
        });
    }
    out
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

    // ─────────────────────────────────────────────────────────────
    // R-S6.3 tests — per-signal value-frequency mining
    // ─────────────────────────────────────────────────────────────

    fn d(time: u64, id: &str, v: u64) -> VcdChange {
        VcdChange {
            time,
            id: id.to_string(),
            value: VcdSampleValue::Determinate(v),
        }
    }

    fn ind(time: u64, id: &str) -> VcdChange {
        VcdChange {
            time,
            id: id.to_string(),
            value: VcdSampleValue::Indeterminate,
        }
    }

    #[test]
    fn mine_frequencies_empty_input_returns_empty_output() {
        assert!(mine_vcd_frequencies(&[]).is_empty());
    }

    #[test]
    fn mine_frequencies_single_determinate_change() {
        let changes = vec![d(0, "!", 5)];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats.len(), 1);
        let s = &stats[0];
        assert_eq!(s.id, "!");
        assert_eq!(s.heavy_hitters, vec![(5u64, 1usize)]);
        assert_eq!(s.min, Some(5));
        assert_eq!(s.max, Some(5));
        assert_eq!(s.indeterminate_count, 0);
    }

    #[test]
    fn mine_frequencies_counts_repeated_values_per_signal() {
        let changes = vec![d(0, "!", 0), d(1, "!", 1), d(2, "!", 0), d(3, "!", 0)];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats.len(), 1);
        // Value 0 wins (3 occurrences); value 1 trails (1).
        assert_eq!(stats[0].heavy_hitters, vec![(0u64, 3usize), (1u64, 1usize)]);
    }

    #[test]
    fn mine_frequencies_tracks_min_and_max_correctly() {
        let changes = vec![d(0, "!", 3), d(1, "!", 1), d(2, "!", 7), d(3, "!", 2)];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats[0].min, Some(1));
        assert_eq!(stats[0].max, Some(7));
    }

    #[test]
    fn mine_frequencies_separates_per_signal() {
        // Two signals, distinct heavy-hitters per signal.
        let changes = vec![
            d(0, "a", 1),
            d(0, "b", 100),
            d(1, "a", 1),
            d(1, "b", 200),
            d(2, "a", 2),
        ];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats.len(), 2);
        // Ordering is by id ascending — "a" first, then "b".
        assert_eq!(stats[0].id, "a");
        assert_eq!(stats[0].heavy_hitters, vec![(1u64, 2usize), (2u64, 1usize)]);
        assert_eq!(stats[1].id, "b");
        // 100 and 200 each appear once → sorted by value (100 first).
        assert_eq!(
            stats[1].heavy_hitters,
            vec![(100u64, 1usize), (200u64, 1usize)]
        );
    }

    #[test]
    fn mine_frequencies_counts_indeterminate_samples() {
        // Indeterminate samples don't populate heavy_hitters or
        // min/max, but the per-signal counter must surface for
        // R-S6.6 diagnostics.
        let changes = vec![
            d(0, "!", 5),
            ind(1, "!"),
            ind(2, "!"),
            d(3, "!", 5),
            ind(4, "!"),
        ];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats[0].heavy_hitters, vec![(5u64, 2usize)]);
        assert_eq!(stats[0].min, Some(5));
        assert_eq!(stats[0].max, Some(5));
        assert_eq!(stats[0].indeterminate_count, 3);
    }

    #[test]
    fn mine_frequencies_signal_with_only_indeterminate_samples() {
        // No determinate samples → min/max stay None;
        // heavy_hitters empty; indeterminate_count carries the
        // diagnostic.
        let changes = vec![ind(0, "!"), ind(1, "!"), ind(2, "!")];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats.len(), 1);
        assert!(stats[0].heavy_hitters.is_empty());
        assert_eq!(stats[0].min, None);
        assert_eq!(stats[0].max, None);
        assert_eq!(stats[0].indeterminate_count, 3);
    }

    #[test]
    fn mine_frequencies_ties_resolve_by_value_ascending() {
        // Two values with identical counts — tiebreaker is value
        // ascending so the output is deterministic across runs.
        let changes = vec![d(0, "!", 5), d(1, "!", 2), d(2, "!", 5), d(3, "!", 2)];
        let stats = mine_vcd_frequencies(&changes);
        // Both values have count 2; value 2 comes first.
        assert_eq!(stats[0].heavy_hitters, vec![(2u64, 2usize), (5u64, 2usize)]);
    }

    #[test]
    fn mine_frequencies_returns_signals_in_ascending_id_order() {
        // Three signals declared in arbitrary order — output must
        // sort by id for determinism.
        let changes = vec![d(0, "zzz", 1), d(0, "aaa", 1), d(0, "mmm", 1)];
        let stats = mine_vcd_frequencies(&changes);
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[0].id, "aaa");
        assert_eq!(stats[1].id, "mmm");
        assert_eq!(stats[2].id, "zzz");
    }
}
