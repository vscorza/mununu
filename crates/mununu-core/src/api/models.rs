use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ============================================================================
// Common Types
// ============================================================================

/// File content with optional name
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileContent {
    pub name: String,
    pub content: String,
}

/// Sidecar file content
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SidecarFile {
    pub name: String,
    pub content: String,
}

// ============================================================================
// Context Summarize Endpoints
// ============================================================================

/// Request for context summarization
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct ContextSummarizeRequest {
    pub context: FileContent,
    #[serde(default)]
    pub sidecars: Vec<SidecarFile>,
    #[serde(default = "default_summarize_format")]
    pub format: SummarizeFormat,
}

fn default_summarize_format() -> SummarizeFormat {
    SummarizeFormat::Json
}

/// Summary output format
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SummarizeFormat {
    Json,
    Table,
}

/// Response from context summarization
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ContextSummarizeResponse {
    pub success: bool,
    pub summary: ContextSummary,
}

/// Context summary information
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextSummary {
    pub context_name: String,
    pub automata: Vec<AutomatonSummary>,
    pub formulas_count: usize,
    pub controllers_count: usize,
    #[serde(default)]
    pub controllers: Vec<ControllerSummary>,
}

/// Automaton summary information
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutomatonSummary {
    pub name: String,
    pub states_count: usize,
    pub transitions_count: usize,
}

/// Controller summary information
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ControllerSummary {
    pub name: String,
    pub source: String,
    pub formula: String,
    pub realizable: bool,
    pub states_count: usize,
    pub transitions_count: usize,
}

// ============================================================================
// Context Synthesize Endpoints
// ============================================================================

/// Request for controller synthesis
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct ContextSynthesizeRequest {
    pub context: FileContent,
    #[serde(default)]
    pub sidecars: Vec<SidecarFile>,
    pub automaton: String,
    /// Formula name to synthesise (mutually exclusive with `template_ref`).
    pub formula: Option<String>,
    /// Template reference to instantiate (mutually exclusive with `formula`).
    #[serde(default)]
    pub template_ref: Option<crate::adapter::templates::TemplateRef>,
    #[serde(default)]
    pub options: SynthesisOptions,
}

/// Synthesis options
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct SynthesisOptions {
    #[serde(default)]
    pub minimize: bool,
    #[serde(default)]
    pub diagnostics: DiagnosticsOptions,
    /// Legacy positional-strategy flag. When `controller_mode` is set, that
    /// takes precedence and `extract_strategy` is ignored. Kept for
    /// backwards compatibility.
    #[serde(default)]
    pub extract_strategy: bool,
    /// Controller extraction mode. Case-insensitive. One of:
    /// `"projection"` (default), `"functional"`, `"permissive"`,
    /// `"signature-memory"`, `"product-game"`, `"parity-game"`.
    /// When `Some`, overrides `extract_strategy`.
    #[serde(default)]
    pub controller_mode: Option<String>,
    /// Output format for the controller: "ctxdsl" (default), "xstate", "systemverilog".
    pub output_format: Option<String>,
}

/// Diagnostics options
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct DiagnosticsOptions {
    #[serde(default)]
    pub counterexample: bool,
    #[serde(default)]
    pub counterstrategy: bool,
    #[serde(default)]
    pub deadlock_traces: bool,
    pub max_counter_traces: Option<u32>,
}

/// Response from controller synthesis
#[derive(Debug, Serialize, PartialEq)]
pub struct ContextSynthesizeResponse {
    pub success: bool,
    pub realizable: bool,
    pub controller: Option<FileContent>,
    /// Controller in the requested native format (xstate, systemverilog).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_native: Option<FileContent>,
    pub diagnostics: SynthesisDiagnostics,
    /// Counterstrategy graph for unrealizable cases (environment's winning strategy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterstrategy: Option<CounterstrategyResult>,
}

/// Synthesis diagnostics (matches ControllerDiagnostics structure)
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct SynthesisDiagnostics {
    #[serde(default)]
    pub messages: Vec<String>,
    #[serde(default)]
    pub violating_initials: Vec<String>,
    pub counterexample_trace: Option<Vec<String>>,
    #[serde(default)]
    pub counterstrategy_traces: Vec<Vec<String>>,
    #[serde(default)]
    pub deadlock_traces: Vec<Vec<String>>,
    pub minimization: Option<MinimizationReport>,
    #[serde(default)]
    pub proof_obligations: Vec<ProofObligation>,
    /// Lasso traces for liveness counterexamples: prefix + repeating cycle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lasso_traces: Vec<LassoTraceApi>,
}

/// Lasso trace: finite prefix followed by infinitely repeating cycle.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LassoTraceApi {
    pub prefix: Vec<String>,
    pub cycle: Vec<String>,
    /// Transition labels between consecutive prefix states.
    /// `prefix_labels[i]` is the label from `prefix[i]` to `prefix[i+1]` (or `cycle[0]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefix_labels: Vec<String>,
    /// Transition labels between consecutive cycle states.
    /// The last element is the label from the last cycle state back to `cycle[0]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycle_labels: Vec<String>,
}

/// Minimization report
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MinimizationReport {
    pub removed_states: usize,
    pub removed_transitions: usize,
    #[serde(default)]
    pub merged_states: Vec<String>,
}

/// Proof obligation
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProofObligation {
    pub state: String,
    pub detail: Option<String>,
}

// ============================================================================
// Context Import Endpoint
// ============================================================================

