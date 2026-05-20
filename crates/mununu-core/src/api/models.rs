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
#[derive(Debug, Deserialize, PartialEq, Eq)]
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
// SV Init Endpoint
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SvInitRequest {
    pub source: FileContent,
    #[serde(default)]
    pub additional_sources: Vec<FileContent>,
}

#[derive(Debug, Serialize)]
pub struct SvInitResponse {
    pub success: bool,
    pub sidecar: String,
    pub schema: String,
    pub signals: Vec<SvSignalInfo>,
    pub inputs: Vec<SvInputInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SvSignalInfo {
    pub name: String,
    pub width: usize,
    pub abstraction: String,
    pub preserve: bool,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SvInputInfo {
    pub name: String,
    pub abstraction: String,
}

// ============================================================================
// SV Discover Endpoint
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SvDiscoverRequest {
    pub source: FileContent,
    pub sidecar: String,
}

#[derive(Debug, Serialize)]
pub struct SvDiscoverResponse {
    pub success: bool,
    pub sidecar: String,
    pub discoveries: Vec<SvDiscoveryResult>,
    pub smt_available: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SvDiscoveryResult {
    pub signal: String,
    pub values_found: usize,
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
