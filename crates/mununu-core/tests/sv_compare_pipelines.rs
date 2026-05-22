//! R.0c integration harness — sweeps every `examples/systemverilog/*.sv`
//! fixture through `compare_pipelines` and asserts the resulting
//! `kmts_pipeline_baseline.json` + `sva_elision_gate.json` stay stable
//! across runs.
//!
//! Two modes:
//!
//!   - **Default (regression mode):** Compares the freshly-computed
//!     records against the committed baseline JSON. Mismatches fail the
//!     test with a per-fixture diff. This is the contract the
//!     simplification phases S.0–S.2b ride on — any code change that
//!     flips a pipeline's verdict polarity or shape stats must update
//!     the baseline deliberately.
//!
//!   - **Update mode (`MUNUNU_R0C_UPDATE_BASELINE=1`):** Re-writes the
//!     baseline files from the current run. Use this when intentionally
//!     accepting a shape change (e.g. after fixture migration in S.2a,
//!     after a new adapter behaviour lands, after a fixture's `.sv`
//!     file changes).
//!
//! Skips gracefully when `yosys` or `sv2v` is absent — production CI
//! ships both; local runs without them are not regression failures.
//!
//! Run with: `cargo test -p mununu-core --test sv_compare_pipelines -- --nocapture`
//! Update with: `MUNUNU_R0C_UPDATE_BASELINE=1 cargo test -p mununu-core --test sv_compare_pipelines -- --nocapture`

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use mununu_core::adapter::sv_pipeline_compare::{
    PipelineComparison, compare_pipelines, sva_gate_to_json, to_baseline_json,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn fixtures_dir() -> PathBuf {
    workspace_root().join("examples/systemverilog")
}

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/kmts_pipeline_baseline.json")
}

fn sva_gate_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sva_elision_gate.json")
}

/// Try to discover any *additional* sources a primary fixture needs.
/// The convention in `examples/systemverilog/` is that the
/// top-with-instantiations file lists its peers as siblings — e.g.
/// `multi_producer_consumer_top.sv` instantiates `producer`,
/// `consumer`, `bounded_buffer` whose definitions live in
/// `multi_producer.sv`, `multi_consumer.sv`, `multi_buffer.sv`. The
/// harness uses a small explicit map for those cases; everything else
/// is treated as single-file.
fn additional_sources_for(primary: &str) -> Vec<&'static str> {
    match primary {
        "multi_producer_consumer_top.sv" => {
            vec!["multi_producer.sv", "multi_consumer.sv", "multi_buffer.sv"]
        }
        _ => Vec::new(),
    }
}

/// Explicit top override per fixture, when the SV filename and the
/// module name don't match (a common occurrence in the existing
/// fixtures — e.g. `multi_producer_consumer_top.sv` declares the
/// module `producer_consumer_top`).
fn top_for(primary: &str) -> Option<&'static str> {
    match primary {
        "multi_producer_consumer_top.sv" => Some("producer_consumer_top"),
        "multi_axilite_master.sv" => Some("axilite_master"),
        "multi_axilite_slave_fixed.sv" => Some("axilite_slave_fixed"),
        "multi_axilite_slave_bug.sv" => Some("axilite_slave_bug"),
        "multi_buffer.sv" => Some("bounded_buffer"),
        "multi_buffer_producer_fixed.sv" => Some("buffer_producer_fixed"),
        "multi_buffer_producer_bug.sv" => Some("buffer_producer_bug"),
        "multi_consumer.sv" => Some("consumer"),
        "multi_producer.sv" => Some("producer"),
        _ => None,
    }
}

