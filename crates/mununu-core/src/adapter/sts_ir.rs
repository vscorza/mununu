//! Symbolic Transition System IR — the frontend-agnostic abstraction seam.
//!
//! > Status: planning / P0 canonical seam (IR-unification track). Design:
//! > `docs/design/sts-ir.md`. This module defines the *interface* the two
//! > abstraction engines need from a symbolic transition system, plus one
//! > implementation ([`BtorSts`]) that wraps a parsed BTOR2 file by
//! > delegating to existing, already-shipped functions. **No existing
//! > call site is rewired yet** — `bit_blast` and `predicate_cube_lift`
//! > still talk to BTOR2 directly; P1 reroutes them onto this seam. P0
//! > makes the seam the *canonical, faithful* interface: register-name
//! > resolution ([`SymbolicTransitionSystem::resolve_register`], the home
//! > of the DR1 #1 blocker fix) and a memory-aware SMT encode (array
//! > theory), so P1 can consume it without behaviour drift.
//!
//! The seam is two traits over a shared metadata trait:
//!
//! - [`StepEval`] — concrete one-step semantics. The *explicit /
//!   Enumerate* edge-strategy (today's `bit_blast`) needs only this.
//! - [`SmtEncode`] — SMT predicate-image. The *predicate-cube / SmtImage*
//!   edge-strategy (today's `predicate_cube_lift`) needs this.
//!
//! Neither trait surface names a BTOR2 or Z3 type: state/input structure
//! is [`StsVar`] (name + width); the concrete step is name-keyed
//! `HashMap<String, u128>`; the SMT predicate-image is expressed over
//! [`PredicateSpec`] (a frontend-agnostic `{name, register, value}`) and
//! returns plain cube-index pairs. A future non-RTL frontend that can
//! emit these two semantics inherits both abstraction policies + the
//! KMTS evaluator + CEGAR with no new abstraction code (the P4/P5 goal).

use std::collections::HashMap;

use crate::adapter::AdapterError;
use crate::adapter::btor2::ast::{Btor2File, Node};
use crate::adapter::btor2::kmts_lift::PredicateSpec;
use crate::adapter::btor2::{bit_blast, parser};

/// A typed state or input variable of a symbolic transition system.
/// Frontend-agnostic: just a name and a bit width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StsVar {
    /// The variable's symbol (e.g. a register name `u_chan0.prediv_q`).
    pub name: String,
    /// Bit width.
    pub width: u32,
}

/// Structure of a symbolic transition system — the metadata both
/// abstraction engines need before they can lift.
pub trait SymbolicTransitionSystem {
    /// State variables (registers / latches), sorted by name.
    fn state_vars(&self) -> Vec<StsVar>;
    /// Free input variables, sorted by name.
    fn input_vars(&self) -> Vec<StsVar>;
    /// P0 (IR-unification track) — resolve a user-facing register name
    /// (which may be an *alias*, e.g. the `_q` flavour Yosys leaves on a
    /// `uext` node after flatten strips the symbol from the state line)
    /// to the **canonical state-cell name** that the SMT predicate-image
    /// binds against (the `_d` symbol the SMT view keys on). Returns
    /// `None` when the name resolves to no state cell, or the resolved
    /// cell carries no symbol.
    ///
    /// This is the architectural home of the DR1 #1 blocker fix: the
    /// predicate-cube path can use this so a sidecar predicate over
    /// `bit_cnt_q` binds to the real counter register even when the
    /// state line is labelled `bit_cnt_d`. The `bit_blast`/sidecar path
    /// already does the equivalent BFS via `resolve_state_by_symbol` +
    /// the `drives` override; surfacing it here lets the cube path share
    /// the same resolution (P1 wires it into `predicate_cube_lift`).
    fn resolve_register(&self, name: &str) -> Option<String>;
}

/// Concrete one-step semantics: given a full assignment of state vars +
/// inputs, compute the next state assignment. This is everything the
/// *explicit / Enumerate* edge-strategy (today's `bit_blast`) needs to
/// build a Sharp CLTS.
pub trait StepEval: SymbolicTransitionSystem {
    /// Step the design one clock: `(state, inputs) ↦ next_state`. Both
    /// maps are keyed by [`StsVar::name`]; absent keys default to 0
    /// (the `setundef -zero` convention).
    fn step(
        &self,
        state: &HashMap<String, u128>,
        inputs: &HashMap<String, u128>,
    ) -> Result<HashMap<String, u128>, AdapterError>;
}

