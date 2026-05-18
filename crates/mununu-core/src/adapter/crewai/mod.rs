//! Native CrewAI adapter — translates CrewAI JSON crew definitions
//! into CTXDSL via the shared `AdapterIR`.
//!
//! Today supports `process = "sequential"` end-to-end. The crew's
//! agents become per-agent automata (Idle → Executing → Done cycle
//! on `agent_<role>_start` / `agent_<role>_complete`); the tasks
//! become a sequential supervisor automaton. All composed
//! asynchronously per Doc C §C.5 (LLM completion latency is
//! non-deterministic — synchronous one-step rendezvous is unsound
//! for liveness without an explicit fairness annotation).
//!
//! `hierarchical` and `consensual` processes emit a structural
//! warning and fall back to sequential. Native support for those
//! is queued as a follow-up.
//!
//! ## Detection
//!
//! [`CrewaiAdapter::detect`] is content-based:
//! - JSON object (starts with `{`)
//! - top-level `"agents"` array AND `"tasks"` array, OR
//! - top-level `"crew"` key wrapping the same shape
//!
//! File-extension based detection (`.crewai.json` / `.crewai.yaml`
//! / `.crewai.yml`) lives in `adapter::mod::detect_format_by_extension`
//! and is wired in once auto-detection learns the new format.

pub mod ast;
pub mod translate;

use super::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, FormatAdapter,
    SourceFormat, SourceInfo,
};
use ast::CrewaiDocument;

/// CrewAI adapter implementing [`FormatAdapter`].
pub struct CrewaiAdapter;

impl FormatAdapter for CrewaiAdapter {
    fn detect(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with('{') {
            return false;
        }
        // Either `agents` + `tasks` at the top level, or wrapped in a
        // `crew` envelope. Keep the heuristic cheap — `auto_translate`
        // calls every adapter's detect() in sequence.
        let has_agents = trimmed.contains("\"agents\"");
        let has_tasks = trimmed.contains("\"tasks\"");
        let has_crew_envelope = trimmed.contains("\"crew\"");
        has_agents && (has_tasks || has_crew_envelope)
    }

