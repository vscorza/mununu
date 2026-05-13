# LinkedIn post — Why mununu has two SystemVerilog pipelines (and won't merge them)

> **Draft for LinkedIn.** Target: 150–200 words. Source artefact: `examples/industrial/dual_frontend_soc/`. Do not publish until the four-gate validation checklist in `publications/README.md` passes.

---

Mununu has two SystemVerilog frontends. One is Rust-native and produces symbolic Kripke structures via SMT — built for protocol-level verification, where registers can be treated as enums and bit-exactness doesn't matter. The other shells out to yosys, flattens, bit-blasts, and verifies the bit-level result.

People keep asking why we don't merge them. We won't — they serve two different verification audiences. The point of the new design doc is to **unify the seams, leave the cores free**: one IR, one composition primitive, one controllability rule, one set of property roles. But two precision tiers, two parsers, two clock semantics. The CIRCT pattern, retrofitted.

The new work also closes the loop with mununu's contract subsystem: when an adapter encounters a closed-IP black-box module, it auto-emits the `BlackBoxInterface.json` + `GapMarkerReport.json` sidecars the contract workflow already consumes. No more hand-authoring what the extractor already parsed.

Worked example with a byte-deterministic transcript: a tiny SoC with an open host + UART + a closed-IP DDR3 PHY.

Full write-up: [Substack link TBD].
Code: github.com/vscorza/mununu.

#formalverification #rtl #systemverilog #hardware #verification