/// Request for importing an external format into CTXDSL.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ContextImportRequest {
    /// Raw file content in the source format.
    pub content: String,
    /// Source format hint: "auto", "tlsf", "aiger", "btor2", "promela", "xstate",
    /// "systemverilog" (hand-written SV adapter), "sv-yosys" (Yosys-driven SV
    /// elaboration → BTOR2 → CLTS — Phase 1 RTL roadmap), "extraction".
    #[serde(default = "default_import_format")]
    pub format: String,
    /// Original filename (used for extension-based detection if format is "auto").
    pub filename: Option<String>,
    /// Optional sidecar content (.mununu.json for SV, .espec.json for extraction).
    /// When provided, the adapter uses this for abstraction/property configuration.
    #[serde(default)]
    pub sidecar: Option<String>,
    /// Additional source files (for multi-module SV compositions).
    #[serde(default)]
    pub additional_sources: Vec<FileContent>,
    /// When `format == "sv-yosys"`, opt in to the `sv2v` preprocessor pass
    /// before Yosys elaboration. Required for modern open-source SV
    /// dialects (SV2009/2012 module-header `import pkg::*;` etc.) that
    /// Yosys's built-in `read_verilog -sv` parser does not accept.
    /// Requires `sv2v` (zachjs/sv2v) on `$PATH` or in `MUNUNU_SV2V_PATH`.
    /// Mirrors the CLI's `--preprocessor sv2v` flag. Ignored by all
    /// other formats.
    #[serde(default)]
    pub use_sv2v: bool,
    /// R.6.7 / V.6 (2026-06-09) — predicate set for the
    /// controllability-aware predicate-cube lift. Each entry is a
    /// `{name, register, value}` triple identifying a register-value
    /// equality predicate that bounds the abstraction. When non-empty
    /// AND `controllable_inputs` is non-empty AND `format == "btor2"`
    /// (or `"sv-yosys"`, which produces BTOR2 internally), the
    /// `predicate_cube_lift` is invoked + the resulting KMTS is
    /// returned. When empty, the import path is unchanged
    /// (legacy CTXDSL emit).
    ///
    /// Mirrors the CLI's `--predicate NAME:REG=VALUE` flag on
    /// `mununu btor2 cegar`.
    #[serde(default)]
    pub predicates: Vec<PredicateSpecRequest>,
    /// R.6.7 / V.6 (2026-06-09) — names of BTOR2 input symbols the
    /// controller drives. Mirrors the CLI's `--controllable-input`
    /// flag. When non-empty AND `predicates` is non-empty, opts the
    /// import path into the R.6.6 controllability-aware lift —
    /// boolean inputs are partitioned into env (uncontrollable) +
    /// ctrl (controllable) classes per-symbol-name + the lift emits
    /// per-combo dual-label transitions.
    #[serde(default)]
    pub controllable_inputs: Vec<String>,

    /// R-S2b.6 / P3 (§Phase 11 slot-3 close follow-up, 2026-06-12)
    /// — filesystem path to the original SystemVerilog source for
    /// the BTOR2 input. Mirrors the CLI's `--sv-source <PATH>`
    /// flag. When set AND the supplied `sidecar` declares a
    /// `simulate_reset` block AND a Verilator binary is
    /// discoverable on the server, the BTOR2 bit-blaster runs a
    /// short concrete reset simulation and feeds post-reset
    /// register valuations into the EnumValues discriminators
    /// (§Phase 9 R-S2b strategy). Default `None` preserves the
    /// legacy "no reset simulation" behaviour.
    #[serde(default)]
    pub sv_source_path: Option<String>,

    /// R-S6.6 / P3 (§Phase 11 slot-3 close follow-up, 2026-06-12)
    /// — filesystem path to the sidecar JSON file. Mirrors the
    /// CLI's `--sidecar <PATH>` flag. When set, the bit-blaster's
    /// `apply_vcd_trace_seeding` orchestration uses this path's
    /// parent directory to resolve relative `vcd_traces` entries
    /// declared in the sidecar (§Phase 9 R-S6 strategy). The
    /// `sidecar` field above carries the file's CONTENT; this
    /// field carries the file's PATH so the bit-blaster can
    /// resolve relative paths within the sidecar's directory
    /// scope. Default `None` — relative VCD paths emit an
    /// AdapterWarning and fall through.
    #[serde(default)]
    pub sidecar_path: Option<String>,
}

/// R.6.7 / V.6 (2026-06-09) — predicate-spec request shape. Mirrors
/// the CLI's `--predicate NAME:REG=VALUE` triple format. Bridges to
/// [`crate::adapter::btor2::kmts_lift::PredicateSpec`].
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct PredicateSpecRequest {
    /// Human-readable predicate name (e.g. `"burst_zero"`).
    pub name: String,
    /// BTOR2 register symbol the predicate is anchored on.
    pub register: String,
    /// Integer value the predicate witnesses (`register == value`).
    pub value: u64,
}

fn default_import_format() -> String {
    "auto".to_string()
}

/// Response from importing an external format.
#[derive(Debug, Serialize, PartialEq)]
pub struct ContextImportResponse {
    pub success: bool,
    /// Translated CTXDSL content.
    pub ctxdsl: String,
    /// Detected source format.
    pub source_format: String,
    /// Translation warnings.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Metadata about the translated source.
    pub signal_count: usize,
    pub state_count: usize,
    pub property_count: usize,
    /// State valuations for structured predicate matching (when available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_valuations: Option<serde_json::Value>,
    /// Per-transition Mealy observations (when the adapter emits them).
    /// Keyed by automaton name; each entry is a list of
    /// `{source, target, labels, observations}` rows. Used by trace renderers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_observations: Option<serde_json::Value>,
}

