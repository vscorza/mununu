//! R.0c — pipeline-faithfulness comparison harness + SVA-elision gate.
//!
//! Runs both SystemVerilog extraction pipelines on the same fixture
//! and produces a structured diff of the per-pipeline shape:
//!
//!   - **Native path:** [`SystemVerilogAdapter::translate_with_path`]
//!     (the legacy hand-rolled parser + explicit-state Kripke builder,
//!     scheduled for removal in S.2b per
//!     [`docs/design/native-sv-abstraction.md`](../../../docs/design/native-sv-abstraction.md)
//!     §9).
//!   - **KMTS path:** [`crate::adapter::yosys::translate_sv_per_module`]
//!     (sv2v → Yosys-no-flatten → per-submodule BTOR2 → BTOR2 reader,
//!     landed in R.0b).
//!
//! Plus the **SVA-elision gate**: feeds the fixture through `sv2v` and
//! greps the elaborated Verilog-2005 output for SVA / temporal
//! constructs that sv2v lowers away. Per the architecture doc §9.3,
//! any fixture that fails the gate is *not* safe for Tier-C
//! native-pipeline drop-in — its inline-SVA properties would be
//! silently elided by sv2v, and the verdict from the KMTS path
//! would match the native one trivially rather than verifying.
//!
//! The harness writes per-fixture comparison records into a baseline
//! JSON (`crates/mununu-core/tests/data/kmts_pipeline_baseline.json`)
//! and the gate's pass/fail map into a separate JSON
//! (`crates/mununu-core/tests/data/sva_elision_gate.json`). A
//! companion integration test in
//! `crates/mununu-core/tests/sv_compare_pipelines.rs` sweeps every
//! `examples/systemverilog/*.sv` fixture and asserts the baseline
//! stays stable across runs (the contract for S.0–S.2b deletions).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::adapter::AdapterError;
use crate::adapter::AdapterOptions;
use crate::adapter::systemverilog::SystemVerilogAdapter;
use crate::adapter::yosys::{self, YosysOptions};

/// Cached regex for tempdir-path normalisation in error messages.
/// Matches per-call tempdir prefixes like
/// `/var/folders/cp/.../mununu-yosys-per-module-73200-2-18-889140000/`
/// (macOS) and `/tmp/mununu-yosys-per-module-73200-2-18-889140000/`
/// (Linux), reducing them to `<tempdir>/` so error-message strings
/// stay byte-stable across runs. Without this the baseline JSON
/// flaps whenever pid / nanos change.
fn tempdir_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?:/[^/\s]+)+/mununu-[a-z0-9-]+-\d+(?:-\d+)*/")
            .expect("tempdir regex compiles")
    })
}

/// Strip per-call tempdir prefixes from an error message so the
/// baseline JSON is stable across runs. Replaces matches with the
/// literal string `<tempdir>/`.
fn normalise_tempdir_paths(msg: &str) -> String {
    tempdir_regex().replace_all(msg, "<tempdir>/").into_owned()
}

/// Per-fixture comparison record. Captures the shape statistics each
/// pipeline produces, the SVA-elision gate verdict, and any per-pipeline
/// errors. The native + KMTS records are independent — one may error
/// while the other succeeds (the harness records both rather than
/// short-circuiting).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineComparison {
    pub fixture: String,
    /// Top module name passed to both pipelines. `None` → adapter
    /// auto-detects (native parser uses the first declared module;
    /// Yosys uses `hierarchy -auto-top`).
    pub top: Option<String>,
    pub native: PipelineRecord,
    pub kmts: KmtsRecord,
    pub sva_gate: SvaElisionGate,
}

/// Shape stats from one pipeline arm. `error` is `Some(_)` when the
/// pipeline failed to produce any output; the numeric fields are
/// `None` in that case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineRecord {
    pub state_count: Option<usize>,
    pub property_count: Option<usize>,
    pub signal_count: Option<usize>,
    pub error: Option<String>,
}

/// KMTS-arm shape: sum across submodules + per-submodule breakdown.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KmtsRecord {
    pub submodule_count: usize,
    pub state_count_sum: Option<usize>,
    pub property_count_sum: Option<usize>,
    pub per_submodule: Vec<KmtsSubmodule>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KmtsSubmodule {
    pub module_name: String,
    pub state_count: usize,
    pub property_count: usize,
}

