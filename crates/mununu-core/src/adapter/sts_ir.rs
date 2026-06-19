//! Symbolic Transition System IR — the frontend-agnostic abstraction seam.
//!
//! > Status: planning / DR0 stub (IR-unification track). Design:
//! > `docs/design/sts-ir.md`. This module defines the *interface* the two
//! > abstraction engines need from a symbolic transition system, plus one
//! > implementation ([`BtorSts`]) that wraps a parsed BTOR2 file by
//! > delegating to existing, already-shipped functions. **No existing
//! > call site is rewired by DR0** — `bit_blast` and `predicate_cube_lift`
//! > still talk to BTOR2 directly; this seam exists to prove the
//! > abstraction is expressible without leaking BTOR2/Z3 types, and to be
//! > the consumption point that P1 reroutes the engines onto.
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
        use crate::adapter::btor2::smt_must_edge::{
            SmtMayVerdict, build_register_nid_map, smt_per_target_may_check,
        };
        use crate::adapter::sidecar::predicate_image::btor2_encode::encode_design;

        if predicates.is_empty() {
            return Vec::new();
        }
        // DR0 uses the BvOnly `encode_design`; P1 swaps in the
        // memory-aware `encode_design_for_lift` (array theory) — see
        // `docs/design/sts-ir.md` §"Z3 scope".
        let n_cubes = 1usize << predicates.len();
        let cfg = z3::Config::new();
        z3::with_z3_config(&cfg, || {
            let view = match encode_design(self.file) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1-bit register `q` with `q' = en`, plus an input `en`. Exercises
    // both the state/input metadata and the concrete-step delegation.
    const STEP_BTOR2: &str =
        "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 input 1 en\n6 next 1 3 5\n";

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
}
