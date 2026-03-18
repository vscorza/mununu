use serde::{Deserialize, Serialize};

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
}

/// Automaton summary information
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutomatonSummary {
    pub name: String,
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
    pub formula: String,
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
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ContextSynthesizeResponse {
    pub success: bool,
    pub realizable: bool,
    pub controller: Option<FileContent>,
    pub diagnostics: SynthesisDiagnostics,
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
    pub formula: Option<String>,
    /// If omitted, use the formula's target automaton(s).
    pub automaton: Option<String>,
}

/// Response from context verification
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ContextVerifyResponse {
    pub success: bool,
    pub all_satisfied: bool,
    pub results: Vec<FormulaVerificationResult>,
}

/// Verification result for a single formula–automaton pair
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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
