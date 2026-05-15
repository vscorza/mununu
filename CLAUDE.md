# CLAUDE.md — Project Instructions for Claude Code

## Project Overview

Mununu is a formal verification tool for analyzing and synthesizing controllers for reactive systems modeled as Compositional Labeled Transition Systems (CLTS). It includes mu-calculus formula evaluation, controller synthesis, LTL pattern support, state variable abstraction, and a DSL for specifying concurrent systems.

## Workspace Structure

Cargo workspace with three crates:

```
crates/
├── mununu-core/      (lib: verification engine, adapters, composition, IR, shared types)
├── mununu-cli/       (bin: `mununu` — CLI + HTTP API server)
└── mununu-extract/   (bin: `mununu-extract` — tree-sitter AST extraction)
```

See `docs/architecture/` for the three-layer model (extraction → adaptation → verification).

## CI Requirements

Before considering work done, verify all CI checks pass locally. The same `make ci` command is used locally and in GitHub Actions — if a contributor cannot reproduce a CI failure with one command, the contract is broken.

```bash
# Inside the pinned dev image (recommended — matches CI exactly):
docker build -f docker/Dockerfile.dev -t mununu-dev .
docker volume create mununu-target
docker run --rm -v $(pwd):/work -v mununu-target:/cargo-target mununu-dev make ci

# Or directly on the host:
make ci    # = make lint && make test
```

`make ci` runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. The `security-audit` job runs `cargo audit` for vulnerabilities. The `dependency-check` job is non-blocking.

## Reproducible Dev Container

`docker/Dockerfile.dev` pins the Rust toolchain and native build deps so every contributor and the CI runner execute the exact same commands.

```bash
# Build image (rare — only on Dockerfile/toolchain changes)
docker build -f docker/Dockerfile.dev -t mununu-dev .

# Create a named volume for cargo's target dir so subsequent runs stay warm.
# The container writes to /cargo-target (CARGO_TARGET_DIR), NOT to the host's
# target/. Host and container caches stay independent.
docker volume create mununu-target

# Ephemeral run (one-off command, warm cargo cache via named volume)
docker run --rm \
  -v $(pwd):/work \
  -v mununu-target:/cargo-target \
  mununu-dev make <verb>

# Persistent container (faster for iterative work)
docker run -d --name mununu-dev-c \
  -v $(pwd):/work \
  -v mununu-target:/cargo-target \
  mununu-dev sleep infinity
docker exec mununu-dev-c make <verb>
docker stop mununu-dev-c && docker rm mununu-dev-c
```

The Makefile verbs are: `build`, `test`, `lint`, `verify`, `ci` (= lint + test), `clean`. Run `make help` for the index.

If you skip the `mununu-target` volume, the container compiles from scratch every run (no cache survives the `--rm`). The host's `target/` is never written to by the container, so host-side `cargo` and container-side `make` do not contend for the same artifacts.

**RTL counterexample validation** uses the sibling `hw-verif:latest` image from `../hw-verification-uba` (the OSS CAD Suite is too heavy for the mununu dev image). Per-target reproductions live under `.claude/reviews/prospector/staging/<TARGET>/repro/Makefile` and are invoked as `docker run --rm -v $(pwd):/work hw-verif:latest make -C <leaf> sim`.

## Pre-commit Hooks

Tests MUST run in pre-commit hooks (not just CI). Install with:

```bash
./scripts/setup-hooks.sh
```

The pre-commit hook runs: `cargo fmt --check`, `cargo clippy`, `cargo test`.

## Build Commands

The root `Makefile` exposes the canonical verbs (`make help` for the index). Direct `cargo` invocations for finer-grained work:

```bash
# Workspace-wide via Makefile (host or `mununu-dev` container — same command)
make build       # cargo build --release for cli + extract
make test        # cargo test --workspace
make lint        # cargo fmt --check + cargo clippy -D warnings
make verify      # build + run mununu against examples/hw/handshake.ctxdsl
make ci          # lint + test (the gate)
make clean       # cargo clean

# Individual crates / finer cargo invocations
cargo build -p mununu-core                    # core library
cargo build -p mununu-cli                     # CLI binary (mununu)
cargo build -p mununu-extract                 # extraction binary (mununu-extract)
cargo build --release -p mununu-cli           # release CLI

cargo test -p mununu-core                     # core lib tests only
cargo test -p mununu-extract                  # extraction tests only
cargo test -p mununu-core test_name           # specific test
cargo test -- --nocapture                     # with output

# Run benchmarks
cargo bench -p mununu-core

# Run server
cargo run -p mununu-cli -- server             # default 127.0.0.1:8080
cargo run -p mununu-cli -- server --addr 0.0.0.0:3000

# Run extraction
cargo run -p mununu-extract -- config.extract.json --source file.ts --output spec.espec.json
```

## Architecture

See `docs/architecture/` for the full three-layer model.

```
crates/mununu-core/src/
├── adapter/        # Format adapter subsystem (external formats → CTXDSL)
│   ├── xstate/     # XState/Statecharts adapter (JSON → CTXDSL)
│   ├── systemverilog/ # SystemVerilog RTL adapter (SV → CTXDSL)
│   ├── tlsf/       # TLSF synthesis format adapter
│   ├── aiger/      # AIGER circuit format adapter
│   ├── promela/    # Promela/SPIN process model adapter
│   ├── extraction/ # Extraction spec adapter (.espec.json → CTXDSL)
│   │   └── ast_extract/ # Shared types: config, domain profiles, call summaries, state space
│   ├── ir.rs       # Shared intermediate representation (AdapterIR)
│   └── emit.rs     # CTXDSL emitter from IR (signal-state + explicit-automaton modes)
├── clts/           # Core CLTS data structure (builder, label store, state management)
├── composition/    # Product/composition engine (sync, async, superset modes)
├── context/        # CLTS registry, composition, synthesis, mu-calculus evaluation
├── context_dsl/    # DSL lexer, parser, AST, canonicalization, incremental loader
├── guard/          # Guard expression parsing, ComparisonOp, identifier sanitization
├── mu_calculus/    # Formula parsing, fixpoint evaluation, simplification
├── ltl/            # LTL to mu-calculus translation
├── abstraction/    # State variable abstraction (value domains, unrolling)
├── api/            # REST API (axum): summarize, synthesize, verify, graphs, import
├── persistence/    # CLTS disk serialization
├── examples/       # Sample CLTS builders
├── iter.rs         # Iterator utilities
└── main.rs         # CLI entry point
```

