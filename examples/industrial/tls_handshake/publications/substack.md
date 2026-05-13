# A contract corpus for hardware verification: a TLS handshake walkthrough

> **Draft for Substack publication.** Source: `examples/industrial/tls_handshake/`. The transcript embedded below reproduces byte-for-byte by running `./examples/industrial/tls_handshake/validate.sh` against the pinned commit. **Do not publish until the four-gate validation checklist in `examples/industrial/tls_handshake/publications/README.md` passes.**

---

Last time, in [post 1 of this series](../../secure_boot_rom/publications/substack.md), we walked through how a model checker handles closed-IP modules — the chaotic-stub default, the explicit gap markers, and the lightweight McMillan check that catches circular reasoning before verification starts. The story ended on a deliberate cliffhanger: if every black-box module in your design starts life as a chaotic stub, and chaotic stubs are useless for liveness, you would spend most of your verification budget writing the same contracts for the same well-known IP over and over.

Most of the closed-IP modules in industry are *not unique*. AXI4-slave is AXI4-slave. AES-CTR is AES-CTR. SHA-256 is SHA-256. The behaviour each contract has to describe is essentially the same across vendors. Hand-writing them per project is wasted work, and the lack of a shared library is a real adoption blocker for compositional verification.

This post walks through the next piece of the story: a **contract corpus** — a queryable, version-pinned repository of vetted black-box contracts — and an annotation grammar that lets a vendor's RTL point directly at a corpus entry. The discovery pipeline resolves the URI, replaces the chaotic stub with the vetted contract, and reports exactly what changed.

The worked example is a **TLS handshake state machine** driving a closed-IP AES-CTR core and a closed-IP TRNG. It is the architecture in every commercial TLS termination device — smart NICs, hardware security modules, embedded TLS accelerators. Mainstream, not academic.

