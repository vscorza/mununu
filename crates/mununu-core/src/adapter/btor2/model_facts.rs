//! `ModelFacts` — the lazy, model-anchored derivation cache (verification-execution-planner
//! Phase 0.a).
//!
//! The predicate-cube edge relations (may / must / hyper-must) and the recoverability
//! ranking each re-derive structural facts about the SAME fixed BTOR2 model — most
//! visibly the theory selection `detect_btor2_memories(file)` inside
//! [`crate::adapter::btor2::kmts_lift::encode_design_for_lift`], which runs once per
//! `encode_*` call (three-plus times per lift, and again on every CEGAR refinement
//! iteration). Those facts are pure functions of the model and do not change while the
//! predicate set is refined, so recomputing them is wasted work.
//!
//! `ModelFacts` borrows a parsed model and computes each derivation **on first access,
//! memoized** (`OnceLock`). It is the first increment of the plan's lazy derivation
//! layer: cheap structural facts live here now; the heavier engine projections
//! (bit-blast / cube / CHC) join the same memo in later increments. Anchored to
//! `Btor2File` (the real shared waist today); the anchor migrates to STS-IR when the hub
//! subsumes it. A model transform (cutpoint / config-pin) produces a *new* model and
//! hence a fresh cache, so the memo never goes stale.

use std::sync::OnceLock;

use crate::adapter::btor2::ast::{Btor2File, Nid};
use crate::adapter::btor2::bit_blast::{MemoryCellMeta, detect_btor2_memories};
use crate::adapter::btor2::dep_graph::cone_leaf_nids;
use crate::adapter::btor2::symbolic_bitblast::effective_bitblast_cap;
use crate::adapter::sidecar::predicate_image::btor2_encode::{
    Btor2SmtView, EncodeError, encode_design_with_theory,
};
use crate::adapter::sidecar::predicate_image::theory::Theory;
use crate::adapter::sts_ir::BtorSts;

/// A pinnable primary INPUT — a free `input` leaf `(nid, name, width)`. The candidate
/// surface for auto-config-value (P2.2b): pinning one to a constant removes its `width` bits
/// from every cone it feeds, shrinking the exact engine's bit-blast toward the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputFact {
    pub nid: Nid,
    pub name: String,
    pub width: u32,
}

/// Lazy, memoized structural derivations of one BTOR2 model. Cheap to construct
/// (`new` stores the borrow and empty cells); each accessor computes its fact at most
/// once for the lifetime of the value.
pub(crate) struct ModelFacts<'m> {
    model: &'m Btor2File,
    memories: OnceLock<Vec<MemoryCellMeta>>,
    theory: OnceLock<Theory>,
    total_bits: OnceLock<u32>,
    free_inputs: OnceLock<Vec<InputFact>>,
}

impl<'m> ModelFacts<'m> {
    pub(crate) fn new(model: &'m Btor2File) -> Self {
        Self {
            model,
            memories: OnceLock::new(),
            theory: OnceLock::new(),
            total_bits: OnceLock::new(),
            free_inputs: OnceLock::new(),
        }
    }

    /// The design's state + input leaf cells (canonical seam `BtorSts::leaf_cells`), or an
    /// empty vec if the model is malformed (facts are best-effort, never fatal).
    fn leaf_cells(&self) -> Vec<crate::adapter::sts_ir::LeafCell> {
        BtorSts::new(self.model).leaf_cells().unwrap_or_default()
    }

    /// Whole-design bit-blast width — the sum of ALL state+input leaf widths (the size the
    /// exact engine faces when NO cone restriction applies, `keep = None`). Memoized.
    pub(crate) fn total_bits(&self) -> u32 {
        *self
            .total_bits
            .get_or_init(|| self.leaf_cells().iter().map(|c| c.width).sum())
    }

    /// The exact engine's cone-of-influence bit width for a property's `seed_atoms` (the
    /// register names from
    /// [`crate::adapter::btor2::symbolic_bitblast::formula_seed_atoms`]) — the register+input
    /// bits it must bit-blast. Mirrors the engine's keep-set cone (`cone_leaf_nids`); an
    /// empty atom set ⇒ the whole design (`keep = None`). Not memoized (varies per atom set).
    pub(crate) fn cone_bits(&self, seed_atoms: &[String]) -> u32 {
        if seed_atoms.is_empty() {
            return self.total_bits();
        }
        let cone = cone_leaf_nids(self.model, seed_atoms);
        self.leaf_cells()
            .iter()
            .filter(|c| cone.contains(&c.nid))
            .map(|c| c.width)
            .sum()
    }

    /// `(cone_bits, cap)` for a property's seed atoms; `cone_bits > cap` is exactly the
    /// condition under which the exact engine bails to a `Skip` — the signal auto-config-value
    /// / auto-cutpoint act on.
    pub(crate) fn cone_vs_cap(&self, seed_atoms: &[String]) -> (u32, u32) {
        let cb = self.cone_bits(seed_atoms);
        (cb, effective_bitblast_cap(cb))
    }

    /// The pinnable primary-input surface — all free `input` leaves, widest first. Memoized.
    pub(crate) fn free_inputs(&self) -> &[InputFact] {
        self.free_inputs.get_or_init(|| {
            let mut v: Vec<InputFact> = self
                .leaf_cells()
                .iter()
                .filter(|c| !c.is_state)
                .map(|c| InputFact {
                    nid: c.nid,
                    name: c.name.clone(),
                    width: c.width,
                })
                .collect();
            // Widest-first (then by nid) — the greedy auto-config-value order (pin the input
            // that removes the most cone bits first).
            v.sort_by(|a, b| b.width.cmp(&a.width).then(a.nid.cmp(&b.nid)));
            v
        })
    }

