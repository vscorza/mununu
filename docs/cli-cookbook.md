# CLI Cookbook

> Concept: common `mununu` invocations grouped by task. The CLI source of truth is `crates/mununu-cli/src/main.rs`; this file is a quick-reference, not a flag manual.

## Mu-calculus evaluation

```bash
mununu context eval context.ctxdsl --formula property_name --automaton automaton_name
```

## Controller synthesis

```bash
mununu context synth context.ctxdsl --formula property_name --automaton automaton_name
```

## Merge context files

```bash
mununu context merge main.ctxdsl sidecar.ctxdsl --output build/
```

## Graph visualization

```bash
mununu context graph context.ctxdsl --output graph.html
```

## Import from external formats

```bash
# XState → synth
mununu context synth machine.xstate \
  --adapter xstate \
  --formula safety \
  --automaton Machine

# SystemVerilog → eval
mununu context eval design.sv \
  --adapter sv \
  --formula safety \
  --automaton FSM
```

## Export controllers to native formats

```bash
# XState
mununu context synth machine.xstate --adapter xstate --formula safety --automaton Machine \
  --output-format xstate --emit-native controller.json

# SystemVerilog
mununu context synth design.sv --adapter sv --formula safety --automaton FSM \
  --output-format systemverilog --emit-native controller.sv
```

## Extraction-spec adapter

```bash
mununu context eval spec.espec.json --mode vulnerable --formula safety --automaton Main
mununu context eval spec.espec.json --adapter extraction --mode fixed --formula safety --automaton Main
```

See [`adapters/extraction.md`](adapters/extraction.md) for mode filtering, property templates, and provenance.

## Templates

```bash
mununu templates --domain agentic     # list agentic-orchestration templates
```

## Agentic orchestration models

Agentic models live in `examples/agentic/` either as native CTXDSL or as XState JSON imported via the XState adapter. See [`adapters/agentic.md`](adapters/agentic.md) and `examples/agentic/README.md`.

## Selecting the predicate source for BTOR2 CEGAR

> Source of truth: [`adapter/btor2/cegar.rs::PredicateSource`](../crates/mununu-core/src/adapter/btor2/cegar.rs#L87) — surface: CLI

The R.5 CEGAR refinement loop accepts three predicate-discovery strategies for the `mununu btor2 cegar` subcommand:

| Value | Mechanism | External dep |
|---|---|---|
| `manual` | User-supplied closure (programmatic use only — not selectable from CLI) | none |
| `wp` (default) | Weakest-precondition heuristic — emits separating predicates over state registers reachable from the failure subgame's classifying transitions that are not yet in the predicate set | none |
| `craig` | Craig interpolation via CVC5 subprocess — for each classifying may-but-not-must transition, asks CVC5 for an interpolant separating source from target cube | **CVC5 ≥ 1.0** ([install instructions](external-tools.md#cvc5)) |

```bash
# Default WP — works out of the box, no external deps:
mununu btor2 cegar --file design.btor2 --predicate-source wp

# Craig interpolation via CVC5 (after `brew install cvc5` / `apt install cvc5`):
mununu btor2 cegar --file design.btor2 --predicate-source craig

# Explicit CVC5 binary path (override the discovery default):
MUNUNU_CVC5_PATH=/opt/cvc5/bin/cvc5 mununu btor2 cegar --file design.btor2 --predicate-source craig
```

When `--predicate-source craig` is selected but CVC5 isn't found at runtime, mununu emits a structured warning + falls back to the WP heuristic automatically — the verification verdict is still computed; only the predicate-discovery mechanism is degraded. See [`docs/external-tools.md`](external-tools.md#cvc5) for full install instructions on macOS, Debian/Ubuntu, and the cross-platform GitHub release tarball.

## Preflight a SystemVerilog design for unfaithful partial-write registers

> Source of truth: [`sv_lint_registers`](../crates/mununu-core/src/adapter/sv_verify.rs#L228) — surface: (CLI+API+UI)

`mununu sv lint` lifts a SystemVerilog design and reports — at CI time (~lift cost, **no** model checking, ~0.1 s) — every register whose partial-write lift the verifier cannot keep faithfully (a plain-vector `q[hi:lo] <= d` whose unwritten bits the front end models as free inputs). These are exactly the registers a state predicate would be *refused* on by the formal gate, surfaced *before* the minutes-long verify. It changes no verdict.

```bash
# Report the unfaithful partial-write registers; exit 2 if any (a CI gate).
mununu --quiet sv lint rtl/mod.sv --frontend slang

# Advisory (report-only, always exit 0):
mununu --quiet sv lint rtl/mod.sv --frontend slang --fail-on none
```

The JSON on stdout lists each flagged `signal` with a `kind` of `"register"` (the root — the register itself) or `"output"` (a downstream combinational output of one). See [`docs/verifying-rtl.md`](verifying-rtl.md) for the soundness background (the monono#partsel guard).
