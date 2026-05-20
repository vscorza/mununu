//! Native microcode adapter — translates the restricted JSON
//! microcode form defined in plan Part 5 + Part 5.5 into CTXDSL via
//! the shared `AdapterIR`.
//!
//! Restricted form (Part 5):
//!   - one side-effect per step,
//!   - explicit sequencing via `next: <step_id>`,
//!   - resources declared up-front (`regs`, `mem`, `interrupts`),
//!   - memory regions tagged `shared` (rendezvous with cache /
//!     memory automata) or `private` (internal to the microprogram),
//!   - fences are first-class,
//!   - loops are expressed as labelled `next` edges; no implicit
//!     control flow.
//!
//! v1 ships JSON input (`.microcode.json`); a custom-syntax frontend
//! is deferred to v1.1 — the JSON IR is the stable point.
//!
//! ## Detection
//!
//! [`MicrocodeAdapter::detect`] is content-based:
//! - JSON object (starts with `{`)
//! - top-level `"steps"` array
//! - at least one of `"regs"`, `"mem"`, `"interrupts"` — distinguishes
//!   microcode JSON from arbitrary JSON objects with a `"steps"` key.

pub mod ast;
pub mod translate;

use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo,
};
use ast::Microcode;

/// Microcode adapter implementing [`FormatAdapter`].
pub struct MicrocodeAdapter;