**Key architectural decisions:**
- `CltsBuilder` grows capacity in 20% increments for memory efficiency
- `bitvec`-backed state sets for mu-calculus fixpoints
- Incremental DSL loading with fingerprinting (`ContextDslCache`)
- Multi-level state abstraction (Boolean, Integer intervals, Symbol sets)

## CLI Examples

```bash
# Evaluate mu-calculus formula
mununu context eval context.ctxdsl --formula property_name --automaton automaton_name

# Synthesize controller
mununu context synth context.ctxdsl --formula property_name --automaton automaton_name

# Merge context files
mununu context merge main.ctxdsl sidecar.ctxdsl --output build/

# Generate graph visualization
mununu context graph context.ctxdsl --output graph.html

# Import from XState and synthesize
mununu context synth machine.xstate --adapter xstate --formula safety --automaton Machine

# Import from SystemVerilog
mununu context eval design.sv --adapter sv --formula safety --automaton FSM

# Agentic AI orchestration models live in examples/agentic/ as native CTXDSL or
# as XState JSON imported via the XState adapter. See examples/agentic/README.md.

# Export controller as XState JSON
mununu context synth machine.xstate --adapter xstate --formula safety --automaton Machine \
  --output-format xstate --emit-native controller.json

# Export controller as SystemVerilog module
mununu context synth design.sv --adapter sv --formula safety --automaton FSM \
  --output-format systemverilog --emit-native controller.sv

# Export controller as GDScript for Godot
mununu context synth game.espec.json --adapter extraction --formula safety --automaton FSM \
  --output-format gdscript --emit-native controller.gd
```

## Git Identity

All commits in this repository must use the following author identity:

```
Name:  Mariano Cerrutti
Email: vscorza@gmail.com
```

