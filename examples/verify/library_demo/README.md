# `library_demo` — parameterised CTXDSL library templates demo

> **Source of truth:** [`crates/mununu-core/src/library.rs`](../../../crates/mununu-core/src/library.rs) and [`crates/mununu-core/library/`](../../../crates/mununu-core/library/) — surface: CLI+API+UI.

Demonstrates plan Part 6 item 7: the shipped library of parameterised CTXDSL component templates. Two templates (`plic`, `watchdog`) instantiated with `count = N` produce five independent automata composed into a single 72-state verification system, from two source files.

## What it demonstrates

- **`mununu library list`** — enumerates the shipped library templates with one-line summaries.
- **`mununu library emit <name> --instance-id <id>`** — emits a template with the `{instance_id}` placeholder substituted (useful as a copy-into-project workflow).
- **`count = N` over a library template** — the verify orchestrator substitutes `{instance_id}` per instance, producing N independent automata from one source file. Plan Part 6 items 6 + 7 compose cleanly.
- **State-space scaling** — 3 PLIC instances × 2 states each = 8 PLIC cross-products. 2 watchdog instances × 3 states each = 9 watchdog cross-products. Combined: 8 × 9 = 72 reachable composed states. All five reachability properties hold.

## Library templates shipped today

| Name | Summary |
|---|---|
| `plic` | RISC-V PLIC interrupt-controller stub (one tracked source × one observer). 2 states. |
| `watchdog` | Watchdog timer (Disabled / Armed / Expired) with kick + clear + expire labels. 3 states. |
| `tracked_memory` | Single-address memory tracker (Initial / Written) with wr / rd / fence labels. 2 states. |

MESI cache is a deliberate omission for v1 of the library: cross-instance peer-snooping needs richer label resolution than `{instance_id}` substitution alone provides. Queued for a follow-up that pairs the cache template with a `cache_coherence` binding strategy (plan Part 6 item 8).

## Files

| File | Purpose |
|---|---|
| `plic.ctxdsl.tpl` | Local copy of `crates/mununu-core/library/plic.ctxdsl.tpl` — the shipped PLIC template |
| `watchdog.ctxdsl.tpl` | Local copy of the shipped watchdog template |
| `verify.toml` | 2 source declarations (plic with `count = 3`, watchdog with `count = 2`) + composition + 5 properties |
| `validate.sh` | End-to-end reproduction script — also exercises `mununu library list` |
| `transcript.txt` | Byte-deterministic expected output |

## Reproduce

```bash
bash examples/verify/library_demo/validate.sh
```

## Authoring workflow

Recommended path for a new verification project:

```bash
# 1. List what's available.
mununu library list

# 2. Copy one template into your project (no substitution; the verify
#    framework substitutes per instance via count = N).
mununu library emit plic > my_project/plic.ctxdsl.tpl

# 3. Declare the source in verify.toml with the desired count.
[[sources]]
id = "my_irq"
adapter = "ctxdsl"
files = ["plic.ctxdsl.tpl"]
count = 4

# 4. Reference each instance from [composition].members or via `<id>.*`.
[composition]
members = ["my_irq_0", "my_irq_1", "my_irq_2", "my_irq_3"]

# 5. Run mununu verify; emit properties against PLIC_my_irq_<i>_Pending etc.
```

The templates are CTXDSL all the way down — the user can fork a template by copying it into their project and editing, or treat the shipped version as canonical.
