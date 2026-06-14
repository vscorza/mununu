//! S-track verdict-polarity parity gate (step 2 of the S-track
//! migration). For each curated SystemVerilog fixture that carries
//! SIDECAR properties (so both pipelines receive the same formulas),
//! this evaluates every property on BOTH the native (`sv-rtl`) and the
//! KMTS (`sv-yosys`) pipeline's realized model and RECORDS whether the
//! verdicts agree, against a committed baseline
//! (`tests/data/sv_verdict_parity.txt`).
//!
//! Why this matters: lift parity (both pipelines produce a model) is
//! necessary but NOT sufficient to retire the native parser — the KMTS
//! path must produce the SAME true/false verdicts. This is the
//! load-bearing gate the native-parser deletion depends on: deletion is
//! safe ONLY when the baseline's `mismatch=` count is 0.
//!
//! **Current state: the gate is RED** — the two pipelines produce
//! materially different verdicts (mismatches in BOTH directions ⇒
//! genuinely different abstractions, not a uniform over-approximation),
//! so the native-parser deletion is BLOCKED pending root-cause +
//! reconciliation of the divergence.
//!
//! Design (a baseline-recorded report, not a hard pass/fail on
//! mismatch — the RED state is real and recorded honestly, like
//! `kmts_pipeline_baseline.json`):
//! - Only SIDECAR-property fixtures are comparable. Inline `@mununu`
//!   annotation properties produce 0 properties on the KMTS arm (Yosys
//!   strips comments) and are excluded — migrating those to sidecars is
//!   separate follow-up work.
//! - A formula is compared only when it evaluates on BOTH (the two
//!   models are differently-named; a formula referencing a name absent
//!   in one model is omitted, not counted).
//! - The test asserts the report MATCHES the committed baseline (drift
//!   detection) + that at least one verdict is comparable (no vacuous
//!   pass). Regenerate with `MUNUNU_VERDICT_PARITY_UPDATE=1`.
//! - Requires `yosys` on PATH; skips (does not fail) when absent, like
//!   the other yosys-dependent integration tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mununu_core::adapter::AdapterOptions;
use mununu_core::adapter::systemverilog::SystemVerilogAdapter;
use mununu_core::{adapter::yosys, context_dsl};

/// Curated fixtures with sidecar properties that lift on both
/// pipelines (per the kmts_pipeline_baseline.json matching
/// property-counts). Bug/fixed pairs are the strongest test: their
/// verdicts should differ bug-vs-fixed AND match native-vs-KMTS.
const COMPARABLE_FIXTURES: &[&str] = &[
    "cwe1245_fsm_bug",
    "cwe1245_fsm_fixed",
    "cwe1260_addr_overlap_bug",
    "cwe1260_addr_overlap_fixed",
    "cwe1262_csr_bypass_bug",
    "cwe1262_csr_bypass_fixed",
    "fifo_overflow_bug",
    "fifo_overflow_fixed",
    "axilite_deadlock_bug",
    "axilite_deadlock_fixed",
    "alu",
    "fifo",
];

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/systemverilog")
}

/// Evaluate every realized formula on the FIRST automaton of a CTXDSL
/// document and reduce each to a single Boolean verdict: SATISFIED iff
/// every initial state satisfies the formula (the same reduction the
/// verify orchestrator uses). Formulas that fail to evaluate (name
/// absent in this model) are omitted from the map.
fn pipeline_verdicts(ctxdsl: &str) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    let Ok(doc) = context_dsl::parse(ctxdsl) else {
        return out;
    };
    let Ok(realized) = context_dsl::realize_context(&doc, &[]) else {
        return out;
    };
    let automata = realized.context.clts_names();
    let Some(over) = automata.first() else {
        return out;
    };
    let Some(clts) = realized.context.clts(over) else {
        return out;
    };
    let env = realized.environment_for(over);
    let inits: Vec<_> = clts.initial_states().iter().copied().collect();
    if inits.is_empty() {
        return out;
    }
    let mut names: Vec<String> = realized.formulas.keys().cloned().collect();
    names.sort();
    for fname in names {
        let Some(rf) = realized.formulas.get(&fname) else {
            continue;
        };
        // A formula that references a name absent in this model errors
        // → omit it (recorded as incomparable by the caller).
        let Ok(result) = realized.context.evaluate_mu(over, &rf.formula, &env, None) else {
            continue;
        };
        let satisfied = inits
            .iter()
            .all(|sid| result.get(sid.index()).map(|b| *b).unwrap_or(false));
        out.insert(fname, satisfied);
    }
    out
}