/// SVA-elision gate verdict per the architecture doc §9.3. A fixture
/// is `passed = true` iff sv2v's elaborated output contains *zero*
/// SVA / temporal tokens — meaning sv2v did not silently drop any
/// SVA-encoded property. A `passed = false` fixture must take an
/// S.2a resolution (rewrite the SVA, encode as sidecar `assumptions`,
/// or retire the fixture) before the native pipeline can be removed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SvaElisionGate {
    pub passed: bool,
    /// Patterns that matched, ordered by first appearance. Empty when
    /// `passed = true`.
    pub matches: Vec<String>,
    /// `Some(msg)` when sv2v itself errored (binary missing, parse
    /// failure on the fixture, …). Gate is reported as `passed = true`
    /// in this case — sv2v could not lower any SVA away because it
    /// could not run.
    pub error: Option<String>,
}

/// SVA / temporal tokens the gate greps for. Per the architecture
/// doc §9.3, an exact match (case-sensitive) on any of these patterns
/// indicates the fixture relies on a construct sv2v elaborates away.
const SVA_PATTERNS: &[&str] = &[
    "assert property",
    "assume property",
    "cover property",
    "$past",
    "$rose",
    "$fell",
    "$stable",
    "s_eventually",
    "s_until",
    "s_always",
    "disable iff",
];

/// Compare both pipelines on one SystemVerilog fixture. `sv_path` is
/// the path to the primary input (the native arm needs this for
/// sidecar discovery — `<stem>.mununu.json` next to the .sv); `content`
/// is its UTF-8 contents. `additional_sources` is forwarded to the
/// Yosys arm as `additional_sources` so multi-file designs compose
/// correctly; the native arm ignores extra sources (its single-module
/// parser scopes to the primary file).
///
/// The function does not error on per-pipeline failures — failures are
/// recorded into the [`PipelineRecord::error`] / [`KmtsRecord::error`]
/// fields. It errors only when both the SVA gate and *both* pipelines
/// fail (no signal at all to record).
pub fn compare_pipelines(
    content: &str,
    sv_path: &Path,
    additional_sources: &HashMap<String, String>,
    top: Option<&str>,
) -> PipelineComparison {
    let fixture = sv_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>")
        .to_string();

    // Native pipeline arm. Accepts only the primary source today
    // (the multi-module path is sidecar-driven via translate_multi_module,
    // outside the compare_pipelines contract).
    let native = match SystemVerilogAdapter::translate_with_path(
        content,
        &AdapterOptions::default(),
        sv_path,
    ) {
        Ok(out) => PipelineRecord {
            state_count: Some(out.source_info.state_count),
            property_count: Some(out.source_info.property_count),
            signal_count: Some(out.source_info.signal_count),
            error: None,
        },
        Err(e) => PipelineRecord {
            state_count: None,
            property_count: None,
            signal_count: None,
            error: Some(normalise_tempdir_paths(&e.message)),
        },
    };

    // KMTS pipeline arm — per-submodule BTOR2.
    let yopts = YosysOptions {
        top: top.map(str::to_string),
        additional_sources: additional_sources
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        primary_source_path: Some(sv_path.display().to_string()),
        per_module_btor: true,
        ..Default::default()
    };
    let kmts = match yosys::translate_sv_per_module(content, &AdapterOptions::default(), &yopts) {
        Ok(outputs) => {
            let per_submodule: Vec<KmtsSubmodule> = outputs
                .iter()
                .map(|o| KmtsSubmodule {
                    module_name: o.module_name.clone(),
                    state_count: o.output.source_info.state_count,
                    property_count: o.output.source_info.property_count,
                })
                .collect();
            let state_count_sum = per_submodule.iter().map(|m| m.state_count).sum();
            let property_count_sum = per_submodule.iter().map(|m| m.property_count).sum();
            KmtsRecord {
                submodule_count: per_submodule.len(),
                state_count_sum: Some(state_count_sum),
                property_count_sum: Some(property_count_sum),
                per_submodule,
                error: None,
            }
        }
        Err(e) => KmtsRecord {
            submodule_count: 0,
            state_count_sum: None,
            property_count_sum: None,
            per_submodule: Vec::new(),
            error: Some(normalise_tempdir_paths(&e.message)),
        },
    };

    // SVA-elision gate — runs sv2v independently of either pipeline.
    let include_dirs = sv_path
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    let mut sources_for_sv2v = vec![sv_path.to_path_buf()];
    for name in additional_sources.keys() {
        // Best-effort: if the additional source is a path that resolves
        // alongside the primary, include it; otherwise skip (sv2v
        // can't pick up in-memory sources, and additional_sources is a
        // name→content map without disk paths).
        if let Some(parent) = sv_path.parent() {
            let candidate = parent.join(name);
            if candidate.exists() {
                sources_for_sv2v.push(candidate);
            }
        }
    }
    let sva_gate = run_sva_elision_gate(&sources_for_sv2v, &include_dirs);

    PipelineComparison {
        fixture,
        top: top.map(str::to_string),
        native,
        kmts,
        sva_gate,
    }
}

