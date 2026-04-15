# Rust Extraction Example

Demonstrates extracting a state machine from a Rust struct + impl block.

## Run

```bash
mununu-extract sample_protocol.extract.json \
  --source sample_protocol.rs \
  --output /tmp/conn.espec.json

mununu context eval /tmp/conn.espec.json \
  --formula no_send_after_close \
  --automaton ConnectionFSM
```

## Expected Result

The property **FAILS** — `send` fires when `closed=true` because it has no guard.

## Note on Rust Extraction

For Rust, the extractor finds fields in the `struct` definition and methods in the `impl` block separately (they're different AST nodes in tree-sitter-rust). Both are matched by the type name specified in `targets[].class`.