#[test]
fn kmts_and_native_pipelines_agree_on_verdicts() {
    // Skip cleanly if yosys is unavailable (CI without the toolchain).
    let probe = yosys::translate_sv(
        "module probe(input logic c); endmodule",
        &AdapterOptions::default(),
        &yosys::YosysOptions::default(),
    );
    if let Err(e) = &probe {
        let msg = e.to_string().to_lowercase();
        if msg.contains("yosys") && (msg.contains("not found") || msg.contains("locate")) {
            eprintln!("SKIP sv_verdict_parity: yosys not available ({e})");
            return;
        }
    }

    let dir = examples_dir();
    let mut comparable = 0usize;
    let mut matches = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    // Per-(fixture/property) verdict record, sorted, for the baseline.
    let mut records: Vec<String> = Vec::new();

    for fixture in COMPARABLE_FIXTURES {
        let sv_path = dir.join(format!("{fixture}.sv"));
        let Ok(content) = std::fs::read_to_string(&sv_path) else {
            eprintln!("  (skip {fixture}: source not found)");
            continue;
        };

        // Native arm (auto-loads the .mununu.json via the path).
        let Ok(native_out) = SystemVerilogAdapter::translate_with_path(
            &content,
            &AdapterOptions::default(),
            &sv_path,
        ) else {
            eprintln!("  (skip {fixture}: native pipeline errored)");
            continue;
        };

        // KMTS arm — pass the same sidecar (parity-gate fairness), then
        // take the primary submodule's CTXDSL.
        let kmts_opts = AdapterOptions {
            sidecar_json: mununu_core::adapter::systemverilog::annotation::find_sidecar(&sv_path)
                .and_then(|p| std::fs::read_to_string(p).ok()),
            ..Default::default()
        };
        let yopts = yosys::YosysOptions {
            primary_source_path: Some(sv_path.display().to_string()),
            per_module_btor: true,
            ..Default::default()
        };
        let Ok(kmts_outputs) = yosys::translate_sv_per_module(&content, &kmts_opts, &yopts) else {
            eprintln!("  (skip {fixture}: KMTS pipeline errored)");
            continue;
        };
        let Some(kmts_primary) = kmts_outputs.first() else {
            eprintln!("  (skip {fixture}: KMTS produced no submodule)");
            continue;
        };

        let native_v = pipeline_verdicts(&native_out.ctxdsl);
        let kmts_v = pipeline_verdicts(&kmts_primary.output.ctxdsl);

        // Compare only formulas that evaluated on BOTH models.
        for (name, nv) in &native_v {
            if let Some(kv) = kmts_v.get(name) {
                comparable += 1;
                let agree = nv == kv;
                if agree {
                    matches += 1;
                } else {
                    mismatches.push(format!("{fixture}/{name}: native={nv} kmts={kv}"));
                }
                records.push(format!(
                    "{fixture}/{name} native={nv} kmts={kv} {}",
                    if agree { "AGREE" } else { "MISMATCH" }
                ));
            }
        }
    }

    records.sort();
    let report = format!(
        "# S-track verdict-polarity parity (native sv-rtl vs KMTS sv-yosys)\n\
         # comparable={comparable} match={matches} mismatch={}\n# GATE: {}\n{}\n",
        mismatches.len(),
        if mismatches.is_empty() {
            "GREEN — deletion-safe"
        } else {
            "RED — native deletion BLOCKED"
        },
        records.join("\n"),
    );

    let baseline_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/sv_verdict_parity.txt");
    if std::env::var("MUNUNU_VERDICT_PARITY_UPDATE").is_ok() {
        std::fs::write(&baseline_path, &report).expect("write verdict-parity baseline");
        eprintln!(
            "updated verdict-parity baseline: {}",
            baseline_path.display()
        );
        return;
    }

    eprintln!(
        "sv_verdict_parity: {comparable} comparable, {matches} match, {} mismatch (GATE {})",
        mismatches.len(),
        if mismatches.is_empty() {
            "GREEN"
        } else {
            "RED"
        }
    );
    assert!(
        comparable > 0,
        "no comparable verdicts were produced — the gate is vacuous (check fixture sidecars / \
         yosys availability)"
    );

    // Drift detection against the recorded baseline. The baseline
    // captures the CURRENT parity state (RED today — the two pipelines
    // produce materially different verdicts, so the native-parser
    // deletion is gated). Regenerate with MUNUNU_VERDICT_PARITY_UPDATE=1
    // when the parity legitimately changes; a diff here flags an
    // unexpected verdict drift on either pipeline.
    let expected = std::fs::read_to_string(&baseline_path).unwrap_or_default();
    assert_eq!(
        report.trim(),
        expected.trim(),
        "verdict-parity drift vs baseline (regenerate with MUNUNU_VERDICT_PARITY_UPDATE=1 if \
         intended). The gate is the `mismatch=` count: native-parser deletion is safe only at \
         mismatch=0."
    );
}
