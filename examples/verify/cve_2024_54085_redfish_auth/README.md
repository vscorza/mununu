# Redfish Authentication-Bypass Pattern (CVE-2024-54085)

> **Status: pattern demonstration.** Hand-authored CTXDSL model of a publicly disclosed firmware-class authentication-bypass shape. **Not** extracted from any specific firmware binary; no source recovery was performed. See [Provenance and limitations](#provenance-and-limitations) below.
>
> **Source of truth:** [`redfish_auth_vulnerable.ctxdsl`](redfish_auth_vulnerable.ctxdsl) and [`redfish_auth_fixed.ctxdsl`](redfish_auth_fixed.ctxdsl) (model bodies); [`verify_vulnerable.toml`](verify_vulnerable.toml) and [`verify_fixed.toml`](verify_fixed.toml) (verify-framework manifests). CTXDSL adapter: [`crates/mununu-core/src/context_dsl/`](../../../crates/mununu-core/src/context_dsl/). Memory-soundness check: [`crates/mununu-core/src/verify/memory_check.rs`](../../../crates/mununu-core/src/verify/memory_check.rs). Surface: CLI+API+UI.

## The example

A baseboard-management-controller (BMC) implements a Redfish HTTP interface. A privileged operation — say, firmware update or admin-user creation — should be reachable only after the server has performed an authentication check on the requester. The vendor advisory for CVE-2024-54085 (AMI MegaRAC SPx, added to the CISA Known Exploited Vulnerabilities catalog on 2025-06-25) describes a shape where a specially crafted Redfish request lets the handler treat the session as authenticated without traversing the auth-check transition. The consequence is a remote-unauthenticated path to the privileged-operation state.

We model the property class — *"every privileged operation in a session must be preceded by an authentication check in that session"* — as a single safety formula over a hand-authored CTXDSL automaton, run it against a vulnerable variant where the bypass exists, then against a patched variant where the shortcut transition is removed. The vulnerable manifest reports the safety property as **VIOLATED** and the bypass state as **reachable**. The patched manifest reports the same safety property as **SATISFIED** and the bypass state as **unreachable**. The before/after diff is the property's verdict, not its text.

This is a *pattern demonstration*, not a finding about any specific firmware build. We did not extract or analyse AMI MegaRAC binaries; the model is built from the public CVE / NVD description. See [Provenance and limitations](#provenance-and-limitations).

## Reproduce

```bash
bash examples/verify/cve_2024_54085_redfish_auth/validate.sh
```

The script builds the `mununu` CLI, runs `mununu memory check` against both manifests (B2b memory-soundness audit), then `mununu verify --print-counterexample` against each, and writes the byte-deterministic [`transcript.txt`](transcript.txt).

## What the transcript shows

### Vulnerable manifest

```
no_unauthenticated_privileged_exec: VIOLATED (0/7 states, 0/1 initial)
  counterexample (from Unconnected):
    1. --[http_connect]--> Connected
    2. --[normal_request]--> AuthCheckInProgress
    3. --[perform_auth_check]--> AuthenticatedViaCheck
    4. --[execute_privileged_op]--> PrivilegedExecLegit
    5. --[execute_privileged_op]--> PrivilegedExecLegit
    (cycle back to step 4)
legit_privileged_path_reachable: SATISFIED (7/7 states, 1/1 initial)
bypass_state_reachable: SATISFIED (7/7 states, 1/1 initial)
```

The safety formula `nu X. ((!PrivilegedExecUnauth) && ([] X))` is **VIOLATED** because every state in the vulnerable automaton has *some* successor sequence leading to `PrivilegedExecUnauth` — including states on the legitimate-looking path, because `session_close` from `PrivilegedExecLegit` returns the session to `Unconnected`, from which the bypass remains reachable. That is why the counterexample lasso the orchestrator surfaces is *not itself* the attack sequence — it's any reachable state at which the formula fails. The complementary `bypass_state_reachable: SATISFIED` verdict confirms the bug state is reachable from the initial state.

### Fixed manifest

```
no_unauthenticated_privileged_exec: SATISFIED (6/7 states, 1/1 initial)
legit_privileged_path_reachable: SATISFIED (5/7 states, 1/1 initial)
bypass_state_reachable: VIOLATED (1/7 states, 0/1 initial)
  counterexample (from Unconnected):
    1. --[http_connect]--> Connected
    2. --[header_spoof_request]--> AuthCheckInProgress
    3. --[perform_auth_check]--> AuthenticatedViaCheck
    4. --[execute_privileged_op]--> PrivilegedExecLegit
    5. --[execute_privileged_op]--> PrivilegedExecLegit
    (cycle back to step 4)
```

The same safety formula is now **SATISFIED**. The reachability property for the bypass state is **VIOLATED** — `PrivilegedExecUnauth` is structurally present in the automaton but no transition reaches it. The lasso surfaced for that violation is the most readable artifact in the transcript: it shows the *fix in action* — even when the attacker sends `header_spoof_request` from the `Connected` state, the patched handler routes the request into `AuthCheckInProgress`, which can only progress via `perform_auth_check` to `AuthenticatedViaCheck` and only then to a privileged-execution state.

## How the model is structured

Two automata composed asynchronously:

- **`HttpClient`** (2 states: `Disconnected`, `Open`) — an environment-side driver that issues every label in the alphabet. State predicates are not referenced on this automaton; it makes the composition non-trivial and mirrors the verify-framework idiom of pairing a driver with a stateful peer.
- **`RedfishAuthHandler`** (7 states) — encodes the joint cross-product of (session-position × audit) on a single automaton, following the `MappingAndRouter` pattern from `examples/agentic/mcp_extracted/cve_2026_25536_transport_*.ctxdsl`. Putting the joint state on one automaton lets the safety property refer to a single state-name predicate (`PrivilegedExecUnauth`) instead of having to express a cross-product condition.

The vulnerable variant declares one extra transition that the fixed variant lacks: `Connected --[header_spoof_request]--> AuthenticatedViaSpoof`. Every other transition is shared between the two files. Diff-clean for review.

The composition is declared in the verify manifests, not inside the CTXDSL files — the latter contain only `alphabet` + `automata` blocks, per the verify-framework idiom shared with [`examples/verify/uart_codesign_protocol_spec/peripheral_protocol.ctxdsl`](../uart_codesign_protocol_spec/peripheral_protocol.ctxdsl).

## Memory-soundness posture

Both manifests declare:

```toml
[sources.memory_abstraction]
kind  = "chaotic"
notes = "Protocol-level model with no shared memory; properties reference automaton state names only."
```

Per the memory soundness matrix in [`docs/abstraction.md`](../../../docs/abstraction.md#memory-soundness-matrix), the `chaotic` posture is **sound for safety properties that do not reference memory**. Our properties reference only state-name predicates on `RedfishAuthHandler`, so the posture is correct. `mununu memory check` returns zero warnings on both manifests; the report is part of the captured transcript.

## Provenance and limitations

Per the project's [claims-integrity policy](../../../docs/policies/claims-integrity.md) (Rules 1, 2, 4, 5):

- **Rule 1 (Models from source, not documentation).** This CTXDSL is hand-authored from the public CVE-2024-54085 / NVD description and the vendor advisory's English-language description of the bypass shape. It is **not** extracted from AMI MegaRAC firmware. No binary recovery, no decompilation, no source-leak inspection was performed. Treat this as a *design-pattern demonstration*, not a finding about a specific build.
- **Rule 2 (Planted bugs are demos).** The bypass transition in the vulnerable model was placed by the author to capture the disclosed shape. The model demonstrates that mununu's safety property machinery distinguishes vulnerable from patched variants of this property class.
- **Rule 4 (Reproduction path).** This finding is **structural**: it exists in the abstract state machine. It has **not** been reproduced against a running AMI MegaRAC service, a Verilator simulation, or any vendor firmware image. Any external user of this example who wishes to claim impact on a real system must add the corresponding extraction + reproduction step themselves.
- **Rule 5 (Abstraction soundness).** The model collapses HTTP-request content to abstract event labels — `header_spoof_request` is a stand-in for "a specially crafted Redfish request that bypasses the auth header check" per the disclosure. This is **over-approximation**: any concrete refinement that does not introduce a new auth-check on the spoofed path is still in the property's violation set. Per the soundness directions in [CLAUDE.md § Soundness Guarantees](../../../CLAUDE.md#soundness-guarantees), safety + over-approximation = SOUND, so a model-level "safety violated" verdict transfers to any concrete realisation of the model. The legit-vs-bypass distinction is captured by the explicit `perform_auth_check` transition the audit-side of the joint state depends on.

## What this example does *not* do

- It does not extract C source from any specific firmware build.
- It does not run under Verilator or against simulated BMC hardware.
- It does not claim to find an un-disclosed weakness — only that mununu can express, evaluate, and counterexample the *property class* the disclosure describes.

## Where it fits in the verify-framework gallery

Adjacent examples in [`examples/verify/`](..) that share the "CTXDSL source + reachability/safety property + per-source memory-abstraction posture" shape:

- [`uart_codesign_protocol_spec/`](../uart_codesign_protocol_spec/) — firmware C source + hand-authored peripheral protocol spec via the codesign adapter.
- [`xstate_pair/`](../xstate_pair/) — minimal two-source XState end-to-end smoke test.
- [`library_demo/`](../library_demo/) — `count = N` parameterised library templates.

Together they cover the verify-framework's per-source compositional analysis from three angles: extracted firmware (codesign), shipped templates (library), and hand-authored protocol patterns (this example).