/// SMT predicate-image: the *predicate-cube / SmtImage* edge-strategy
/// (today's `predicate_cube_lift`). Batched per the Z3-scope-reuse
/// consideration in `docs/design/sts-ir.md` §"Z3 scope".
pub trait SmtEncode: SymbolicTransitionSystem {
    /// Sound over-approximating may-relation over predicate cubes.
    ///
    /// Cubes are indexed `0..2^|predicates|`; bit `i` of a cube index is
    /// the polarity of `predicates[i]` (identical encoding to
    /// `predicate_cube_lift`). Returns every `(src, tgt)` pair for which
    /// a concrete witness `∃ s ⊨ src, s' ⊨ tgt. (s, s') ∈ R` exists — a
    /// pair is **excluded only when the SMT backend proves it
    /// impossible**, so the relation is a sound over-approximation
    /// (timeouts / unresolved predicates conservatively keep the edge).
    ///
    /// The must-relation (`∀∀` / `∀∃` / hyper-must) follows the same
    /// shape over the same encoding (`smt_must_edge::smt_per_target_must_*`);
    /// DR0 ships only the may-relation to prove the seam.
    fn may_edges(&self, predicates: &[PredicateSpec], timeout_ms: u32) -> Vec<(usize, usize)>;

    /// P1 #3 (IR-unification track) — sound under-approximating
    /// **must-relation** over predicate cubes, the canonical ∀∃ KMTS
    /// must per Bruns–Godefroid CONCUR 2000:
    ///
    /// ```text
    /// (src, tgt) ∈ R_must  ⟺  ∀ s ⊨ src. ∃ inputs, s'. (s, s') ∈ R ∧ s' ⊨ tgt
    /// ```
    ///
    /// Same cube encoding as [`SmtEncode::may_edges`] (indices
    /// `0..2^|predicates|`). A pair is **included only when the SMT
    /// backend proves the ∀∃ obligation** (UNSAT of its negation), so the
    /// relation is a sound under-approximation: timeouts / unresolved
    /// predicates / encoder failure conservatively *drop* the edge (never
    /// fabricate a must-witness). By construction `R_must ⊆ R_may` (∀∃ ⟹
    /// ∃), so callers can promote each returned pair from `MayOnly` to
    /// `Sharp`.
    ///
    /// The ∀∃ *standard* form is the default companion to `may_edges`;
    /// the stricter ∀∀ form and the generalised hyper-must form remain
    /// available directly via `smt_must_edge::smt_per_target_must_check`
    /// / `smt_hyper_must_check` on the sampling-candidate path.
    fn must_edges(&self, predicates: &[PredicateSpec], timeout_ms: u32) -> Vec<(usize, usize)>;
}

/// The BTOR2 implementation of the STS-IR seam — a thin borrow over a
/// parsed [`Btor2File`]. Every method delegates to an already-shipped
/// function, so DR0 changes no behaviour and rewires no call site.
pub struct BtorSts<'a> {
    file: &'a Btor2File,
}

impl<'a> BtorSts<'a> {
    /// Wrap a parsed BTOR2 file as a symbolic transition system.
    pub fn new(file: &'a Btor2File) -> Self {
        Self { file }
    }

    fn vars_of(&self, want_state: bool) -> Vec<StsVar> {
        let symbols = parser::collect_symbols(self.file);
        let mut out: Vec<StsVar> = self
            .file
            .lines
            .iter()
            .filter_map(|line| {
                let (sort, is_state) = match &line.node {
                    Node::State { sort, .. } => (*sort, true),
                    Node::Input { sort, .. } => (*sort, false),
                    _ => return None,
                };
                if is_state != want_state {
                    return None;
                }
                let name = symbols.get(&line.nid)?.clone();
                let width = parser::bv_width(self.file, sort)?;
                Some(StsVar { name, width })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup();
        out
    }
}

impl SymbolicTransitionSystem for BtorSts<'_> {
    fn state_vars(&self) -> Vec<StsVar> {
        self.vars_of(true)
    }

    fn input_vars(&self) -> Vec<StsVar> {
        self.vars_of(false)
    }

    fn resolve_register(&self, name: &str) -> Option<String> {
        // BFS-backward from any node carrying `name` (direct state match,
        // or an Op / Output alias) to the nearest state cell, then return
        // that cell's own symbol — the name the SMT view + predicate-image
        // bind against. Mirrors the sidecar resolver's `drives` path.
        let nid = parser::resolve_state_by_symbol(self.file, name)?;
        parser::collect_symbols(self.file).get(&nid).cloned()
    }
}

impl StepEval for BtorSts<'_> {
    fn step(
        &self,
        state: &HashMap<String, u128>,
        inputs: &HashMap<String, u128>,
    ) -> Result<HashMap<String, u128>, AdapterError> {
        // Delegate to the shipped concrete-step evaluator unchanged.
        bit_blast::simulate_one_step(self.file, state, inputs)
    }
}

