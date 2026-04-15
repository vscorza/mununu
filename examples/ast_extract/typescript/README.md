# TypeScript Extraction Example

Demonstrates extracting a state machine from a TypeScript class and verifying a safety property.

## Files

- `sample_server.ts` — TypeScript class with lifecycle state machine (3 boolean fields, 4 methods)
- `sample_server.extract.json` — extraction config targeting the Server class

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
