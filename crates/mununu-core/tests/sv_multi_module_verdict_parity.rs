//! R-MM-6 — multi-module verdict-parity gate (native `sv-rtl` vs KMTS
//! `sv-yosys`). The load-bearing safety net before S.2b deletes the native
//! multi-module path: it must not lose multi-module coverage, so the KMTS
//! path has to produce the SAME verdicts the native path does.
//!
//! ## The two input forms
//!
//! The two pipelines consume multi-module designs in structurally different
//! forms — and S.2b's deletion makes the KMTS form the survivor:
//!
//! - **Native (`sv-rtl`)**: composes a list of STANDALONE modules wired by
//!   the `mununu_sv_multi_v1` sidecar's `connections` (no top module). Entry
//!   point: `SystemVerilogAdapter::translate_multi_module_content`.
//! - **KMTS (`sv-yosys`)**: composes a TOP module that structurally
//!   instantiates submodules; instance connectivity is discovered from the
//!   Yosys netlist. Entry point:
//!   `yosys::multi_module::compose_sv_multi_module`.
//!
//! So this gate pairs the SAME logical design expressed in BOTH forms: the
//! existing `multi_buffer_overflow_{bug,fixed}.mununu.json` sidecars (native)
//! and the `multi_buffer_overflow_{bug,fixed}_top.sv` top modules (KMTS).
//!
//! ## Per-pipeline equivalent formulas (the cross-encoding decision)
//!
//! The two pipelines encode predicates differently — native uses the
//! `bounded_counter` abstraction's `count_3` predicate; KMTS uses the
//! bit-blaster's numeric instance-qualified valuation `u_buffer__count == 3`
//! (R-MM-4c qualification + R-MM-5b-i all-numeric valuations). They are two
//! encodings of the SAME intended property — "the buffer count never reaches
//! 3 (overflow)". The gate evaluates each pipeline's encoding on its own
//! model and requires the verdicts AGREE.
//!
//! ## What it proves
//!
//! On a real bug/fixed pair (the OpenPiton-inspired buffer overflow), with a
//! discriminating safety property:
//! - native(bug) == KMTS(bug) == FALSE   (overflow reachable)
//! - native(fixed) == KMTS(fixed) == TRUE (backpressure prevents overflow)
//!
//! The fixed case is the stronger test: its producer's `push` is **Mealy**
//! (`push = (state==SENDING) && !full`, a function of the buffer's
//! combinational `full` output), so it exercises a Mealy-output rendezvous
//! across the composition — not just the Moore-output case
//! (`producer_consumer_top`'s `valid`) the earlier R-MM increments covered.
//!
//! Gated on `yosys` being on PATH; skips cleanly otherwise.

use mununu_core::adapter::AdapterOptions;
use mununu_core::adapter::systemverilog::SystemVerilogAdapter;
use mununu_core::adapter::yosys::{self, multi_module};
use mununu_core::context_dsl;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn yosys_available() -> bool {
    std::process::Command::new("yosys")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/systemverilog")
}