/// Run sv2v over `sources` and grep its output for SVA tokens.
/// Returns a [`SvaElisionGate`] with `passed = matches.is_empty()`.
fn run_sva_elision_gate(sources: &[PathBuf], include_dirs: &[PathBuf]) -> SvaElisionGate {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Per-call unique tempdir under std::env::temp_dir(); cleaned up
    // on drop. Avoids pulling tempfile out of dev-dependencies.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("mununu-sva-gate-{pid}-{counter}-{nanos}"));
    if let Err(e) = std::fs::create_dir_all(&tmp) {
        return SvaElisionGate {
            passed: true,
            matches: Vec::new(),
            error: Some(format!("tempdir: {e}")),
        };
    }

    let out_path = tmp.join("sva_gate.elab.v");
    let result = (|| -> SvaElisionGate {
        if let Err(e) = yosys::preprocess_sv(sources, include_dirs, &out_path) {
            return SvaElisionGate {
                passed: true,
                matches: Vec::new(),
                error: Some(normalise_tempdir_paths(&e.message)),
            };
        }
        match std::fs::read_to_string(&out_path) {
            Ok(elaborated) => sva_elision_grep(&elaborated),
            Err(e) => SvaElisionGate {
                passed: true,
                matches: Vec::new(),
                error: Some(format!("read elaborated.v: {e}")),
            },
        }
    })();

    // Best-effort cleanup; leaving tempdir behind is not a soundness
    // problem, just operational hygiene.
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Pure-string variant: grep `elaborated_v` for the SVA/temporal
/// patterns. Exposed publicly so callers can run the gate on
/// already-elaborated Verilog without re-invoking sv2v.
pub fn sva_elision_grep(elaborated_v: &str) -> SvaElisionGate {
    let mut matches: Vec<String> = Vec::new();
    for pat in SVA_PATTERNS {
        if elaborated_v.contains(pat) {
            matches.push((*pat).to_string());
        }
    }
    // The `##\d` pattern (SVA cycle delay) is not a literal substring;
    // detect with a simple regex-free scan.
    if has_cycle_delay(elaborated_v) {
        matches.push("##<n>".to_string());
    }
    let passed = matches.is_empty();
    SvaElisionGate {
        passed,
        matches,
        error: None,
    }
}

/// Detect SVA's `##<digits>` cycle-delay syntax in elaborated output.
fn has_cycle_delay(text: &str) -> bool {
    // Linear scan: look for "##" followed by an ASCII digit. Avoids
    // pulling in a regex dependency for this single check.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'#' && bytes[i + 1] == b'#' && bytes[i + 2].is_ascii_digit() {
            return true;
        }
        i += 1;
    }
    false
}

/// Serialise a list of comparison records to deterministic JSON.
/// The keys are sorted so the baseline file is line-stable across
/// runs (verdict polarity assertions diff cleanly).
pub fn to_baseline_json(records: &[PipelineComparison]) -> String {
    // Sort by fixture name for deterministic output.
    let mut sorted: Vec<&PipelineComparison> = records.iter().collect();
    sorted.sort_by(|a, b| a.fixture.cmp(&b.fixture));
    serde_json::to_string_pretty(&sorted).expect("PipelineComparison is JSON-serialisable")
}

/// Serialise the SVA-elision gate map (fixture → gate) to deterministic JSON.
pub fn sva_gate_to_json(records: &[PipelineComparison]) -> String {
    let mut map: std::collections::BTreeMap<String, &SvaElisionGate> =
        std::collections::BTreeMap::new();
    for rec in records {
        map.insert(rec.fixture.clone(), &rec.sva_gate);
    }
    serde_json::to_string_pretty(&map).expect("SvaElisionGate is JSON-serialisable")
}

