//! SV-direct property verbs — lift a SystemVerilog module to BTOR2 (sv2v + Yosys)
//! and decide a property in **one** call.
//!
//! An external tool (e.g. an agent writing RTL) can hand raw SV plus a target atom
//! and get a verdict, with no manual `sv emit-btor2-per-module` → `btor2 verify-*`
//! round-trip. Each function is a thin bridge: [`sv_to_btor2`] then the matching
//! BTOR2 verb core ([`decide_reach_portfolio_parallel`],
//! [`response_liveness_rescue_atoms`], [`verify_recoverability`]). The SV → BTOR2
//! lift requires sv2v + Yosys on the host (see [`docs/verifying-rtl.md`]); a missing
//! tool returns a structured error.
//!
//! Atoms are validated **before** the (toolchain-gated) lift, so a malformed
//! request fails fast and identically to the BTOR2-direct verbs.
//!
//! **Reset**: these verbs lift the design with the reset as a *free input* (no
//! reset-gating). Sound, and the honest default — recoverability in particular is
//! reset-dependent (see `docs/design/recoverability-vs-sva.md` §3.2). Reset-gating
//! (as `sv verify-auto` offers) is a future option.

use crate::adapter::btor2::kmts_lift::PredicateSpec;
use crate::adapter::btor2::parser;
use crate::adapter::btor2::predicate_expr::parse_predicate_expr;
use crate::adapter::liveness_rescue::{
    Atom, LivenessVerdict, parse_response_atom, parse_response_pairs,
    response_liveness_rescue_atoms, response_liveness_rescue_conjunction,
};
use crate::adapter::reach_portfolio::{ReachOutcome, decide_reach_portfolio_parallel};
use crate::adapter::recoverability::verify_recoverability_with_predicates;
use crate::adapter::yosys::{SvFrontend, YosysOptions, sv_to_btor2};
use crate::verdict::PropertyVerdict;

/// The SV → BTOR2 lift inputs shared by every SV-direct verb.
pub struct SvLift {
    /// Primary SystemVerilog source.
    pub source: String,
    /// Additional SV sources (packages / includes) as `(name, content)`.
    pub additional_sources: Vec<(String, String)>,
    /// Top module (Yosys auto-detects when `None`).
    pub top: Option<String>,
    /// Run sv2v before Yosys (modern SV module-header imports, etc.).
    pub use_sv2v: bool,
    /// Extra on-disk include-search directories (`-I<dir>`), so
    /// `` `include "frag.vh" `` resolves against the original source tree
    /// without the fragment being read as a standalone compilation unit.
    /// Feeds [`YosysOptions::extra_include_dirs`]; empty by default.
    pub include_dirs: Vec<std::path::PathBuf>,
    /// RTL front-end selection. `Slang` forces the yosys-slang plugin, which
    /// lifts modern-SV constructs `read_verilog`/sv2v reject. Feeds
    /// [`YosysOptions::frontend`]; `Auto` by default.
    pub frontend: SvFrontend,
}

impl SvLift {
    /// Lift to a single flattened BTOR2 string (sv2v optional + Yosys).
    fn lift(&self) -> Result<String, String> {
        let yopts = YosysOptions {
            top: self.top.clone(),
            additional_sources: self.additional_sources.clone(),
            use_sv2v: self.use_sv2v,
            extra_include_dirs: self.include_dirs.clone(),
            frontend: self.frontend,
            ..Default::default()
        };
        sv_to_btor2(&self.source, &yopts)
            .map_err(|e| format!("SV → BTOR2 (sv2v + Yosys): {}", e.message))
    }
}

/// `sv verify` — lift SV and decide `bad`-reachability of its assertions with the
/// multi-engine safety portfolio.
pub fn sv_verify_safety(lift: &SvLift) -> Result<ReachOutcome, String> {
    let btor2 = lift.lift()?;
    let file =
        parser::parse(&btor2).map_err(|e| format!("parsing the lifted BTOR2: {}", e.message))?;
    Ok(decide_reach_portfolio_parallel(&file))
}

/// `sv verify-liveness` — lift SV and decide `AG(request → AF grant)` via the l2s
/// reduction + the portfolio. The atoms are parsed first (a malformed atom errors
/// before the lift).
pub fn sv_verify_liveness(
    lift: &SvLift,
    request: &str,
    grant: &str,
) -> Result<(LivenessVerdict, ReachOutcome), String> {
    let ante: Atom = parse_response_atom(request)?;
    let cons: Atom = parse_response_atom(grant)?;
    let btor2 = lift.lift()?;
    response_liveness_rescue_atoms(&btor2, &ante, &cons, false).ok_or_else(|| {
        "could not build the liveness monitor — an atom likely binds no signal in the design"
            .to_string()
    })
}