// ============================================================================
// Context Graphs Endpoints
// ============================================================================

/// Request for graph generation
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct ContextGraphsRequest {
    pub context: FileContent,
    #[serde(default)]
    pub sidecars: Vec<SidecarFile>,
    pub automaton: Option<String>,
    #[serde(default = "default_graph_types")]
    pub graph_types: Vec<GraphType>,
    #[serde(default)]
    pub include_controllers: bool,
    /// Override the minimize setting for controller graphs.
    pub minimize_controllers: Option<bool>,
}

fn default_graph_types() -> Vec<GraphType> {
    vec![GraphType::Dsl]
}

/// Graph type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GraphType {
    Dsl,
    Unrolled,
}

/// Response from graph generation
#[derive(Debug, Serialize, PartialEq)]
pub struct ContextGraphsResponse {
    pub success: bool,
    pub context: ContextSummary,
    pub graphs: Vec<GraphData>,
}

/// Graph data with metadata
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphData {
    pub automaton: String,
    pub graph_type: GraphTypeResponse,
    pub elements: Vec<GraphElement>,
    pub metadata: GraphMetadata,
}

/// Graph type in response (serialized as string)
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GraphTypeResponse {
    Dsl,
    Unrolled,
    Controller,
}

impl From<GraphType> for GraphTypeResponse {
    fn from(gt: GraphType) -> Self {
        match gt {
            GraphType::Dsl => GraphTypeResponse::Dsl,
            GraphType::Unrolled => GraphTypeResponse::Unrolled,
        }
    }
}

/// Graph metadata
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphMetadata {
    pub states_count: usize,
    pub transitions_count: usize,
    #[serde(default)]
    pub initial_states: Vec<String>,
}

/// Graph element (node or edge) for visualization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphElement {
    pub data: GraphElementData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<GraphPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classes: Option<String>,
}

/// Graph element data (node or edge)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum GraphElementData {
    Node {
        id: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        vars: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        actions: Vec<String>,
        /// Structured per-state variable valuations (e.g. `{is_red: "0", phase: "green"}`).
        /// Sourced from `Clts::state_valuation()` — populated by adapter side-channels
        /// (SV Kripke, BTOR2, extraction) injected through `ContextDoc.state_valuations`.
        #[serde(skip_serializing_if = "Option::is_none")]
        valuations: Option<BTreeMap<String, String>>,
    },
    Edge {
        id: String,
        source: String,
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        action_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        guard: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect: Option<String>,
        /// R.5 Item K sub-item K.4 (2026-06-05) — CTXDSL transition
        /// modality, rendered as `"sharp"` / `"may_only"` /
        /// `"must_hyper_only"`. Absent on edges that don't come from
        /// a CLTS transition (start-arrows, controller bridges).
        /// Default (Sharp) is also serialized as `None` to keep
        /// pre-K.4 JSON byte-for-byte compatible for the dominant case.
        #[serde(skip_serializing_if = "Option::is_none")]
        modality: Option<String>,
    },
}

/// Graph position coordinates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphPosition {
    pub x: f64,
    pub y: f64,
}

// ============================================================================
// Context Verify Endpoints
// ============================================================================

/// Request for context verification (formula evaluation)
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct ContextVerifyRequest {
    pub context: FileContent,
    #[serde(default)]
    pub sidecars: Vec<SidecarFile>,
    /// If omitted, evaluate ALL formulas defined in the context.
    /// Mutually exclusive with `template_ref`.
    pub formula: Option<String>,
    /// Template reference to instantiate (mutually exclusive with `formula`).
    #[serde(default)]
    pub template_ref: Option<crate::adapter::templates::TemplateRef>,
    /// If omitted, use the formula's target automaton(s).
    pub automaton: Option<String>,
    /// When true, compute counterstrategy for failed formulas via formula inversion.
    #[serde(default)]
    pub counterstrategy: bool,
    /// When true and counterstrategy is enabled, minimize the counterstrategy automaton.
    #[serde(default)]
    pub minimize_counterstrategy: bool,
    /// Labels to hide (reclassify as internal) before evaluation.
    #[serde(default)]
    pub hide: Vec<String>,
    /// When true, apply bisimulation minimization before evaluation.
    #[serde(default)]
    pub minimize: bool,
    /// Stub .espec.json content to compose as sidecars (interface automata).
    #[serde(default)]
    pub stubs: Vec<SidecarFile>,
}

/// Response from context verification
#[derive(Debug, Serialize, PartialEq)]
pub struct ContextVerifyResponse {
    pub success: bool,
    pub all_satisfied: bool,
    pub results: Vec<FormulaVerificationResult>,
}

/// Verification result for a single formula–automaton pair
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FormulaVerificationResult {
    pub formula_name: String,
    pub automaton: String,
    pub satisfied: bool,
    pub total_states: usize,
    pub satisfying_states: usize,
    pub initial_states: Vec<String>,
    pub initial_satisfying: Vec<String>,
    pub initial_violating: Vec<String>,
    pub satisfying_state_names: Vec<String>,
    /// Present when counterstrategy was requested and formula is not satisfied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterstrategy: Option<CounterstrategyResult>,
}

