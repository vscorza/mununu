# CI and Agent Integration

> **Source of truth:** [`crates/mununu-cli/src/main.rs`](https://github.com/vscorza/mununu/blob/main/crates/mununu-cli/src/main.rs) (CI-gate exit codes) + [`crates/mununu-core/src/api/server.rs`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/api/server.rs) (HTTP routes) — surface: CLI+API.

This page covers driving mununu from **CI** (GitHub Actions and friends, via the CLI's exit codes) and from an **external agent** that writes RTL (via the HTTP API). For the property vocabulary itself — safety / response-liveness / recoverability and the no-sidecar `sv verify-auto` — see [`docs/verifying-rtl.md`](https://github.com/vscorza/mununu/blob/main/docs/verifying-rtl.md).

Every verdict is one of `holds` / `violated` / `unknown` / `skipped`; a definite `holds` / `violated` is **sound**, `unknown` / `skipped` are honest abstentions.

---

## 1. In CI: the verify verbs are gates

> **Source of truth:** [`FailOn` / `ci_exit_code` / `worst_verdict`](https://github.com/vscorza/mununu/blob/main/crates/mununu-cli/src/main.rs) — surface: CLI.

Every verify verb — `btor2 verify` / `verify-liveness` / `verify-recoverability`, the SV-direct `sv verify` / `verify-liveness` / `verify-recoverability`, and `sv verify-auto` — maps its verdict to a **process exit code**, so a workflow step fails on a real violation with no JSON parsing.

| Exit code | Meaning |
|-----------|---------|
| `0` | property holds (or all properties hold) — the step passes |
| `2` | a property is **violated** — the step fails |
| `3` | a property is **unknown** *and* `--fail-on unknown` was passed |
| `1` | tool / usage error (missing file, unparseable atom, missing toolchain) |

- **`--fail-on <violated \| unknown \| none>`** picks the gate policy (default `violated`). An undecided `unknown` does **not** fail the build by default — it is "not decided," not "broken." Use `--fail-on unknown` for a strict gate that also fails on `⊥`, or `--fail-on none` for report-only (always exit `0`).
- **`--quiet`** (global) suppresses the `logs/mununu.log` workspace file and the startup banner — errors only to stderr, so the workspace stays clean and stdout is the JSON.

### A GitHub Actions workflow

The SV toolchain (slang + sv2v + Yosys) is not bundled; run in the `mununu-sva` container that pins it.

```yaml
# .github/workflows/verify.yml
name: formal-verify
on: [push, pull_request]
jobs:
  recoverability:
    runs-on: ubuntu-latest
    container: ghcr.io/vscorza/mununu-sva:latest   # slang + sv2v + yosys + mununu
    steps:
      - uses: actions/checkout@v4
      # Fail the PR if the FSM can no longer get back to idle.
      - name: recoverability gate
        run: |
          mununu --quiet sv verify-recoverability rtl/fsm.sv \
            --preprocess-sv2v --target "state_q == 0"
      # Verify the module's own assertions; fail on any violation, allow ⊥.
      - name: assertion gate
        run: |
          mununu --quiet sv verify-auto rtl/fsm.sv --preprocess-sv2v --json \
            | tee verify-auto.json
```

`sv verify-auto` gates on the **worst** property verdict (`violated` > `unknown` > `holds`; a `skipped` property counts as pass). The `--json` stream carries the per-property detail for a summary or `jq` step.

---

## 2. From an agent that writes RTL: the HTTP API

> **Source of truth:** [`sv_verify_auto_handler`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/api/handlers.rs) + the `/api/v1/sv/*` routes — surface: API.

Start the server (built with `--features api`) on a host that has the SV toolchain:

```bash
mununu server --addr 0.0.0.0:8080
```

An RTL-writing agent has two request styles:

**Check the module's own assertions (no sidecar, no formula):**

```jsonc
POST /api/v1/sv/verify-auto
{ "source": "module m(...); ... assert property (...); endmodule",
  "use_sv2v": true,
  "must_edge_inference": "smt-hyper-must" }   // sound νμ verdicts; all fields optional
// → { "properties": [ { "name": "...", "outcome": "holds"|"violated"|"unknown"|"skipped" } ],
//     "unsupported": [ ... ], "notes": [ ... ] }
```

**Name any property — including the branching ones SVA cannot express — in one call:**

```jsonc
POST /api/v1/sv/verify-recoverability
{ "source": "module m(...); endmodule", "use_sv2v": true, "target": "state_q == 0" }
// → { "verdict": "holds"|"violated"|"unknown", "property": "AG EF (state_q == 0)" }
```

`POST /api/v1/sv/verify` (safety of the module's assertions) and `/api/v1/sv/verify-liveness` (`{request, grant}`) round out the trio. All return the canonical `verdict`, so the agent gates on `verdict === "violated"`.

The agent can skip the SV lift entirely and post pre-lifted BTOR2 to `/api/v1/btor2/verify{,-liveness,-recoverability}` — same responses.

---

## 3. What to plan for

- **Toolchain on the host.** `sv verify-auto` and the `sv verify-*` verbs shell out to **slang + sv2v + Yosys**, discovered via `MUNUNU_<TOOL>_PATH` then `$PATH`. A missing tool is a *structured error / exit `1`*, not a crash. The supported environment is the pinned `docker/Dockerfile.sva` image ([External-Tools](External-Tools)).
- **Assertion-free RTL.** `sv verify-auto` checks the SVA the design *carries*. If the agent emits no `assert property`, use the target-atom verbs (`sv verify-recoverability`, `sv verify-liveness`) to name the property directly.
- **Coverage is a fragment.** Assertions outside the supported SVA fragment come back in `unsupported`; some designs hit the abstraction ceiling and return `unknown`. A definite verdict is sound; treat `unknown` / `skipped` as "not decided here," never as "safe."
- **Safety-⊥ escalation is automatic.** When the cube abstraction leaves a *safety* AG-invariant `⊥`, `verify-auto` retries it with the multi-engine reachability portfolio (exact ⊕ native ⊕ spacer ⊕ btormc ⊕ Pono) and records a `portfolio-rescue` note if it decides. `--no-rescue` / `rescue_bottom_safety: false` opts out.

> **Source of truth:** [`escalate_bottom_safety`](https://github.com/vscorza/mununu/blob/main/crates/mununu-core/src/adapter/slang/verify_auto.rs) — surface: CLI+API.

---

## See also

- [`docs/verifying-rtl.md`](https://github.com/vscorza/mununu/blob/main/docs/verifying-rtl.md) — the property verbs, the no-sidecar flow, and the honest caveats.
- [External-Tools](External-Tools) — installing slang / sv2v / Yosys and the discovery env vars.
- [API-Reference](API-Reference) — the full HTTP surface.
