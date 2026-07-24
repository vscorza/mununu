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

use crate::adapter::btor2::ast::Btor2File;
use crate::adapter::btor2::bit_blast::{MemoryCellMeta, detect_btor2_memories};
use crate::adapter::sidecar::predicate_image::btor2_encode::{
    Btor2SmtView, EncodeError, encode_design_with_theory,
};
use crate::adapter::sidecar::predicate_image::theory::Theory;

/// Lazy, memoized structural derivations of one BTOR2 model. Cheap to construct
/// (`new` stores the borrow and empty cells); each accessor computes its fact at most
/// once for the lifetime of the value.
pub(crate) struct ModelFacts<'m> {
    model: &'m Btor2File,
    memories: OnceLock<Vec<MemoryCellMeta>>,
    theory: OnceLock<Theory>,
}

impl<'m> ModelFacts<'m> {
    pub(crate) fn new(model: &'m Btor2File) -> Self {
        Self {
            model,
            memories: OnceLock::new(),
            theory: OnceLock::new(),
        }
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
}
