# Case Study: Test BPMN Order Approval - Property Verification

## Summary

This case study documents property verification results for a test BPMN order approval process. This is a test case to validate the mining workflow infrastructure.

**Status**: Test Case  
**Severity**: N/A (Test)  
**Domain**: BPMN

## Specification Information

- **Repository**: Local test file
- **File SHA**: `[See artifact]`
- **File Path**: `test_mining_cache/test_bpmn_local.json`
- **Specification Type**: BPMN Process (JSON)
- **Date Discovered**: 2024-12-19

## Property Verification

### Properties Tested

The following properties from the BPMN property library were verified:

1. **proper_completion_reachable**: Ensures that from any reachable state, a proper completion state is reachable
2. **no_deadlocks**: Ensures that the process never reaches a deadlock state
3. **compensation_completes**: Ensures that if compensation is triggered, it eventually completes

### Verification Results

See `test_mining_workspace/results/batch_summary.json` for detailed results.

## Analysis

### Test Purpose

This test case validates:
- BPMN file discovery and translation
- Property library integration
- Triage framework functionality
- Disclosure artifact generation

### Assumptions and Semantic Choices

#### Tool Assumptions

- **Controllability Model**: Skolem paradigm - for all non-controllable choices, there exists one controllable choice that satisfies the formula
- **State Space**: States are represented as automaton states in the CLTS
- **Transition Semantics**: Transitions are interpreted based on BPMN JSON structure
- **Formula Evaluation**: μ-calculus formulas are evaluated using memoized fixpoint computation

#### Specification Assumptions

- BPMN JSON follows the expected schema (states, transitions, variables)
- Process name defaults to "bpm_process" if not specified
- Automaton name is "Process" for BPMN translations

## Reproducibility

### Artifact Information

- **Artifact Location**: `test_mining_workspace/disclosure/artifacts.json`
- **Tool Version**: `henos 0.1.0`
- **Configuration**: Default configuration

### Reproduction Steps

1. Create test BPMN JSON file:
   ```bash
   cp tests/data/bpm/realizable/order_approval.json test_mining_cache/test_bpmn_local.json
   ```

2. Run mining workflow:
   ```bash
   cargo run -- mining workflow test_bpmn_local \
     --workspace-dir test_mining_workspace \
     --cache-dir test_mining_cache \
     --property-lib bpmn
   ```

3. Review results:
   - Verification results: `test_mining_workspace/results/`
   - Triage results: `test_mining_workspace/triage/`
   - Disclosure artifacts: `test_mining_workspace/disclosure/`

## Disclosure

### Generated Artifacts

- **Issue Templates**: `test_mining_workspace/disclosure/issues/issue_*.md`
- **Artifacts JSON**: `test_mining_workspace/disclosure/artifacts.json`
- **Summary**: `test_mining_workspace/disclosure/disclosure_summary.txt`

### Upstream Communication

- **Issue Filed**: No (test case only)
- **Response**: N/A
- **Resolution**: N/A

## Lessons Learned

### For Specification Authors

This test case demonstrates the importance of:
- Clear process definitions in BPMN
- Proper state and transition modeling
- Complete property specifications

### For Tool Developers

This test validates:
- End-to-end mining workflow functionality
- Artifact generation and formatting
- Integration between components

### For Verification Practitioners

This example shows:
- How to run mining workflows
- How to interpret triage results
- How to use disclosure artifacts

## References

- BPMN Property Library: `src/mining/properties/bpmn.rs`
- Disclosure Workflow: `src/mining/disclosure.rs`
- Mining Workflow: `src/mining/workflow.rs`

