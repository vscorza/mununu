# LinkedIn post — A contract corpus for hardware verification: walking a TLS handshake

> **Draft for LinkedIn.** Target: 150–200 words. Source artefact: `examples/industrial/tls_handshake/`. Do not publish until the four-gate validation checklist in `examples/industrial/tls_handshake/publications/README.md` passes.

---

Every TLS device on the planet wires an open handshake state machine to a few closed-IP crypto blocks — AES, RNG, sometimes HMAC. Verifying the handshake requires *some* assumption about what those blocks do. Hand-writing a fresh contract for every project is wasted work — most of those contracts are about the same well-known IP.

Mununu just shipped a contract corpus: a queryable repository of vetted black-box contracts. Vendors annotate their RTL with `@mununu_interface contract://rtl_crypto/aes_ctr@1.0.0?alt=strict_iv`; the discovery pipeline resolves the URI against the corpus and replaces the default chaotic stub with the vetted contract automatically. Misses are surfaced as structured diagnostics so you know what's missing.

I wrote a worked example: a TLS handshake driving a closed-IP AES-CTR core and a closed-IP TRNG. The AES URI hits the corpus and the gap downgrades from `output_sequencing` to `latency_bound`. The TRNG URI misses on purpose, showing the user the missing-contract diagnostic. Every command runs against the open-source mununu binary on main; the transcript is byte-deterministic, regenerable by running `./examples/industrial/tls_handshake/validate.sh`.

Full write-up: [Substack link TBD].
Code: github.com/vscorza/mununu.

#formalverification #tls #hardware #verification