/// Translate any non-fatal error (e.g. yosys/sv2v missing on $PATH)
/// into a `is_tool_missing` predicate so callers can downgrade tool-
/// absence to a SKIP rather than a regression failure.
pub fn is_tool_missing(err: &AdapterError) -> bool {
    let msg = err.message.as_str();
    msg.contains("yosys binary not found")
        || msg.contains("failed to spawn yosys")
        || msg.contains("sv2v binary not found")
        || msg.contains("failed to spawn sv2v")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sva_elision_grep_passes_clean_verilog() {
        let v = r#"
            module foo(input wire clk);
              reg [3:0] x;
              always @(posedge clk) x <= x + 1;
            endmodule
        "#;
        let gate = sva_elision_grep(v);
        assert!(gate.passed, "clean Verilog should pass; got {gate:?}");
        assert!(gate.matches.is_empty());
    }

    #[test]
    fn sva_elision_grep_flags_assert_property() {
        let v = r#"
            module foo(input wire clk, input wire a, input wire b);
              assert property (@(posedge clk) a |-> ##1 b);
            endmodule
        "#;
        let gate = sva_elision_grep(v);
        assert!(!gate.passed, "fixture with SVA should fail the gate");
        assert!(gate.matches.iter().any(|m| m == "assert property"));
        assert!(gate.matches.iter().any(|m| m == "##<n>"));
    }

    #[test]
    fn sva_elision_grep_flags_temporal_system_tasks() {
        let v = r#"
            module foo(input clk, input a);
              wire prev_a = $past(a);
              wire rose_a = $rose(a);
            endmodule
        "#;
        let gate = sva_elision_grep(v);
        assert!(!gate.passed);
        assert!(gate.matches.iter().any(|m| m == "$past"));
        assert!(gate.matches.iter().any(|m| m == "$rose"));
    }

    #[test]
    fn tempdir_normaliser_strips_macos_paths() {
        let m = "yosys exited at /var/folders/cp/abc/T/mununu-yosys-per-module-73200-2-18-889140000/work.sv: Module not found";
        let normalised = normalise_tempdir_paths(m);
        assert!(
            normalised.starts_with("yosys exited at <tempdir>/work.sv: Module not found"),
            "got {normalised:?}"
        );
        assert!(!normalised.contains("/var/folders"));
        assert!(!normalised.contains("889140000"));
    }

    #[test]
    fn tempdir_normaliser_strips_linux_paths() {
        let m = "spawn failed at /tmp/mununu-sva-gate-12345-0-789012345/sva_gate.elab.v";
        let normalised = normalise_tempdir_paths(m);
        assert!(
            normalised.contains("<tempdir>/sva_gate.elab.v"),
            "got {normalised:?}"
        );
        assert!(!normalised.contains("12345"));
    }

    #[test]
    fn tempdir_normaliser_is_idempotent() {
        let m = "no tempdir mentioned here";
        assert_eq!(normalise_tempdir_paths(m), m);
        let normalised =
            normalise_tempdir_paths("path /var/folders/cp/x/T/mununu-yosys-1-2-3-4/foo.btor2");
        assert_eq!(normalise_tempdir_paths(&normalised), normalised);
    }

    #[test]
    fn cycle_delay_detector_recognises_sva_syntax() {
        assert!(has_cycle_delay("a |-> ##1 b"));
        assert!(has_cycle_delay("##42"));
        assert!(!has_cycle_delay("##abc"));
        assert!(!has_cycle_delay("#5"));
        assert!(!has_cycle_delay(""));
    }

    #[test]
    fn baseline_json_is_deterministic() {
        let recs = vec![
            PipelineComparison {
                fixture: "b.sv".into(),
                top: None,
                native: PipelineRecord {
                    state_count: Some(4),
                    property_count: Some(0),
                    signal_count: Some(2),
                    error: None,
                },
                kmts: KmtsRecord {
                    submodule_count: 1,
                    state_count_sum: Some(4),
                    property_count_sum: Some(0),
                    per_submodule: vec![KmtsSubmodule {
                        module_name: "b".into(),
                        state_count: 4,
                        property_count: 0,
                    }],
                    error: None,
                },
                sva_gate: SvaElisionGate {
                    passed: true,
                    matches: Vec::new(),
                    error: None,
                },
            },
            PipelineComparison {
                fixture: "a.sv".into(),
                top: None,
                native: PipelineRecord {
                    state_count: Some(2),
                    property_count: Some(0),
                    signal_count: Some(1),
                    error: None,
                },
                kmts: KmtsRecord {
                    submodule_count: 1,
                    state_count_sum: Some(2),
                    property_count_sum: Some(0),
                    per_submodule: Vec::new(),
                    error: None,
                },
                sva_gate: SvaElisionGate {
                    passed: true,
                    matches: Vec::new(),
                    error: None,
                },
            },
        ];
        let json1 = to_baseline_json(&recs);
        let json2 = to_baseline_json(&recs);
        assert_eq!(json1, json2);
        // a.sv before b.sv after sorting
        let pos_a = json1.find("\"a.sv\"").unwrap();
        let pos_b = json1.find("\"b.sv\"").unwrap();
        assert!(pos_a < pos_b, "fixtures must be sorted alphabetically");
    }
}
