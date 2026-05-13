# How a model checker handles closed-IP modules: a secure boot walkthrough

> **Draft for Substack publication.** Source: `examples/industrial/secure_boot_rom/`. The transcript embedded below reproduces byte-for-byte by running `./examples/industrial/secure_boot_rom/validate.sh` against the pinned commit. **Do not publish until the four-gate validation checklist in `docs/design/black-box-modules.md` §10.3 passes.**

---

Every chip you buy today contains modules you cannot see inside. The crypto engine in your secure boot ROM, the DDR PHY in your SoC, the memory controller in your accelerator — these arrive as black boxes from vendors, often with no more than a datasheet and a synthesised netlist. The data sheet promises behaviour; the netlist hides the implementation.

This is fine for design closure. It is a problem for *verification*.

A formal verifier sees only what it can see. If you cannot see inside the crypto engine, the verifier cannot prove a property that depends on its internals. The honest answer is to verify what you *can* see — the surrounding controller logic — against a *contract* describing how the closed IP behaves. If the contract is right, the proof transfers. If the contract is wrong, the proof is meaningless.

Mununu, an open-source compositional model checker for reactive systems, has just shipped a contract subsystem that makes this discipline practical: structured contracts attached to module boundaries, an automatic discharge check that catches circular reasoning, and a discovery pipeline that produces *visible defaults* for un-annotated black boxes so you cannot accidentally trust something you have not authored.

This post walks the whole story end-to-end against a real example: a secure boot ROM that verifies a firmware signature using a closed-IP SHA-256 engine and a closed-IP RSA-verify block.

