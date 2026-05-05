# TypeScript Extraction Example

Demonstrates extracting a state machine from a TypeScript class and verifying a safety property.

## Files

- `sample_server.ts` — TypeScript class with lifecycle state machine (3 boolean fields, 4 methods)
- `sample_server.extract.json` — extraction config targeting the Server class
- `compound_guard.ts` / `compound_guard.extract.json` — multi-condition guard extraction
- `indirect_guard.ts` / `indirect_guard.extract.json` — getter-method guard extraction
- `parallel_workers.ts` — minimal `Worker` class used for the **compositional-extraction** demo
- `parallel_workers_compositional.extract.json` — extract config with `composition.instances[]` (2 worker instances) + `composition.resources[]` (a hand-modelled shared file) + `no_clobber` / `clobber_reachable` properties
- `parallel_workers_compositional.expected.txt` — reference verdicts for the compositional run (smoke-test against this)

## Run

```bash
# Step 1: Extract (produces .espec.json)
mununu-extract sample_server.extract.json \
  --source sample_server.ts \
  --output /tmp/server.espec.json

# Step 2: Verify
mununu context eval /tmp/server.espec.json \
  --formula no_requests_after_close \
  --automaton ServerLifecycle
```

## Expected Result

The property **FAILS** — `handleRequest` fires in closed states because it does not check `_closed`. This is the same vulnerability pattern as the MCP SDK's F5 finding.

The extractor derives 8 states (2³ from 3 boolean fields) and ~40 transitions. The guard inversion heuristic correctly detects that `if (this._started) { throw }` means "started must be false for the method to proceed."

## Compositional Run (parallel_workers)

```bash
# Step 1: extract — produces 3 automata (worker_a, worker_b, shared_file)
mununu-extract ast parallel_workers_compositional.extract.json \
  --source parallel_workers.ts \
  --output /tmp/parallel_workers.espec.json

# Step 2: verify — race detected, witness valid
mununu context eval /tmp/parallel_workers.espec.json \
  --formula no_clobber --automaton two_writer_race      # → 0/9 (fails — race)
mununu context eval /tmp/parallel_workers.espec.json \
  --formula clobber_reachable --automaton two_writer_race  # → 9/9 (witness valid)
```

The full walkthrough — schema, semantics, troubleshooting, and how to read the verdicts — lives in the [Compositional Extraction Tutorial](../../../wiki/Compositional-Extraction-Tutorial.md) wiki page.