/// `sv verify-liveness-all` — lift SV and decide the **conjunction** of
/// response-liveness properties `⋀ᵢ AG(aᵢ → AF bᵢ)` via the l2s reduction + the
/// portfolio (one lift, one `bad`-reachability query per response). Each `responses`
/// entry is a `"ANTE => CONS"` pair; every pair is parsed first, so a malformed
/// response errors before the (toolchain-gated) lift — matching [`sv_verify_liveness`].
///
/// Returns the combined verdict + the per-response [`ReachOutcome`] (same order as
/// `responses`), via [`response_liveness_rescue_conjunction`].
pub fn sv_verify_liveness_all(
    lift: &SvLift,
    responses: &[String],
) -> Result<(LivenessVerdict, Vec<ReachOutcome>), String> {
    let pairs = parse_response_pairs(responses)?;
    let btor2 = lift.lift()?;
    response_liveness_rescue_conjunction(&btor2, &pairs, false).ok_or_else(|| {
        "could not build a liveness monitor — an atom likely binds no signal in the design, \
         or no responses were given"
            .to_string()
    })
}

/// `sv verify-recoverability` — lift SV and decide `AG EF target`. The target atom is
/// validated before the lift.
pub fn sv_verify_recoverability(lift: &SvLift, target: &str) -> Result<PropertyVerdict, String> {
    sv_verify_recoverability_with_predicates(lift, target, &[])
}

/// [`sv_verify_recoverability`] with optional extra abstraction predicates for the
/// cube-path escalation (P2 Slice 1). The target atom is validated before the
/// (toolchain-gated) lift; the extras only refine the cube if the exact engine abstains.
pub fn sv_verify_recoverability_with_predicates(
    lift: &SvLift,
    target: &str,
    extra_predicates: &[PredicateSpec],
) -> Result<PropertyVerdict, String> {
    parse_predicate_expr(target).map_err(|e| {
        format!("recoverability target `{target}` is not a register-comparison atom: {e:?}")
    })?;
    let btor2 = lift.lift()?;
    verify_recoverability_with_predicates(&btor2, target, extra_predicates)
}

/// `sv verify-recoverability --refine` — lift SV and decide `AG EF target`, returning the canonical
/// verdict PLUS a structured [`VerdictRefinement`]: a `Vacuous` witness, an auto `config_partition`
/// over the detected reset, a `bot_diagnosis` hint, and — when `discover_assumptions` is set and the
/// property does not hold — discovered `holds_under` assumptions. The refinement is diagnostic-only — it
/// never changes the canonical verdict. Sidecar-free (operates on the same lift as the plain verb).
pub fn sv_verify_recoverability_refined(
    lift: &SvLift,
    target: &str,
    extra_predicates: &[PredicateSpec],
    config_specs: &[(String, Vec<u64>)],
    discover_assumptions: bool,
) -> Result<(PropertyVerdict, crate::verdict::VerdictRefinement), String> {
    parse_predicate_expr(target).map_err(|e| {
        format!("recoverability target `{target}` is not a register-comparison atom: {e:?}")
    })?;
    let btor2 = lift.lift()?;
    Ok(
        crate::adapter::recoverability::verify_recoverability_refined(
            &btor2,
            target,
            extra_predicates,
            config_specs,
            discover_assumptions,
        ),
    )
}

/// `sv check-fsm` — lift SV and auto-scan every FSM-like state register for a reachable
/// illegal encoding (no user input). The SV-direct peer of `btor2 check-fsm`.
pub fn sv_check_fsm(
    lift: &SvLift,
    max_width: u32,
) -> Result<Vec<crate::adapter::fsm_scan::FsmFinding>, String> {
    let btor2 = lift.lift()?;
    crate::adapter::fsm_scan::fsm_encoding_scan(&btor2, max_width)
}

/// Whether a lint finding names a state register (or its alias) or a
/// combinational output derived from one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SvLintSignalKind {
    /// A state register — the root of the finding: the RTL front-end left some of
    /// its bits undriven (modelled as free inputs).
    Register,
    /// A combinational output that reads such a register — flagged downstream of a
    /// `Register` finding (a property over it would be refused for the same reason).
    Output,
}