    /// The free inputs that lie IN the cone of `seed_atoms` — the auto-config-value candidates
    /// (pinning an out-of-cone input cannot shrink this cone). Widest first.
    pub(crate) fn pinnable_cone_inputs(&self, seed_atoms: &[String]) -> Vec<InputFact> {
        if seed_atoms.is_empty() {
            return self.free_inputs().to_vec();
        }
        let cone = cone_leaf_nids(self.model, seed_atoms);
        self.free_inputs()
            .iter()
            .filter(|i| cone.contains(&i.nid))
            .cloned()
            .collect()
    }

    /// Inferred `$mem` / array memory cells (`detect_btor2_memories`), computed once.
    pub(crate) fn memories(&self) -> &[MemoryCellMeta] {
        self.memories
            .get_or_init(|| detect_btor2_memories(self.model))
    }

    /// The SMT theory the edge encoder should use — `BvUfArray` iff the model has any
    /// inferred memory, else `BvOnly`. Identical selection to `encode_design_for_lift`,
    /// but the underlying `detect_btor2_memories` runs at most once per model.
    pub(crate) fn theory(&self) -> Theory {
        *self.theory.get_or_init(|| {
            if self.memories().is_empty() {
                Theory::BvOnly
            } else {
                Theory::BvUfArray
            }
        })
    }

    /// Encode the design for the SMT edge relations, reusing the memoized theory.
    /// Verdict-equivalent to `encode_design_for_lift` (same theory, same encoder); the
    /// theory selection is not recomputed per call.
    pub(crate) fn encode(&self) -> Result<Btor2SmtView, EncodeError> {
        encode_design_with_theory(self.model, self.theory())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser;

    #[test]
    fn model_facts_theory_matches_direct_detection_and_memoizes() {
        // A memory-free design ⇒ BvOnly; the memoized value equals a direct detection.
        const MEM_FREE: &str =
            "1 sort bitvec 1\n2 zero 1\n3 state 1 x\n4 init 1 3 2\n5 not 1 3\n6 next 1 3 5\n";
        let file = parser::parse(MEM_FREE).expect("parse");
        let facts = ModelFacts::new(&file);
        assert!(
            facts.memories().is_empty(),
            "the toy design has no inferred memory"
        );
        assert_eq!(facts.theory(), Theory::BvOnly);
        // Second reads return the same memoized results (no divergence).
        assert!(facts.memories().is_empty());
        assert_eq!(facts.theory(), Theory::BvOnly);
        // The memoized memory set equals a fresh direct detection.
        assert_eq!(facts.memories().len(), detect_btor2_memories(&file).len());
    }

    // A design with an 8-bit counter `ctrl` (next = ctrl+1, self-contained cone), an 8-bit
    // register `other` fed by an 8-bit input `cfg`, and a 1-bit input `en`.
    const COST_FIXTURE: &str = "\
1 sort bitvec 8
2 sort bitvec 1
3 state 1 ctrl
4 one 1
5 add 1 3 4
6 next 1 3 5
7 input 1 cfg
8 input 2 en
9 state 1 other
10 next 1 9 7
";

    #[test]
    fn total_bits_sums_all_leaf_widths() {
        let file = parser::parse(COST_FIXTURE).expect("parse");
        let facts = ModelFacts::new(&file);
        // ctrl(8) + cfg(8) + en(1) + other(8) = 25.
        assert_eq!(facts.total_bits(), 25);
        // Memoized: second read agrees.
        assert_eq!(facts.total_bits(), 25);
    }

    #[test]
    fn free_inputs_are_inputs_only_widest_first() {
        let file = parser::parse(COST_FIXTURE).expect("parse");
        let facts = ModelFacts::new(&file);
        let names: Vec<(&str, u32)> = facts
            .free_inputs()
            .iter()
            .map(|i| (i.name.as_str(), i.width))
            .collect();
        // Only the two inputs (not the state registers), widest first.
        assert_eq!(names, vec![("cfg", 8), ("en", 1)]);
    }

    #[test]
    fn cone_bits_restricts_to_the_property_cone() {
        let file = parser::parse(COST_FIXTURE).expect("parse");
        let facts = ModelFacts::new(&file);
        // Empty atoms ⇒ the whole design (matches the exact engine's keep = None).
        assert_eq!(facts.cone_bits(&[]), facts.total_bits());
        // `ctrl`'s cone is self-contained (next = ctrl+1) ⇒ just its own 8 bits; `cfg`/`en`/
        // `other` are excluded. Cone restriction genuinely shrinks the count.
        let ctrl_cone = facts.cone_bits(&["ctrl".to_string()]);
        assert!(
            ctrl_cone < facts.total_bits(),
            "ctrl's cone ({ctrl_cone}) must exclude the unrelated leaves"
        );
        assert_eq!(ctrl_cone, 8, "ctrl's cone is ctrl alone");
        // `other`'s cone pulls in `cfg` (other' = cfg) ⇒ 8 + 8 = 16, and `cfg` is a pinnable
        // in-cone input while none is in ctrl's cone.
        assert_eq!(facts.cone_bits(&["other".to_string()]), 16);
        assert_eq!(
            facts
                .pinnable_cone_inputs(&["other".to_string()])
                .iter()
                .map(|i| i.name.clone())
                .collect::<Vec<_>>(),
            vec!["cfg".to_string()]
        );
        assert!(facts.pinnable_cone_inputs(&["ctrl".to_string()]).is_empty());
    }
}