#[test]
fn pipeline_comparison_matches_baseline() {
    let mut sv_files: Vec<PathBuf> = fs::read_dir(fixtures_dir())
        .expect("read examples/systemverilog/")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "sv"))
        .collect();
    sv_files.sort();
    assert!(!sv_files.is_empty(), "no .sv fixtures");

    // Sweep every fixture. Per-fixture tool-missing errors downgrade
    // to a global SKIP so this test does not regress on machines
    // without yosys / sv2v.
    let mut records = Vec::with_capacity(sv_files.len());
    let mut tool_missing = false;
    for sv in &sv_files {
        let name = sv
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>");
        let content = match fs::read_to_string(sv) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("WARN: skip {name}: read error: {e}");
                continue;
            }
        };
        let mut additional = HashMap::new();
        for src in additional_sources_for(name) {
            let path = fixtures_dir().join(src);
            if let Ok(body) = fs::read_to_string(&path) {
                additional.insert(src.to_string(), body);
            }
        }
        let record = compare_pipelines(&content, sv, &additional, top_for(name));
        // Tool-missing detection: if both pipeline arms error with the
        // same tool-missing pattern, treat the whole run as a SKIP.
        let n_err = record.native.error.as_deref();
        let k_err = record.kmts.error.as_deref();
        let s_err = record.sva_gate.error.as_deref();
        if [n_err, k_err, s_err].iter().flatten().any(|msg| {
            msg.contains("yosys binary not found")
                || msg.contains("failed to spawn yosys")
                || msg.contains("sv2v binary not found")
                || msg.contains("failed to spawn sv2v")
        }) {
            tool_missing = true;
        }
        records.push(record);
    }

    if tool_missing {
        eprintln!(
            "SKIP: yosys and/or sv2v not on $PATH (set MUNUNU_YOSYS_PATH / MUNUNU_SV2V_PATH or install them). \
             Production CI ships both. Skipping pipeline-comparison regression."
        );
        return;
    }

    let baseline_json = to_baseline_json(&records);
    let gate_json = sva_gate_to_json(&records);

    if std::env::var("MUNUNU_R0C_UPDATE_BASELINE").is_ok() {
        fs::write(baseline_path(), &baseline_json)
            .unwrap_or_else(|e| panic!("write {}: {e}", baseline_path().display()));
        fs::write(sva_gate_path(), &gate_json)
            .unwrap_or_else(|e| panic!("write {}: {e}", sva_gate_path().display()));
        eprintln!(
            "R.0c UPDATE: wrote baseline ({} fixtures) to {} and gate to {}",
            records.len(),
            baseline_path().display(),
            sva_gate_path().display()
        );
        return;
    }

    // Regression mode — compare against the committed baselines.
    let baseline_committed = match fs::read_to_string(baseline_path()) {
        Ok(s) => s,
        Err(_) => {
            // First-run: write the baseline so the next run has
            // something to compare against. Mark the test as
            // succeeded but emit a warning so reviewers know to
            // commit the file.
            fs::write(baseline_path(), &baseline_json)
                .unwrap_or_else(|e| panic!("write {}: {e}", baseline_path().display()));
            fs::write(sva_gate_path(), &gate_json)
                .unwrap_or_else(|e| panic!("write {}: {e}", sva_gate_path().display()));
            eprintln!(
                "R.0c BOOTSTRAP: baseline files did not exist; wrote them now. Commit them and re-run to enable regression mode."
            );
            return;
        }
    };
    let gate_committed = fs::read_to_string(sva_gate_path()).unwrap_or_default();

    assert_baseline_match(
        baseline_path().display().to_string(),
        &baseline_committed,
        &baseline_json,
        &records,
    );
    assert_baseline_match(
        sva_gate_path().display().to_string(),
        &gate_committed,
        &gate_json,
        &records,
    );

    eprintln!(
        "R.0c sweep: {} fixtures compared against baseline; clean.",
        records.len()
    );
}

fn assert_baseline_match(
    path: String,
    committed: &str,
    fresh: &str,
    records: &[PipelineComparison],
) {
    if committed.trim() == fresh.trim() {
        return;
    }
    // Compute a short summary of which fixtures changed so the failure
    // message is actionable without printing the whole JSON diff.
    let mut moved = Vec::new();
    if let (Ok(c_val), Ok(f_val)) = (
        serde_json::from_str::<serde_json::Value>(committed),
        serde_json::from_str::<serde_json::Value>(fresh),
    ) && let (Some(c_arr), Some(f_arr)) = (c_val.as_array(), f_val.as_array())
    {
        let c_by_name: HashMap<&str, &serde_json::Value> = c_arr
            .iter()
            .filter_map(|v| Some((v.get("fixture")?.as_str()?, v)))
            .collect();
        for v in f_arr {
            if let Some(name) = v.get("fixture").and_then(|f| f.as_str()) {
                match c_by_name.get(name) {
                    Some(c_v) if *c_v == v => {}
                    Some(_) => moved.push(format!("changed: {name}")),
                    None => moved.push(format!("new:     {name}")),
                }
            }
        }
        for name in c_by_name.keys() {
            if !f_arr
                .iter()
                .any(|v| v.get("fixture").and_then(|f| f.as_str()) == Some(*name))
            {
                moved.push(format!("removed: {name}"));
            }
        }
    }
    panic!(
        "R.0c baseline mismatch at {path} (across {} fixtures):\n  {}\nRe-run with MUNUNU_R0C_UPDATE_BASELINE=1 to accept the new shape (only if the change is intentional).",
        records.len(),
        moved.join("\n  ")
    );
}