impl SvLintSignalKind {
    /// The stable lowercase tag used across the CLI JSON / API / UI surfaces.
    pub fn as_str(self) -> &'static str {
        match self {
            SvLintSignalKind::Register => "register",
            SvLintSignalKind::Output => "output",
        }
    }
}

/// One `sv lint` finding — a named signal whose lift the engine cannot keep
/// faithfully (monono#partsel): its cone reaches an ANONYMOUS free input, so a
/// state predicate over it would be *refused* (skipped) by the verifier rather
/// than mis-decided. See [`sv_lint_registers`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SvLintFinding {
    /// The offending signal's symbol (register name or the combinational output).
    pub signal: String,
    /// Whether `signal` is the register itself or a downstream output.
    pub kind: SvLintSignalKind,
}

/// `sv lint` — lift SV and report, at CI time (~lift cost, no bit-blast / no
/// model checking), every register whose partial-write lift the verifier can NOT
/// keep faithfully.
///
/// This is the read-only, cap-immune preflight for the monono#partsel soundness
/// refusal shipped in #464/#465: the yosys-slang lift of a plain-vector partial
/// register assignment (`q[hi:lo] <= d`) models `q`'s *unwritten* bits as free
/// `input`s (havoc) and aliases the register name to the `concat` mixing them, so
/// a state predicate over `q` reads those free inputs and would be unsound — the
/// verifier *refuses* (skips) such a property up front
/// ([`parser::signal_reaches_anonymous_input`]). `sv lint` surfaces exactly those
/// registers *before* the ~minutes-long formal gate runs, so a design whose lift
/// is unfaithful is caught in ~0.1 s. It changes **no verdict**: a faithful state
/// cell (read_verilog / sv2v) and a packed-2-D `q[idx] <= d` split (anonymous
/// *sub-registers*, no free inputs — the NID-indexed cone keeps them) are both
/// NOT flagged; the latter decides.
///
/// Findings are deterministic (sorted by name, deduplicated); a name that is both
/// a register alias and an output is reported once, as a `Register`.
pub fn sv_lint_registers(lift: &SvLift) -> Result<Vec<SvLintFinding>, String> {
    let btor2 = lift.lift()?;
    lint_undriven_partial_writes(&btor2)
}

