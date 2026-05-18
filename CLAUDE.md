<!-- last reviewed: 2026-05-16 -->

# CLAUDE.md — Project Instructions for Claude Code

> **How to read this file.** This file is loaded into context for every Claude Code session in this repo. Sections tagged `[RULE]` are binding constraints, not suggestions — violating them blocks merge. Sections tagged `[REFERENCE]` and `[CONTEXT]` are background. When a task touches a subsystem documented under `docs/`, follow the link rather than guessing from prior knowledge.

## Contents

1. [Project Overview](#project-overview) `[CONTEXT]`
2. [Workspace Structure](#workspace-structure) `[REFERENCE]`
3. [CI Requirements](#ci-requirements) `[RULE]`
4. [Git Identity](#git-identity) `[RULE]`
5. [Surface Parity](#surface-parity) `[RULE]`
6. [Soundness Guarantees](#soundness-guarantees) `[RULE]`
7. [Abstraction Guidelines](#abstraction-guidelines) `[RULE]`
8. [Documentation Traceability](#documentation-traceability) `[RULE]`
9. [Claims Integrity](#claims-integrity) `[RULE]`
10. [Adapter / Emitter Capability Use](#adapter--emitter-capability-use) `[RULE]`
11. [Git Operations & Destructive Commands](#git-operations--destructive-commands) `[RULE]`
12. [Code Reuse, Dead Code, Performance, Security](#code-reuse-dead-code-performance-security) `[RULE]`
13. [Available Agents and Skills](#available-agents-and-skills) `[REFERENCE]`
14. [Private Files Policy](#private-files-policy) `[RULE]`
15. [Reference Docs Index](#reference-docs-index) `[REFERENCE]`

---

## Project Overview

Mununu is a formal verification tool for analyzing and synthesizing controllers for reactive systems modeled as Compositional Labeled Transition Systems (CLTS). It includes mu-calculus formula evaluation, controller synthesis, LTL pattern support, state variable abstraction, and a DSL (CTXDSL) for specifying concurrent systems.

See `docs/architecture/` for the three-layer model (extraction → adaptation → verification).

## Workspace Structure

Cargo workspace, three crates:

```
crates/
├── mununu-core/      (lib: verification engine, adapters, composition, IR, shared types)
├── mununu-cli/       (bin: `mununu` — CLI + HTTP API server)
└── mununu-extract/   (bin: `mununu-extract` — tree-sitter AST extraction)
```

Edition 2024. Toolchain pinned by `rust-toolchain.toml`; see [`docs/toolchain.md`](docs/toolchain.md) for the bump procedure and current clippy-lint compatibility notes.

## CI Requirements

The gate is `make ci` — the same command runs locally, in pre-commit hooks, and in GitHub Actions. If a contributor cannot reproduce a CI failure with `make ci`, the contract is broken.

```bash
make ci    # = cargo fmt --check + cargo clippy -D warnings + cargo test --workspace
```

For CI-exact reproduction inside the pinned dev container, see [`docs/dev-container.md`](docs/dev-container.md). For finer-grained cargo invocations (per-crate, specific tests, benches, server, extractor), see [`docs/build-recipes.md`](docs/build-recipes.md). Install pre-commit hooks via `./scripts/setup-hooks.sh` — the hook runs `cargo fmt --check`, `cargo clippy`, and `cargo test`.

The `security-audit` CI job runs `cargo audit`. The `dependency-check` job is non-blocking.

## Git Identity

All commits in this repository must use:

```
Name:  Mariano Cerrutti
Email: vscorza@gmail.com
```

Pass `--author="Mariano Cerrutti <vscorza@gmail.com>"` when committing, or set it via the commit environment. Do not use the machine's default hostname-based email.

## Surface Parity

Every user-visible capability ships on **CLI**, **HTTP API**, and **UI** in the same PR. The three surfaces are:

- **CLI**: a clap subcommand or flag in `crates/mununu-cli/src/main.rs`.
- **API**: a handler in `crates/mununu-core/src/api/handlers.rs` (route in `server.rs`, types in `models.rs`).
- **UI**: a typed client in `mununu-ui/src/api/endpoints.ts` or a hook/component that exercises the behavior.

The `/parity-check` skill verifies this automatically. PRs that drift the three surfaces are rejected. Single-surface exceptions (e.g., a developer-only CLI convenience) must declare themselves inline using the Documentation Traceability surface tag format: `surface: CLI-only — <one-line justification>`.

## Soundness Guarantees

Mununu is a model checker that exhaustively verifies finite-state models. Its verdicts are correct **for the model**. Whether they transfer to the real system depends on the abstraction:

- **Safety + over-approximation → SOUND.** If the model says safe, the real system is safe (within the modeled scope). Extra behaviors can only add violations, not hide them.
- **Liveness + over-approximation → UNSOUND.** The model may show spurious progress from noop loops, havoc branches, or async interleaving without fairness.
- **Safety + under-approximation → UNSOUND.** The model may miss violations from behaviors it doesn't capture.

When contributing adapters or modifying the Kripke builder:

- Document every `eval_expr → None` choice as over-approx or under-approx using `// SOUNDNESS:` comments.
- Never mix directions within a single model without documenting it.
- Add a soundness regression test for any new abstraction decision.

Strategy extraction uses **signature-based selection** from iteration ranks. Both players have memoryless winning strategies (positional determinacy of parity games, Zielonka 1998); memoryless on the model-checking product = finite-memory on the plant. See [`docs/synthesis.md`](docs/synthesis.md) for `ControllerMode` options, lasso trace format, counterstrategy emission, and the Skolem-paradigm rules for nondeterminism vs. controllability.

Contract-specific soundness (chaotic-stub default, cyclic-discharge handling, codesign rules) is in [`docs/design/black-box-modules.md`](docs/design/black-box-modules.md) and [`docs/design/hw-sw-codesign-extraction.md`](docs/design/hw-sw-codesign-extraction.md).

## Abstraction Guidelines

Every adapter, every CTXDSL source, every sidecar makes an abstraction decision — what to enumerate, what to bucket, what to drop. The Soundness Guarantees section above tells you the three directions (safety + over-approximation = sound, etc.); the **per-subsystem recipe** — which mununu primitive to reach for, when, and what each primitive preserves — lives in [`docs/abstraction.md`](docs/abstraction.md). Read it before authoring a new adapter, a new Kripke-builder rule, or a non-trivial CTXDSL source.

The load-bearing rules at a glance:

- **Pick the *minimum* abstraction that keeps the property decidable.** Coarser is fine when safety is the only concern; finer is required when liveness or order-sensitive properties are in play.
- **Declare the posture explicitly** — `AbstractionType::Symbols([…])` in a sidecar, a comment block at the top of a hand-authored CTXDSL, or a `mem { x : shared }`-style tag in the microcode discipline.
- **Add a `// SOUNDNESS:` annotation** at every `eval_expr → None` choice and every adapter decision that drops information. State whether it is over- or under-approximation. `/soundness-check` is the audit skill.
- **Automated extraction is viable when the abstraction shape is uniform across instances and the source format carries the structural information** (`c-codesign` + register-map; the planned `microcode` adapter; the planned chaotic-stub generator). Everything else — pipelines, caches, buses, PLICs, watchdogs — gets a **parameterised CTXDSL library template**, not an extractor.
- **Add a regression test** for every abstraction decision that touches an adapter or the Kripke builder. The test must exercise the abstract case and at least one concrete case that maps into the same abstraction class.

For benchmarks of state-space cost per abstraction choice, see `docs/abstraction.md`'s "What this doc deliberately does not do" — they are not yet shipped, and any claim about absolute state-space numbers must be measured per-model.

## Documentation Traceability

**Rule.** Every documentation page or section that describes a feature, flag, endpoint, file format, syntax form, or behavior must be traceable to a *live* code artifact (Rust `struct` / `enum` / `fn`, clap arg, axum handler, TypeScript client, or a checked-in configuration file) that is reachable from at least one of mununu's three user-facing surfaces — **CLI**, **HTTP API**, or **UI**. Documentation that cannot point at such an artifact is either obsolete, aspirational, or unverifiable, and must be marked or removed.

**Why.** Doc rot is the single most common reason users distrust the tool. Anchoring docs to symbols lets reviewers grep for drift the moment code is renamed, removed, or made unreachable from any surface.

**Where this applies.** `wiki/**`, `docs/**` (except `docs/architecture/` design notes and planning docs tagged `> Status: planning`), `README.md`, `examples/**/README.md`, and the `description:` / inline guidance inside `.claude/agents/**` and `.claude/skills/**`.

**Anchor format.** Every H2 / H3 section that documents a concrete capability must carry a *Source of truth* line near the top:

```markdown
> Source of truth: [`<symbol or path>`](<relative path>#L<line>) — surface: CLI | API | UI | (CLI+API+UI)
```

The path must be a real file in this workspace. Line numbers are optional but recommended for items inside long files. The surface tag declares which of the three surfaces exposes the documented behavior; single-surface tagging requires inline justification (`surface: CLI-only — <reason>`).

**Not covered.**

- Pedagogical "concept" sections (what mu-calculus is, what a Kripke structure is). Tag them `> Concept: <one-line>` so `/docs-traceability` skips them.
- Planning, vision, or future-work documents. Tag them `> Status: planning` at the top of the file.
- Tutorial prose that walks through running an example — these anchor to the example file path itself.

**Acceptable "alive".** The symbol is referenced (not just declared) by code reaching a user-facing surface; or the configuration file is consumed by a binary or test target that ships.

**Unacceptable.** Anchors to `#[allow(dead_code)]` symbols, deleted-test references, non-default feature flags no surface enables, files outside the repo, closed-source dependencies, or `mununu-private/`.

**Drift detection.** The `/docs-traceability` skill walks every Markdown file under covered paths, parses each `> Source of truth:` line, and verifies the anchor resolves, the symbol exists at the cited line, and the surface tag matches at least one reachable mention on that surface. Broken anchors fail the check. New sections without an anchor fail unless tagged `> Concept:` or `> Status: planning`.

**Renames and removals.** The same commit that renames or removes a symbol must update every documentation anchor that points at it. `/parity-check` confirms a *feature* is wired across surfaces; `/docs-traceability` confirms a *doc page* still points at code that exists. Both must pass for a change to ship.

## Claims Integrity

Every public claim about mununu's ability to find bugs, verify properties, or improve security of external systems must be backed by reproducible evidence against real implementations. This applies to README examples, wiki case studies, blog posts linked from the repo, conference papers, and any material that references real-world systems.

The load-bearing rules at a glance:

- **Models from source, not documentation.** Models written from API docs are "design pattern demonstrations," never findings about the real system.
- **Planted bugs are demos, not findings.** Language must reflect this.
- **Severity honesty.** Distinguish vulnerability vs. correctness issue vs. structural gap vs. design-pattern violation.
- **Reproduction path required.** Either a real-system test case or an explicit "structural, not yet reproduced" disclosure.
- **Verification-first workflow.** The CTXDSL model + `mununu context eval` is the oracle, not human code reading.
- **RTL counterexamples** must be validated under Verilator in the sibling `hw-verif:latest` image; model-only traces are labeled as such.
- **Hand-authored LLVM IR fixtures** demonstrate matcher behavior, not end-to-end clang handling. Real `.c` files are required for "the extractor handles `<idiom>`" claims.
- **Editorial framing for publications**: lead with the example, not the feature.

The full policy — with the abstraction-soundness procedure, the extraction-pipeline contract, the RTL evidence rules, the C-extractor unit-vs-end-to-end distinction, and the editorial framing rules for LinkedIn / Substack / talks — is at [`docs/policies/claims-integrity.md`](docs/policies/claims-integrity.md). Read it before publishing anything that references a real system.

## Adapter / Emitter Capability Use

The CLTS data model and CTXDSL grammar already express more than most adapters reach for. When writing or modifying an extractor, adapter, or emitter, prefer these primitives over re-encoding source-language features as state-name suffixes or parallel single-label edges.

- **Multi-label transitions.** A single CLTS edge carries a `SmallVec<[LabelId; 4]>` of labels — see `crates/mununu-core/src/clts/mod.rs:265`. CTXDSL: `transition s -> t on label a, label b;` (parser at `crates/mununu-core/src/context_dsl/parser.rs:733`, AST at `crates/mununu-core/src/context_dsl/ast.rs:156`). Multi-labeled edges are *one* transition, not parallel ones.
- **Per-state predicates.** Kripke-style state labeling via `state_variable_bitset` (`crates/mununu-core/src/clts/mod.rs:1173`) and `state_valuation` (`crates/mununu-core/src/clts/mod.rs:1178`). CTXDSL: `predicates { predicate foo = state S1; }` (parser at `crates/mununu-core/src/context_dsl/parser.rs:485`). Use these instead of encoding state attributes into state names.
- **Per-state structured valuations.** Hand-write display-only metadata directly on a state: `state S1 { valuations { signal_a = 1; phase = idle; } };`. Realize merges these with adapter-side `ContextDoc.state_valuations` and registers them via `Clts::with_valuation_for_state`. The CTXDSL emitter round-trips them.
- **Per-label controllability.** `LabelControllability { Controllable, Internal, Uncontrollable }` at `crates/mununu-core/src/clts/mod.rs:248`. Declare controllability in the automaton's `controllable { ... }` / `internal { ... }` blocks; do not fold it into label-name prefixes.
- **Rich modal guards.** `Guard { labels, current, next, control, max_steps }` at `crates/mununu-core/src/mu_calculus/mod.rs:323`. A single `[...]` or `<...>` modality can constrain all five axes — labels, current-state predicates (`req_cur` / `forb_cur`), next-state predicates (`req_next` / `forb_next`), controllability class (`ctrl = controllable | environment | all`), and step bound (`steps`). Syntax: `[(labels = {a}, req_next = {active}, ctrl = controllable)] φ`. Reach for `req_next` / `forb_next` whenever a property is naturally phrased "after this transition the system must be in a state where …" — the most under-used primitive.

**Reference implementations.** Signal-state emit path with turn-aware `[(ctrl=Controllable)]` guards: `crates/mununu-core/src/adapter/emit.rs:587-872`. SystemVerilog Kripke valuations: `crates/mununu-core/src/adapter/systemverilog/kripke.rs:450`.

**Anti-pattern.** Do not re-encode source features as state-name suffixes (e.g. `S1_req_high`) or as parallel single-label edges between the same source/target pair when a multi-label edge fits.

**Rule.** When adding or modifying an adapter or emitter, prefer these primitives. If a primitive is intentionally unused, leave a one-line comment explaining why (e.g. `// AIGER inputs are single-bit; multi-label has no semantic content here`). Reviewers and the `/soundness-check` skill flag silent under-use.

## Git Operations & Destructive Commands

**Destructive git operations require explicit user instruction in the current session prompt.** This applies to:

- `git reset --hard` (discards working tree changes — irrecoverable from reflog)
- `git push --force` / `git push -f` / `git push --force-with-lease` (overwrites remote history)
- `git checkout -- <paths>` / `git restore <paths>` (discards working tree changes for paths)
- `git clean -f` / `git clean -fd` (deletes untracked files)
- `git branch -D <branch>` (force-deletes a branch with unmerged commits)
- `git stash drop` / `git stash clear`
- `git rebase --interactive` followed by squash/drop of unpushed-but-staged work

**Why.** Working-tree state is not recoverable from `git reflog` — only commits are. A `--hard` reset on a working tree carrying hours of uncommitted work permanently loses that work unless an editor's local-history snapshot exists. (Incident: `.claude/plans/update-the-graph-endpoint-crystalline-frost.md` — a botched revert + `--hard` reset cost ~1730 lines of in-flight work that VSCode local history, Time Machine, and APFS snapshots could not recover.)

**How to apply.**

- Never run any of the above to "clean up" or "reset state" unless the user has typed an instruction containing the specific command.
- If a botched `git revert` or merge-conflict resolution is in progress, prefer `git revert --abort`, `git merge --abort`, `git cherry-pick --abort`, or `git reset --soft` (preserves working tree).
- If the working tree must be discarded for a routine operation (e.g. switching branches), first `git stash push -u -m "<reason>"` to preserve untracked files, and surface the stash to the user.
- For pre-commit hook failures, *create a new commit* — never `--amend` and never `--no-verify` unless the user explicitly authorised it.
- Before any operation that may modify the working tree of files the user has been actively editing, snapshot via a savepoint branch + push to remote (`git checkout -b recovery/savepoint-<date> && git add -A && git commit --no-verify -m "savepoint" && git push -u origin <branch>`).

**Propagation.** Any subagent invocation that performs git operations on a real repo (not an `isolation: "worktree"` clone) inherits this restriction. Subagents may take destructive actions only inside `isolation: "worktree"` agents.

## Code Reuse, Dead Code, Performance, Security

- **Before writing new utility code**, check if the functionality already exists. Prefer established libraries over hand-rolling for common tasks. Pin with ranges in `Cargo.toml`.
- **Remove unused code and dependencies promptly.** Use `cargo clippy` and `cargo audit` to catch issues. No "just in case" packages.
- **Test naming describes behavior, not implementation.** Prefer integration tests over excessive mocking. Pre-commit hook is the primary CI gate; GitHub Actions is secondary.
- **API handler performance.** Every handler re-parses and re-realizes context from scratch — keep handler logic lightweight after realization. **Never run controller synthesis in summary endpoints.** Summarize reports declarations; synthesis belongs only in the synthesis endpoint. Add timing instrumentation (`tracing::info!` with `Instant`) to any new handler; log parse, realize, and work phases separately. The UI client has a 10-second default timeout and a 120-second extended timeout; design accordingly.
- **Formula inversion (fixpoint duality).** When inverting mu-calculus formulas, do NOT negate fixpoint variable references inside the body — the dual fixpoint's changed starting point handles the semantics. Negating variables causes infinite oscillation between all-true and all-false.
- **Wiki maintenance.** Wiki pages live in `wiki/` and push to the GitHub wiki repo. Update when DSL syntax, endpoints, UI flow, composition modes, or formula operators change. Every CTXDSL example in wiki pages must be tested against the binary before publishing. Every wiki page must comply with Documentation Traceability.
- **Security (OWASP).** Never interpolate user input into commands or templates. Validate and constrain all external input. No sensitive data in logs.

## Available Agents and Skills

These live under `.claude/agents/**` and `.claude/skills/**`. Invoke them rather than reproducing their logic inline.

| Name | When to invoke |
|---|---|
| `/quality-session` | Metric-driven refactoring session with before/after measurement. Not a point-in-time review. |
| `/quality-inventory` | One-shot SLOC / pub-count / nesting / dead-code inventory over a scope. |
| `/design-review` | Qualitative KISS / DRY / SOLID / YAGNI review of a target scope. |
| `/parity-check` | Verifies a capability ships across CLI + API + UI. Run before claiming a feature is "done." |
| `/docs-traceability` | Walks Markdown under covered paths and verifies every `Source of truth:` anchor. |
| `/soundness-check` | Flags `eval_expr → None` choices that are undocumented or silent under-uses of CLTS / CTXDSL primitives. |
| `/review-orchestrator` | Coordinates a full review across `/parity-check`, `/docs-traceability`, and the qualitative skills. |
| `/domain-adequacy` | Checks that a domain profile (agentic, codesign) covers the controllability / composition / label conventions for its target. |
| `target-executor` | RTL counterexample reproduction agent. See its Phase 3.5 for the Verilator-under-`hw-verif:latest` procedure and `.claude/reviews/prospector/staging/RTL-002/repro/` for the canonical pattern. |

When extending an agent or skill, follow Documentation Traceability for the `description:` and inline guidance.

## Private Files Policy

Sensitive or unpublished materials live in the sibling private repo, not here: `/Users/marianocerrutti/git_repo/mununu-private/`.

| Goes in `mununu` (public) | Goes in `mununu-private` |
|---|---|
| Rust source code, adapters, CLI | Paper sources (LaTeX, BibTeX, figures) |
| Adapter formats: TLSF, AIGER, Promela, XState, SystemVerilog, extraction | Benchmark scripts and expected outputs (`artifact/`) |
| Extraction adapter (`src/adapter/extraction/`) | Extraction specs with CVE/vulnerability data (`tools/extraction_specs/`) |
| JSON Schema for `.espec.json` (`tools/extraction_spec_schema.json`) | Protocol CTXDSL models (`examples/protocols/`) |
| `mununu extraction validate` and `... check` subcommands | MCP extracted CTXDSL (`examples/agentic/mcp_extracted/`) |
| Shared IR, emitter, format detection | Governance policy scripts (`tools/validate_governance.sh`) |
| Unit and integration tests | Tutorial materials, slides, cheatsheets |
| Public documentation (README, wiki) | Internal evaluation data, drafts, notes |

**Extraction-framework boundary.** Tool capabilities (adapter code, validation logic, provenance checking, JSON schema) belong in `mununu`. Private content (actual specs referencing CVEs, generated CTXDSL containing vulnerability details, repo-specific CI policy) belongs in `mununu-private`.

**Rule.** Before adding any file to `mununu/` that you would not want publicly visible, move it to `mununu-private/` instead. Add the path to `mununu/.gitignore` as a safety net. `.gitignore` already excludes `/paper/`, `/artifact/`, `/examples/syntcomp/`, `/examples/scalable/`, and `/tutorial/`. If you add a new sensitive directory, add it to `.gitignore` immediately.

## Reference Docs Index

Reference and how-to material — read when the task calls for it, not on every session.

- [`docs/architecture/`](docs/architecture/) — three-layer model, internal data flow.
- [`docs/dev-container.md`](docs/dev-container.md) — pinned Docker dev container.
- [`docs/build-recipes.md`](docs/build-recipes.md) — finer-grained `cargo` invocations beyond `make ci`.
- [`docs/toolchain.md`](docs/toolchain.md) — Rust version pinning and clippy compatibility notes.
- [`docs/cli-cookbook.md`](docs/cli-cookbook.md) — common `mununu` CLI invocations.
- [`docs/synthesis.md`](docs/synthesis.md) — `ControllerMode`, signature-based extraction, lasso traces, Skolem-paradigm rules.
- [`docs/abstraction.md`](docs/abstraction.md) — Per-subsystem abstraction recipe: which mununu primitive to reach for, when, what each preserves, and where automated extraction is viable vs where parameterised templates are the right pattern.
- [`docs/docker.md`](docs/docker.md) — Dockerfile best practices for Rust services in this project.
- [`docs/adapters/agentic.md`](docs/adapters/agentic.md) — Agentic AI orchestration via native CTXDSL, XState, CrewAI, or LangGraph.
- [`wiki/Verify-Project-Flow.md`](wiki/Verify-Project-Flow.md) — General N-source verify framework: `verify.toml` manifest, alphabet bindings, composition, the orchestrator pipeline, and the example fleet.
- [`wiki/Agentic-Adapters.md`](wiki/Agentic-Adapters.md) — Native CrewAI + LangGraph adapter details: accepted JSON shapes, translation semantics, the three agentic property templates.
- [`docs/adapters/extraction.md`](docs/adapters/extraction.md) — `.espec.json` extraction adapter, mode filtering, property templates.
- [`docs/adapters/tlsf-aiger.md`](docs/adapters/tlsf-aiger.md) — Turn-based compound-label encoding for TLSF and AIGER.
- [`docs/policies/claims-integrity.md`](docs/policies/claims-integrity.md) — Full claims-integrity policy (10 rules + editorial framing).
- [`docs/design/black-box-modules.md`](docs/design/black-box-modules.md) — Document A (foundations of black-box contracts).
- [`docs/design/rtl-frontend-unification.md`](docs/design/rtl-frontend-unification.md) — Document B (SV frontends).
- [`docs/design/contract-corpus-and-config.md`](docs/design/contract-corpus-and-config.md) — Document D (corpus + annotations).
- [`docs/design/hw-sw-codesign-extraction.md`](docs/design/hw-sw-codesign-extraction.md) — Document C (HW/SW codesign).
- [`docs/design/c-extraction-correctness-scope.md`](docs/design/c-extraction-correctness-scope.md) — Honest scope statement for the C extractor.

## Environment Variables

| Variable | Purpose |
|---|---|
| `RUST_LOG=mununu=info` | Enable logging |