You can reproduce every command. The example lives at [`examples/industrial/tls_handshake/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/tls_handshake) in the mununu repository. The transcript below is byte-deterministic — run `validate.sh` against the same commit and you will get the same output character-for-character.

## The problem the corpus solves

A TLS handshake state machine looks roughly like this:

```
Idle → ClientHello → ServerHello → KeyDeriv → NonceReq → CipherReady →
   Finished → Application (records loop)
```

Every transition out of `KeyDeriv` waits for the AES core to confirm a derived key. Every transition out of `NonceReq` waits for the TRNG to deliver a fresh nonce. Every record in the `Application` loop waits for the AES core to encrypt or decrypt.

If you model the AES core as a chaotic stub, the handshake might never make it past `KeyDeriv` in the model — the chaotic stub is free to stall forever. Your safety properties still hold (over-approximation + safety = sound), but you cannot prove the handshake completes, you cannot prove a bounded latency, you cannot prove the device makes forward progress. To prove anything beyond raw safety, you need a *contract* for the AES core: an assumption you make about its behaviour, on top of which your proof can build.

The question is where that contract comes from.

Today, the answer in most formal-verification flows is "you write it by hand." Every project, every team, every audit. Most of that work duplicates work that someone, somewhere, has already done — because AES-CTR is AES-CTR. The vendor's datasheet specifies the same handshake; the same standard (NIST SP 800-38A) defines the same modes; the same safety obligations apply.

Mununu's design document for this work — [Document D: contract corpus + unified config](https://github.com/vscorza/mununu/blob/main/docs/design/contract-corpus-and-config.md) — proposes a shared library. The precedent is well-established outside hardware: the Accellera Open Verification Library (OVL) is a parameterised assertion-checker library; SV-COMP ships 30,000+ public C verification benchmarks; SMT-LIB is the gold standard for queryable verification artefacts. The hardware side does not yet have an equivalent for *component contracts*. Document D's contribution is putting one together.

## What the corpus actually looks like

The corpus is a directory tree. One file per entry:

```text
corpus/
└── rtl_crypto/
    └── aes_ctr@1.0.0.json
```

Inside the file:

```json
{
  "id": "rtl_crypto/aes_ctr",
  "version": "1.0.0",
  "domain": "rtl_crypto",
  "name": "aes_ctr",
  "description": "AES-CTR symmetric cipher block contract. Captures the canonical request/done handshake plus the safety guarantee that the cipher never emits ciphertext bytes before a nonce has been loaded for the current session. ILLUSTRATIVE: this entry demonstrates the corpus schema for Document D's TLS handshake industrial example; it is not derived from any specific vendor silicon.",
  "parameters": { "key_bits": 128, "block_bits": 128, "counter_bits": 64 },
  "alternatives": [
    { "id": "strict_iv",   "label": "Strict IV uniqueness",
      "description": "The contract asserts that the IV+counter combination is monotonically advanced per block; reuse is treated as a guarantee violation." },
    { "id": "permissive",  "label": "Permissive — caller-managed IV",
      "description": "Drops the IV-uniqueness guarantee. Caller is responsible for tracking IV freshness." }
  ],
  "provenance": {
    "tier": "mununu_verified",
    "verified_against": "NIST SP 800-38A counter-mode reference (illustrative — no real vendor silicon was modelled)"
  },
  "soundness_flag": "safety+liveness"
}
```

Three things to notice:

1. **Parameter-matched lookup.** A query carrying `{key_bits: 128, block_bits: 128}` is scored against this entry's `parameters` map. Full matches rank ahead of partial matches; a *mismatch* (entry says 128, query says 256) disqualifies the entry entirely.

2. **Named alternatives.** A single entry can ship multiple verification styles. Here, `strict_iv` adds an IV-uniqueness guarantee that `permissive` does not. The user picks one at HITL-review time; the choice is recorded in the audit trail.

3. **Provenance tier.** Three trust levels: `mununu_verified` > `vendor:<name>` > `community`. The ranker uses the tier as a tie-breaker after parameter-match exactness. Every entry must declare its origin honestly. The example's entry is tagged `illustrative` — it is not derived from any specific vendor silicon, and the `verified_against` field says so.

A direct corpus query looks like this:

```text
$ mununu contract query rtl_crypto/aes_ctr --corpus corpus
found 1 contract candidate(s) for rtl_crypto/aes_ctr:
  1. rtl_crypto/aes_ctr @ 1.0.0  [mununu-verified (NIST SP 800-38A counter-mode reference (illustrative — no real vendor silicon was modelled))]
       AES-CTR symmetric cipher block contract. …
```

That is step 1 of the example transcript. Useful for sanity-checking what is in the corpus before the discovery pipeline reaches for it.

## The annotation grammar — closing the loop

The corpus is one half of the integration. The other half is the **annotation grammar** ([Document D §D.5](https://github.com/vscorza/mununu/blob/main/docs/design/contract-corpus-and-config.md#d5-source-comment-annotation-grammar)). A vendor's RTL — or, equivalently, the JSON sidecar describing a closed-IP module — carries an annotation that points directly at a corpus entry:

```verilog
(* mununu_blackbox *)
(* mununu_interface = "contract://rtl_crypto/aes_ctr@1.0.0?alt=strict_iv" *)
(* mununu_guarantee = "G(start -> eventually done)" *)
module aes_ctr_v1 (input clk, input start, output done, ...);
endmodule
```

The annotation grammar carries six tags today (with two more reserved): `@mununu_blackbox`, `@mununu_assume`, `@mununu_guarantee`, `@mununu_interface`, `@mununu_controllable`, `@mununu_uncontrollable`. The same vocabulary applies across SystemVerilog, C, TypeScript, Rust, and Python — the parser strips the language-specific wrapper and feeds the inner tag and value to a single downstream consumer. Today only the SystemVerilog wrappers are wired up; the other languages are explicit follow-up scope.

The `contract://` URI inside `@mununu_interface` has a small grammar:

```text
contract://<domain>/<name>[@<version>][?alt=<alternative>]
```

The grammar is permissive about whitespace and case-of-scheme, strict about structure. A URI with no `/` after the scheme is malformed; a URI that does not start with `contract://` is treated as an opaque sidecar path rather than a corpus lookup.

The discovery pipeline parses each `@mununu_interface` annotation and resolves the URI against the corpus. Five outcomes are possible — every annotation produces exactly one `CorpusResolution` record so the audit trail is complete:

| Status | Meaning |
|---|---|
| `Resolved` | URI parsed, corpus had a matching entry; `?alt=…` checked against the entry's alternatives. |
| `NotFound` | URI parsed, no corpus entry matched the `(domain, name)` tuple. |
| `NoCorpus` | URI parsed, but the caller did not supply `--corpus`. |
| `Malformed` | URI started with `contract://` but failed to parse. |
| `SidecarReference` | Value did not start with `contract://` — treated as an opaque sidecar path. |

The five-way split is deliberate. Silently dropping an annotation is the bug Document A §2.iii was designed to prevent: the user must always know whether their proof relies on a vendor-declared contract that the verifier actually resolved.

## The example, walked end-to-end

The TLS handshake example wires three components:

```
┌───────────────────────────────────────────────┐
│ TLS Handshake (open, fully verifiable)        │
│  ├─ ClientHello → ServerHello → KeyDeriv      │
│  ├─ NonceReq → CipherReady → Finished         │
│  └─ Application loop (records over AES-CTR)   │
└───────────────────────────────────────────────┘
        │                       │
        ▼                       ▼
┌──────────────────┐    ┌──────────────────┐
│ AES-CTR core     │    │ TRNG (random)    │
│ (Vendor v1)      │    │ (Vendor V2)      │
│ → corpus hit     │    │ → corpus miss    │
└──────────────────┘    └──────────────────┘
```

The AES interface's `@mununu_interface` annotation hits the corpus. The TRNG's annotation deliberately references a corpus entry that does not exist (`contract://rtl_crypto/trng@2.0.0`), so the transcript surfaces the `NotFound` path. The four-step discovery sequence below — directly from the byte-deterministic `transcript.txt` — shows what the user sees.

### Step 2: AES with corpus

```text
$ mununu contract discover examples/industrial/tls_handshake/aes_ctr_interface.json --corpus corpus
WARN contract gap detected — chaotic stub default in effect module="AES_CTR_v1" kind=latency_bound labels="done, cipher_out" location="rtl/vendor/aes_ctr_v1.sv:8" soundness="bounded-time properties cannot be discharged without an authored latency bound." description="Phase-2 discovery — 1 guarantee clause(s) + 1 corpus resolution(s) found on AES_CTR_v1; latency bound still unauthored"
phase-1 discovery: 8 label(s), 1 gap marker(s) for module `AES_CTR_v1`
  corpus: resolved `contract://rtl_crypto/aes_ctr@1.0.0?alt=strict_iv` → rtl_crypto/aes_ctr@1.0.0 [alt `strict_iv` ok]
```

Three observations:

1. The corpus resolution is reported on its own line: `resolved … [alt strict_iv ok]`. The alternative requested in the URI was checked against the entry's declared alternatives and matched.
2. The default gap marker for outputs (`output_sequencing` — "no progress assumption discharged") was **downgraded** to `latency_bound`. A corpus hit is equivalent in effect to discovering a guarantee clause in source: the verifier no longer needs to assume the AES core makes zero progress, but it still does not have a bound on *how fast*. The user now knows exactly what they would gain by adding `latency_bound` to the AES contract locally.
3. The `WARN` line is the structured diagnostic Document A §2.iii mandates. It carries the module, the gap kind, the labels affected, the source location, and a one-line soundness consequence. Nothing is silent.

### Step 3: TRNG without a corpus entry

```text
$ mununu contract discover examples/industrial/tls_handshake/rng_interface.json --corpus corpus
WARN contract gap detected — chaotic stub default in effect module="TRNG_V2" kind=output_sequencing labels="rand_valid, rand_out" location="rtl/vendor/trng_v2.sv:8" soundness="safety verdicts hold; liveness verdicts depending on these labels are unsound (no progress assumption)." description="Phase-1 discovery — no sequencing fragment yet for TRNG_V2"
phase-1 discovery: 5 label(s), 1 gap marker(s) for module `TRNG_V2`
  corpus: `contract://rtl_crypto/trng@2.0.0` not found (rtl_crypto/trng@2.0.0)
```

Here the corpus has no `trng@2.0.0` entry. The diagnostic line names what is missing: `rtl_crypto/trng@2.0.0`. The default `output_sequencing` gap is preserved — no downgrade, because nothing was discovered. The user sees that they need to author a local contract for the TRNG (or, in a more mature ecosystem, contribute one back to the corpus so the next project does not have to redo the work).

This is the right user experience for "I have a vendor that hasn't been catalogued yet." Mununu does not silently fall back to a less-safe model. The annotation is preserved in the sidecar, the gap is preserved, the user is told precisely what to do next.

### Step 4: AES with the annotation, but no `--corpus`

```text
$ mununu contract discover examples/industrial/tls_handshake/aes_ctr_interface.json
WARN contract gap detected — chaotic stub default in effect …
phase-1 discovery: 8 label(s), 1 gap marker(s) for module `AES_CTR_v1`
  corpus: `contract://rtl_crypto/aes_ctr@1.0.0?alt=strict_iv` referenced but no corpus supplied (use --corpus)
```

The annotation parses cleanly. The verifier has no corpus to query (the user did not pass `--corpus`). The diagnostic surfaces the `NoCorpus` status, tells the user what flag is missing, and preserves the gap. The user sees what they would gain by pointing at a corpus.

### Step 5: strict mode

```text
$ mununu contract discover examples/industrial/tls_handshake/aes_ctr_interface.json --corpus corpus --strict-contracts
…
phase-1 discovery: 8 label(s), 1 gap marker(s) for module `AES_CTR_v1`
  corpus: resolved `contract://rtl_crypto/aes_ctr@1.0.0?alt=strict_iv` → rtl_crypto/aes_ctr@1.0.0 [alt `strict_iv` ok]
--strict-contracts: 1 unresolved contract gap(s) — refusing to proceed
```

This is the safety-critical CI mode. A corpus hit is necessary but **not sufficient**: the residual `LatencyBound` gap remains, and `--strict-contracts` exits non-zero. The user must either author the latency bound locally (closing the gap entirely) or accept the gap explicitly (running without `--strict-contracts`). For safety-critical pipelines this is the right gate: the proof only ships when every gap is closed.

## Verifying the handshake

With the closed-IP modules characterised (chaotic stubs decorated with what corpus hits could give them), the rest of the verification proceeds normally:

```text
$ mununu context eval examples/industrial/tls_handshake/tls_handshake.ctxdsl \
    --formula safety_protocol_respected --automaton TLSSession
Formula 'safety_protocol_respected' over automaton 'TLSSession':
  States satisfying: 4/4
    Computing|ClientHello|Generating, Computing|ServerHello|Generating, Computing|ServerHello|Idle, Idle|Idle|Idle
  Initial states satisfying: 1/1
    Idle|Idle|Idle
```

The safety property — every reachable state respects the protocol — holds across all reachable composed states. The composition is sparse because the synchronous composition strictly requires all three automata to share the firing label; the four states above are the ones where all three are simultaneously available. Safety properties are sound under over-approximation, so the result transfers to any real system whose AES and TRNG modules behave at *least* as the chaotic stubs permit.

Two reachability properties on the standalone handshake automaton confirm the example is non-trivial:

```text
Formula 'application_reachable' over automaton 'TLSHandshake':
  States satisfying: 8/8
Formula 'idle_always_reachable' over automaton 'TLSHandshake':
  States satisfying: 8/8
```

Application is reachable from `Idle`; `Idle` is reachable from every state via teardown. The composition is well-formed and the handshake can both progress and abort.

## What this *does not* claim

This is the part that gets short-changed in academic papers and inflated in marketing material. The CLAUDE.md claims-integrity rules are explicit about which side of this line each statement belongs to.

- It does **not** claim mununu found a vulnerability in any real TLS implementation. The vendor-IP modules are stylised; the handshake controller is hand-authored for the demonstration.
- It does **not** claim the AES-CTR corpus entry is a complete or accurate model of any specific commercial AES core. The entry is tagged `illustrative — no real vendor silicon was modelled` in its provenance, in writing, where the file lives.
- It does **not** prove that any real TLS device using this architecture is secure. The proof is conditional on the contracts; the contracts are conditional on the vendor honouring the cited standard.
- It does **not** ship a TRNG corpus entry. The TRNG interface deliberately references a corpus entry that does not exist so the transcript exercises the `NotFound` path. A real project would author a local `trng@2.0.0` entry, then re-run discovery to clear the gap.

The corpus-hit verdict is "the verifier found a vetted reference contract for this IP." It is not "this IP is correct." The contract still has to be honoured by the silicon. That has to be checked in a different way — vendor-supplied tests, audited reference implementations, equivalence-checking against a golden RTL. Mununu's contribution is making the contract *visible and queryable*, not making it true.

## What's next

This is post 3 of a four-document arc. Post 1 ([secure boot ROM](../../secure_boot_rom/publications/substack.md)) covered the chaotic-stub default + discharge graph. Post 2 ([dual-frontend SoC](../../dual_frontend_soc/publications/substack.md)) covered the two RTL frontends, one IR principle. This post covered the contract corpus + annotation grammar. The capstone — formal verification across the HW/SW boundary — is on the way.

The TLS handshake example is reproducible right now. Clone the mununu repo, `cd` into the example directory, run `validate.sh`, and you get exactly the transcript quoted above. The corpus, the annotations, the gap-downgrade behaviour — all on `main`.

If you have a closed-IP module whose contract you would like to contribute to the corpus, the schema is `corpus/<domain>/<name>@<version>.json` and the format is in [Document D §D.2](https://github.com/vscorza/mununu/blob/main/docs/design/contract-corpus-and-config.md). Community-tier provenance is the default; mununu-verified is a curated promotion.

Code: [github.com/vscorza/mununu](https://github.com/vscorza/mununu).