/// The pure BTOR2 core of [`sv_lint_registers`] — parse the flattened design and
/// return every named register/output whose cone reaches an anonymous free input.
/// Split out so the regression suite can exercise it on a captured BTOR2 fixture
/// with no sv2v / Yosys on the host.
fn lint_undriven_partial_writes(btor2: &str) -> Result<Vec<SvLintFinding>, String> {
    use crate::adapter::btor2::ast::Node;
    use crate::adapter::btor2::parser::signal_reaches_anonymous_input;

    let file = parser::parse(btor2).map_err(|e| format!("BTOR2 parse: {e}"))?;

    // Candidate name universe = exactly the names `signal_reaches_anonymous_input`
    // can match on: State + Op(symbol) → a register (the partsel alias `a_q` is a
    // `uext`/`concat` Op carrying the register's name), Output(symbol) → a
    // combinational output. `Register` wins when a name appears as both.
    let mut kinds: std::collections::BTreeMap<String, SvLintSignalKind> =
        std::collections::BTreeMap::new();
    for line in &file.lines {
        let (name, kind) = match &line.node {
            Node::State {
                symbol: Some(s), ..
            }
            | Node::Op {
                symbol: Some(s), ..
            } => (s, SvLintSignalKind::Register),
            Node::Output {
                symbol: Some(s), ..
            } => (s, SvLintSignalKind::Output),
            _ => continue,
        };
        kinds
            .entry(name.clone())
            .and_modify(|k| {
                if kind == SvLintSignalKind::Register {
                    *k = SvLintSignalKind::Register;
                }
            })
            .or_insert(kind);
    }

    // BTreeMap keeps the output sorted-by-name (deterministic).
    Ok(kinds
        .into_iter()
        .filter(|(name, _)| signal_reaches_anonymous_input(&file, name))
        .map(|(signal, kind)| SvLintFinding { signal, kind })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lift_of(source: &str) -> SvLift {
        SvLift {
            source: source.to_string(),
            additional_sources: Vec::new(),
            top: None,
            use_sv2v: false,
            include_dirs: Vec::new(),
            frontend: SvFrontend::Auto,
        }
    }

    // The atom guards fire BEFORE the toolchain-gated lift, so they are testable in
    // make-ci (no sv2v / Yosys needed).
    #[test]
    fn liveness_rejects_malformed_atom_before_lift() {
        let e = sv_verify_liveness(&lift_of("module m; endmodule"), "x == y", "st == 1");
        assert!(
            e.is_err(),
            "a relational request atom must be rejected pre-lift"
        );
    }

    #[test]
    fn recoverability_rejects_malformed_target_before_lift() {
        let e = sv_verify_recoverability(&lift_of("module m; endmodule"), "not an atom !!");
        assert!(e.is_err(), "a malformed target must be rejected pre-lift");
    }

    // monono#partsel lint core — the yosys-slang lift of a plain-vector partial
    // register assignment (`a_q[11:8] <= val`), captured verbatim so the
    // regression runs in make-ci without slang. `a_q`'s unwritten bits are free
    // `input`s (nid 17, 21) mixed into the `concat` its name aliases; `o_partsel`
    // reads them. `b_q`/`c_q`/`d_q` (and their outputs) reach only the *named*
    // `val` input — faithful, never flagged. `sv lint` reports exactly the pair
    // the verifier would refuse: `a_q` (register) + `o_partsel` (output).
    const SLANG_PARTSEL_LIFT: &str = r#"1 sort bitvec 1
2 input 1 clk
3 one 1 rst_n
4 sort bitvec 4
5 input 4 val
6 sort bitvec 16
7 const 6 0000000000000000
8 state 6
9 ite 6 3 8 7
10 redor 1 9
11 output 10 o_concat
12 state 6
13 ite 6 3 12 7
14 redor 1 13
15 output 14 o_lowconcat
16 sort bitvec 8
17 input 16
18 const 4 0000
19 state 4
20 ite 4 3 19 18
21 input 4
22 sort bitvec 12
23 concat 22 20 17
24 concat 6 21 23
25 redor 1 24
26 output 25 o_partsel
27 state 6
28 ite 6 3 27 7
29 redor 1 28
30 output 29 o_plain
31 uext 6 24 0 a_q
32 uext 6 9 0 b_q
33 uext 6 13 0 c_q
34 uext 6 28 0 d_q
35 slice 16 9 7 0
36 concat 22 5 35
37 slice 4 9 15 12
38 concat 6 37 36
39 ite 6 3 38 7
40 next 6 8 39
41 const 22 000000000000
42 concat 6 41 5
43 ite 6 3 42 7
44 next 6 12 43
45 ite 4 3 5 18
46 next 4 19 45
47 ite 6 3 42 7
48 next 6 27 47
49 constd 6 0
50 init 6 8 49
51 constd 6 0
52 init 6 12 51
53 constd 4 0
54 init 4 19 53
55 constd 6 0
56 init 6 27 55
"#;

    #[test]
    fn lint_reports_only_the_undriven_partial_write_signals() {
        let findings =
            lint_undriven_partial_writes(SLANG_PARTSEL_LIFT).expect("lint parses the BTOR2");
        let names: Vec<&str> = findings.iter().map(|f| f.signal.as_str()).collect();
        // Exactly the register whose unwritten bits are free inputs, plus the one
        // output that reads it — deterministic, sorted, deduplicated.
        assert_eq!(
            names,
            vec!["a_q", "o_partsel"],
            "lint must flag the undriven partial-write register + its output, and \
             NOTHING faithful (b_q/c_q/d_q reach only the named `val`)"
        );
        assert_eq!(
            findings[0].kind,
            SvLintSignalKind::Register,
            "a_q is a register"
        );
        assert_eq!(
            findings[1].kind,
            SvLintSignalKind::Output,
            "o_partsel is an output"
        );
    }

    #[test]
    fn lint_is_clean_when_every_register_is_faithful() {
        // A fully-driven single register: no free inputs, no findings.
        const FAITHFUL: &str = "1 sort bitvec 1
2 input 1 d
3 state 1 q
4 next 1 3 2
5 zero 1
6 init 1 3 5
";
        let findings = lint_undriven_partial_writes(FAITHFUL).expect("lint parses");
        assert!(
            findings.is_empty(),
            "a faithful register driven by a named input must not be flagged; got {findings:?}"
        );
    }

    #[test]
    fn lint_rejects_malformed_btor2() {
        assert!(
            lint_undriven_partial_writes("not btor2 at all").is_err(),
            "a malformed BTOR2 body must surface a parse error, not an empty pass"
        );
    }

    // P2 industrial anchor — recoverability on REAL OpenTitan RTL. From every reachable
    // state the AES cipher-control FSM can return to CIPHER_CTRL_IDLE (= 6'b001001 = 9):
    // `AG EF (aes_cipher_ctrl_cs == 9)`, the branching νμ property SVA cannot express,
    // decides HOLDS end-to-end through the SV lift + the recoverability verb. NOTE: the
    // vendored design is the EXTRACTED control FSM (18 state bits — within the exact
    // engine's ~40-bit cap, so the exact engine decides it); the OVER-CAP cube +
    // smt-hyper-must scale path is proven separately by the wide-fixture differential
    // tests in `crate::adapter::recoverability` (an 80-bit design where the exact engine
    // abstains and the cube path decides both polarities).
    #[test]
    #[ignore = "requires sv2v + Yosys + z3 (mununu-sva docker image); run with --ignored"]
    fn e2e_opentitan_aes_cipher_ctrl_recoverability_holds() {
        use std::path::PathBuf;
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/verify/dc_opentitan_aes_cipher_control_fsm/source");
        let read = |n: &str| {
            std::fs::read_to_string(dir.join(n)).unwrap_or_else(|e| panic!("read {n}: {e}"))
        };
        let lift = SvLift {
            source: read("aes_cipher_control_fsm.sv"),
            additional_sources: vec![
                ("aes_pkg.sv".into(), read("aes_pkg.sv")),
                ("aes_reg_pkg.sv".into(), read("aes_reg_pkg.sv")),
                ("prim_util_pkg.sv".into(), read("prim_util_pkg.sv")),
                ("prim_assert.sv".into(), read("prim_assert.sv")),
            ],
            top: Some("aes_cipher_control_fsm".into()),
            use_sv2v: true,
            include_dirs: Vec::new(),
            frontend: SvFrontend::Auto,
        };
        let verdict = sv_verify_recoverability(&lift, "aes_cipher_ctrl_cs == 9")
            .expect("recoverability decides on the AES cipher-control FSM");
        assert_eq!(
            verdict,
            PropertyVerdict::Holds,
            "AG EF idle must HOLD on the AES cipher-control FSM (every reachable state can \
             return to CIPHER_CTRL_IDLE); got {verdict:?}"
        );
    }

    // monono#partsel — the real slang lift of a plain-vector partial register
    // assignment. `a_q[11:8] <= val` leaves `a_q`'s other bits undriven → slang
    // models them as free inputs → `sv lint` flags `a_q`; `b_q` is written in full
    // → faithful → not flagged. The `--frontend slang` path is what produces the
    // free-input shape (read_verilog/sv2v models it differently), so this pins the
    // slang front end. Runs only in the mununu-sva image.
    #[test]
    #[ignore = "requires yosys-slang (mununu-sva docker image); run with --ignored"]
    fn e2e_sv_lint_flags_slang_partial_write_register() {
        const SRC: &str = r#"module partsel_lint (
  input  logic       clk,
  input  logic       rst_n,
  input  logic [3:0] val
);
  logic [15:0] a_q;  // only [11:8] written -> unwritten bits are free inputs (slang)
  logic [15:0] b_q;  // written in full -> faithful state cell
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      a_q <= '0;
      b_q <= '0;
    end else begin
      a_q[11:8] <= val;
      b_q       <= {12'b0, val};
    end
  end
endmodule
"#;
        let lift = SvLift {
            source: SRC.to_string(),
            additional_sources: Vec::new(),
            top: Some("partsel_lint".into()),
            use_sv2v: false,
            include_dirs: Vec::new(),
            frontend: SvFrontend::Slang,
        };
        let findings = sv_lint_registers(&lift).expect("lint lifts + scans the design");
        let names: Vec<&str> = findings.iter().map(|f| f.signal.as_str()).collect();
        assert!(
            names.contains(&"a_q"),
            "the partial-write register a_q must be flagged; got {names:?}"
        );
        assert!(
            !names.contains(&"b_q"),
            "the fully-written register b_q is faithful and must NOT be flagged; got {names:?}"
        );
    }

    // The full lift → verdict path needs sv2v + Yosys; covered by the e2e suite in
    // the mununu-sva image, not make-ci.
}
