# Python Extraction Example

Demonstrates extracting a state machine from a Python class.

## Run

```bash
mununu-extract sample_handler.extract.json \
  --source sample_handler.py \
  --output /tmp/handler.espec.json

mununu context eval /tmp/handler.espec.json \
  --formula no_requests_when_rate_limited \
  --automaton HandlerFSM
```

## Expected Result

The property **FAILS** — `handle_request` fires when `_rate_limited=True` because it has no guard.
