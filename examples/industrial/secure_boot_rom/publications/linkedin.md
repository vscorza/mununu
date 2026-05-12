# LinkedIn post — Verifying secure boot when you can't see inside the crypto blocks

> **Draft for LinkedIn.** Target: 150–200 words. Source artefact: `examples/industrial/secure_boot_rom/`. Do not publish until the four-gate validation checklist in `docs/design/black-box-modules.md` §10.3 passes.

---

Every chip you buy contains crypto blocks you cannot see inside — vendor SHA-256, vendor RSA-verify, vendor DDR PHY. Datasheets describe behaviour; netlists hide implementation. That is a problem for formal verification: the verifier sees only what it can see.

Mununu just shipped a contract subsystem that makes this discipline practical: chaotic-stub defaults for un-annotated black boxes, automatic discharge-graph analysis that catches circular reasoning before verification starts, and a lightweight McMillan-style rank witness that auto-accepts well-formed circular contracts.

I wrote a worked example: a secure boot ROM with a closed-IP SHA-256 engine and a closed-IP RSA-verify block — the architecture used in OpenTitan, ARM TrustZone, Google Titan M. Every command in the demonstration runs against the open-source mununu binary on main; the transcript is byte-deterministic, regenerable by running `./examples/industrial/secure_boot_rom/validate.sh`.

The property verified: "no host-bus unlock without a completed verify," sound under chaotic crypto. The property *not* verified: "valid firmware eventually boots" — that one needs a vendor-supplied latency contract.

Full write-up: [Substack link TBD].
Code: github.com/vscorza/mununu.

#formalverification #secureboot #hardware #verification