/// Counterstrategy result: the environment's winning region and strategy as a graph.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CounterstrategyResult {
    /// States where the environment can force the property to be violated.
    pub environment_winning_states: Vec<String>,
    /// Cytoscape graph elements for the counterstrategy automaton.
    pub graph_elements: Vec<GraphElement>,
    /// The inverted formula (for transparency/debugging).
    pub inverted_formula: String,
    /// Whether bisimulation minimization was applied.
    pub minimized: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_content_serialization() {
        let file = FileContent {
            name: "test.ctxdsl".to_string(),
            content: "context test {}".to_string(),
        };
        let json = serde_json::to_string(&file).unwrap();
        let deserialized: FileContent = serde_json::from_str(&json).unwrap();
        assert_eq!(file, deserialized);
    }

    #[test]
    fn test_graph_type_conversion() {
        assert_eq!(
            GraphTypeResponse::from(GraphType::Dsl),
            GraphTypeResponse::Dsl
        );
        assert_eq!(
            GraphTypeResponse::from(GraphType::Unrolled),
            GraphTypeResponse::Unrolled
        );
    }

    #[test]
    fn test_graph_type_serialization() {
        let graph_type = GraphType::Dsl;
        let json = serde_json::to_string(&graph_type).unwrap();
        assert_eq!(json, "\"dsl\"");

        let deserialized: GraphType = serde_json::from_str("\"unrolled\"").unwrap();
        assert_eq!(deserialized, GraphType::Unrolled);
    }

    #[test]
    fn test_graph_element_data_node() {
        let node = GraphElementData::Node {
            id: "node1".to_string(),
            label: "State 1".to_string(),
            parent: Some("parent".to_string()),
            vars: vec!["x".to_string(), "y".to_string()],
            actions: vec!["start".to_string()],
            valuations: None,
        };
        let json = serde_json::to_string(&node).unwrap();
        let deserialized: GraphElementData = serde_json::from_str(&json).unwrap();
        assert_eq!(node, deserialized);
    }

    #[test]
    fn test_graph_element_data_edge() {
        let edge = GraphElementData::Edge {
            id: "edge1".to_string(),
            source: "node1".to_string(),
            target: "node2".to_string(),
            label: Some("transition".to_string()),
            action: Some("action".to_string()),
            action_type: Some("controllable".to_string()),
            guard: Some("x > 0".to_string()),
            effect: Some("x := x + 1".to_string()),
            modality: None,
        };
        let json = serde_json::to_string(&edge).unwrap();
        let deserialized: GraphElementData = serde_json::from_str(&json).unwrap();
        assert_eq!(edge, deserialized);
    }

    #[test]
    fn test_graph_element_serialization() {
        let element = GraphElement {
            data: GraphElementData::Node {
                id: "node1".to_string(),
                label: "State 1".to_string(),
                parent: None,
                vars: vec![],
                actions: vec![],
                valuations: None,
            },
            position: Some(GraphPosition { x: 100.0, y: 200.0 }),
            classes: Some("state start".to_string()),
        };
        let json = serde_json::to_string(&element).unwrap();
        let deserialized: GraphElement = serde_json::from_str(&json).unwrap();
        assert_eq!(element, deserialized);
    }

    #[test]
    fn test_default_graph_types() {
        let types = default_graph_types();
        assert_eq!(types, vec![GraphType::Dsl]);
    }

    #[test]
    fn test_default_summarize_format() {
        let format = default_summarize_format();
        assert_eq!(format, SummarizeFormat::Json);
    }

    #[test]
    fn test_context_graphs_request_deserialization() {
        let json = r#"
        {
            "context": {
                "name": "test.ctxdsl",
                "content": "context test {}"
            },
            "sidecars": [],
            "automaton": null,
            "graph_types": ["dsl", "unrolled"]
        }
        "#;
        let request: ContextGraphsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.context.name, "test.ctxdsl");
        assert_eq!(request.automaton, None);
        assert_eq!(request.graph_types.len(), 2);
    }

    #[test]
    fn test_synthesis_options_defaults() {
        let options: SynthesisOptions = serde_json::from_str("{}").unwrap();
        assert!(!options.minimize);
        assert!(!options.diagnostics.counterexample);
    }
}

// ============================================================================
// Extraction Endpoints
// ============================================================================

/// Response for listing available domain profiles.
#[derive(Debug, Serialize)]
pub struct ExtractionDomainsResponse {
    pub profiles: Vec<DomainProfileInfo>,
}

/// Summary of a domain profile.
#[derive(Debug, Serialize)]
pub struct DomainProfileInfo {
    pub name: String,
    pub language: String,
    pub description: String,
}

/// Response for listing supported composition modes (sync vs async).
/// Surfaces the same options the espec / extract config accepts in
/// `composition.type`. Mirrors the shape of `ExtractionDomainsResponse`.
#[derive(Debug, Serialize)]
pub struct CompositionModesResponse {
    pub modes: Vec<CompositionModeInfo>,
}

/// Description of one composition mode.
#[derive(Debug, Serialize)]
pub struct CompositionModeInfo {
    pub name: &'static str,
    pub description: &'static str,
}

/// Phase B request: scan source for concurrency idioms and propose
/// `composition.instances[]` / `shared[]` blocks. Output is
/// suggestion-grade — the user reviews each finding before promoting
/// it into the extract config.
#[derive(Debug, Deserialize)]
pub struct ProposeCompositionRequest {
    /// Source content to scan.
    pub source: String,
    /// Source language (`typescript` / `python` / `rust`).
    /// When omitted, the API requires the caller to specify it — there
    /// is no `source.file` extension to infer from at this endpoint.
    pub language: Option<String>,
}