    fn translate(content: &str, _options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        let doc: CrewaiDocument = serde_json::from_str(content).map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("CrewAI JSON parse error: {e}"),
            location: None,
        })?;
        let crew = doc.into_crew();

        if crew.agents.is_empty() {
            // Distinguish "JSON parsed but the schema is empty" from
            // "agents key was an explicit empty array" — both produce
            // an empty agents vec via serde's default, but the
            // empty-schema case usually means the caller passed
            // truncated or wrong content.
            let bytes = content.len();
            let looks_empty_object = content.trim().len() <= 4; // "{}" plus whitespace
            let hint = if looks_empty_object {
                format!(
                    " (received {bytes} bytes — looks like an empty object; check the request content reached the server)"
                )
            } else if !content.contains("\"agents\"") {
                format!(
                    " (received {bytes} bytes with no `agents` key — is this really a CrewAI JSON file?)"
                )
            } else {
                format!(" (received {bytes} bytes — the agents array deserialised as empty)")
            };
            return Err(AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                message: format!("CrewAI document has no agents — nothing to translate.{hint}"),
                location: None,
            });
        }

        let mut warnings: Vec<AdapterWarning> = Vec::new();
        let ir = translate::to_ir(&crew, &mut warnings);

        let result = super::emit::emit(&ir).map_err(|e| AdapterError {
            kind: AdapterErrorKind::EmitError,
            message: format!("CTXDSL emission failed: {e}"),
            location: None,
        })?;

        let signal_count = 0;
        let state_count: usize = ir.automata.iter().map(|a| a.states.len()).sum();
        let property_count = ir.properties.len();

        Ok(AdapterOutput {
            ctxdsl: result.ctxdsl,
            warnings,
            source_info: SourceInfo {
                format: SourceFormat::XState, // shared variant until SourceFormat::Crewai lands in a follow-up
                title: Some(crew.name.unwrap_or_else(|| "Crew".to_string())),
                signal_count,
                state_count,
                property_count,
            },
            sidecars: Vec::new(),
            state_valuations: Default::default(),
            transition_observations: Default::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::WarningKind;

    const TWO_AGENT_SEQ: &str = r#"
    {
      "name": "ResearchAndWriteCrew",
      "agents": [
        { "role": "Researcher", "goal": "Find the truth" },
        { "role": "Writer", "goal": "Make it readable" }
      ],
      "tasks": [
        { "description": "Gather sources", "agent": "Researcher", "expected_output": "List of refs" },
        { "description": "Draft article", "agent": "Writer", "expected_output": "1000-word draft" }
      ],
      "process": "sequential"
    }
    "#;

    const WRAPPED: &str = r#"
    {
      "crew": {
        "name": "Tiny",
        "agents": [
          { "role": "Solo" }
        ],
        "tasks": [
          { "agent": "Solo" }
        ],
        "process": "sequential"
      }
    }
    "#;

    #[test]
    fn detects_flat_crewai_json() {
        assert!(CrewaiAdapter::detect(TWO_AGENT_SEQ));
    }

    #[test]
    fn detects_wrapped_crewai_json() {
        assert!(CrewaiAdapter::detect(WRAPPED));
    }

    #[test]
    fn does_not_detect_xstate_json() {
        let xstate = r#"{"id": "x", "initial": "s0", "states": {"s0": {}}}"#;
        assert!(!CrewaiAdapter::detect(xstate));
    }

    #[test]
    fn does_not_detect_plain_text() {
        assert!(!CrewaiAdapter::detect("hello world"));
        assert!(!CrewaiAdapter::detect(""));
    }

    #[test]
    fn translates_sequential_two_agent_crew() {
        let options = AdapterOptions::default();
        let out = CrewaiAdapter::translate(TWO_AGENT_SEQ, &options).expect("valid JSON");
        // Per-agent automata + 1 supervisor.
        assert!(out.ctxdsl.contains("automaton Agent_Researcher"));
        assert!(out.ctxdsl.contains("automaton Agent_Writer"));
        assert!(out.ctxdsl.contains("Supervisor"));
        // Asynchronous composition.
        assert!(out.ctxdsl.contains("asynchronous"));
        // Default sequential process emits no warnings.
        assert!(
            out.warnings.is_empty(),
            "expected no warnings; got: {:?}",
            out.warnings
        );
    }

    #[test]
    fn translates_wrapped_envelope() {
        let options = AdapterOptions::default();
        let out = CrewaiAdapter::translate(WRAPPED, &options).expect("valid wrapped JSON");
        assert!(out.ctxdsl.contains("automaton Agent_Solo"));
    }

    #[test]
    fn empty_agents_errors() {
        let bad = r#"{"agents": [], "tasks": []}"#;
        let options = AdapterOptions::default();
        let err = CrewaiAdapter::translate(bad, &options).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::IrConsistencyError);
    }

    #[test]
    fn invalid_json_errors() {
        let options = AdapterOptions::default();
        let err = CrewaiAdapter::translate("{not json", &options).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
    }

    #[test]
    fn hierarchical_process_warns_and_falls_back() {
        let hier = r#"
        {
          "agents": [
            { "role": "Manager", "allow_delegation": true },
            { "role": "Worker" }
          ],
          "tasks": [
            { "agent": "Manager" },
            { "agent": "Worker" }
          ],
          "process": "hierarchical"
        }
        "#;
        let options = AdapterOptions::default();
        let out = CrewaiAdapter::translate(hier, &options).unwrap();
        // Translated with a warning, not an error.
        assert!(out.ctxdsl.contains("automaton Agent_Manager"));
        assert_eq!(out.warnings.len(), 1);
        assert!(
            matches!(out.warnings[0].kind, WarningKind::ApproximateTranslation),
            "expected ApproximateTranslation warning"
        );
    }

    #[test]
    fn role_with_spaces_sanitises_to_underscore() {
        let json = r#"
        {
          "agents": [{ "role": "Senior Researcher" }],
          "tasks": [{ "agent": "Senior Researcher" }]
        }
        "#;
        let out = CrewaiAdapter::translate(json, &AdapterOptions::default()).unwrap();
        // Spaces become underscores in the automaton name.
        assert!(out.ctxdsl.contains("Agent_Senior_Researcher"));
    }
}