You can reproduce every command. The example lives at [`examples/industrial/secure_boot_rom/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/secure_boot_rom) in the mununu repository. The transcript below is byte-deterministic — run `validate.sh` against the same commit and you will get the same output character-for-character.

## The problem, concretely

A secure boot ROM does a job that sounds simple until you write it down: when power comes up, load the firmware from flash, hash it, verify the signature against a stored public key, and either unlock the host bus (firmware is authentic) or refuse to boot (firmware was tampered with). A bug here is catastrophic — `checkm8` on iPhones, the secure-boot bypasses in various IoT devices, the various TrustZone bootloader incidents over the years. Real-world firmware signature checks have been bypassed by exactly the kind of property a model checker is designed to catch.

The architecture is mainstream. Here is the OpenTitan reference shape, also used in Apple's Secure Enclave, ARM TrustZone bootloaders, Google's Titan M:

```
┌─────────────────────────────────────────┐
│ Secure Boot ROM (open, verifiable)      │
│  ├─ BootController FSM                  │
│  ├─ Flash-read sequencing               │
│  └─ Host-bus arbiter                    │
└─────────────────────────────────────────┘
        │                  │
        ▼                  ▼
┌─────────────┐    ┌─────────────────┐
│ SHA-256 IP  │    │  RSA-verify IP  │
│ (Vendor V1) │    │  (Vendor V2)    │
└─────────────┘    └─────────────────┘
```

The boot controller is yours; the crypto blocks are someone else's. You want to prove things like:

1. **Safety.** The host bus is never unlocked unless we previously completed a successful verify.
2. **Confidentiality.** Key material does not appear on the host bus during the verify operation.
3. **Liveness.** A valid firmware eventually boots.

(1) and (2) are safety properties. (3) is a liveness property. The verifier handles them very differently when the crypto blocks are black boxes.

## The chaotic stub — the conservative default

Mununu's design document for this work — [Document A: black-box modules in compositional extraction](https://github.com/vscorza/mununu/blob/main/docs/design/black-box-modules.md) — opens with a question: what is the most general environment for a black-box module? The answer borrows from Kupferman, Vardi, and Wolper's module-checking framework (J. ACM 2000): treat the module as a single state that nondeterministically produces any output its interface admits, any time. Pessimism is mandatory.

In CTXDSL, mununu's input language, this looks like a tiny automaton:

```
automaton SHA256_V1 {
    states {
        state Idle initial;
        state Computing;
    }
    transitions {
        transition Idle -> Computing on label start_hash;
        transition Computing -> Computing on label sha_compute_tick;
        // Output: hash_valid rises → back to Idle. This may never
        // fire (chaotic — no bounded latency contract).
        transition Computing -> Idle on label hash_valid_rises;
    }
}
```

The crucial line is the last transition: `hash_valid_rises` *can* fire from `Computing`, but mununu cannot assume it *will*. The `Computing → Computing` self-loop is always available. From the boot controller's perspective, the SHA engine might never finish.

Is this realistic? No — real SHA-256 hardware finishes in 64 cycles. Is it *sound*? Yes — any real behaviour is a subset of this chaotic one. Verdicts about the boot controller's safety transfer to the real device. Verdicts about its liveness do not, because the real device offers a progress guarantee the chaotic stub does not.

## Discovery — the visible default

The risk with a chaotic stub is that you forget it is there. You write your boot controller, you verify your safety properties, you ship — and quietly, your verdicts assume "the SHA engine might never finish," which means your liveness claims are vacuous.

Mununu's discovery pipeline makes the default visible. You hand it a description of the black-box interface — just the port list with directions:

```json
{
  "name": "SHA256_V1",
  "ports": [
    { "name": "clk",        "direction": "Input" },
    { "name": "reset_n",    "direction": "Input" },
    { "name": "start_hash", "direction": "Input" },
    { "name": "data_in",    "direction": "Input" },
    { "name": "hash_valid", "direction": "Output" },
    { "name": "hash_out",   "direction": "Output" }
  ],
  "source_file": "rtl/vendor/sha256_v1.sv",
  "source_line": 8
}
```

You run `mununu contract discover sha256_interface.json`, and the verifier reports back:

```
WARN contract gap detected — chaotic stub default in effect
     module="SHA256_V1" kind=output_sequencing
     labels="hash_valid, hash_out"
     location="rtl/vendor/sha256_v1.sv:8"
     soundness="safety verdicts hold; liveness verdicts depending on
                these labels are unsound (no progress assumption)."
phase-1 discovery: 6 label(s), 1 gap marker(s) for module `SHA256_V1`
```

The warning is not optional. It fires every time. It names the module, the gap kind, the labels involved, the source location, and — critically — the *soundness consequence*: which verdicts are still trustworthy and which are not.

For CI in safety-critical projects, `--strict-contracts` turns the warning into a hard error. Either the user authors a contract closing the gap, or the build fails. This is the discipline `docs/design/black-box-modules.md` §2.iii enforces: the default is sound but never silent.

## The discharge graph — catching circular reasoning

Once you start authoring contracts, you can write proofs that contradict themselves. This is a classic A/G pitfall: module A's assumption is discharged by module B's guarantee, B's guarantee is conditioned on B's assumption, B's assumption is discharged by A's guarantee, A's guarantee depends on A's assumption. The cycle closes; the proof appears to hold; it is unsound.

McMillan's *Circular Compositional Reasoning about Liveness* (CHARME 1999) tells you when this kind of reasoning is sound. There has to be a temporal "well-founded ordering" — a way to assign a rank to each clause such that the cycle decreases strictly at every step except one inductive base. Without that witness, the cycle is invalid.

Mununu builds the discharge graph from your contract set and runs Tarjan's strongly-connected-components algorithm on it. Singleton SCC: Pnueli 1985 rule applies, you are fine. Non-trivial SCC: circular reasoning required.

Here is what an accidentally circular contract set looks like in the example:

```json
{
  "clauses": [
    { "id": "G_boot_pubkey_loaded", "kind": "guarantee",  "owner": "BootController" },
    { "id": "A_rsa_pubkey_loaded",  "kind": "assumption", "owner": "RSA_V2" },
    { "id": "G_rsa_verify_ok",      "kind": "guarantee",  "owner": "RSA_V2" },
    { "id": "A_boot_verify_ok",     "kind": "assumption", "owner": "BootController" }
  ],
  "discharges": [
    { "discharger": "G_boot_pubkey_loaded", "dischargee": "A_rsa_pubkey_loaded" },
    { "discharger": "A_rsa_pubkey_loaded",  "dischargee": "G_rsa_verify_ok" },
    { "discharger": "G_rsa_verify_ok",      "dischargee": "A_boot_verify_ok" },
    { "discharger": "A_boot_verify_ok",     "dischargee": "G_boot_pubkey_loaded" }
  ]
}
```

The boot controller's `pubkey_loaded` guarantee depends on RSA's `verify_ok` guarantee, which depends on the boot controller's `verify_ok` assumption, which depends back on `pubkey_loaded`. Mununu sees it:

```
discharge: circular reasoning required (no mu-rank witness)
  cycles:
    - [G_boot_pubkey_loaded -> A_rsa_pubkey_loaded -> G_rsa_verify_ok -> A_boot_verify_ok]
  → mununu refuses to silently accept circular discharge.
    HITL must approve, or one cycle clause must be rewritten
    to be unconditional.
  Tip: assign `mu_rank` to each clause for the lightweight
       McMillan-style automatic discharge (task A8).
```

This is the kind of bug that lives in design reviews for years — the dependency between three datasheet promises that nobody quite traces all the way around. The verifier catches it before verification even starts.

## The lightweight McMillan check

Sometimes the cycle is intentional and sound. Arbiter↔master fairness is a canonical case: each side's progress is contingent on the other's, and a well-founded induction over alternation depth makes the discharge work. McMillan's 1999 paper formalises this; the full rule requires step-indexed reasoning that mununu does not currently implement.

What mununu *does* implement is a lightweight version: if every clause in the cycle carries a `mu_rank` annotation, and the ranks strictly descend around the cycle except at one base edge, the cycle is auto-discharged with provenance tag `mununu-verified circular discharge (mu-rank)`.

Add ranks to the same cycle:

```json
{ "id": "G_boot_pubkey_loaded", "kind": "guarantee",  "owner": "BootController", "mu_rank": 4 },
{ "id": "A_rsa_pubkey_loaded",  "kind": "assumption", "owner": "RSA_V2",         "mu_rank": 3 },
{ "id": "G_rsa_verify_ok",      "kind": "guarantee",  "owner": "RSA_V2",         "mu_rank": 2 },
{ "id": "A_boot_verify_ok",     "kind": "assumption", "owner": "BootController", "mu_rank": 1 }
```

Walking the cycle: 4 → 3 → 2 → 1 → 4. Three strict descents, one base edge. Mununu accepts:

```
discharge: circular with mu-rank witness (auto-accepted, McMillan-style)
  - cycle [G_boot_pubkey_loaded -> A_rsa_pubkey_loaded -> G_rsa_verify_ok -> A_boot_verify_ok],
    base edge: A_boot_verify_ok -> G_boot_pubkey_loaded
```

This is intentionally conservative. It catches the cases where the rank ordering is obvious. For cycles that require deeper temporal reasoning, mununu falls back to "user-approved circular discharge" — the human takes responsibility, with the provenance tag making the assumption auditable later.

## The properties, verified

With the contract machinery in place, the actual verification is uneventful. Three properties over the boot controller automaton:

```
safety_no_execution_without_signature  →  SAT (1/1 reachable states)
bootvalid_reachable                    →  SAT (BootValid reachable from Reset)
reset_always_reachable                 →  SAT (Reset reachable from any state)
```

The safety verdict holds *under chaotic crypto*. That is the substantive claim. A real secure boot ROM whose surrounding logic looked like this would inherit the safety property regardless of what the SHA and RSA engines did internally — as long as they conform to the interface alphabet (which any conforming hardware would).

The liveness verdict ("eventually boots") is the one that does not transfer. Under chaotic crypto, the SHA engine might never complete, so the boot might never finish. To prove liveness, you would have to author a vendor-supplied `latency_bound` contract — the kind of clause that the discovery pipeline's gap marker explicitly invites you to add.

## What this example does not claim

Per [mununu's claims-integrity rules](https://github.com/vscorza/mununu/blob/main/CLAUDE.md), the example is explicit about its boundaries:

- It does not claim mununu found a vulnerability in any commercial secure-boot ROM. The vendor-IP black boxes are stylised; the boot controller is hand-authored for the demonstration.
- It does not claim the closed-IP contract clauses are accurate to any vendor's real datasheet. They are illustrative of the *contract shape*.
- It does not prove a real device is secure. The proof is conditional on the contracts; the contracts are conditional on the vendors honouring their datasheets.
- This particular example does not exercise vendor source-comment annotations (`@mununu_guarantee` on the wrapper module) or contract-corpus lookups. Those mechanisms shipped in milestone M3 (Document D, the contract corpus and config document) and a separate worked example — the TLS handshake at `examples/industrial/tls_handshake/` — exercises them end-to-end. The secure boot ROM example deliberately stays at the chaotic-stub baseline so the reader can see the unannotated default behaviour.

What the example *does* claim: every concept Document A introduces — chaotic stub, gap markers, controllability rule, discharge graph, mu-rank witness — is exercised end-to-end against the mununu binary on `main`, with a byte-deterministic transcript anyone can reproduce.

## Where this fits in the literature

The vocabulary is borrowed:

- Chaotic-stub semantics → Kupferman, Vardi, Wolper, "Module Checking" (CAV '96 / J. ACM 47(2), 2000).
- Contract-shaped automata at module boundaries → de Alfaro, Henzinger, "Interface Automata" (ESEC/FSE 2001).
- Assume-guarantee discharge → Pnueli, "In Transition from Global to Modular Temporal Reasoning about Programs" (NATO ASI 13, 1985); Abadi, Lamport, "Conjoining Specifications" (ACM TOPLAS 17(3), 1995).
- Circular reasoning rule → McMillan, "Circular Compositional Reasoning about Liveness" (CHARME 1999).
- Controlled / external variable distinction → Alur, Henzinger, "Reactive Modules" (Formal Methods in System Design 15(1), 1999).
- Compositional verification framing → de Roever, Langmaack, Pnueli (eds.), *Compositionality: The Significant Difference* (COMPOS '97, LNCS 1536).
- The "compositionality is *the* difference" thesis itself comes from that workshop's preface.

What mununu contributes is the *integration*: a single workflow that pulls all of these into a CLI you can run today against a real example, with the right diagnostics where they matter, and the discipline of refusing to silently accept anything unsound.

## What's next

This is post 1 of a four-document arc. The rest of the arc has since shipped (design + implementation where noted):

- **Document B** — RTL frontend unification (custom SV path + yosys/BTOR2 path → one contract surface). **Design + B1–B3 implementation shipped.** Worked example: [`examples/industrial/dual_frontend_soc/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/dual_frontend_soc).
- **Document D** — Contract corpus + source-comment annotation grammar (the corpus you query for vendor contracts; the `@mununu_*` tag vocabulary; the `contract://` URI resolution). **Design + D1, D2, D4, A6 implementation shipped.** Worked example: [`examples/industrial/tls_handshake/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/tls_handshake). L\* assumption learning (D5/D6) was reassessed post-M3 as long-tail follow-up and is not queued.
- **Document A task A7** (HITL stage-4 review surface) — **shipped after this post was drafted**: `mununu contract review` CLI subcommand + `POST /api/v1/contract/review` HTTP endpoint + a Review sub-tab in the UI surface the proposed clauses extracted from the annotations + corpus references shipped in Document D.
- **Document C** — HW/SW codesign extraction (peripheral RTL + firmware C, with a register-map sidecar coupling the two sides for cross-boundary formal verification). **Design landed; implementation is the next milestone.**

The repo: [github.com/vscorza/mununu](https://github.com/vscorza/mununu).
The example: [`examples/industrial/secure_boot_rom/`](https://github.com/vscorza/mununu/tree/main/examples/industrial/secure_boot_rom).
The design doc: [`docs/design/black-box-modules.md`](https://github.com/vscorza/mununu/blob/main/docs/design/black-box-modules.md).

— Mariano Cerrutti