fn read(f: &str) -> String {
    std::fs::read_to_string(examples_dir().join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"))
}

/// Reduce a realized model + formula to one Boolean verdict: SATISFIED iff
/// every initial state satisfies the formula (the same reduction the verify
/// orchestrator and the single-module parity gate use). Returns `None` when
/// the model has no initial states or the formula fails to evaluate.
fn verdict_over(
    realized: &mununu_core::context_dsl::RealizedContext,
    over: &str,
    formula: &str,
) -> Option<bool> {
    let clts = realized.context.clts(over)?;
    let env = realized.environment_for(over);
    let f = mununu_core::mu_calculus::parser::parse(formula).ok()?;
    let result = realized.context.evaluate_mu(over, &f, &env, None).ok()?;
    let inits: Vec<_> = clts.initial_states().iter().copied().collect();
    if inits.is_empty() {
        return None;
    }
    Some(
        inits
            .iter()
            .all(|s| result.get(s.index()).map(|b| *b).unwrap_or(false)),
    )
}

/// Native (`sv-rtl`) verdict: compose the sidecar's standalone modules and
/// evaluate the native-encoded formula over the `system` composition.
fn native_verdict(
    sidecar_file: &str,
    sources: &HashMap<String, String>,
    formula: &str,
) -> Option<bool> {
    let out = SystemVerilogAdapter::translate_multi_module_content(
        &read(sidecar_file),
        sources,
        &AdapterOptions::default(),
    )
    .ok()?;
    let doc = context_dsl::parse(&out.ctxdsl).ok()?;
    let realized = context_dsl::realize_context(&doc, &[]).ok()?;
    let names = realized.context.clts_names();
    let over = names
        .iter()
        .find(|n| *n == "system")
        .or_else(|| names.first())?
        .clone();
    verdict_over(&realized, &over, formula)
}

/// KMTS (`sv-yosys`) verdict: compose the top module's netlist and evaluate
/// the KMTS-encoded formula over the composed automaton.
fn kmts_verdict(
    top_file: &str,
    top_name: &str,
    modules: &[(String, String)],
    formula: &str,
) -> Option<bool> {
    let yopts = yosys::YosysOptions {
        top: Some(top_name.into()),
        per_module_btor: true,
        additional_sources: modules.to_vec(),
        ..Default::default()
    };
    let comp =
        multi_module::compose_sv_multi_module(&read(top_file), &AdapterOptions::default(), &yopts)
            .ok()?;
    let ctxdsl = multi_module::clts_to_ctxdsl(&comp.composed, "Circuit", "mm_system").ok()?;
    let doc = context_dsl::parse(&ctxdsl).ok()?;
    let realized = context_dsl::realize_context(&doc, &[]).ok()?;
    let over = realized.context.clts_names().first()?.clone();
    verdict_over(&realized, &over, formula)
}

#[test]
fn multi_module_buffer_overflow_native_kmts_parity() {
    if !yosys_available() {
        eprintln!("skip: yosys not installed");
        return;
    }
    if !examples_dir()
        .join("multi_buffer_overflow_bug_top.sv")
        .exists()
    {
        eprintln!("skip: multi-module fixtures not found");
        return;
    }

    let buffer = read("multi_buffer.sv");
    let prod_bug = read("multi_buffer_producer_bug.sv");
    let prod_fixed = read("multi_buffer_producer_fixed.sv");

    // Native sidecar source maps (modules by basename).
    let sources_bug: HashMap<String, String> = HashMap::from([
        ("multi_buffer_producer_bug.sv".to_string(), prod_bug.clone()),
        ("multi_buffer.sv".to_string(), buffer.clone()),
    ]);
    let sources_fixed: HashMap<String, String> = HashMap::from([
        (
            "multi_buffer_producer_fixed.sv".to_string(),
            prod_fixed.clone(),
        ),
        ("multi_buffer.sv".to_string(), buffer.clone()),
    ]);

    // The SAME intended property — "buffer count never reaches 3 (overflow)"
    // — in each pipeline's predicate encoding.
    let native_formula = "nu X. (!count_3 && [] X)";
    let kmts_formula = "nu X. (!(u_buffer__count == 3) && [] X)";

    // Native (sidecar-composed) verdicts.
    let native_bug = native_verdict(
        "multi_buffer_overflow_bug.mununu.json",
        &sources_bug,
        native_formula,
    )
    .expect("native bug verdict");
    let native_fixed = native_verdict(
        "multi_buffer_overflow_fixed.mununu.json",
        &sources_fixed,
        native_formula,
    )
    .expect("native fixed verdict");

    // KMTS (netlist-composed) verdicts.
    let kmts_bug = kmts_verdict(
        "multi_buffer_overflow_bug_top.sv",
        "buffer_overflow_bug_top",
        &[
            ("multi_buffer_producer_bug.sv".to_string(), prod_bug),
            ("multi_buffer.sv".to_string(), buffer.clone()),
        ],
        kmts_formula,
    )
    .expect("kmts bug verdict");
    let kmts_fixed = kmts_verdict(
        "multi_buffer_overflow_fixed_top.sv",
        "buffer_overflow_fixed_top",
        &[
            ("multi_buffer_producer_fixed.sv".to_string(), prod_fixed),
            ("multi_buffer.sv".to_string(), buffer),
        ],
        kmts_formula,
    )
    .expect("kmts fixed verdict");

    // Parity: native and KMTS AGREE on each design (mismatch = 0).
    assert_eq!(
        native_bug, kmts_bug,
        "PARITY MISMATCH on bug: native={native_bug} kmts={kmts_bug}"
    );
    assert_eq!(
        native_fixed, kmts_fixed,
        "PARITY MISMATCH on fixed: native={native_fixed} kmts={kmts_fixed}"
    );

    // Discrimination: the property actually distinguishes bug from fixed on
    // BOTH pipelines (otherwise parity would be a vacuous always-agree).
    assert_ne!(
        native_bug, native_fixed,
        "native verdict must differ bug vs fixed"
    );
    assert_ne!(
        kmts_bug, kmts_fixed,
        "kmts verdict must differ bug vs fixed"
    );

    // Concrete expected polarity: bug overflows (no_overflow FALSE), the
    // fixed backpressure holds (no_overflow TRUE).
    assert!(!native_bug, "bug overflows → no_overflow is FALSE (native)");
    assert!(
        native_fixed,
        "fixed backpressure prevents overflow → no_overflow is TRUE (native)"
    );
}