impl FormatAdapter for MicrocodeAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('{') {
            return false;
        }
        let has_steps = trimmed.contains("\"steps\"");
        let has_regs = trimmed.contains("\"regs\"");
        let has_mem = trimmed.contains("\"mem\"");
        let has_irq = trimmed.contains("\"interrupts\"");
        has_steps && (has_regs || has_mem || has_irq)
    }

    fn translate(content: &str, _options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let program: Microcode = serde_json::from_str(content).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("microcode JSON parse error: {e}"),
            location: None,
        })?;
        let title = program.name.clone();

        let mut warnings: Vec<AdapterWarning> = Vec::new();
        let ir = translate::to_ir(program, &mut warnings)?;

        let result = super::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let state_count: usize = ir.automata.iter().map(|a| a.states.len()).sum();

        Ok(AdapterOutput {
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::XState, // shared variant until SourceFormat::Microcode lands
                title: Some(title),
                signal_count: 0,
                state_count,
                property_count: 0,
            },
            sidecars: Vec::new(),
            state_valuations: Default::default(),
            transition_observations: Default::default(),
            partition_summary: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const STORE_FENCE_LOAD: &str = r#"
    {
      "name": "store_then_fence_then_load",
      "regs": { "acc": { "width": 32 }, "ptr": { "width": 32 } },
      "mem":  { "x":  { "kind": "shared", "attr": "cacheable" } },
      "steps": [
        { "id": "entry",
          "ops": [{ "op": "write_reg", "reg": "ptr", "value": "0x1000" }],
          "next": "issue_store" },
        { "id": "issue_store",
          "ops": [{ "op": "write_mem", "region": "x", "source_reg": "acc" }],
          "next": "sync_barrier" },
        { "id": "sync_barrier",
          "ops": [{ "op": "fence", "order": "rw" }],
          "next": "observe_done" },
        { "id": "observe_done",
          "ops": [{ "op": "read_mem", "region": "x", "into_reg": "acc" }],
          "next": "halt" },
        { "id": "halt", "ops": [] }
      ]
    }
    "#;

    const PRIVATE_REGION: &str = r#"
    {
      "name": "scratch_loop",
      "mem": { "scratch": { "kind": "private" } },
      "steps": [
        { "id": "init",  "ops": [{ "op": "write_mem", "region": "scratch", "source_reg": "x" }], "next": "done" },
        { "id": "done",  "ops": [] }
      ]
    }
    "#;

    const IRQ_FLOW: &str = r#"
    {
      "name": "irq_handler",
      "interrupts": { "ext_7": { "maskable": true } },
      "steps": [
        { "id": "entry", "ops": [{ "op": "irq_ack", "source": "ext_7" }], "next": "done" },
        { "id": "done", "ops": [] }
      ]
    }
    "#;

    #[test]
    fn detects_canonical_microcode_json() {
        assert!(MicrocodeAdapter::detect(STORE_FENCE_LOAD));
        assert!(MicrocodeAdapter::detect(PRIVATE_REGION));
        assert!(MicrocodeAdapter::detect(IRQ_FLOW));
    }

    #[test]
    fn does_not_detect_unrelated_json() {
        // Lacks both `steps` and the resource keys.
        let xstate = r#"{"id": "x", "initial": "s0", "states": {"s0": {}}}"#;
        assert!(!MicrocodeAdapter::detect(xstate));
        // Has `steps` but no resource declarations — too generic.
        let bare_steps = r#"{"steps": [{"id": "a"}]}"#;
        assert!(!MicrocodeAdapter::detect(bare_steps));
    }

    #[test]
    fn translates_store_fence_load_into_5_state_automaton() {
        let out =
            MicrocodeAdapter::translate(STORE_FENCE_LOAD, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("automaton store_then_fence_then_load"));
        // 5 states, each with the canonical step name.
        for st in [
            "state entry",
            "state issue_store",
            "state sync_barrier",
            "state observe_done",
            "state halt",
        ] {
            assert!(out.ctxdsl.contains(st), "missing `{st}`:\n{}", out.ctxdsl);
        }
        // Initial state — first step.
        assert!(out.ctxdsl.contains("state entry initial"));
        // Canonical rendezvous labels for shared memory + fence.
        assert!(out.ctxdsl.contains("wr_mem_x"));
        assert!(out.ctxdsl.contains("rd_mem_x"));
        assert!(out.ctxdsl.contains("fence_rw"));
        // Internal-only register write.
        assert!(out.ctxdsl.contains("wr_reg_ptr"));
        assert_eq!(out.source_info.state_count, 5);
    }

    #[test]
    fn private_memory_region_emits_private_labels() {
        let out = MicrocodeAdapter::translate(PRIVATE_REGION, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("wr_priv_scratch"));
        // No `wr_mem_scratch` — the private tag suppresses rendezvous.
        assert!(!out.ctxdsl.contains("wr_mem_scratch"));
    }

    #[test]
    fn irq_ack_emits_canonical_label() {
        let out = MicrocodeAdapter::translate(IRQ_FLOW, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("irq_ack_ext_7"));
    }

    #[test]
    fn empty_steps_errors() {
        let bad = r#"{"name": "x", "regs": {"a": {"width": 1}}, "steps": []}"#;
        let err = MicrocodeAdapter::translate(bad, &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::IrConsistencyError);
    }

    #[test]
    fn missing_name_errors() {
        let bad = r#"{"regs": {"a": {"width": 1}}, "steps": [{"id": "s"}]}"#;
        let err = MicrocodeAdapter::translate(bad, &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::ParseError); // serde-level "missing field name"
    }

    #[test]
    fn duplicate_step_id_errors() {
        let bad = r#"
        {
          "name": "dup",
          "regs": {"a": {"width": 1}},
          "steps": [
            { "id": "x", "ops": [] },
            { "id": "x", "ops": [] }
          ]
        }
        "#;
        let err = MicrocodeAdapter::translate(bad, &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::IrConsistencyError);
        assert!(err.message.contains("duplicate"), "got: {}", err.message);
    }

    #[test]
    fn unknown_next_errors() {
        let bad = r#"
        {
          "name": "bad_next",
          "regs": {"a": {"width": 1}},
          "steps": [
            { "id": "a", "ops": [], "next": "ghost" }
          ]
        }
        "#;
        let err = MicrocodeAdapter::translate(bad, &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::IrConsistencyError);
        assert!(err.message.contains("ghost"), "got: {}", err.message);
    }

    #[test]
    fn tag_field_folds_into_emitted_label() {
        // The microcode v1 lets multiple microcode instances share
        // one memory automaton — each ops can declare a `tag` that
        // gets folded into the emitted label, so caches /
        // memories can distinguish writes by issuer.
        //
        // Both ops live on non-terminal steps so their labels fire:
        // translate.rs drops ops declared on terminal steps (no
        // `next`) with an `ApproximateTranslation` warning, which
        // would otherwise silently elide the `rd_mem` label.
        let json = r#"
        {
          "name": "tagged",
          "mem": { "x": { "kind": "shared" } },
          "steps": [
            { "id": "a",
              "ops": [{ "op": "write_mem", "region": "x", "tag": "core_0" }],
              "next": "b" },
            { "id": "b",
              "ops": [{ "op": "read_mem", "region": "x", "tag": "core_1" }],
              "next": "halt" },
            { "id": "halt", "ops": [] }
          ]
        }
        "#;
        let out = MicrocodeAdapter::translate(json, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("wr_mem_x_core_0"));
        assert!(out.ctxdsl.contains("rd_mem_x_core_1"));
    }

    #[test]
    fn invalid_json_errors() {
        let err = MicrocodeAdapter::translate("{not json", &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
    }

    #[test]
    fn step_id_with_dashes_sanitises_to_underscore() {
        let json = r#"
        {
          "name": "with-dashes",
          "regs": {"a": {"width": 1}},
          "steps": [
            { "id": "step-1", "ops": [], "next": "step-2" },
            { "id": "step-2", "ops": [] }
          ]
        }
        "#;
        let out = MicrocodeAdapter::translate(json, &AdapterOptions::default()).unwrap();
        assert!(out.ctxdsl.contains("automaton with_dashes"));
        assert!(out.ctxdsl.contains("state step_1"));
        assert!(out.ctxdsl.contains("state step_2"));
    }
}
