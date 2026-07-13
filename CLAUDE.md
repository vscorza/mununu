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
15. [Reasoning & Recommendation Honesty](#reasoning--recommendation-honesty) `[RULE]`
16. [Reference Docs Index](#reference-docs-index) `[REFERENCE]`

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

### SVA-verification e2e validation (slang) `[REFERENCE]` (added 2026-06-27)

The SVA front-end (`mununu sv extract-sva` / `mununu sv verify-auto`) shells out to **slang** (SystemVerilog elaboration), **sv2v**, and **yosys**. Per the subprocess-tools-are-not-bundled policy, these are NOT in `mununu-dev`, so the slang-gated end-to-end tests are `#[ignore]`d and do not run in `make ci`. Their mechanisms are covered by non-ignored unit tests; the `#[ignore]` tests validate the full chain when the tools are present.

**Run them in the `mununu-sva` image** (`docker/Dockerfile.sva` — extends `mununu-dev` with pinned slang + sv2v + yosys):

```bash
docker build -f docker/Dockerfile.dev -t mununu-dev .     # once (or on toolchain bump)
docker build -f docker/Dockerfile.sva -t mununu-sva .     # once
docker volume create mununu-target                        # once (warm cargo cache)
docker run --rm -v "$(pwd)":/work -v mununu-target:/cargo-target \
  mununu-sva cargo test -p mununu-core --lib --all-features e2e_ -- --ignored --nocapture
```

**Host caveat.** slang ships prebuilts only for `linux-x86_64` and `macos-arm64`. On an Intel (x86_64) macOS host there is no native slang binary, and the workspace's `z3-sys` link also differs from the dev image — so the Linux `mununu-sva` image (which runs natively on x86_64 hosts) is the supported way to run these tests there. The `e2e_csrng_real_sva_verdict_breakdown` test reads only vendored fixtures (`examples/verify/m2_opentitan_csrng_main_sm` + the standard prim_assert macros from `examples/verify/m0_opentitan_prim_arbiter`), so it is fully reproducible.

**[RULE] Validate any slang / SVA-touching change in the `mununu-sva` image — never on the bare host (added 2026-07-13).** The dev host commonly has `sv2v` + `yosys` but **not** `slang`. That combination is a trap, not a convenience:

- `mununu sv verify-auto` / `sv extract-sva` (and the correct SVA→cube path) locate slang via `locate_slang()`; with slang absent they cannot run, so a "green" host run has simply *skipped* the real path.
- The **sv2v-only lift is worse than useless for SVA**: `sv2v` (0.0.13) silently **drops** `assert property (@…)` during conversion (both concurrent and immediate forms), so `sv_to_btor2` yields a BTOR2 with **0 `bad` nodes**. The plain `mununu sv verify` verb (sv2v+yosys, no slang) then reports a **VACUOUS `holds`** — the property was never checked. (2026-07-13 incident: an sv-yosys "safety-cube" wiring looked like it produced no result / a spurious `holds`; the whole chain was the sv2v drop, invisible without slang. Root-caused via `MUNUNU_KEEP_YOSYS_TMP=1` — `preprocessed.sv` had 0 asserts.)

**How to comply.** Any change that exercises `adapter/slang/**`, `sv extract-sva`, `sv verify-auto`, or an SV→BTOR2 path that must preserve assertions is validated by running the relevant `#[ignore]`d `e2e_` tests **in the `mununu-sva` image** (command above), not by a host `cargo test`. Pure-Rust / `btor2` / non-slang paths may still be validated on the host. When a host run of an SV path returns `holds` with no counterexample, treat it as *unverified* until reproduced under `mununu-sva` — a bare-host `holds` on SVA is presumed vacuous. slang honours `MUNUNU_SLANG_PATH`; the image provides a pinned build.

### Pre-push workspace check (added 2026-06-08)

**Rule.** Before `git push`, when a commit touches a field of a struct with multiple construction sites (`OriginalTransition`, `TransitionSpec`, `SignalAnnotation`, `CegarOptions`, `PredicateCubeLiftOptions`, `TransitionDecl`, etc.), run the CI-equivalent workspace check:

```bash
cargo check --workspace --all-features --tests
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo fmt --check
cargo test --workspace --all-features --doc    # <-- doctests too (revised 2026-06-08)
```

**Why.** Per-crate `cargo check` (the typical hook-light pattern under [Pre-commit hook serialisation](#pre-commit-hook-serialisation)) does NOT compile:
- the **legacy root-level bin** at `src/main.rs` (gated on `--features cli`, separate from the workspace `crates/mununu-cli/src/main.rs`).
- crates that depend on the touched type through workspace edges.
- test crates that construct the type in fixtures.
- **doctests** inside `///` examples that construct the type literally. `cargo check --tests` does NOT compile doctests — they have a separate compilation phase that only `cargo test --doc` (or `make ci`) runs. Doctest gaps surface only at CI.

Three recent incidents exposed the gap:
- `3923822` (K.2b): added `modality` to `OriginalTransition`; the api-feature build at `crates/mununu-core/src/api/graph.rs` had a variable-name typo CI caught (fix `04beae6`).
- `cfee81d` (K.1b-unrolled): added `additional_targets` to `OriginalTransition`; the legacy root bin's `src/main.rs:3738` site was missed; user surfaced the CI failure (fix `04f01f9`).
- `cfee81d` (K.1b-unrolled, second-order): the doctests in `crates/mununu-core/src/abstraction/unrolling.rs:911,945` constructed `OriginalTransition` literals without the new field; `c3ecb38` + `04f01f9` + `832f552` all ran CI red because the pre-push check (revised at `c3ecb38`) did not include doctests (fix `4a07df7`).

**Scope.** Trigger this check whenever the diff touches a `pub` field of any of the load-bearing struct types listed above. For commits that don't touch IR / sidecar / CegarOptions shape (e.g. parser-internal changes, doc edits), the lighter per-crate pattern in the next section is sufficient.

**Hook compatibility.** The full workspace check is HEAVY per the hook-serialisation rule below — do NOT run it concurrent with another commit's hook. Run it as a pre-push step (no other heavy work in flight), not as a concurrent background task.

### Pre-commit hook serialisation

**Rule (revised 2026-06-03).** When a pre-commit hook is running, **never** launch a second heavy cargo workload that competes for the rustc compilation pool or the nextest thread pool. Lightweight per-crate cargo work is allowed.

**Heavy** (FORBIDDEN to run concurrent with a hook):
- `cargo nextest run --workspace` / `cargo test --workspace` / `make ci`
- `cargo build --workspace` / `cargo check --workspace` / `cargo clippy --workspace`
- A second `git commit` whose hook would fire (don't chain commits — the prior hook must complete first)
- Any command that compiles or tests across the entire workspace at once

**Lightweight** (ALLOWED to run concurrent with a hook's test phase):
- `cargo check -p <one_crate>` (compile-only, much smaller resource footprint)
- `cargo clippy -p <one_crate>` (single crate)
- `cargo test -p <one_crate> --lib <specific_test_name>` (single test, single crate)
- `cargo fmt` / `cargo fmt --check` (no compilation)

**Always allowed** (no cargo invocation at all):
- File edits in the working tree (including the source files the hook is validating — the hook reads the COMMITTED state, not the working tree)
- Plan-doc edits in `.claude/plans/`
- Drafting commit messages, reading source, git status / log / diff queries
- Background CI status checks via `gh run view`

**Why.** macOS schedules pre-commit hooks at lowered priority (`STAT=SN`) when launched in the background. Two **heavy** cargo workloads competing for the same compilation pool reproduce the 2026-05-27 R.2.5 incident where 36 / 1853 tests took 1h 32min wall-clock. Lightweight per-crate work uses a fraction of the cores and shares the pool without starvation. Test execution itself is sub-millisecond per area; the wall-clock cost is entirely scheduling — so the relaxation is safe as long as no second heavy workload competes.

**How to comply.**

- Before kicking off any commit that runs the pre-commit hook (any `git commit` without `--no-verify`), check that no other **heavy** cargo / make-ci process is running.
- While a commit is in flight, you MAY draft + lightweight-validate the next commit's code in the working tree. You may NOT start a second hook or run a workspace-wide cargo invocation.
- Do not chain commits back-to-back. Land commit N's hook (verify via `git log` showing the new HEAD, or the sentinel file exit code) before launching commit N+1's `git commit`.
- Single-test validation runs (e.g. `cargo test -p mununu-core --lib <specific_test>`) are now permitted concurrent with a hook's test phase. Full per-crate test runs (`cargo test -p mununu-core --lib`, no test filter) are also permitted but use more resources — apply judgment based on what the hook is currently doing.
- Skipping the hook with `--no-verify` requires explicit per-commit user authorisation, per [Git Operations & Destructive Commands](#git-operations--destructive-commands).

**Propagation to subagents.** Any subagent that issues `git commit` or runs cargo inherits the heavy-vs-lightweight distinction. Subagents may freely run lightweight per-crate cargo work concurrent with another subagent's hook; they MUST NOT start a second hook or workspace-wide compile while another hook is in flight.

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

Mununu is a model checker that exhaustively verifies finite-state models. Its verdicts are correct **for the model**. Whether they transfer to the real system depends on the abstraction *and* on the alternation structure of the property.

### 2-valued abstraction (legacy primitives, hand-written CTXDSL, XState, microcode, agentic, …)

When the abstract model carries no may/must distinction (every transition is treated as definite), only the corners of the lattice are sound:

- **Safety + over-approximation → SOUND.** If the model says safe, the real system is safe (within the modeled scope). Extra behaviours can only add violations, not hide them.
- **Liveness + over-approximation → UNSOUND.** The model may show spurious progress from noop loops, havoc branches, or async interleaving without fairness.
- **Safety + under-approximation → UNSOUND.** The model may miss violations from behaviours it doesn't capture.
- **Liveness + under-approximation → SOUND** but trivially weak (fewer paths only).

Properties with **alternating fixpoints** (νμ-style; GR(1) response patterns; nested obligations) collapse under plain over- or under-approximation — the outer ν needs a *may*-style upper bound while the inner μ needs a *must*-style lower bound, and a single direction cannot supply both. For these properties, use the KMTS path below.

### KMTS + Kleene 3-valued domain (canonical recipe for SystemVerilog/BTOR2; available to other adapters that opt in)

The KMTS pipeline tracks **may** and **must** edges separately and evaluates the full modal-mu calculus over a 3-valued (`KleeneT` / `KleeneF` / `KleeneBot`) domain. Bruns–Godefroid (CONCUR 2000) gives the soundness theorem: definite verdicts (`KleeneT`, `KleeneF`) transfer to the concrete system at **every alternation depth**, including νμ. A `KleeneBot` verdict triggers a CEGAR refinement step (R.5) that splits one abstract predicate and reruns.

Use the KMTS path whenever the property has a μ inside a ν (or a ν inside a μ), or when you need a sound liveness verdict on an over-approximating abstraction. See [`docs/abstraction.md`](docs/abstraction.md) §"The canonical recipe — KMTS predicates" and [`docs/design/kmts-theory.md`](docs/design/kmts-theory.md) for the theory and the per-adapter migration table.

### Discipline for contributors

- Document every `eval_expr → None` choice as over-approx or under-approx using a `// SOUNDNESS:` comment; `/soundness-check` flags undeclared sites.
- Never mix directions within a single model without documenting it.
- Add a soundness regression test for any new abstraction decision.
- When emitting a CLTS for a property with alternating fixpoints, prefer the KMTS path (`Transition::modality`, `state_3valued_predicates`) over the legacy primitives. Legacy adapters produce `Sharp`-everywhere KMTSes vacuously, which only carry the 2-valued guarantees above.

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

### Documentation Cadence Guideline

**Rule.** When more than **10 commits** have landed on `main` since the last commit touching `docs/`, `wiki/`, `README.md`, or `examples/`, the contributor's next commit should either:

1. Include a `docs/` or `wiki/` delta capturing accumulated changes, OR
2. Run `/docs-traceability` and record the verdict (no drift detected) in the commit message or a `.claude/reviews/docs-traceability/` note.

**Why.** The per-commit discipline already documents inline changes (commit messages, module `//!` docs, `// SOUNDNESS:` annotations). But user-facing docs (`docs/abstraction.md`, the wiki, example READMEs) drift in batches — a stretch of feature commits without wiki touches accumulates surface that authors of CTXDSL / API / UI consumers can't discover. The cadence rule turns that drift into a visible signal.

**How to check.**

```bash
make docs-audit                       # default threshold = 10 commits
make docs-audit DOCS_THRESHOLD=20     # custom threshold
```

`scripts/docs-audit.sh` (the underlying script) reports:
- The commit count since the last docs/wiki touch.
- Whether the threshold has been exceeded.
- A categorised breakdown of which code areas (CTXDSL syntax, API surface, CLI surface, composition, formula operators, adapters) saw recent commits — narrowing the search space for the actual wiki / docs update.
- A cross-repo reminder when `api/models.rs` was touched (mununu-ui's `src/api/types.ts` may need a matching type update).

**Cadence enforcement.** The check is **advisory**, not blocking — exit code 1 on threshold breach surfaces the warning but does not fail CI or hooks. The expectation is that contributors invoke `make docs-audit` periodically (e.g. before a multi-commit push, or at the close of a multi-session R-track sub-item arc).

**Cross-repo doc sync (mununu ↔ mununu-ui).** When `api/models.rs` field shapes change, `mununu-ui/src/api/types.ts` typically needs a matching update. There is no automated typegen between the two repos today; the audit script flags this as a manual reminder. Future work: an OpenAPI-driven typegen pipeline would make this drift detectable mechanically.

**Surface-of-truth anchors take precedence.** When the cadence rule fires, the contributor's first move is `/docs-traceability` (anchor-by-anchor verification of the existing docs) — that surfaces the most actionable drift. Adding new doc sections to cover new code is a secondary follow-up.

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

## Reasoning & Recommendation Honesty

**Rule.** Never justify a decision, recommendation, or deferral with an argument that mimics human limitations the agent does not have. In particular, do **not** invoke "the session is long," "end of session," "to be safe after a marathon," "I'm running low on steam," or any fatigue/attention-erosion framing as a reason to stop, defer, or hedge. The agent does not tire; turn N is produced at the same capability as turn 1. There is no quality decay that accumulates with wall-clock or turn count.

**Why.** Such framing is both false and load-bearing in the wrong direction: it dresses up a non-reason as a reason, which corrupts the actual cost/benefit the user is trying to weigh. (Incident 2026-06-29: a soundness-critical spike was deferred citing "rushing at the tail of a marathon" — a borrowed human failure mode. The genuine factors were intrinsic task difficulty and context-degradation, the latter of which actually argued for doing the work *while context was warm*, not deferring it.)

**The genuine session-linked factors — name these instead, when they truly apply:**

- **Context degradation** — long context gets summarized; earlier-established invariants (exact decisions, constraints, quantifier directions) can be lost or distorted. This argues for doing dependent work *while the relevant context is still fresh*, not for deferring it to a "fresh session" that must re-derive everything.
- **Token / compute cost** — a longer session spends more. This is a real budget axis and is the **user's** to weigh; surface it as a cost, do not assume it as a stop condition.
- **Intrinsic task difficulty / soundness-criticality** — some work (e.g. SMT quantifier placement) demands care *regardless of when it is done*. The correct response is procedural rigor (differential tests, oracles, validation), not postponement. "This is hard and must be validated" is a real reason; "this is hard and the session is long" is not.

**How to apply.** When tempted to defer or hedge, state the *actual* reason. If the only reason is genuinely "the user should make this call," say exactly that — do not pad it with a fabricated fatigue rationale. Build/cache state (warm `target/`, warm docker volumes) typically *improves* over a session, so longer-running work is often cheaper to iterate, not riskier.

## Reference Docs Index

Reference and how-to material — read when the task calls for it, not on every session.

- [`docs/architecture/`](docs/architecture/) — three-layer model, internal data flow.
- [`docs/dev-container.md`](docs/dev-container.md) — pinned Docker dev container.
- [`docs/build-recipes.md`](docs/build-recipes.md) — finer-grained `cargo` invocations beyond `make ci`.
- [`docs/toolchain.md`](docs/toolchain.md) — Rust version pinning and clippy compatibility notes.
- [`docs/cli-cookbook.md`](docs/cli-cookbook.md) — common `mununu` CLI invocations.
- [`docs/verifying-rtl.md`](docs/verifying-rtl.md) — the property verbs (`btor2 verify` / `verify-liveness` / `verify-recoverability`), no-sidecar `sv verify-auto`, and the agent-over-HTTP integration path (with the toolchain / embedded-SVA / fragment caveats).
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
