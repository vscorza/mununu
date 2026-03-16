# HENOS Work Plan

**Last Updated:** 2025-01-XX  
**Status:** Planning Phase  
**Estimated Total Timeline:** 12-16 weeks

This document outlines the comprehensive work plan for HENOS development, including IR migration, property verification, API enhancements, frontend integration, and AI adapter implementation.

---

## Table of Contents

1. [IR Migration Phases](#1-ir-migration-phases)
2. [Property Verification System](#2-property-verification-system)
3. [API Updates](#3-api-updates)
4. [HENOS-Web Integration](#4-henos-web-integration)
5. [AI Adapter/Client Implementation](#5-ai-adapterclient-implementation)
6. [Overall Timeline and Dependencies](#6-overall-timeline-and-dependencies)
7. [Cleanup and Deprecation](#7-cleanup-and-deprecation)

---

## Important Notes

### μ-Calculus Translation Reference

**CRITICAL:** When implementing behavioral properties or translating LTL to μ-calculus, always reference:
- [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) - **Primary reference** for LTL→μ-calculus patterns

**Key Points:**
- `[]` and `<>` operate on the **NEXT state only** - use fixpoints (μ/ν) for temporal properties
- `G φ` (always) → `ν X. (φ ∧ [] X)` - **NOT** `[] φ`
- `F φ` (eventually) → `μ X. (φ ∨ [] X)` - **NOT** `<> φ`
- Common mistake: Using `[] φ` for "always" without fixpoint - this only checks the next state!

See the cheatsheet for complete patterns including response, safety, liveness, and domain-specific patterns.

### Code Cleanup Strategy

After IR migration, old direct BPMN→ctxdsl translation paths should be removed:
- Translation flow should be: **BPMN → IR → CLTS** (not BPMN → CLTS directly)
- See [Cleanup and Deprecation](#7-cleanup-and-deprecation) section for details

---

## 1. IR Migration Phases

### Phase 1: Create IR Module

**Duration:** Week 1  
**Dependencies:** None  
**Priority:** High

**References:**
- [`docs/bpm/ir_migration_plan.md`](../bpm/ir_migration_plan.md) - Complete IR migration specification
- [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md) - IR schema definition (Section 2)

#### Tasks

1. **Create IR module structure**
   - Create `src/translation/ir/` directory
   - Create `mod.rs`, `model.rs`, `validation.rs`, `serialization.rs`
   - Set up module exports

2. **Extract IR types from `bpm.rs`**
   - Move `BpmModel`, `BpmState`, `BpmTransition`, etc. to `ir/model.rs`
   - Convert from `Deserialize` structs to regular Rust types
   - Add `StateKind` enum for type safety
   - **Reference:** See IR schema in [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md#22-ir-schema)

3. **Implement IR validation**
   - Port validation logic from builder to `validation.rs`
   - Add comprehensive validation rules:
     - State uniqueness
     - Initial state existence
     - Transition reference validation
     - Variable consistency checks
   - Implement `validate_ir()` function
   - **Reference:** Validation rules in [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md#23-ir-semantics)

4. **Implement IR serialization**
   - Implement `Serialize`/`Deserialize` for IR types
   - Create `from_json()`/`to_json()` helpers
   - Ensure backward compatibility with existing JSON format
   - **Reference:** JSON schema format in [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md#22-ir-schema)

5. **Update `bpm.rs` to use IR module**
   - Import IR types from `ir` module
   - Replace inline `BpmModel` struct with IR types
   - Update builder to use `ir::from_json` and `ir::validate_ir`
   - **Cleanup Note:** After IR migration is complete, identify and remove any old BPMN→ctxdsl translation logic in `bpm.rs` that's now replaced by the IR→CLTS translation path. The translation should follow: BPMN → IR → CLTS, not BPMN → CLTS directly.

#### Validation

- **Unit Tests:**
  - IR validation rules (all error cases)
  - IR serialization/deserialization round-trip
  - StateKind enum conversions
- **Integration Tests:**
  - Existing BPM translation tests should pass
  - IR module integration with builder
- **Manual Tests:**
  - Verify JSON format compatibility
  - Test with sample BPMN files

---

### Phase 2: Refactor BPMN XML Extractor

**Duration:** Week 1-2  
**Dependencies:** Phase 1 complete  
**Priority:** High

**References:**
- [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md) - BPMN structure and IR translation (Sections 1, 3)
- [`docs/bpm/ir_migration_plan.md`](../bpm/ir_migration_plan.md) - Extractor refactoring details (Section 3.2)

#### Tasks

1. **Create `parse_bpmn_xml_to_ir` function**
   - Extract XML parsing logic from `parse_bpmn_xml`
   - Change return type from `String` (JSON) to `BpmModel` (IR)
   - Update internal data structures to build IR directly
   - Handle subprocess expansion
   - **Reference:** BPMN→IR translation rules in [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md#3-translation-bpmn--ir)

2. **Update `BpmnXmlExtractor`**
   - Keep `extract` method returning JSON for backward compatibility
   - Add helper function `extract_ir()` returning `BpmModel` directly
   - Update `extract` to call `parse_bpmn_xml_to_ir` and serialize to JSON
   - **Cleanup Note:** Once IR→CLTS translation is stable, the old `parse_bpmn_xml` function (that returns JSON string) can be deprecated/removed
   - **Action Items:**
     - After Phase 3 completion, mark `parse_bpmn_xml()` as deprecated
     - Update all internal callers to use `parse_bpmn_xml_to_ir()` instead
     - Remove `parse_bpmn_xml()` in a future cleanup phase (after API migration)

3. **Create separate BPM JSON extractor**
   - Extract JSON reading logic from `bpm.rs` into `bpm_json.rs`
   - Create `BpmJsonExtractor` that reads JSON and converts to IR
   - Update pipeline registration

#### Validation

- **Unit Tests:**
  - `parse_bpmn_xml_to_ir` with various BPMN structures
  - Subprocess expansion logic
  - Gateway extraction
  - Event extraction
- **Integration Tests:**
  - BPMN XML → IR → CLTS pipeline
  - Ensure existing BPMN XML tests pass
- **Manual Tests:**
  - Test with complex BPMN files (multiple processes, subprocesses)
  - Verify IR structure matches expectations

---

### Phase 3: Refactor BPM Builder

**Duration:** Week 2  
**Dependencies:** Phase 2 complete  
**Priority:** High

**References:**
- [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md) - IR→CLTS translation specification (Section 4)
- [`docs/bpm/ir_migration_plan.md`](../bpm/ir_migration_plan.md) - Builder refactoring details (Section 3.3)

#### Tasks

1. **Update `BpmBuilder` to accept IR**
   - Change `build_product` to deserialize JSON to IR using `ir::from_json`
   - Add `validate_ir` call before building
   - Create `build_from_ir` method with existing build logic
   - **Handle multiple processes**: 
     - Update `build_multiple_processes_context()` to accept `Vec<BpmModel>` instead of JSON arrays
     - Add `parse_bpmn_xml_to_ir_multiple()` function or update `parse_bpmn_xml_to_ir()` to return `Vec<BpmModel>` for multiple processes
     - Update `build_product` to handle multiple IR models directly
   - **Reference:** IR→CLTS translation rules in [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md#4-translation-ir--clts-context)

2. **Remove embedded JSON deserialization**
   - Remove `#[derive(Deserialize)]` from IR types (move to serialization layer)
   - Update all builder code to use typed IR structures

3. **Simplify builder code**
   - Remove JSON parsing/validation logic (now in IR module)
   - Focus builder on IR → CLTS translation logic
   - **Cleanup Note:** After migration, review `bpm.rs` for any remaining BPMN→ctxdsl logic that should be removed if it's now handled by IR→CLTS translation. The old direct BPMN→ctxdsl path should be replaced by BPMN→IR→CLTS path.
   - **Action Items:**
     - Identify all functions in `bpm.rs` that perform direct BPMN→ctxdsl translation
     - Mark them as deprecated or remove if IR→CLTS translation covers the same functionality
     - Update all call sites to use IR→CLTS path
     - Remove duplicate translation logic

#### Validation

- **Unit Tests:**
  - Builder with valid IR
  - Builder with invalid IR (should fail validation)
  - Gateway decomposition
  - Variable translation
- **Integration Tests:**
  - All existing BPM builder tests should pass
  - Test IR validation errors propagate correctly
- **Manual Tests:**
  - Verify CLTS output matches expected format
  - Test gateway composition generation

---

### Phase 4: API Updates (IR)

**Duration:** Week 2-3  
**Dependencies:** Phase 3 complete  
**Priority:** Medium

#### Tasks

1. **Add IR to API models**
   - Add `BpmIr` type to `src/api/models.rs` (serializable IR representation)
   - Implement conversion from `BpmModel` to `BpmIr`

2. **Update translation endpoint**
   - Add `include_ir: bool` option to `TranslateOptions`
   - Include IR in response if requested
   - Update OpenAPI schema

3. **Add IR extraction endpoint**
   - New endpoint: `POST /api/v1/translate/bpm/ir`
   - Returns IR only (no CLTS translation)
   - Useful for debugging and tooling integration

#### Validation

- **Unit Tests:**
  - API model serialization/deserialization
  - IR conversion logic
- **Integration Tests:**
  - Translation endpoint with `include_ir: true`
  - IR extraction endpoint
  - OpenAPI schema validation
- **Manual Tests:**
  - Test API endpoints with Postman/curl
  - Verify IR in response matches expected structure

---

## 2. Property Verification System

### Phase 5: Structural Property Checks

**Duration:** Week 3-4  
**Dependencies:** Phase 3 complete (IR module available)  
**Priority:** High

**References:**
- [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md) - Structural property examples (Section 1)
- [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md) - Common BPMN pitfalls and structural problems (Part 1)
- [`docs/bpm/bpm_formalization.md`](../bpm/bpm_formalization.md) - BPMN structure and semantics (Section 1)

#### Tasks

1. **Create structural analysis module**
   - Create `src/bpm/analysis/structural.rs`
   - Implement graph traversal utilities
   - Implement pattern detection functions
   - **Reference:** See structural analysis examples in [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md#category-1-structural-properties)

2. **Implement gateway matching checks**
   - AND-split → AND-join validation
   - OR-split → OR-join validation
   - Detect gateway mismatches
   - Generate fix suggestions
   - **Reference:** Gateway mismatch patterns in [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md#2-gateway-mismatch-and-missing-synchronization)

3. **Implement reachability checks**
   - Check all states are reachable from initial states
   - Check end events are reachable
   - Detect dead states
   - **Reference:** Reachability examples in [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md)

4. **Implement loop analysis**
   - Detect cycles in process flow
   - Verify loops have exit conditions
   - Detect infinite loops (livelocks)
   - **Reference:** Livelock patterns in [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md#7-livelocks--infinite-cycles-that-are-structurally-legal)

5. **Implement boundary event validation**
   - Verify interrupting vs non-interrupting semantics
   - Check boundary event attachments
   - Validate compensation flows
   - **Reference:** Boundary event semantics in [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md#6-boundary-events-interrupting-vs-non-interrupting-creates-hidden-concurrency)

6. **Create structural property result format**
   - Define `StructuralCheckResult` struct
   - Include severity, location, evidence, suggestions
   - Serialize to JSON for API responses
   - **Reference:** Unified suggestion format in [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md#unified-suggestion-format)

#### Validation

- **Unit Tests:**
  - Gateway matching detection (all mismatch types)
  - Reachability analysis (various graph structures)
  - Loop detection and exit condition verification
  - Boundary event validation
- **Integration Tests:**
  - Structural checks on real BPMN examples
  - Verify results match expected issues
- **Manual Tests:**
  - Test with known problematic BPMN files
  - Verify fix suggestions are actionable

---

### Phase 6: Behavioral Property Checks

**Duration:** Week 5-7  
**Dependencies:** Phase 5 complete, existing μ-calculus evaluator  
**Priority:** High

**References:**
- [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) - **CRITICAL:** μ-calculus translation patterns (use this to avoid modal operator confusion)
- [`docs/archive/mu_calculus/mu_calculus_grammar_semantics.md`](../archive/mu_calculus/mu_calculus_grammar_semantics.md) - μ-calculus syntax and semantics
- [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md) - Behavioral property examples (Section 2)
- [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md) - Property verification architecture

#### Tasks

1. **Create behavioral analysis module**
   - Create `src/bpm/analysis/behavioral.rs`
   - Integrate with existing μ-calculus evaluator
   - Create property templates
   - **Important:** Use [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) for all LTL→μ-calculus translations
   - **Note:** Remember that `[]` and `<>` operate on NEXT state only - use fixpoints (μ/ν) for temporal properties

2. **Implement deadlock detection**
   - Property: "No deadlock states exist"
   - μ-calculus formula for deadlock detection
   - Generate counterexample traces
   - **Reference:** See deadlock examples in [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md)

3. **Implement liveness checks**
   - Property: "All paths eventually reach an end event" → `μ X. (endEvent ∨ [] X)`
   - Property: "No livelocks (infinite cycles without progress)" → Use `GF(progress)` pattern
   - Generate counterexamples
   - **Reference:** LTL→μ-calculus: `F φ` → `μ X. (φ ∨ [] X)` (from cheatsheet)

4. **Implement safety properties**
   - Property: "Proper completion (no leftover tokens)" → `ν X. (proper_completion ∧ [] X)`
   - Property: "Gateway synchronization (all branches complete before join)" → Safety pattern
   - Custom safety properties from IR analysis
   - **Reference:** LTL→μ-calculus: `G φ` → `ν X. (φ ∧ [] X)` (from cheatsheet)

5. **Implement property templates**
   - Response pattern: `G(trigger → F(response))` → `ν X. ((¬trigger ∨ μ Y. (response ∨ [] Y)) ∧ [] X)`
   - Safety pattern: `G(condition → invariant)` → `ν X. ((¬condition ∨ invariant) ∧ [] X)`
   - Liveness pattern: `GF(progress)` → `ν Y. (μ X. (progress ∨ [] X) ∧ [] Y)`
   - Allow user-defined properties
   - **CRITICAL:** Always reference [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) when translating LTL patterns
   - **Common Mistake to Avoid:** Don't use `[] φ` for "always" - use `ν X. (φ ∧ [] X)`

6. **Create behavioral property result format**
   - Define `BehavioralCheckResult` struct
   - Include formula, satisfaction, counterexample traces
   - Integrate with existing `ControllerDiagnostics`

#### Validation

- **Unit Tests:**
  - Deadlock detection on known deadlock cases
  - Liveness property verification
  - Safety property verification
  - Property template instantiation
- **Integration Tests:**
  - Behavioral checks on translated CLTS automata
  - Verify counterexamples are valid
  - Test with real BPMN examples
- **Manual Tests:**
  - Test with processes known to have behavioral issues
  - Verify counterexample traces are understandable
  - Test property template system

---

### Phase 7: Property Verification API

**Duration:** Week 7-8  
**Dependencies:** Phases 5-6 complete  
**Priority:** Medium

#### Tasks

1. **Add property verification endpoints**
   - `POST /api/v1/bpm/verify/structural` - Structural property checks
   - `POST /api/v1/bpm/verify/behavioral` - Behavioral property checks
   - `POST /api/v1/bpm/verify/all` - Combined verification

2. **Define request/response models**
   - `StructuralVerificationRequest` - BPMN input + options
   - `BehavioralVerificationRequest` - BPMN input + property templates
   - `VerificationResponse` - Unified result format

3. **Integrate with translation pipeline**
   - Verify IR directly (structural)
   - Translate to CLTS and verify (behavioral)
   - Combine results

4. **Add verification metadata to translation response**
   - Optional verification in translation endpoint
   - Include structural/behavioral results

#### Validation

- **Unit Tests:**
  - API model serialization
  - Verification result formatting
- **Integration Tests:**
  - End-to-end verification pipeline
  - Verify results match manual analysis
  - Test with various BPMN files
- **Manual Tests:**
  - Test API endpoints with various inputs
  - Verify result format is useful for UI
  - Test error handling

---

## 3. API Updates

### Phase 8: Enhanced Translation API

**Duration:** Week 8  
**Dependencies:** Phases 4, 7 complete  
**Priority:** Medium

#### Tasks

1. **Update translation endpoint with verification options**
   - Add `verify: VerificationOptions` to `TranslateOptions`
   - Include structural/behavioral checks if requested
   - Return verification results in response

2. **Add batch translation endpoint**
   - `POST /api/v1/translate/bpm/batch` - Process multiple BPMN files
   - Return results with per-file status

3. **Add translation status endpoint**
   - `GET /api/v1/translate/bpm/status/{job_id}` - Check translation job status
   - Support async translation for large files

4. **Update OpenAPI documentation**
   - Document all new endpoints
   - Add examples for request/response
   - Update schemas

#### Validation

- **Unit Tests:**
  - Request/response serialization
  - Option handling
- **Integration Tests:**
  - End-to-end translation with verification
  - Batch processing
  - Error handling
- **Manual Tests:**
  - API documentation accuracy
  - Test with various client tools
  - Performance testing

---

## 4. HENOS-Web Integration

### Phase 9: HENOS-Web Translation Integration

**Duration:** Week 9-10  
**Dependencies:** Phase 8 complete  
**Priority:** High

**References:**
- [`docs/ui/ui_integration_plan.md`](../ui/ui_integration_plan.md) - UI integration plan
- [`docs/ui/server_implementation_plan.md`](../ui/server_implementation_plan.md) - Server implementation details
- [`docs/api/README.md`](../api/README.md) - API documentation
- [`docs/api/henos_web_integration_guide.md`](../api/henos_web_integration_guide.md) - Frontend integration guide
- [`docs/api/PHASE9_IMPLEMENTATION.md`](../api/PHASE9_IMPLEMENTATION.md) - Phase 9 implementation steps

#### Tasks

1. **Update API client in henos-web**
   - Update TypeScript types (generate from OpenAPI schema)
   - Update API service functions for new endpoints
   - Add error handling for new response formats
   - **Reference:** API client setup in [`docs/ui/ui_integration_plan.md`](../ui/ui_integration_plan.md)

2. **Add IR visualization (optional)**
   - Display IR structure in UI
   - Show IR when `include_ir: true` in translation
   - IR tree/graph visualization component

3. **Update translation UI**
   - Add verification options to translation form
   - Display structural/behavioral verification results
   - Show verification warnings/errors in UI
   - **Reference:** UI workflow in [`docs/ui/ui_integration_plan.md`](../ui/ui_integration_plan.md#31-core-features)

4. **Add verification results display**
   - Structural check results panel
   - Behavioral check results panel
   - Counterexample trace visualization
   - Fix suggestion display

#### Validation

- **Unit Tests:**
  - API client functions
  - UI component rendering
  - Data transformation logic
- **Integration Tests:**
  - End-to-end translation flow in UI
  - Verification display
  - Error handling in UI
- **Manual Tests:**
  - User acceptance testing
  - UI/UX validation
  - Cross-browser testing
  - Responsive design testing

---

### Phase 10: HENOS-Web Property Verification UI

**Duration:** Week 10-11  
**Dependencies:** Phase 9 complete  
**Priority:** Medium

#### Tasks

1. **Add dedicated verification page**
   - Property verification form
   - BPMN file upload/input
   - Verification options configuration

2. **Create verification results dashboard**
   - Summary view (pass/fail counts)
   - Detailed results per property
   - Filterable/sortable results table

3. **Add property template editor**
   - UI for creating custom property templates
   - Property library browser
   - Template sharing/export

4. **Add fix suggestions UI**
   - Display suggested fixes for structural issues
   - Allow applying fixes (if API supports)
   - Preview fixes before applying

#### Validation

- **Unit Tests:**
  - Component logic
  - Form validation
  - Data transformations
- **Integration Tests:**
  - Verification workflow end-to-end
  - Fix suggestion application
- **Manual Tests:**
  - User workflow testing
  - UI responsiveness
  - Accessibility testing

---

## 5. AI Adapter/Client Implementation

### Phase 11: AI Adapter Foundation

**Duration:** Week 11-12  
**Dependencies:** Phase 5 complete (IR available)  
**Priority:** Medium

**References:**
- [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md) - Complete AI integration documentation
- [`docs/ai/ai_agent_integration_index.md`](../ai/ai_agent_integration_index.md) - AI integration document index

#### Tasks

1. **Create AI adapter module structure**
   - Create `src/ai/` directory
   - Create `adapter.rs`, `client.rs`, `prompts.rs`, `config.rs`
   - Define error types
   - **Reference:** Architecture in [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md)

2. **Define AI adapter trait**
   - `AiAdapter` trait with methods:
     - `analyze_ir(ir: &BpmModel) -> Result<AnalysisResult>`
     - `suggest_properties(ir: &BpmModel) -> Result<Vec<PropertySuggestion>>`
     - `check_business_rules(ir: &BpmModel, domain: &str) -> Result<BusinessCheckResult>`
   - **Reference:** Trait definitions in [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md)

3. **Create configuration system**
   - API key management (environment variables, config file)
   - Provider selection (OpenAI)
   - Model selection and parameters (GPT-4, GPT-3.5-turbo, etc.)
   - Rate limiting configuration
   - **Reference:** Configuration requirements in [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md)

4. **Implement base client**
   - HTTP client abstraction
   - Request/response handling
   - Error handling and retries
   - Logging and monitoring
   - **Reference:** Implementation details in [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md)

#### Validation

- **Unit Tests:**
  - Trait interface compliance
  - Configuration parsing
  - Error handling
- **Integration Tests:**
  - Mock AI provider responses
  - Client error scenarios
- **Manual Tests:**
  - Configuration setup
  - API key management

---

### Phase 12: OpenAI Integration

**Duration:** Week 12-14  
**Dependencies:** Phase 11 complete  
**Priority:** Medium

**References:**
- [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md) - OpenAI integration details
- [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) - **CRITICAL:** Use when AI suggests LTL properties

#### Tasks

1. **Implement OpenAI client**
   - Create `src/ai/providers/openai.rs`
   - Implement OpenAI API client (ChatGPT)
   - Support multiple models (GPT-4, GPT-3.5-turbo, GPT-4-turbo)
   - Handle streaming responses (if needed)
   - **Reference:** Provider implementation in [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md)

2. **Create prompt templates**
   - IR-to-text conversion for prompts
   - Business property analysis prompts
   - Property suggestion prompts
   - Compliance check prompts
   - **Important:** Include [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) in prompts so AI uses correct μ-calculus patterns
   - **Reference:** Prompt templates in [`docs/ai/ai_prompt_experiments.md`](../ai/ai_prompt_experiments.md)

3. **Implement response parsing**
   - Parse JSON responses from OpenAI
   - Handle structured outputs (function calling if available)
   - Extract property suggestions
   - Extract business rule violations
   - **Important:** When AI suggests LTL properties, translate using cheatsheet patterns

4. **Add rate limiting and error handling**
   - Implement rate limiting for OpenAI API
   - Handle API errors (rate limits, timeouts)
   - Implement exponential backoff retries
   - Support different rate limits for different models

5. **Add model selection and optimization**
   - Allow configuration of which OpenAI model to use
   - Optimize prompts for different models
   - Handle model-specific capabilities and limitations

#### Validation

- **Unit Tests:**
  - Prompt generation
  - Response parsing
  - Error handling
  - Model selection logic
- **Integration Tests:**
  - Mock OpenAI API responses
  - Rate limiting behavior
  - Error recovery
  - Different model responses
- **Manual Tests:**
  - Real API integration (with API key)
  - Verify response quality across different models
  - Test rate limiting
  - Compare results between GPT-4 and GPT-3.5-turbo

---

### Phase 13: Business Property Checks with AI

**Duration:** Week 14-15  
**Dependencies:** Phase 12 complete  
**Priority:** High

**References:**
- [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md) - AI integration documentation
- [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md) - Business property examples (Section 3)
- [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md) - Business property verification strategies (Part 2)
- [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json) - **CRITICAL:** Use this when AI suggests LTL properties that need μ-calculus translation

#### Tasks

1. **Implement business property analysis**
   - Create `src/bpm/analysis/business.rs`
   - Integrate AI adapter for business rule checks
   - Domain-specific analysis (finance, healthcare, etc.)
   - **Reference:** AI adapter interface in [`docs/ai/ai_integration_consolidated.md`](../ai/ai_integration_consolidated.md)

2. **Create property suggestion system**
   - IR analysis → AI analysis pipeline
   - Generate property suggestions from AI
   - Categorize suggestions (compliance, best practices, security)
   - **Reference:** Business property examples in [`docs/bpm/bpm_verification_examples.md`](../bpm/bpm_verification_examples.md#category-3-business-properties)
   - **Important:** When AI suggests LTL properties, translate using [`docs/ltl_templates/ai_ltl_to_mu_cheatsheet.json`](../ltl_templates/ai_ltl_to_mu_cheatsheet.json)

3. **Implement compliance checking**
   - Predefined compliance templates (SOX, GDPR, HIPAA, etc.)
   - AI-powered compliance verification
   - Generate compliance reports
   - **Reference:** Compliance property strategies in [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md#adding-business-suggestions-beyond-structural)

4. **Create unified property result format**
   - Combine structural, behavioral, and business results
   - Rank suggestions by severity and confidence
   - Generate unified verification report
   - **Reference:** Unified suggestion format in [`docs/bpm/bpm_integration_approach.md`](../bpm/bpm_integration_approach.md#unified-suggestion-format)

5. **Add business property API endpoint**
   - `POST /api/v1/bpm/verify/business` - Business property checks
   - Accept domain specification
   - Return AI-generated suggestions

#### Validation

- **Unit Tests:**
  - Property suggestion parsing
  - Result formatting
  - Compliance template matching
- **Integration Tests:**
  - End-to-end business property checking
  - AI integration with mock responses
  - Unified result generation
- **Manual Tests:**
  - Real AI analysis on sample BPMN files
  - Verify suggestion quality and relevance
  - Test compliance checking
  - Verify cost/usage tracking

---

### Phase 14: AI Integration in HENOS-Web

**Duration:** Week 15-16  
**Dependencies:** Phases 10, 13 complete  
**Priority:** Medium

#### Tasks

1. **Add AI configuration UI**
   - API key management interface
   - Provider selection (OpenAI)
   - Model selection and parameters (GPT-4, GPT-3.5-turbo, etc.)

2. **Add business property checking UI**
   - Business property check form
   - Domain selection
   - Compliance template selection

3. **Display AI-generated suggestions**
   - Business property suggestions panel
   - Compliance check results
   - Confidence scores and explanations

4. **Add AI usage tracking**
   - Display API usage/costs
   - Rate limit warnings
   - Usage statistics

#### Validation

- **Unit Tests:**
  - UI component logic
  - Configuration handling
- **Integration Tests:**
  - End-to-end AI integration in UI
  - Configuration persistence
- **Manual Tests:**
  - User workflow with AI features
  - Verify suggestion display
  - Test configuration management
  - Verify usage tracking accuracy

---

### Phase 15: Context-Aware Business Improvement Suggestions (Future Enhancement)

**Duration:** Week 16-18 (post-MVP)  
**Dependencies:** Phase 13 complete  
**Priority:** Low (Future Enhancement)

**Status:** Planned for future implementation. This phase extends Phase 13 to provide context-aware business improvement suggestions that take into account organizational constraints and improvement directions.

**References:**
- [`docs/ai/ai_prompt_experiments.md`](../ai/ai_prompt_experiments.md#8-task-4--context-aware-business-improvement-suggestions-future-enhancement) - Context-aware prompt design
- Phase 13 tasks and validation

#### Tasks

1. **Define context schema**
   - Budget context (type, range, constraints)
   - Staff structure (model, capacity, change authority)
   - Digitalization level (tools, data maturity, integration constraints)
   - Improvement directions (efficiency, risk, quality, compliance, cultural/identity inclusion, ecology)
   - **Reference:** Context categories in [`docs/ai/ai_prompt_experiments.md`](../ai/ai_prompt_experiments.md#81-context-categories)

2. **Extend API models**
   - Add `ContextAwareImprovementRequest` with context fields
   - Add `ContextAwareImprovementSuggestion` with fit checks and directional impacts
   - Update `BusinessVerificationRequest` to optionally include context
   - **Reference:** Enhanced output format in [`docs/ai/ai_prompt_experiments.md`](../ai/ai_prompt_experiments.md#83-enhanced-output-format)

3. **Implement context-aware prompt generation**
   - Build prompt templates that incorporate context
   - Default assumptions for missing context
   - Validate context values against allowed enums
   - **Reference:** Prompt template in [`docs/ai/ai_prompt_experiments.md`](../ai/ai_prompt_experiments.md#84-prompt-template-future)

4. **Update AI adapter**
   - Extend `check_business_rules` to accept context
   - Parse enhanced suggestion format
   - Validate fit checks and directional impacts
   - Integrate with existing business analysis module

5. **Add context-aware API endpoint**
   - `POST /api/v1/bpm/verify/business/context-aware` - Context-aware business improvements
   - Accept context parameters
   - Return suggestions with fit checks and directional impacts

6. **Update validation script**
   - Add Task 4 validation (context-aware suggestions)
   - Validate fit checks and directional impacts
   - Test with various context combinations

#### Validation

- **Unit Tests:**
  - Context schema validation
  - Prompt generation with context
  - Suggestion parsing and validation
  - Default assumption logic
- **Integration Tests:**
  - End-to-end context-aware suggestions
  - Various context combinations
  - Fit check accuracy
- **Manual Tests:**
  - Real AI analysis with context
  - Verify suggestions respect constraints
  - Test inclusion/ecology impact assessment

**Note:** This phase is marked as "Future Enhancement" and can be implemented after MVP is complete. It extends the basic business property checking from Phase 13 with organizational context awareness.

---

## 6. Overall Timeline and Dependencies

### Timeline Summary

| Phase | Duration | Dependencies | Priority |
|-------|----------|--------------|----------|
| Phase 1: IR Module | Week 1 | None | High |
| Phase 2: XML Extractor | Week 1-2 | Phase 1 | High |
| Phase 3: Builder Refactor | Week 2 | Phase 2 | High |
| Phase 4: API Updates (IR) | Week 2-3 | Phase 3 | Medium |
| Phase 5: Structural Checks | Week 3-4 | Phase 3 | High |
| Phase 6: Behavioral Checks | Week 5-7 | Phase 5 | High |
| Phase 7: Property Verification API | Week 7-8 | Phases 5-6 | Medium |
| Phase 8: Enhanced Translation API | Week 8 | Phases 4, 7 | Medium |
| Phase 9: HENOS-Web Translation | Week 9-10 | Phase 8 | High |
| Phase 10: HENOS-Web Verification UI | Week 10-11 | Phase 9 | Medium |
| Phase 11: AI Adapter Foundation | Week 11-12 | Phase 5 | Medium |
| Phase 12: OpenAI Integration | Week 12-14 | Phase 11 | Medium |
| Phase 13: Business Property Checks | Week 14-15 | Phase 12 | High |
| Phase 14: AI Integration in Web | Week 15-16 | Phases 10, 13 | Medium |
| Phase 15: Context-Aware Improvements | Week 16-18 | Phase 13 | Low (Future) |

**Total Estimated Duration:** 16 weeks (4 months) for MVP, 18 weeks with Phase 15  
**Note:** Phase 15 is a future enhancement and not required for MVP

### Critical Path

1. Phase 1 → Phase 2 → Phase 3 (IR Migration) - Blocks everything
2. Phase 5 → Phase 6 → Phase 7 (Property Verification) - Blocks API and Web
3. Phase 11 → Phase 12/13 → Phase 14 (AI Integration) - Can run parallel after Phase 5

### Parallel Work Opportunities

- **Weeks 9-12:** HENOS-Web work (Phases 9-10) can run parallel with AI foundation (Phase 11)
- **Weeks 12-14:** OpenAI integration (Phase 12) includes model selection and optimization
- **Weeks 8-11:** Property verification API (Phase 7-8) can overlap with structural/behavioral work

---

## 7. Validation Strategy

### Testing Pyramid

1. **Unit Tests (70%)**
   - Fast, isolated tests
   - Test individual functions and methods
   - Mock external dependencies
   - Target: >80% code coverage

2. **Integration Tests (20%)**
   - Test component interactions
   - Test with real dependencies (IR, CLTS)
   - Test API endpoints
   - Test translation pipelines

3. **Manual Tests (10%)**
   - User acceptance testing
   - UI/UX validation
   - End-to-end workflows
   - Performance testing

### Test Categories by Phase

#### IR Migration Phases (1-3)
- **Unit Tests:** IR validation, serialization, builder logic
- **Integration Tests:** End-to-end translation pipelines
- **Manual Tests:** Sample BPMN file translation

#### Property Verification (5-7)
- **Unit Tests:** Structural analysis functions, property templates
- **Integration Tests:** Verification on known test cases
- **Manual Tests:** Real BPMN file verification

#### API Updates (4, 7-8)
- **Unit Tests:** Request/response serialization, option handling
- **Integration Tests:** API endpoint testing
- **Manual Tests:** API documentation, client tool testing

#### HENOS-Web (9-10)
- **Unit Tests:** Component logic, API client
- **Integration Tests:** UI workflows, error handling
- **Manual Tests:** User acceptance, UI/UX, cross-browser

#### AI Integration (11-14)
- **Unit Tests:** Adapter logic, prompt generation, response parsing
- **Integration Tests:** Mock OpenAI API responses, model selection
- **Manual Tests:** Real OpenAI API integration, suggestion quality, different model comparison

---

## 8. Risk Mitigation

### Technical Risks

1. **IR Migration Breaking Changes**
   - **Mitigation:** Comprehensive test coverage, incremental migration
   - **Contingency:** Rollback plan for each phase

2. **AI API Rate Limits/Costs**
   - **Mitigation:** Rate limiting, caching, usage tracking
   - **Contingency:** Fallback to rule-based analysis, local models

3. **Property Verification Performance**
   - **Mitigation:** Optimize analysis algorithms, caching
   - **Contingency:** Async processing for large models

4. **API Breaking Changes**
   - **Mitigation:** Version API endpoints, maintain backward compatibility where possible
   - **Contingency:** Deprecation timeline for old endpoints

### Schedule Risks

1. **AI Integration Delays**
   - **Mitigation:** Start with mock implementations, parallel work
   - **Contingency:** Defer to post-MVP enhancement

2. **HENOS-Web Integration Complexity**
   - **Mitigation:** Early API contract definition, iterative integration
   - **Contingency:** Minimal UI initially, enhance incrementally

---

## 9. Success Criteria

### Phase 1-3 (IR Migration)
- ✅ All existing tests pass
- ✅ IR module is well-documented
- ✅ Translation pipeline produces identical CLTS output
- ✅ Code coverage >80% for IR module

### Phase 5-7 (Property Verification)
- ✅ Structural checks detect all known issues in test cases
- ✅ Behavioral checks verify properties correctly
- ✅ Counterexamples are valid and understandable
- ✅ API endpoints return correct results

### Phase 9-10 (HENOS-Web)
- ✅ Translation UI works end-to-end
- ✅ Verification results display correctly
- ✅ User can understand and act on suggestions
- ✅ UI is responsive and accessible

### Phase 11-14 (AI Integration)
- ✅ AI adapter supports OpenAI (multiple models: GPT-4, GPT-3.5-turbo)
- ✅ Business property suggestions are relevant and accurate
- ✅ Compliance checking works for at least 2 domains
- ✅ Cost/usage tracking is accurate

---

## 10. Next Steps

### Immediate Actions (Week 1)

1. **Setup**
   - Create branch for IR migration
   - Set up test infrastructure
   - Document current state

2. **Phase 1 Kickoff**
   - Create IR module structure
   - Extract IR types
   - Begin validation implementation

### Ongoing Activities

1. **Documentation**
   - Update API documentation as endpoints change
   - Document IR schema
   - Create user guides for new features

2. **Testing**
   - Continuous integration for all phases
   - Performance benchmarking
   - Security review for AI integrations

3. **Code Review**
   - Review each phase before merging
   - Maintain code quality standards
   - Document architectural decisions

---

## 7. Cleanup and Deprecation

### Phase 16: Code Cleanup (Post-Migration)

**Duration:** Week 16+ (after all phases complete)  
**Dependencies:** Phases 1-3 complete, IR→CLTS translation stable  
**Priority:** Medium

#### Objectives

Remove deprecated BPMN→ctxdsl translation logic that's been replaced by the IR-based pipeline.

#### Tasks

1. **Identify deprecated code**
   - Review `src/translation/bpm_xml.rs` for old `parse_bpmn_xml()` function
   - Review `src/translation/bpm.rs` for direct BPMN→ctxdsl logic
   - Identify all call sites of deprecated functions
   - Document what each deprecated function does and its replacement

2. **Update call sites**
   - Replace all internal calls to deprecated functions
   - Ensure all translation paths use: BPMN → IR → CLTS
   - Update tests to use new IR-based paths

3. **Deprecation warnings**
   - Add `#[deprecated]` attributes to old functions
   - Add deprecation messages pointing to new IR-based functions
   - Document migration path in function docs

4. **Remove deprecated code**
   - After deprecation period (1-2 releases), remove deprecated functions
   - Remove unused helper functions
   - Clean up duplicate translation logic

5. **Update documentation**
   - Remove references to old translation paths
   - Update architecture diagrams
   - Update user guides

#### Files to Review

**`src/translation/bpm_xml.rs`:**
- `parse_bpmn_xml()` - Returns JSON string, replaced by `parse_bpmn_xml_to_ir()`
- Any direct BPMN→ctxdsl logic (should use IR→CLTS)

**`src/translation/bpm.rs`:**
- Old embedded `BpmModel` struct (now in `ir/model.rs`)
- Direct BPMN JSON→ctxdsl logic (should use IR→CLTS)
- Duplicate validation logic (now in `ir/validation.rs`)

**Test files:**
- Update tests to use IR module
- Remove tests for deprecated paths
- Add tests for IR→CLTS translation

#### Validation

- **Unit Tests:** All tests pass with new IR-based paths
- **Integration Tests:** End-to-end translation works correctly
- **Manual Tests:** Verify no functionality is lost after cleanup

---

## Appendix A: File Structure Changes

### New Files

```
src/
├── translation/
│   └── ir/                      # NEW: IR module
│       ├── mod.rs
│       ├── model.rs
│       ├── validation.rs
│       └── serialization.rs
├── bpm/
│   └── analysis/                # NEW: Property verification
│       ├── mod.rs
│       ├── structural.rs
│       ├── behavioral.rs
│       └── business.rs
└── ai/                          # NEW: AI integration
    ├── mod.rs
    ├── adapter.rs
    ├── client.rs
    ├── prompts.rs
    ├── config.rs
    └── providers/
        ├── mod.rs
        └── openai.rs
```

### Modified Files

```
src/
├── translation/
│   ├── bpm_xml.rs              # Refactored: Use IR, remove old JSON generation
│   ├── bpm.rs                  # Refactored: Use IR, remove old BPMN→ctxdsl logic
│   └── bpm_json.rs             # NEW: Separate JSON extractor
└── api/
    ├── models.rs               # NEW: IR, verification models
    ├── handlers.rs             # NEW: Verification endpoints
    └── openapi.rs              # Updated: New schemas
```

### Files to Review for Removal/Deprecation

After IR migration is complete, review and potentially remove:

1. **`src/translation/bpm_xml.rs`:**
   - Old `parse_bpmn_xml()` function that returns JSON string (if replaced by `parse_bpmn_xml_to_ir()`)
   - Any direct BPMN→ctxdsl translation logic that's now handled by IR→CLTS

2. **`src/translation/bpm.rs`:**
   - Old embedded `BpmModel` struct (now in `ir/model.rs`)
   - Direct BPMN JSON→ctxdsl logic that bypasses IR (should use IR→CLTS path)
   - Any duplicate translation logic that's now in IR→CLTS translation

3. **Test files:**
   - Update tests to use IR module
   - Remove tests for deprecated direct translation paths

---

## Appendix B: API Endpoint Summary

### Existing Endpoints (Modified)

- `POST /api/v1/translate/bpm` - Add `include_ir` option, verification options

### New Endpoints

- `POST /api/v1/translate/bpm/ir` - Extract IR only
- `POST /api/v1/bpm/verify/structural` - Structural property checks
- `POST /api/v1/bpm/verify/behavioral` - Behavioral property checks
- `POST /api/v1/bpm/verify/business` - Business property checks (AI)
- `POST /api/v1/bpm/verify/all` - Combined verification
- `POST /api/v1/translate/bpm/batch` - Batch translation
- `GET /api/v1/translate/bpm/status/{job_id}` - Translation job status

