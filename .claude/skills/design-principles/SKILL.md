---
name: design-review
description: >
  Evaluates code against KISS, DRY, SOLID, and YAGNI principles.
  Checks adapter architecture and module boundaries.
  Use when asked to review design, architecture, or code structure.
---

Review $ARGUMENTS (or changed files if no args) for design principle violations.

This is a formal verification tool with a modular adapter architecture. Key design constraints:
- Adapters (TLSF, AIGER, Promela, XState, CrewAI, LangGraph, A2A, extraction) share an IR
- The delegation pattern is intentional: CrewAI/LangGraph/A2A parse native JSON -> build XState JSON -> delegate to XState adapter pipeline
- BDD/LTL/mu-calculus core must stay allocation-lean

## Principles

**KISS** — Flag functions over ~40 lines of logic. Adapter `translate()` methods can be longer if they handle many construct cases, but each case should be simple. Check that guard expressions and LTL formula builders aren't over-nested.

**DRY** — Search for duplicated logic across:
- Adapter implementations (common parsing patterns should be in shared IR)
- Test setup code (should use test helpers or fixtures)
- CLI argument handling vs API endpoint handling

**SOLID**:
- **Single Responsibility**: each adapter module handles one format. The shared IR handles emission. Don't mix parsing and emission in one function.
- **Open/Closed**: new adapters should be addable without modifying existing adapter code. Check the adapter registry/dispatch.
- **Interface Segregation**: adapter traits should be minimal. Don't force adapters to implement methods they don't need.
- **Dependency Inversion**: core verification (`clts/`, `ltl/`, `mu_calculus/`) must not depend on adapter code or CLI code.

**YAGNI** — Flag:
- Unused struct fields or enum variants
- Generic parameters that are only ever instantiated with one type
- Speculative adapter infrastructure for formats not yet supported
- Dead code behind feature flags that are never tested in CI

## Output Format

For each violation: quote the offending code, name the principle, suggest a concrete simpler alternative.