/// Phase B response: list of detected concurrency findings, in source
/// order. An empty `findings` list is the common case (no concurrency
/// patterns present); not an error.
#[derive(Debug, Serialize)]
pub struct ProposeCompositionResponse {
    pub findings:
        Vec<crate::adapter::extraction::ast_extract::concurrency_detect::DetectedConcurrency>,
}

/// Request for AST-based extraction from source code.
#[derive(Debug, Deserialize)]
pub struct ExtractionExtractRequest {
    /// Extraction config content (.extract.json).
    pub config: String,
    /// Source code content.
    pub source: String,
    /// Source language (typescript, python, rust). Auto-detected if omitted.
    pub language: Option<String>,
}

/// Response from AST-based extraction.
#[derive(Debug, Serialize)]
pub struct ExtractionExtractResponse {
    pub success: bool,
    /// Generated .espec.json content.
    pub espec: String,
    /// Extraction warnings.
    pub warnings: Vec<String>,
    /// Automata extracted.
    pub automata: Vec<ExtractionAutomatonInfo>,
}

/// Summary of an extracted automaton.
#[derive(Debug, Serialize)]
pub struct ExtractionAutomatonInfo {
    pub id: String,
    pub state_count: usize,
    pub transition_count: usize,
}

// ============================================================================
// Extraction Validate Endpoint
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ExtractionValidateRequest {
    pub spec: String,
    pub source: String,
    #[serde(default = "default_drift_window")]
    pub drift_window: usize,
}

fn default_drift_window() -> usize {
    5
}