When committing, pass `--author="Mariano Cerrutti <vscorza@gmail.com>"` or set it via the commit environment. Do not use the machine's default hostname-based email.

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG=mununu=info` | Enable logging |

## Rust Version

The workspace pins an exact Rust version in two places:

- [rust-toolchain.toml](rust-toolchain.toml) — `channel = "1.95.0"`. rustup honors this at runtime, so host devs, CI, and the dev container all use the same toolchain.
- [docker/Dockerfile.dev](docker/Dockerfile.dev) — `ARG RUST_VERSION=1.95`. Mirrors the toolchain pin so the base image's bundled toolchain matches.

**Bumping the toolchain** is a two-file change in one commit:

1. Edit `rust-toolchain.toml`'s `channel`.
2. Edit the matching `ARG RUST_VERSION` in `docker/Dockerfile.dev`.
3. Run `make ci` locally — new clippy lints often surface on bumps and fixing them is part of the bump commit.

Drift between the two pins is silent: rustup will auto-download the toolchain `rust-toolchain.toml` names regardless of the image's bundled version, so CI may pass against a "wrong" toolchain. Treat any PR that touches one but not the other as a review red flag.

Edition is 2024 (set in each `Cargo.toml`).

## Clippy Compatibility

The pinned toolchain locks the lint set, so contributors don't get bitten by surprise lints from upstream Rust releases. Common patterns to avoid (from past bumps — keep these in mind when writing new code):

- **`unnecessary_unwrap`**: After checking `x.is_some()`, use `if let Some(v) = x` instead of `x.unwrap()`.
- **`needless_return`**: Don't use explicit `return` at the end of a function body.
- **`redundant_closure`**: Use `foo` instead of `|x| foo(x)` when passing to `.map()` etc.
- **`collapsible_match`** (Rust 1.95+): A nested `if` inside a `match` arm should become an arm guard — `Pat if cond => { ... }` rather than `Pat => { if cond { ... } }`.
- **`implicit_borrowing` (Edition 2024)**: Closure patterns like `|(&(a, _), _)|` are not allowed in Rust 2024 — use `|((a, _), _)| *a` instead.

When bumping, run `cargo clippy --workspace --all-targets -- -D warnings` against the new toolchain *before* committing the pin change so any new lint failures land in the same commit.

## Governance Rules

### Testing Best Practices

- Write test names that describe behavior, not implementation details.
- Prefer integration tests over excessive mocking.
- Pre-commit hook is the primary CI gate; GitHub Actions is secondary.

### Code Reuse & Library Policy

- Before writing new utility code, check if the functionality already exists.
- Prefer established libraries over hand-rolling for common tasks.
- Pin with ranges in Cargo.toml.

### Dead Code & Dependency Hygiene

- Remove unused code and dependencies promptly.
- Use `cargo clippy` and `cargo audit` to catch issues.
- Do not leave "just in case" packages.

### API & Endpoint Performance

- **Every API handler re-parses and re-realizes the context from scratch.** Keep handler logic lightweight after realization. Avoid running expensive operations (synthesis, composition) unless explicitly requested.
- **Never run controller synthesis in summary/informational endpoints.** The summarize handler must only report declarations, not execute synthesis. Synthesis is expensive (state-space exploration) and belongs only in the synthesis endpoint.
- **Formula inversion (fixpoint duality):** When inverting mu-calculus formulas, do NOT negate fixpoint variable references inside the body. The dual fixpoint's changed starting point (mu starts empty, nu starts full) handles the semantics. Negating variables causes infinite oscillation between all-true and all-false.
- **Add timing instrumentation** (`tracing::info!` with `Instant`) to any new endpoint handler. Log parse, realize, and work phases separately for debugging.
- **API timeout awareness:** The UI client has a 10-second default timeout (`apiClient`) and 120-second extended timeout (`aiApiClient`). Ensure endpoints complete within 10 seconds for standard operations; use the extended client only for explicitly heavy operations (counterstrategy, synthesis).

### Wiki Maintenance

- Wiki pages live in `wiki/` directory and are pushed to the GitHub wiki repo.
- Update wiki pages when: DSL syntax changes, new endpoints are added, UI flow changes, new composition modes or formula operators are introduced.
- Every CTXDSL example in wiki pages must be tested against the binary before publishing.
- Keep the References page current with new publications and tool comparisons.
- Every wiki page must comply with **Documentation Traceability** (below). Pages that describe behavior without a source-of-truth anchor are treated as orphaned and slated for removal.

### Documentation Traceability

**Rule**: every documentation page or section that describes a feature, flag, endpoint, file format, syntax form, or behavior must be traceable to a *live* code artifact (Rust `struct` / `enum` / `fn`, clap arg, axum handler, TypeScript client, or a checked-in configuration file) that is reachable from at least one of mununu's three user-facing surfaces — **CLI**, **HTTP API**, or **UI**. Documentation that cannot point at such an artifact is either obsolete, aspirational, or unverifiable, and must be marked or removed.

**Why**: doc rot is the single most common reason users distrust the tool. Anchoring docs to symbols (not prose) lets reviewers grep for drift the moment code is renamed, removed, or made unreachable from any surface. It also forces feature work to ship docs alongside code, instead of after.

**Where this applies**: `wiki/**`, `docs/**` (except `docs/architecture/` design notes and `docs/witness_refinement_plan.md`-style planning docs, which must be tagged `> Status: planning` at the top), `README.md`, `examples/**/README.md`, and the `description:` / inline guidance inside `.claude/agents/**` and `.claude/skills/**`.

**Anchor format**: every H2 / H3 section that documents a concrete capability must carry a *Source of truth* line near the top of the section, formatted as:

```markdown
> Source of truth: [`<symbol or path>`](<relative path>#L<line>) — surface: CLI | API | UI | (CLI+API+UI)
```

The `<relative path>` must be a real file in this workspace (`crates/...`, `mununu-ui/...`, `tools/...`, etc.). Line numbers are optional but recommended when pointing at a specific item inside a long file. The surface tag declares which of the three surfaces actually exposes the documented behavior:

- **CLI**: a clap subcommand or flag in `crates/mununu-cli/src/main.rs`
- **API**: a handler in `crates/mununu-core/src/api/handlers.rs` (route in `server.rs`, types in `models.rs`)
- **UI**: a typed client in `mununu-ui/src/api/endpoints.ts` or a hook/component that exercises the behavior

A section may legitimately tag a single surface (e.g., a CLI-only developer convenience) but must justify the single-surface choice in the same line: `surface: CLI-only — developer convenience that decomposes into /import + /eval on the API`.

**What this does NOT cover**:

- Pedagogical "concept" sections that explain *what* mu-calculus is, *what* a Kripke structure is, etc. — these anchor to a textbook reference, not to code. Tag them `> Concept: <one-line>` so the docs-traceability skill skips them.
- Planning, vision, or future-work documents — tag them `> Status: planning` at the top of the file.
- Tutorial prose that walks the reader through running an example — these anchor to the example file path (which itself must be reachable from all three surfaces under the existing parity rules).

**Acceptable forms of "alive"**:

- The symbol is referenced (not just declared) by code that ends up on a user-facing surface — `git grep` finds at least one call site that the CLI, API, or UI exercises.
- The configuration file is consumed by a binary or test target that ships (`tools/extraction_spec_schema.json`, `rust-toolchain.toml`, `docker/Dockerfile.dev`).

**Unacceptable**:

- Anchoring to a symbol that is `#[allow(dead_code)]`, only referenced by deleted tests, or behind a non-default feature flag that no surface enables.
- Anchoring to a file outside the repo, a closed-source dependency, or `mununu-private/`.

**Drift detection**: the `/docs-traceability` skill (run by `/review-orchestrator` and `/domain-adequacy`) walks every Markdown file under the covered paths, parses the `> Source of truth:` lines, and verifies each anchor (a) resolves to a real path, (b) the symbol exists at the cited line (when a line is given), and (c) the surface tag matches at least one reachable mention on that surface. Broken anchors fail the check. New sections without an anchor fail the check unless tagged `> Concept:` or `> Status: planning`.

**When code is renamed or removed**: the same commit must update every documentation anchor that points at the old symbol. The `/parity-check` and `/docs-traceability` skills are complementary — `/parity-check` confirms a *feature* is wired across surfaces; `/docs-traceability` confirms a *doc page* still points at code that exists. Both must pass for a change to ship.

### Soundness Guarantees

Mununu is a model checker that exhaustively verifies finite-state models. Its verdicts are correct for the model. Whether they transfer to the real system depends on the abstraction:

- **Safety properties + over-approximation → SOUND.** If the model says safe, the real system is safe (within the modeled scope). Over-approximation admits all real behaviors plus possibly more — extra behaviors can only add violations, not hide them.
- **Liveness properties + over-approximation → UNSOUND.** The model may show spurious progress from noop loops, havoc branches, or async interleaving without fairness.
- **Safety properties + under-approximation → UNSOUND.** The model may miss violations from behaviors it doesn't capture (e.g., skipped constructs, guard eval returning None).

When contributing adapters or modifying the Kripke builder:
- Document every `eval_expr → None` choice as over-approx or under-approx using `// SOUNDNESS:` comments
- Never mix directions within a single model without documenting it
- Add a soundness regression test for any new abstraction decision

Strategy extraction uses **signature-based selection** from iteration ranks:
- The winning region / realizability verdict is always correct for ALL mu-calculus formulas (any alternation depth)
- Counterstrategies are also positional — both players have memoryless winning strategies (positional determinacy of parity games, Zielonka 1998)
- Memoryless on the model-checking product = finite-memory on the plant. The memory is the iteration-rank signature from fixpoint evaluation.

### Black-Box Modules and Contracts

Mununu's contract subsystem makes black-box components first-class: peripheral RTL with no body, vendor IP behind an interface, firmware calling out to an unverified library. The framework spans Documents A (foundations), B (RTL frontend unification), D (corpus + annotations), and C (HW/SW codesign). The shipped surface is in [`crates/mununu-core/src/contract/`](crates/mununu-core/src/contract/), [`crates/mununu-core/src/corpus/`](crates/mununu-core/src/corpus/), [`crates/mununu-core/src/codesign/`](crates/mununu-core/src/codesign/), and [`crates/mununu-core/src/mununu_annotations/`](crates/mununu-core/src/mununu_annotations/).

**The chaotic-stub default is sound by construction, but never silent.** When an adapter encounters a black-box module it has no body for (yosys `(* blackbox *)`, custom-SV instantiation without a sidecar entry, software call falling through to `CallEffect::Unknown`), it produces a *chaotic stub*: every behaviour the interface admits, at every timing. Pessimism is mandatory — under-approximation is unsound for safety. The user must never accept the default *silently*; every black-box discovery emits a structured `tracing::warn!` diagnostic naming the module, the gap kind, the labels affected, and the soundness consequence. `--strict-contracts` promotes those warnings to hard errors for safety-critical CI.

**The discharge graph catches circular reasoning automatically.** When the user authors assume/guarantee clauses across components, the realiser runs Tarjan SCC over the `guarantor → consumer` graph at [`contract/discharge.rs`](crates/mununu-core/src/contract/discharge.rs). Acyclic discharge = standard Pnueli 1985 linear discharge. Cyclic = mununu attempts a lightweight mu-rank witness check (Document A §3.x); if no witness, the cycle is reported and verification refuses to silently proceed without explicit user approval. No PR may introduce a circular discharge that lacks either a rank witness or an explicit user sign-off.

**Annotations are the in-source contract anchors.** The `@mununu_*` tag vocabulary (`@mununu_blackbox`, `@mununu_assume`, `@mununu_guarantee`, `@mununu_interface`, `@mununu_controllable`, `@mununu_uncontrollable`) appears on the source side of the boundary — Verilog attributes `(* mununu_xxx = "…" *)` for SV today, with Doxygen-style `/** @mununu_xxx ... */` reserved for C/TS/Rust/Python under Task C5. The grammar parser at [`mununu_annotations/mod.rs`](crates/mununu-core/src/mununu_annotations/mod.rs) strips the wrapper and feeds the inner tag + value to a single downstream consumer. The discovery pipeline lifts annotations into `BlackBoxInterface.annotations[]` and the HITL review surface turns them into `ProposedClause` objects the user accepts / edits / rejects.

**The corpus replaces hand-authored stubs with vetted contracts.** A `@mununu_interface = "contract://rtl_protocol/axi4_slave@2.0.1?alt=strict"` annotation resolves against the corpus at [`corpus/`](corpus/) during discovery; a hit downgrades the default `OutputSequencing` gap to `LatencyBound` (the corpus entry implies the missing progress assumption) and tags the discharged clauses with `provenance: Corpus { id }`. Misses are surfaced as structured diagnostics — the user must either author a local entry or accept the chaotic-stub default. Contract entries follow Document D §D.2's schema; the corpus is file-backed today, with SQLite + remote service tiers reserved for follow-up.

**HW/SW codesign uses a register-map sidecar as the coupling spec.** [`crates/mununu-core/src/codesign/register_map.rs`](crates/mununu-core/src/codesign/register_map.rs) defines the JSON schema (mirrored at [`tools/register_map_schema.json`](tools/register_map_schema.json)). Each register-field bit maps to both an SV signal path and a C accessor expression. `mununu codesign couple` emits the CTXDSL coupling fragment (alphabet of rendezvous labels + chaotic peripheral stub + asynchronous composition block) the user splices into hand-authored firmware. `mununu codesign verify` does the splice automatically, realises the composition, and evaluates a named formula. Asynchronous composition is the only sound choice (Doc C §C.5 — bus arbitration is non-deterministic; synchronous one-step rendezvous is unsound for racy access).

**Three-surface parity is enforced** for every new contract / codesign capability. CLI subcommand + HTTP route + UI panel must all ship in the same PR — the `parity-check` skill flags drift. Today: `mununu contract validate|gaps|discover|sidecars|query|review`, `mununu codesign couple|verify`. HTTP routes mirror at `/api/v1/contract/*` and `/api/v1/codesign/*`. UI: `ContractPanel` (Validate / Discover / Query / Review sub-tabs) and `CodesignPanel` in `mununu-ui`.

**Sidecars are co-located, not centralised.** Adapter-detected black-boxes auto-emit `<module>.interface.json` + `<module>.gap_report.json` sidecars next to the source they describe. `.contract.todo.json` skeletons are written next to source when discovery produces gap markers. The originally-proposed `.mununu/` central project directory (Doc D §D.3) was scoped down post-M3 — co-located sidecars proved more discoverable than a central tree, and the only piece of the central tree that survived is the `.mununu/coupling/register_maps/` slot for Doc C's register-map sidecars.

**Soundness rules specific to codesign** (Doc C §C.5):
- Bus arbitration is non-deterministic → asynchronous composition is mandatory; synchronous coupling is unsound for racy access.
- Interrupt latency is a top-level environment assumption → treat as an unguaranteed clause at the top of the discharge graph; HITL must approve.
- Embedded C memory model defaults to sequential consistency with an explicit warning → under-approximation alternatives must be opt-in and labelled.

**Pointers to the design documents** (which carry the precedent review, the implementation plans, and the open questions):

- [`docs/design/black-box-modules.md`](docs/design/black-box-modules.md) — Document A.
- [`docs/design/rtl-frontend-unification.md`](docs/design/rtl-frontend-unification.md) — Document B.
- [`docs/design/contract-corpus-and-config.md`](docs/design/contract-corpus-and-config.md) — Document D.
- [`docs/design/hw-sw-codesign-extraction.md`](docs/design/hw-sw-codesign-extraction.md) — Document C.

### Strategy Extraction & Synthesis Modes

Three `ControllerMode` options:
- **Projection** (default): keeps ALL transitions between winning states. Not a strategy — just the winning region as a sub-CLTS.
- **Functional** (`--extract-strategy`): picks ONE controllable transition per state — the one whose target has the lexicographically smallest signature (best mu-progress). Deterministic, correct for all formulas.
- **Permissive**: keeps ALL controllable transitions whose target signature is ≤ the source's. Maximally permissive supervisor (Ramadge-Wonham canonical). Nondeterministic, composable with other supervisors.

The **signature** of a state is its tuple of iteration ranks per fixpoint variable (outermost first). For mu-variables, smaller rank = closer to goal. The functional strategy picks the most progressive move; the permissive supervisor enables all non-regressive moves.
- **Lasso traces**: counterexample traces for liveness include lasso format `prefix -> (cycle)^ω` with transition labels (`prefix_labels`, `cycle_labels`). The cycle detection uses DFS in the losing region. The last `cycle_labels` entry is the closing edge back to `cycle[0]`.
- **Counterstrategy in synthesis response**: The `/context/synthesize` endpoint automatically returns a `counterstrategy` field (with Cytoscape graph elements) for unrealizable cases. The graph is filtered to states reachable from initials via kept transitions (post-strategy-extraction BFS).
- **Formula inversion**: do NOT negate fixpoint variable references inside the body. Keep variables positive — the dual fixpoint's changed starting point handles the semantics.
- **Nondeterminism and controllability (Skolem paradigm)**: The controller chooses WHICH label to trigger, but cannot choose WHICH outcome occurs when multiple transitions share the same label (nondeterminism). Nondeterministic outcomes are always adversarial — ALL must satisfy — regardless of whether the label is controllable or uncontrollable. Controllability only determines who TRIGGERS the label (controller vs environment), not the outcome.

### Adapter / Emitter Capability Use

The CLTS data model and CTXDSL grammar already express more than most adapters reach for. When writing or modifying an extractor, adapter, or emitter, prefer these primitives over re-encoding source-language features as state-name suffixes or parallel single-label edges:

- **Multi-label transitions**: a single CLTS edge carries a `SmallVec<[LabelId; 4]>` of labels — see `crates/mununu-core/src/clts/mod.rs:265`. CTXDSL syntax: `transition s -> t on label a, label b;` (parser at `crates/mununu-core/src/context_dsl/parser.rs:733`, AST at `crates/mununu-core/src/context_dsl/ast.rs:156`). Multi-labeled edges are *one* transition, not parallel transitions — use this when a source event carries several semantic tags (e.g. controllability class + signal name + payload kind).
- **Per-state predicates**: Kripke-style state labeling is supported via `state_variable_bitset` (`crates/mununu-core/src/clts/mod.rs:1173`) and `state_valuation` (`crates/mununu-core/src/clts/mod.rs:1178`). CTXDSL declaration: `predicates { predicate foo = state S1; }` (parser at `crates/mununu-core/src/context_dsl/parser.rs:485`). Use these instead of encoding state attributes into state names.
- **Per-state structured valuations**: hand-write display-only metadata directly on a state via `state S1 { valuations { signal_a = 1; phase = idle; } };` — keys are identifiers (reserved keywords are also accepted to mirror adapter signal names), values are integer literals or identifiers. Realize merges these on top of any side-channel `ContextDoc.state_valuations` from adapters and registers them via `Clts::with_valuation_for_state`. The CTXDSL emitter writes the same block back when `StateSpec.valuations` is set, so `IR → emit → parse → realize` round-trips. Use this for hand-authored examples that should display the same `{key=value}` lines under each state node as adapter-driven flows (BTOR2, SV-yosys).
- **Per-label controllability**: `LabelControllability { Controllable, Internal, Uncontrollable }` at `crates/mununu-core/src/clts/mod.rs:248`. Transition controllability is *derived* from labels — declare it once in the automaton's `controllable { ... }` / `internal { ... }` blocks (`crates/mununu-core/src/context_dsl/ast.rs:81`), do not fold it into label-name prefixes.
- **Rich modal guards**: `Guard { labels, current, next, control, max_steps }` at `crates/mununu-core/src/mu_calculus/mod.rs:323`. A single `[...]` or `<...>` modality can constrain *all five* axes — labels, current-state predicates (`req_cur` / `forb_cur`), next-state predicates (`req_next` / `forb_next`), controllability class (`ctrl = controllable | environment | all`), and step bound (`steps`). Parser keys at `crates/mununu-core/src/mu_calculus/parser.rs:420`. Syntax: `[(labels = {a}, req_next = {active}, ctrl = controllable)] φ`. Reach for `req_next` / `forb_next` whenever a property is naturally phrased "after this transition the system must be in a state where …" — no adapter currently exploits this and it is the most under-used primitive.

**Reference implementations** that already exercise the rich surface:
- Signal-state emit path with turn-aware `[(ctrl=Controllable)]` guards: `crates/mununu-core/src/adapter/emit.rs:587-872`.
- SystemVerilog Kripke valuations: `crates/mununu-core/src/adapter/systemverilog/kripke.rs:450`.

**Anti-pattern**: do not re-encode source features as state-name suffixes (e.g. `S1_req_high`) or as parallel single-label edges between the same source/target pair when a multi-label edge fits.

**Rule**: when adding or modifying an adapter or emitter, prefer these primitives. If a primitive is intentionally unused, leave a one-line comment explaining why (e.g. `// AIGER inputs are single-bit; multi-label has no semantic content here`). Reviewers and the `/soundness-check` skill flag silent under-use.

### Agentic Orchestration Models

Agentic AI orchestration is currently modeled in two ways:

1. **Native CTXDSL** under `examples/agentic/` — files like `mcp_auth.ctxdsl` and `handoff_protocol.ctxdsl` describe agent / supervisor / worker FSMs directly as automata + properties + controllers.
2. **XState JSON** via the existing XState adapter — files like `examples/agentic/support_pipeline.xstate.json` use the standard `__mununu` block to declare controllable/uncontrollable events and properties; the XState adapter handles parallel regions and translates them to a synchronous composition.

The property templates registry has an `agentic` domain (see `crates/mununu-core/src/adapter/templates/`) that ships parameterized formulas (mutual-exclusion, no-livelock, bounded-handoff) usable from either entry point.

There is **no native CrewAI / LangGraph / A2A JSON parser** in the Rust workspace today. The Python scripts under `tools/` are the only path for live introspection of CrewAI/LangGraph/A2A Python objects; for JSON input you must either rewrite as XState or hand-author CTXDSL. Adding native parsers is a deliberate future-work item, not a shipped feature.

### Extraction Spec Adapter

The extraction adapter (`src/adapter/extraction/`) translates extraction spec JSON files (`.espec.json`) into CTXDSL. These specs are produced by the extraction pipeline for analyzing real source code:

```
Source code → human extraction → JSON spec (.espec.json) → Rust adapter → CTXDSL
```

**Key features:**
- **Mode filtering**: Each transition can be tagged `"mode": "vulnerable"`, `"mode": "fixed"`, or `"mode": "both"` (default). The `--mode` CLI flag selects which transitions to include.
- **Declarative automata**: States, transitions, compositions, properties, and controllers are all declared in the JSON spec — no per-target code in the adapter.
- **Provenance tracking**: Source commit, file, line numbers, CVE references, and attack chains are preserved from the extraction spec through to CTXDSL comments.
- **Detection**: Content-based (`"extraction_spec_v1"` + `"model_config"`) and extension-based (`.espec.json`).

**Usage:**
```bash
mununu context eval spec.espec.json --mode vulnerable --formula safety --automaton Main
mununu context eval spec.espec.json --adapter extraction --mode fixed --formula safety --automaton Main
```

**Spec format**: See `tools/extraction_specs/*.espec.json` in the mununu-private repo for examples. The `model_config` section carries declarative automaton definitions with `states`, `transitions` (mode-filtered), `composition`, `properties` (with per-property `over` targets), and `controllers`.

**Property templates**: Properties in `.espec.json`, `.mununu.json`, and XState `__mununu` blocks can use `template_ref` instead of raw `formula` to reference a named property template. Templates are parameterized mu-calculus patterns (e.g., `no_deadlock`, `reachable(TARGET)`, `bounded(OVERFLOW, UNDERFLOW)`). Resolution happens at adapter translation time — the emitter and evaluator see no difference. See `crates/mununu-core/src/adapter/templates/` for the registry and catalog.

### Game Engine Adapters (Godot)

The extraction adapter supports game state machine verification through `.espec.json` specs and GDScript source extraction:

- **GDScript extraction**: Tree-sitter grammar (`tree-sitter-gdscript` v6) extracts enum-based state machines from `.gd` files. The `game_fsm` domain profile in `src/adapter/extraction/ast_extract/domain.rs` configures controllability (player input = uncontrollable, system actions = controllable), asynchronous composition, and `ev_` label prefix.
- **Game examples**: `examples/game/` contains `.espec.json` files demonstrating softlock detection, quest deadlocks, and NPC AI loops — all using property templates.
- **CLI**: `mununu context eval game.espec.json --adapter extraction --template no_deadlock --automaton FSM`
- **CLI templates**: `mununu templates --domain game` lists templates relevant to game verification.
- **UI workflow**: The `gameengine` workflow in `mununu-ui/src/types/workflow.ts` provides a 5-step pipeline: Load → Extract → Edit Spec → Translate → Verify.

### TLSF/AIGER Adapter Encoding (Turn-Based Compound Labels)

The TLSF and AIGER adapters use a **turn-based compound-label encoding** in `src/adapter/emit.rs`:

- **Compound labels**: `env_{bits}` (uncontrollable, one per input assignment) and `ctrl_{bits}` (controllable, one per output assignment) replace the old per-signal `set_`/`clr_` labels.
- **Turn bit**: States include a turn bit (LSB). `turn=0` = env's turn (round boundary), `turn=1` = ctrl's turn (intermediate). State count is `2^(N+1)` where N = inputs + outputs.
- **Turn-based routing**: From `turn=0` states, only env transitions exist (→ `turn=1`). From `turn=1` states, only ctrl transitions exist (→ `turn=0`). This ensures the evaluator's Skolem paradigm naturally alternates ∀ env / ∃ ctrl.
- **Game-aware formulas**: Emitted as mu-calculus (not LTL) with `[(ctrl=Controllable)]` modals. Propositional checks use `(turn || φ)` to skip at intermediate states. This is critical — without it, formulas would be checked at intermediate states where the controller hasn't responded yet.
- **LTL-to-mu-calculus translation in the emitter** (`ltl_to_game_mu_inner`): Bypasses the standard LTL translator (which uses `[ctrl=All]`) and emits mu-calculus directly with turn guards. Key patterns:
  - `G φ` → `ν X. ((turn || φ) ∧ [c] X)` — skip check at ctrl-turn
  - `F φ` → `μ X. ((!turn ∧ φ) ∨ [c] X)` — only count at env-turn (round boundary)
  - `X φ` → `[c] [c] φ` — two steps = one round (turn alternates each step)
  - Where `[c]` = `[(ctrl=Controllable)]`
- **SYNTCOMP verdict differences**: lilydemo15 and lilydemo16 are realizable under our Mealy encoding (valid alternating-grant strategy exists) but SYNTCOMP reference says unrealizable. This is a semantic difference, not a bug.

### Docker Best Practices

When writing or reviewing Dockerfiles for this project (Rust binary service):

- **Multi-stage builds**: Use a full `rust` builder image to compile, then copy only the binary into a slim runtime image (`debian:bookworm-slim` or `gcr.io/distroless/cc`). CI tools stay in the build stage; the production image stays clean.
- **Pin exact tags**: Never use `FROM ubuntu:latest` or `FROM rust:latest`. Use `FROM rust:1.82-slim-bookworm` — builds must stay reproducible. The tutorial Dockerfile uses `ubuntu:24.04`; pin it to a digest or full minor tag if you update it.
- **Combine RUN commands**: Chain with `&&` and clean up in the same layer: `RUN apt-get update && apt-get install -y --no-install-recommends curl && rm -rf /var/lib/apt/lists/*`.
- **Always use `.dockerignore`**: Exclude `target/`, `.git/`, `.env`, `*.key`, `wiki/`, `tutorial/` from the build context to avoid leaking secrets and slow builds.
- **Never run as root**: Add `RUN useradd -m appuser && chown -R appuser /app` and `USER appuser` before the entrypoint.
- **Order layers by change frequency**: Copy `Cargo.toml` and `Cargo.lock` first and run a dummy build to cache dependencies, then copy `src/`. Cache only busts when dependencies change, not on every source edit.
- **Add a HEALTHCHECK**: `HEALTHCHECK --interval=30s --timeout=5s CMD curl -f http://localhost:PORT/health || exit 1` — Kubernetes/ECS uses this to gate traffic.
- **Use ARG vs ENV correctly**: `ARG` for build-time values (versions, feature flags). `ENV` is baked into the image and visible in `docker inspect` — never put secrets in `ENV`; use runtime secret injection instead.

### Claims Integrity — Public Materials

**Every public claim about mununu's ability to find bugs, verify properties, or improve security of external systems must be backed by reproducible evidence against real implementations.**

This applies to: README examples, wiki case studies, blog posts linked from the repo, conference papers, and any material that references real-world systems (MCP servers, protocol implementations, hardware designs).

#### Rules

1. **Models from source, not documentation.** If a claim says "mununu found X in system Y," the CTXDSL model must be extracted from Y's actual source code via an auditable extraction spec (line-anchored JSON referencing exact commit + file + line numbers). Models written from API docs or design descriptions must be labeled as "design pattern demonstrations," never as findings about the real system.

2. **Planted bugs are demos, not findings.** A hand-written model with a deliberately introduced defect demonstrates the tool's verification capability. It does not demonstrate that the real system has the defect. Language must reflect this: "we demonstrate the property class" vs. "we found a bug."

3. **Severity honesty.** A missing guard that creates a theoretical race window in single-threaded Node.js is not the same as an exploitable RCE. Claims must distinguish:
   - Security vulnerability (data leak, privilege escalation, RCE)
   - Reliability/correctness issue (error during edge-case concurrency)
   - Structural gap (missing guard with no demonstrated impact)
   - Design pattern violation (deviation from spec, not necessarily a bug)

4. **Reproduction path required.** Every claimed finding must include either:
   - A test case or scenario that triggers the behavior in the real implementation, OR
   - An honest statement that the finding is structural (present in the state machine) but not yet reproduced against the running system

5. **Abstraction soundness.** When abstracting from a real implementation (e.g., collapsing a `Map<K,V>` to a 3-state enum, or bounding an integer counter to a small domain):
   - Every abstraction must be documented in the extraction spec with: what was abstracted, what was lost, and why it is sound for the properties being checked.
   - Over-approximation (model admits more behaviors than reality) is conservative for safety properties. Under-approximation (model admits fewer behaviors) is unsound — state this explicitly if it applies.
   - After verification, at least one concrete execution trace must exercise the abstracted path against the real implementation (test case, curl command, or script). If the model says a property fails, demonstrate the violation is reproducible on the real system — not an artifact of the abstraction. If the model says it holds, demonstrate a representative concrete case passes.
   - Abstractions that cannot be validated by a concrete scenario must be flagged and must not support public claims.

6. **Verification-first workflow.** When analyzing external codebases for vulnerabilities, the CTXDSL model and mununu verification is the **oracle** — not human reasoning about source code. The workflow is: extract → generate CTXDSL → run `mununu context eval` on the composition FIRST → then interpret the result. Do not pre-conclude whether a property holds or fails based on code reading. The tool explores all reachable states and finds traces humans miss. Build the model, run the check, let the tool speak. Only after the formal result, validate against the real implementation.

7. **Extraction pipeline for claims about real systems.** The pipeline is:
   ```
   Source code → Extraction spec (JSON) → validate_extraction_spec.py → spec_to_ctxdsl.py → mununu eval/synth
   ```
   The extraction spec is the auditable artifact. It must be committed alongside the claim. The spec validator must pass against the pinned commit.

8. **Editorial framing for publications (LinkedIn, Substack, blog posts, conference talks).** Long-form public content must lead with a realistic, impactful example — a concrete system, a concrete property, and a concrete consequence — and treat mununu features as secondary, introduced only where they directly serve the example.
   - **Lede is the example, not the tool.** The headline, opening paragraph, and pull quotes must describe the system and what could go wrong with it (a UART controller drops a byte under back-pressure; a payment FSM admits a double-spend interleaving; an MCP server leaks a session across tenants). They must not headline a mununu feature ("we shipped a new coupling-synthesis pass", "our register-map sidecar now supports …"). Feature launches belong in release notes or technical READMEs, not in public storytelling pieces.
   - **Features appear when they earn the right.** A capability (a new adapter, a new template, a new CLI flag, a new emit mode) may only be named when (a) it is what made the example tractable, or (b) it changes how the reader would reproduce the result. It is introduced inline, *after* the example has been set up, and tied back to a code anchor per Documentation Traceability rules.
   - **No feature catalogs.** A bulleted list of "what's new" without an example attached to each bullet is rejected. If the post genuinely needs to enumerate features, those features must be grouped under the example each one supported, not under the release.
   - **Impact must be concrete.** "Realistic and impactful" means the example references real systems with public source-of-truth (per Rule 1), or — for Class C demonstrations — clearly labels itself as a pattern study with named real-world analogs. Synthetic toy automata ("a 3-state safety property") do not qualify as the lede of a public post.
   - **One example per post, by default.** A LinkedIn or Substack post lives or dies on a single clear story. Multi-example posts are allowed only when the examples share a single, named thread (e.g., "three controller-synthesis examples in industrial UART variants").
   - **Applies to**: LinkedIn posts, Substack/Medium articles, conference talks, demo videos, podcast prep notes, and any agent-generated content suggestion that targets these channels. Does not apply to internal release notes, CHANGELOG entries, wiki reference pages, or API documentation, which may lead with the feature.

9. **RTL / SystemVerilog pipeline evidence integrity.** The same verification-first principles apply to the SV Kripke pipeline:
   - **Never present hand-written data as pipeline output.** If `discovered_values` in a `.mununu.json` sidecar were written by a human or AI agent (not by `mununu sv discover`), they must be disclosed as hand-written. Claims like "SMT discovers x=3" require actually running the discover command.
   - **Run the pipeline, don't simulate it.** Before presenting verification results in any public material, execute the actual commands and capture real terminal output. Do not fabricate or predict mununu output.
   - **Properties must come from specifications, not bug knowledge.** Adding a detector register to catch a known bug and then verifying it fires is circular. Properties should come from protocol specs, safety invariants, or security requirements.
   - **Distinguish syntactic from SMT-discovered values.** Literals found directly in `case` labels are syntactic. Values found through combinational logic inversion are SMT-discovered. Don't claim SMT discovery for trivially visible constants.
   - **Show counterexample traces.** When a property fails, capture the violating state/transition trace, not just "unrealizable."
   - **Validate the trace under simulation.** Whenever an RTL diagnostic (counterexample or counterstrategy from `mununu context synth --counterexample`) is used to support a public claim, the trace must be reproduced against the close-to-source SystemVerilog under Verilator, in the sibling `hw-verif:latest` Docker image. The reproduction lives in `staging/{target_id}/repro/` with a `.sv` per case-modifier variant relevant to the bug, a Verilator C++ testbench that drives the trace inputs (using `force` for fault hypotheses), and a `Makefile` with a `sim` verb. The simulation transcript is the evidence; a model-only lasso/counterexample is labeled "LTS witness only — not reproduced in simulation" and downgrades the rigor by one level. See the `target-executor` agent's Phase 3.5 for the procedure and `.claude/reviews/prospector/staging/RTL-002/repro/` for the canonical pattern.
   - These rules apply to both human and AI-agent authored content.

10. **C extractor claims — hand-authored IR fixtures vs end-to-end real-clang extraction.** The codesign C extractor at [`crates/mununu-core/src/codesign/c_extract_llvm.rs`](crates/mununu-core/src/codesign/c_extract_llvm.rs) has two kinds of validation: unit tests on hand-authored IR text fragments, and end-to-end runs against real C source compiled by real clang. These are **different claims** and must be reported as such.
   - **A unit test that constructs an IR fragment in a Rust string literal demonstrates the matcher's behaviour on that IR shape.** It does NOT demonstrate that clang produces that IR shape for the corresponding C source — the test author wrote the IR by hand based on what they *believed* clang would emit. Treat hand-authored IR fixtures as documenting the extractor's *behaviour on a given input*, never as evidence that the extractor handles a particular C idiom end-to-end.
   - **The claim "the extractor handles `<C-idiom>`" requires a real `.c` file** committed under `examples/industrial/codesign_uart/motivating_examples/` (or similar) that (a) compiles via `clang -O0 -emit-llvm -S`, (b) is consumed by `mununu codesign extract-c`, and (c) produces the claimed access structure. A `validate-motivating-examples.sh`-style script that exercises every claimed example end-to-end is the canonical evidence. PRs that introduce a new extractor capability MUST add the corresponding real example or honestly downgrade the claim to "the matcher handles this IR shape; the corresponding clang-emitted IR has not been validated."
   - **Honest gap statements are required.** Phase L5's "interprocedural call-graph walk" closes Example 4 only for callees that touch globals directly. Pointer-aliased callees (alloca-store-load round-trip) are queued as L5.5 — that gap must be stated, not glossed over. The Doc C correctness-scope note at [`docs/design/c-extraction-correctness-scope.md`](docs/design/c-extraction-correctness-scope.md) is the canonical home for these statements.
   - **Hand-authored IR fixtures stay valuable** for fast unit tests of single regex shapes / single matcher branches. They must just be labelled in the test name or doc-comment as "synthetic IR" and not cited in PR descriptions or publications as evidence of end-to-end handling of any C idiom.

#### What this does NOT restrict

- Tutorial and pedagogical examples can use hand-written CTXDSL freely.
- Benchmark models (SYNTCOMP, protocol verification) follow their own established methodology.
- The adapter test suites use synthetic inputs by design.
- Academic papers may present idealized models if clearly labeled as such in the methodology section.
- Example `.mununu.json` sidecars with hand-written `discovered_values` are acceptable for tutorials IF disclosed as hand-written in the accompanying documentation.

### Security (OWASP)

- Never interpolate user input into commands or templates.
- Validate and constrain all external input.
- No sensitive data in logs.

### Git Operations & Destructive Commands

**Destructive git operations require explicit user instruction in the current session prompt.** This applies to:

- `git reset --hard` (discards working tree changes — irrecoverable from reflog)
- `git push --force` / `git push -f` / `git push --force-with-lease` (overwrites remote history)
- `git checkout -- <paths>` / `git restore <paths>` (discards working tree changes for paths)
- `git clean -f` / `git clean -fd` (deletes untracked files)
- `git branch -D <branch>` (force-deletes a branch with unmerged commits)
- `git stash drop` / `git stash clear`
- `git rebase --interactive` followed by squash/drop of unpushed-but-staged work

**Why**: working-tree state is not recoverable from `git reflog` — only commits are. A `--hard` reset on a working tree carrying hours of uncommitted work permanently loses that work unless an editor's local history snapshot exists. (See incident: `.claude/plans/update-the-graph-endpoint-crystalline-frost.md` — a botched revert + `--hard` reset cost ~1730 lines of in-flight work that VSCode local history, Time Machine, and APFS snapshots could not recover.)

**How to apply**:
- Never run any of the above commands "to clean up" or "to reset state" unless the user has typed an instruction containing the specific command (e.g. "do a hard reset to main", "force-push the rebase").
- If a botched `git revert` or merge-conflict resolution is in progress, prefer `git revert --abort`, `git merge --abort`, `git cherry-pick --abort`, or `git reset --soft` (which preserves working tree).
- If the working tree must be discarded for a routine operation (e.g. switching branches), first run `git stash push -u -m "<reason>"` to preserve untracked files too. Surface the stash to the user.
- For pre-commit hook failures, *create a new commit* — never `--amend` and never `--no-verify` unless the user explicitly authorised it.
- Before any operation that may modify the working tree of files the user has been actively editing, snapshot via a "savepoint" branch + push to remote (`git checkout -b recovery/savepoint-<date> && git add -A && git commit --no-verify -m "savepoint" && git push -u origin <branch>`).

**Agents must propagate this rule.** Any subagent invocation that performs git operations on a real repo (not an `isolation: "worktree"` clone) inherits this restriction. Subagents may take destructive actions only inside `isolation: "worktree"` agents (the worktree is a throwaway clone).

## Private Files Policy

Sensitive or unpublished materials must live in the **sibling private repo**, not in this repo:

```
/Users/marianocerrutti/git_repo/mununu-private/
```

### What belongs where

| Goes in `mununu` (public) | Goes in `mununu-private` |
|---------------------------|--------------------------|
| Rust source code, adapters, CLI | Paper sources (LaTeX, BibTeX, figures) |
| Adapter formats: TLSF, AIGER, Promela, XState, SystemVerilog, extraction | Benchmark scripts and expected outputs (`artifact/`) |
| Extraction adapter (`src/adapter/extraction/`) | Extraction specs with CVE/vulnerability data (`tools/extraction_specs/`) |
| JSON Schema for `.espec.json` (`tools/extraction_spec_schema.json`) | Protocol CTXDSL models (`examples/protocols/`) |
| `mununu extraction validate` and `mununu extraction check` subcommands | MCP extracted CTXDSL files (`examples/agentic/mcp_extracted/`) |
| Shared IR, emitter, format detection | Governance policy scripts (`tools/validate_governance.sh`) |
| Unit and integration tests | Tutorial materials, slides, cheatsheets |
| Public documentation (README, wiki) | Internal evaluation data, drafts, notes |

**Extraction framework boundary rule**: Tool capabilities (adapter code, validation logic, provenance checking, JSON schema) belong in `mununu`. Private content (actual extraction specs referencing CVEs, generated CTXDSL containing vulnerability details, repo-specific CI policy) belongs in `mununu-private`.

### Rule

Before adding any file to `mununu/` that you would not want publicly visible, move it to `mununu-private/` instead. Then add the path to `mununu/.gitignore` as a safety net.

The `.gitignore` excludes `/paper/`, `/artifact/`, `/examples/syntcomp/`, `/examples/scalable/`, and `/tutorial/`. If you add a new sensitive directory, add it to `.gitignore` immediately.
