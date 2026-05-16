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