#[derive(Debug, Serialize)]
pub struct ExtractionValidateResponse {
    pub success: bool,
    pub summary: ValidationSummaryApi,
    pub anchors: Vec<AnchorResultApi>,
    pub uncovered: Vec<UncoveredAccessApi>,
    pub commit_match: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ValidationSummaryApi {
    pub total: usize,
    pub exact: usize,
    pub drifted: usize,
    pub mismatch: usize,
    pub error: usize,
    pub uncovered_accesses: usize,
}

#[derive(Debug, Serialize)]
pub struct AnchorResultApi {
    pub id: String,
    pub section: String,
    pub status: String,
    pub line: Option<u32>,
    pub found_line: Option<u32>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UncoveredAccessApi {
    pub line: u32,
    pub field: String,
    pub content: String,
}

// ============================================================================
// Context Predicates Endpoint
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ContextPredicatesRequest {
    pub context: FileContent,
    #[serde(default)]
    pub sidecars: Vec<SidecarFile>,
    pub automaton: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ContextPredicatesResponse {
    pub success: bool,
    pub predicates: std::collections::HashMap<String, Vec<String>>,
}

// ============================================================================
// BTOR2 CEGAR Endpoint (U.0 slot-6 refinement-trace viewer)
// ============================================================================

/// U.0 (slot 6) — request for the CEGAR refinement endpoint
/// (`POST /api/v1/btor2/cegar`). Mirrors the CLI `mununu btor2 cegar`:
/// runs the predicate-abstraction-refinement loop over a BTOR2 design and
/// returns the per-iteration refinement trace the UI viewer renders.
#[derive(Debug, Deserialize)]
pub struct Btor2CegarRequest {
    /// BTOR2 source content.
    pub content: String,
    /// μ-calculus formula evaluated over the lifted KMTS.
    pub formula: String,
    /// Initial predicate set (bootstraps the `2^|P|` cube space). At least
    /// one entry is required. Reuses the `PredicateSpecRequest` shape.
    pub predicates: Vec<PredicateSpecRequest>,
    /// Optional R.6.6 controllability split — controller-driven input
    /// symbols (mirrors `--controllable-input`).
    #[serde(default)]
    pub controllable_inputs: Vec<String>,
    /// Predicate-discovery source: `"wp"` (default) | `"craig"`.
    #[serde(default)]
    pub predicate_source: Option<String>,
    /// Max CEGAR iterations (default 16).
    #[serde(default)]
    pub max_iterations: Option<usize>,
    /// Must-edge inference policy (kebab-case; default `"off"`):
    /// `"sampling-confluence"` | `"smt-per-target"` |
    /// `"smt-per-target-standard"` | `"smt-hyper-must"`.
    #[serde(default)]
    pub must_edge_inference: Option<String>,
    /// May-edge inference policy (kebab-case; default `"off"`):
    /// `"smt-all-pairs"` for the sound all-pairs may-relation. Mirrors the
    /// CLI `--may-edge-inference`.
    #[serde(default)]
    pub may_edge_inference: Option<String>,
    /// R-S8 symbolic-init config-values, one entry per register as
    /// `"REG=v1,v2,..."` (mirrors the CLI `--config-values`). Each register's
    /// admissible power-up set seeds the predicate-cube initial states. Empty =
    /// no synthetic init sidecar.
    #[serde(default)]
    pub config_values: Vec<String>,
    /// CTXDSL Phase 2 (2026-06-22) — opt-in (default `false`): when set, the
    /// response gains a `ctxdsl` field carrying the final refined cube model
    /// and the checked formula as a self-contained CTXDSL document (the
    /// `predicates_3v` Kleene labels, transition modality, and a
    /// `mu_formulas` block). Mirrors the CLI `--emit-ctxdsl`.
    #[serde(default)]
    pub emit_ctxdsl: bool,
}

/// cegar-extraction Stage 2 (2026-06-22) — request for the SV-direct
/// CEGAR endpoint (`POST /api/v1/sv/cegar`). Mirrors the CLI
/// `mununu sv cegar`: lifts SystemVerilog to a single flattened BTOR2
/// (sv2v + Yosys) in one call, then runs the same predicate-abstraction
/// refinement loop as `/btor2/cegar` and returns the same
/// [`Btor2CegarResponse`]. The CEGAR fields below are identical to
/// [`Btor2CegarRequest`]; only the source half differs (SV + Yosys
/// options instead of raw BTOR2 `content`).
#[derive(Debug, Deserialize)]
pub struct SvCegarRequest {
    /// SystemVerilog primary source content.
    pub source: String,
    /// Additional SV source files (multi-file designs / packages /
    /// `include` targets), staged alongside the primary source.
    #[serde(default)]
    pub additional_sources: Vec<FileContent>,
    /// Top module name. Recommended for multi-module designs so Yosys
    /// flattens from the right root; `None` lets Yosys auto-detect.
    #[serde(default)]
    pub top: Option<String>,
    /// Run sv2v before Yosys (required for modern SV: module-header
    /// `import pkg::*;`, structs, interfaces). Mirrors the CLI
    /// `--preprocess-sv2v`.
    #[serde(default)]
    pub use_sv2v: bool,
    /// Yosys `setundef -anyseq` (per-cycle havoc on undefined nets).
    /// Mirrors the CLI `--setundef-anyseq`.
    #[serde(default)]
    pub setundef_anyseq: bool,
    /// Yosys `setundef -anyconst` (one nondeterministic constant input
    /// per undefined bit — the Caliptra CWE-1245 power-up policy).
    /// Mirrors the CLI `--setundef-anyconst`.
    #[serde(default)]
    pub setundef_anyconst: bool,

    // --- CEGAR parameters (identical to Btor2CegarRequest) ---
    /// μ-calculus formula evaluated over the lifted KMTS.
    pub formula: String,
    /// Initial predicate set (bootstraps the `2^|P|` cube space).
    pub predicates: Vec<PredicateSpecRequest>,
    /// R.6.6 controllability split — controller-driven input symbols.
    #[serde(default)]
    pub controllable_inputs: Vec<String>,
    /// Predicate-discovery source: `"wp"` (default) | `"craig"`.
    #[serde(default)]
    pub predicate_source: Option<String>,
    /// Max CEGAR iterations (default 16).
    #[serde(default)]
    pub max_iterations: Option<usize>,
    /// Must-edge inference policy (default `"off"`).
    #[serde(default)]
    pub must_edge_inference: Option<String>,
    /// May-edge inference policy (default `"off"`).
    #[serde(default)]
    pub may_edge_inference: Option<String>,
    /// R-S8 symbolic-init config-values (`"REG=v1,v2,..."`).
    #[serde(default)]
    pub config_values: Vec<String>,
    /// CTXDSL Phase 2 — opt-in CTXDSL of the final refined model + formula.
    #[serde(default)]
    pub emit_ctxdsl: bool,
}

/// XL.6a — SVA-extraction endpoint (`POST /api/v1/sv/extract-sva`). Mirrors the
/// CLI `mununu sv extract-sva`: runs the slang SVA front-end over the SV
/// source(s) and returns the translated mu-calculus property set. slang is a
/// full SV-2017 parser, so no sv2v / Yosys / `top` is needed; no model
/// verification happens here (that is `/sv/verify-auto`).
#[derive(Debug, Deserialize)]
pub struct SvExtractSvaRequest {
    /// SystemVerilog primary source content.
    pub source: String,
    /// Additional SV sources (packages / `include` targets — e.g. the standard
    /// OpenTitan `prim_assert` macros), staged alongside + on the include path.
    #[serde(default)]
    pub additional_sources: Vec<FileContent>,
}

/// Response for `/api/v1/sv/extract-sva` — mirrors
/// [`crate::adapter::slang::translate::TranslationReport`].
#[derive(Debug, Serialize)]
pub struct SvExtractSvaResponse {
    pub translated: Vec<TranslatedAssertionView>,
    pub unsupported: Vec<UnsupportedAssertionView>,
    pub required_shadows: Vec<ShadowSignalView>,
}

/// A successfully translated assertion (mirrors `TranslatedAssertion`).
#[derive(Debug, Serialize)]
pub struct TranslatedAssertionView {
    pub name: String,
    /// `"assert"` | `"assume"` | `"cover"`.
    pub kind: String,
    /// mu-calculus formula (parses via the evaluator's parser).
    pub formula: String,
    /// XL.2 `AG EF` recoverability companion — `Some` only for covers.
    pub recoverability_companion: Option<String>,
}

/// An assertion outside the supported fragment (mirrors `UnsupportedAssertion`).
#[derive(Debug, Serialize)]
pub struct UnsupportedAssertionView {
    pub name: String,
    pub kind: Option<String>,
    pub reason: String,
}

/// A `__past` shadow register a translated formula needs (mirrors `ShadowSignal`).
#[derive(Debug, Serialize)]
pub struct ShadowSignalView {
    pub base: String,
    pub width: u32,
}

/// XL.6b — automated SVA verification endpoint (`POST /api/v1/sv/verify-auto`).
/// Mirrors the CLI `mununu sv verify-auto`: extract the design's SVA, lift, and
/// verify each property against the model with no sidecar. slang parses SV
/// directly, but the verify lift uses sv2v + Yosys, so `top` / `use_sv2v` apply.
#[derive(Debug, Deserialize)]
pub struct SvVerifyAutoRequest {
    /// SystemVerilog primary source content.
    pub source: String,
    /// Additional SV sources (packages / `include` targets).
    #[serde(default)]
    pub additional_sources: Vec<FileContent>,
    /// Top module for the SV → BTOR2 lift (auto-detect when omitted).
    #[serde(default)]
    pub top: Option<String>,
    /// Run sv2v before Yosys (modern SV). Mirrors `--preprocess-sv2v`.
    #[serde(default)]
    pub use_sv2v: bool,
    /// Max CEGAR iterations per property (default 16).
    #[serde(default)]
    pub max_iterations: Option<usize>,
    /// Must-edge inference per property (default `"off"`; `"smt-hyper-must"`
    /// gives sound νμ verdicts).
    #[serde(default)]
    pub must_edge_inference: Option<String>,
    /// Reset-gating: drop recognized `disable iff (reset)` guards and pin the
    /// reset input inactive at the model level (default `true`). Set `false`
    /// to keep the guard and leave the reset free.
    #[serde(default)]
    pub gate_reset: Option<bool>,
    /// Auto-inject behavioral stubs for cut flop primitives (e.g. OpenTitan's
    /// `prim_sparse_fsm_flop`) so the register survives the lift (default
    /// `true`). Set `false` to leave cut flops reported as black-boxed.
    #[serde(default)]
    pub auto_stub_flops: Option<bool>,
    /// H.J.b — config concretization: pin wide config inputs to constants so
    /// comparisons against them become decidable. Each entry `"signal=value"`
    /// (e.g. `"cfg_detect_timer_i=7"`). The verdicts are then SCOPED to these
    /// values (surfaced as a `config-concretization` note). Default empty.
    #[serde(default)]
    pub config_values: Vec<String>,
    /// H.H — counter upper bounds: seed a `signal <= value` cube-partition to
    /// refine a counter-monotonicity property (`cnt_q >= $past(cnt_q)`) whose ⊥ is
    /// caused by the abstract 32-bit wraparound. Each entry `"signal<=value"` (the
    /// `"signal=value"` spelling is also accepted, same meaning). Sound (a
    /// partition, not an assumption — the must-edges verify it); requires
    /// `must_edge_inference` on. Bounds are also auto-derived from `config_values`;
    /// a manual entry here overrides the inferred one. Surfaced as a `counter-bound`
    /// note. Default empty.
    #[serde(default)]
    pub counter_bounds: Vec<String>,
}

/// Response for `/api/v1/sv/verify-auto`.
#[derive(Debug, Serialize)]
pub struct SvVerifyAutoResponse {
    pub properties: Vec<PropertyVerdictView>,
    /// Assertions that did not translate (reuses the extract-sva view shape;
    /// `kind` is `None` here).
    pub unsupported: Vec<UnsupportedAssertionView>,
    /// Model-level diagnostics: state-register count + black-boxed (cut)
    /// modules. Lets a SKIPPED outcome point at its root cause.
    pub diagnostics: ModelDiagnosticsView,
    /// H.J — human-facing provenance notes: every abstraction / scoping decision
    /// the run made (config concretizations, reset-gating, flop stubs, cut
    /// modules, the abstraction posture, the coverage summary), so a verdict's
    /// scope and caveats are explicit in the payload.
    #[serde(default)]
    pub notes: Vec<VerificationNoteView>,
}

/// One provenance note (mirrors
/// [`crate::adapter::slang::verify_auto::VerificationNote`]).
#[derive(Debug, Serialize)]
pub struct VerificationNoteView {
    /// Machine-stable kebab category (e.g. `"config-concretization"`).
    pub kind: String,
    /// `"info"` | `"scope-caveat"` | `"soundness-caveat"`.
    pub level: String,
    /// One-line human summary.
    pub summary: String,
    /// Longer explanation (the why + the soundness/scope implication).
    pub detail: String,
    /// Structured operands (e.g. `["cfg_detect_timer_i=7"]`).
    pub items: Vec<String>,
}

/// Model-level lift diagnostics (mirrors
/// [`crate::adapter::slang::verify_auto::ModelDiagnostics`]).
#[derive(Debug, Serialize)]
pub struct ModelDiagnosticsView {
    /// Number of state register lines in the lifted model.
    pub state_register_count: usize,
    /// Modules instantiated without a body, cut to free inputs (registers they
    /// drive are not modeled as state). Empty for a self-contained design.
    pub blackboxed_modules: Vec<String>,
    /// Reset inputs pinned inactive at the model level (`"<signal>=<value>"`),
    /// with their `disable iff` guards dropped from the formulas. Empty when
    /// reset-gating is off or no `disable iff` reset was recognized.
    pub gated_resets: Vec<String>,
    /// Cut flop-primitive modules for which a behavioral stub was auto-injected
    /// so the register survives the lift (e.g. `prim_sparse_fsm_flop`).
    pub auto_provided_stubs: Vec<String>,
}

/// One property's auto-verification verdict (mirrors `PropertyVerdict`).
#[derive(Debug, Serialize)]
pub struct PropertyVerdictView {
    pub name: String,
    /// `"assert"` | `"assume"` | `"cover"`.
    pub kind: String,
    pub formula: String,
    /// `"holds"` | `"violated"` | `"unknown"` | `"skipped"`.
    pub outcome: String,
    /// `false_cells` (violated) / `unknown_cells` (unknown) / the skip reason.
    pub detail: Option<String>,
    /// The cube predicates auto-seeded for this property (atom strings).
    pub seeded_predicates: Vec<String>,
}

/// U.0 — CEGAR refinement trace, JSON-shaped for the refinement-trace
/// viewer. Mirrors [`crate::adapter::btor2::cegar::CegarTrace`].
#[derive(Debug, Serialize)]
pub struct Btor2CegarResponse {
    pub success: bool,
    /// Per-iteration refinement records (iteration 0 = initial evaluation).
    pub iterations: Vec<CegarIterationView>,
    /// Predicate set at termination (initial + every added predicate).
    pub final_predicates: Vec<PredicateView>,
    /// Why the loop stopped: `"converged"` |
    /// `"bounded-iterations-reached"` | `"predicate-source-exhausted"`.
    pub terminated_with: String,
    /// Cell-count summary of the final 3-valued verdict.
    pub verdict: CegarVerdictSummary,
    /// `true` when the eager `predicate_cube_lift` was used (R.2.5 MVP).
    pub lazy_lift_pending: bool,
    /// Whether prior-iteration approximants were threaded forward.
    pub approximant_reuse_enabled: bool,
    /// Soundness / advisory warnings produced during the run.
    pub warnings: Vec<String>,
    /// Track I.1 (2026-06-24) — cube cells that **falsify** the formula
    /// (definite-False), each decoded to its predicate valuation. Capped (the
    /// `verdict.false_cells` count is the full total). Empty unless the outcome
    /// is VIOLATED. Makes a failing verdict actionable ("falsified where
    /// `idle=false, err=true`").
    pub violating_cells: Vec<WitnessCellView>,
    /// Track I.1 — cube cells the abstraction **cannot decide** (`Unknown`/⊥),
    /// each decoded to its predicate valuation. Capped (`verdict.unknown_cells`
    /// is the full total). These are the cells a finer predicate set would need
    /// to resolve.
    pub undecided_cells: Vec<WitnessCellView>,
    /// Track I.1 (trace slice, 2026-06-24) — reachability countertrace for a
    /// VIOLATED verdict: a path of definite-False cube cells from an initial
    /// cell to a trap (or the farthest reachable failing cell). Omitted from
    /// the JSON unless the property is actually violated at the initial cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<CounterTraceView>,
    /// Track I.1 (undecided-explanation slice, 2026-06-24) — when the final
    /// verdict still carries ⊥ (unknown) cells, the registers the failure
    /// subgame flagged as load-bearing for those indefinite verdicts (deduped,
    /// from `CegarTrace::init_refinement_candidates`). The actionable "why
    /// undecided": adding predicates over these registers — or promoting their
    /// init policy — may resolve the abstraction. Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refinement_candidates: Vec<String>,
    /// CTXDSL Phase 2 (2026-06-22) — the final refined cube model + the
    /// checked formula as CTXDSL, present only when the request set
    /// `emit_ctxdsl: true`. Omitted from the JSON otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctxdsl: Option<String>,
}

/// Track I.1 — a cube cell witnessing a non-HOLDS verdict, viewer-shaped:
/// the cube index plus the predicate valuation (`name → holds`) at that cell.
#[derive(Debug, Serialize)]
pub struct WitnessCellView {
    pub cube_index: usize,
    /// Predicate valuation at the cell (`name → holds`), keyed by predicate name.
    pub valuation: std::collections::BTreeMap<String, bool>,
}

/// Track I.1 (trace slice) — a reachability countertrace, viewer-shaped: the
/// ordered sequence of failing cube cells plus whether the path ends in a trap
/// (a cell whose every successor stays `False`, so the violation is locked in).
#[derive(Debug, Serialize)]
pub struct CounterTraceView {
    pub steps: Vec<WitnessCellView>,
    pub ends_in_trap: bool,
}

/// One CEGAR iteration, viewer-shaped.
#[derive(Debug, Serialize)]
pub struct CegarIterationView {
    pub iteration: usize,
    /// Predicate-set size at the start of this iteration.
    pub predicate_count: usize,
    /// `true` iff this iteration's verdict carried `KleeneBot` cells
    /// (a failure subgame drove a refinement).
    pub had_failure_subgame: bool,
    /// Predicates the source added in response to this iteration.
    pub predicates_added: Vec<PredicateView>,
    /// Proxy counter for game-position evaluations (approximant-reuse
    /// diagnostics).
    pub game_position_evaluations: usize,
    /// Cell-count summary of this iteration's 3-valued verdict.
    pub verdict: CegarVerdictSummary,
}

/// Cell counts of a 3-valued (Kleene) verdict over the cube space.
#[derive(Debug, Serialize)]
pub struct CegarVerdictSummary {
    /// KleeneT (definitely-true) cells.
    pub true_cells: usize,
    /// KleeneF (definitely-false) cells.
    pub false_cells: usize,
    /// KleeneBot (unknown — needs refinement) cells.
    pub unknown_cells: usize,
}

/// A predicate spec, response-shaped.
#[derive(Debug, Serialize)]
pub struct PredicateView {
    pub name: String,
    pub register: String,
    pub value: u64,
}