impl SmtEncode for BtorSts<'_> {
    fn may_edges(&self, predicates: &[PredicateSpec], timeout_ms: u32) -> Vec<(usize, usize)> {
        use crate::adapter::btor2::kmts_lift::encode_design_for_lift;
        use crate::adapter::btor2::smt_must_edge::{
            SmtMayVerdict, build_register_nid_map, smt_per_target_may_check,
        };

        if predicates.is_empty() {
            return Vec::new();
        }
        // P0 — faithful, memory-aware encode (array theory for `$mem`
        // cells), matching the production `predicate_cube_lift`. (DR0 used
        // the BvOnly `encode_design`.)
        let n_cubes = 1usize << predicates.len();
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = match encode_design_for_lift(self.file) {
                Ok(v) => v,
                // Encoder can't build the view (e.g. an unsupported op):
                // no may-edges rather than an unsound guess. Mirrors the
                // predicate_cube_lift fallback.
                Err(_) => return Vec::new(),
            };
            let nid_map = build_register_nid_map(&view);
            let mut edges = Vec::new();
            for i in 0..n_cubes {
                for j in 0..n_cubes {
                    if matches!(
                        smt_per_target_may_check(
                            &view, i as u64, j as u64, predicates, &nid_map, timeout_ms,
                        ),
                        SmtMayVerdict::May
                    ) {
                        edges.push((i, j));
                    }
                }
            }
            edges
        })
    }

    fn must_edges(&self, predicates: &[PredicateSpec], timeout_ms: u32) -> Vec<(usize, usize)> {
        use crate::adapter::btor2::kmts_lift::encode_design_for_lift;
        use crate::adapter::btor2::smt_must_edge::{
            SmtMustVerdict, build_register_nid_map, smt_per_target_must_check_standard,
        };

        if predicates.is_empty() {
            return Vec::new();
        }
        // Mirrors `may_edges` exactly — same faithful memory-aware encode,
        // same cube encoding, same all-pairs sweep — but runs the ∀∃
        // standard must-check and keeps a pair only on a definite `Must`
        // (UNSAT) verdict. NotMust / Unknown / encoder failure drop the
        // edge (sound under-approximation: never fabricate a must-witness).
        let n_cubes = 1usize << predicates.len();
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = match encode_design_for_lift(self.file) {
                Ok(v) => v,
                Err(_) => return Vec::new(),
            };
            let nid_map = build_register_nid_map(&view);
            let mut edges = Vec::new();
            for i in 0..n_cubes {
                for j in 0..n_cubes {
                    if matches!(
                        smt_per_target_must_check_standard(
                            &view, i as u64, j as u64, predicates, &nid_map, timeout_ms,
                        ),
                        SmtMustVerdict::Must
                    ) {
                        edges.push((i, j));
                    }
                }
            }
            edges
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1-bit register `q` with `q' = en`, plus an input `en`. Exercises
    // both the state/input metadata and the concrete-step delegation.
    const STEP_BTOR2: &str =
        "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 input 1 en\n6 next 1 3 5\n";

    // P0 — a 4-bit state cell labelled `cnt_d` on the state line, with the
    // `cnt_q` flavour surviving only on a `uext` alias. This mirrors the
    // uart_tx pattern where Yosys' flatten strips the `_q` symbol from the
    // state line (the DR1 #1 blocker).
    const ALIASED_BTOR2: &str = "1 sort bitvec 4\n2 zero 4\n3 state 4 cnt_d\n4 init 4 3 2\n5 uext 4 3 0 cnt_q\n6 next 4 3 2\n";

    #[test]
    fn btor_sts_resolve_register_follows_uext_alias_to_state_cell() {
        // The DR1 #1 blocker fix, homed in the seam: a predicate over the
        // alias `cnt_q` resolves to the canonical state-cell name `cnt_d`.
        let file = parser::parse(ALIASED_BTOR2).expect("parse");
        let sts = BtorSts::new(&file);
        assert_eq!(sts.resolve_register("cnt_q").as_deref(), Some("cnt_d"));
        // The canonical name resolves to itself.
        assert_eq!(sts.resolve_register("cnt_d").as_deref(), Some("cnt_d"));
        // A name matching no state cell does not resolve.
        assert_eq!(sts.resolve_register("nonexistent"), None);
    }

    #[test]
    fn btor_sts_reports_state_and_input_vars() {
        let file = parser::parse(STEP_BTOR2).expect("parse");
        let sts = BtorSts::new(&file);
        assert_eq!(
            sts.state_vars(),
            vec![StsVar {
                name: "q".into(),
                width: 1
            }]
        );
        assert_eq!(
            sts.input_vars(),
            vec![StsVar {
                name: "en".into(),
                width: 1
            }]
        );
    }

    #[test]
    fn btor_sts_step_delegates_to_simulate_one_step() {
        let file = parser::parse(STEP_BTOR2).expect("parse");
        let sts = BtorSts::new(&file);
        // q' = en: with en=1, q goes 0 → 1.
        let next = sts
            .step(
                &HashMap::from([("q".to_string(), 0u128)]),
                &HashMap::from([("en".to_string(), 1u128)]),
            )
            .expect("step");
        assert_eq!(next.get("q"), Some(&1u128));
        // en=0 keeps q at 0.
        let next0 = sts
            .step(
                &HashMap::from([("q".to_string(), 1u128)]),
                &HashMap::from([("en".to_string(), 0u128)]),
            )
            .expect("step");
        assert_eq!(next0.get("q"), Some(&0u128));
    }

    #[test]
    fn btor_sts_may_edges_match_predicate_image() {
        // Toggle: `q' = !q`. Predicate `q == 1` → cube_0 = {q=0},
        // cube_1 = {q=1}. The transition relation forces 0→1 and 1→0, so
        // the sound may-relation is exactly {(0,1),(1,0)} — never the
        // self-loops (0,0)/(1,1).
        let toggle =
            "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 not 1 3\n6 next 1 3 5\n";
        let file = parser::parse(toggle).expect("parse");
        let sts = BtorSts::new(&file);
        let preds = vec![PredicateSpec {
            name: "q_is_1".into(),
            register: "q".into(),
            value: 1,
        }];
        let edges = sts.may_edges(&preds, 5_000);
        assert!(edges.contains(&(0, 1)), "0→1 must be a may-edge: {edges:?}");
        assert!(edges.contains(&(1, 0)), "1→0 must be a may-edge: {edges:?}");
        assert!(
            !edges.contains(&(0, 0)) && !edges.contains(&(1, 1)),
            "self-loops are provably impossible: {edges:?}"
        );
    }

    #[test]
    fn btor_sts_must_edges_match_predicate_image() {
        // P1 #3 — same deterministic toggle `q' = !q`. The transition is
        // a function (no inputs), so the ∀∃ must-relation coincides with
        // the may-relation: 0→1 and 1→0 are forced, the self-loops are
        // provably never must. Confirms the seam's must companion is sound
        // and the cube encoding matches `may_edges`.
        let toggle =
            "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 not 1 3\n6 next 1 3 5\n";
        let file = parser::parse(toggle).expect("parse");
        let sts = BtorSts::new(&file);
        let preds = vec![PredicateSpec {
            name: "q_is_1".into(),
            register: "q".into(),
            value: 1,
        }];
        let must = sts.must_edges(&preds, 5_000);
        assert!(must.contains(&(0, 1)), "0→1 must be a must-edge: {must:?}");
        assert!(must.contains(&(1, 0)), "1→0 must be a must-edge: {must:?}");
        assert!(
            !must.contains(&(0, 0)) && !must.contains(&(1, 1)),
            "self-loops are never must (the toggle never stays): {must:?}"
        );
        // R_must ⊆ R_may by construction.
        let may: std::collections::HashSet<_> = sts.may_edges(&preds, 5_000).into_iter().collect();
        for e in &must {
            assert!(may.contains(e), "must-edge {e:?} must also be a may-edge");
        }
    }
}
