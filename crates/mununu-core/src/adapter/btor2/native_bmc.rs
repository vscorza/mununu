//! Native BMC — in-process **bounded model checking** on the Z3
//! transition-relation seam ([`crate::adapter::sidecar::predicate_image::btor2_encode`]).
//!
//! This is the first slice of P1's *in-house* scalable-safety back-end: mununu's
//! OWN scalable safety verdict, no subprocess (no btormc/Pono dependency), running
//! everywhere Z3 links (make-ci / mununu-dev). It unrolls the BTOR2 transition
//! relation `k` steps in Z3 (QF_BV) and asks "is a `bad` reachable within `k`
//! steps?":
//!
//! - **SAT** ⇒ a concrete init→bad execution exists ⇒ [`BmcOutcome::Violated`] at
//!   that depth — a **sound** counterexample w.r.t. the BTOR2 model.
//! - **UNSAT within `k`** ⇒ [`BmcOutcome::NoCexWithin`] — **bounded**, NOT a proof
//!   of safety (that is k-induction / IMC, the follow-up slices).
//!
//! # Why it matters
//!
//! No 40-bit cone cap (Z3 QF_BV, not BDD), so it decides reachable-`bad` on wide
//! datapaths the exact engine abstains on — the scale win the P1 anchors
//! (`synth_pipeline(W)`, HWMCC) demand. It is bit-precise, so a `Violated` verdict
//! is cross-checkable against the exact engine + btormc under the differential
//! oracle.
//!
//! # Soundness
//!
//! BMC only ever claims **Violated** (a reachable `bad`), never **Safe** — so it
//! cannot produce a spurious safety proof. A `Violated` is `Init(s⁰) ∧ ⋀ⱼ T(sʲ,sʲ⁺¹)
//! ∧ ⋀ⱼ C(sʲ) ∧ Bad(sᵏ)` being SAT: a concrete run from an initial state, honouring
//! every `constraint`, reaching `bad`. State cells **without** an `init` line are
//! left free at frame 0 — that is exactly BTOR2's semantics (an init-less state is
//! nondeterministic), so a free-init counterexample is a real run of the model.
//! (The SAFE direction's free-init subtlety that the exact engine guards against
//! does not arise here — BMC never asserts SAFE.)
//!
//! # Scope (slice 1)
//!
//! Pure-bitvector (`Theory::BvOnly`) designs. An array/memory design (HWMCC memory
//! track) currently returns [`BmcError::Encode`]; wiring the `BvUfArray` theory is
//! a follow-up. k-induction (unbounded SAFE proof) and IMC are the next slices.

use std::collections::HashMap;

use crate::adapter::btor2::ast::{Btor2File, Nid, Node};
use crate::adapter::sidecar::predicate_image::btor2_encode::{EncodeError, encode_design};
use z3::ast::{Ast, BV, Bool};

/// The outcome of a bounded model-checking run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmcOutcome {
    /// A `bad` is reachable at exactly `depth` steps from an initial state — a
    /// sound counterexample. `depth = 0` means `bad` holds in an initial state.
    Violated { depth: u32 },
    /// No `bad` is reachable within `k` steps. **Bounded** — this is NOT a proof
    /// of safety (a deeper counterexample may exist; k-induction / IMC decide that).
    NoCexWithin { k: u32 },
}

/// Why a BMC run could not produce a verdict.
#[derive(Debug, Clone)]
pub enum BmcError {
    /// The design could not be encoded to a Z3 QF_BV transition relation (an
    /// unsupported op, or an array/memory sort — the `BvUfArray` follow-up).
    Encode(EncodeError),
    /// The design declares no `bad` property — nothing to check.
    NoBadProperty,
}

impl std::fmt::Display for BmcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BmcError::Encode(e) => write!(f, "native BMC: encode failed: {e:?}"),
            BmcError::NoBadProperty => write!(f, "native BMC: design has no `bad` property"),
        }
    }
}

/// Bounded model check: is a `bad` reachable within `max_k` steps of the BTOR2
/// transition relation? See [`BmcOutcome`] for the verdict semantics.
///
/// Runs entirely in-process on Z3 (no subprocess). `max_k` bounds the unrolling
/// depth; the returned `Violated { depth }` is the SHALLOWEST counterexample
/// (frames are checked in increasing order), which is the most useful witness.
pub fn bmc_bad_reachable(file: &Btor2File, max_k: u32) -> Result<BmcOutcome, BmcError> {
    // Extract the property + init + assumption node references (pure, pre-Z3).
    let bad_ops: Vec<Nid> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Bad { signal } => Some(signal.nid()),
            _ => None,
        })
        .collect();
    if bad_ops.is_empty() {
        return Err(BmcError::NoBadProperty);
    }
    let init_pairs: Vec<(Nid, Nid)> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Init { state, value, .. } => Some((*state, value.nid())),
            _ => None,
        })
        .collect();
    let constraint_ops: Vec<Nid> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Constraint { signal } => Some(signal.nid()),
            _ => None,
        })
        .collect();

    let cfg = z3::Config::new();
    z3::with_z3_config(&cfg, || {
        let view = encode_design(file).map_err(BmcError::Encode)?;

        // Fresh Z3 variables for every frame's state + inputs. Frame j's state is
        // `frame_state[j]`; its inputs `frame_input[j]` drive the transition to
        // frame j+1 (and any combinational `bad`/`constraint` read at frame j).
        let n = max_k as usize;
        let fresh = |src: &HashMap<Nid, BV>, prefix: &str| -> HashMap<Nid, BV> {
            src.iter()
                .map(|(nid, bv)| (*nid, BV::fresh_const(prefix, bv.get_size())))
                .collect()
        };
        let frame_state: Vec<HashMap<Nid, BV>> =
            (0..=n).map(|_| fresh(&view.state_curr, "bmc_s")).collect();
        let frame_input: Vec<HashMap<Nid, BV>> =
            (0..=n).map(|_| fresh(&view.inputs, "bmc_i")).collect();

        let one1 = BV::from_u64(1, 1);

        // Current-cycle BV of a `bad`/`constraint` operand. `signal_bvs` holds the
        // encode-walk cache (combinational nodes), but a property can reference a
        // STATE cell or an INPUT directly (`bad = q`) — fall back to those maps so
        // such operands are not silently dropped.
        let curr_bv = |nid: &Nid| -> Option<&BV> {
            view.signal_bvs
                .get(nid)
                .or_else(|| view.state_curr.get(nid))
                .or_else(|| view.inputs.get(nid))
        };

        // (state_curr → frame j) ∪ (inputs → frame j) — the substitution that
        // instantiates a current-cycle term (a `bad`/`constraint` condition) at
        // frame j.
        let curr_subs = |j: usize| -> Vec<(&BV, &BV)> {
            let mut pairs: Vec<(&BV, &BV)> = Vec::new();
            for (nid, bv) in &view.state_curr {
                pairs.push((bv, &frame_state[j][nid]));
            }
            for (nid, bv) in &view.inputs {
                pairs.push((bv, &frame_input[j][nid]));
            }
            pairs
        };

        // The transition relation instantiated for frame j → j+1: current state +
        // inputs at frame j, next state at frame j+1.
        let transition_at = |j: usize| -> Bool {
            let mut pairs = curr_subs(j);
            for (nid, bv) in &view.state_next {
                pairs.push((bv, &frame_state[j + 1][nid]));
            }
            view.transition.substitute(&pairs)
        };

        // `bad` at frame j: OR over all `bad` operands of (operand == 1).
        let bad_at = |j: usize| -> Bool {
            let pairs = curr_subs(j);
            let disj: Vec<Bool> = bad_ops
                .iter()
                .filter_map(|op| curr_bv(op).map(|bv| bv.substitute(&pairs).eq(&one1)))
                .collect();
            let refs: Vec<&Bool> = disj.iter().collect();
            if refs.is_empty() {
                Bool::from_bool(false)
            } else {
                Bool::or(&refs)
            }
        };

        // Each `constraint` at frame j: (operand == 1). Applied at every frame.
        let constraints_at = |j: usize| -> Vec<Bool> {
            let pairs = curr_subs(j);
            constraint_ops
                .iter()
                .filter_map(|op| curr_bv(op).map(|bv| bv.substitute(&pairs).eq(&one1)))
                .collect()
        };

        let solver = z3::Solver::new();
        // Init at frame 0 (init-less states stay free — BTOR2 semantics).
        for (state, value_nid) in &init_pairs {
            if let (Some(s0), Some(vbv)) =
                (frame_state[0].get(state), view.signal_bvs.get(value_nid))
            {
                solver.assert(s0.eq(vbv));
            }
        }
        for c in constraints_at(0) {
            solver.assert(&c);
        }

        // Incremental unroll: check `bad` at each frame, extend the path if clean.
        for k in 0..=n {
            solver.push();
            solver.assert(bad_at(k));
            let sat = matches!(solver.check(), z3::SatResult::Sat);
            solver.pop(1);
            if sat {
                return Ok(BmcOutcome::Violated { depth: k as u32 });
            }
            if k < n {
                solver.assert(transition_at(k));
                for c in constraints_at(k + 1) {
                    solver.assert(&c);
                }
            }
        }
        Ok(BmcOutcome::NoCexWithin { k: max_k })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser;

    fn bmc(content: &str, max_k: u32) -> BmcOutcome {
        let file = parser::parse(content).expect("parse btor2");
        bmc_bad_reachable(&file, max_k).expect("bmc runs")
    }

    // `q` init 0, `next q = 1`, `bad = q`. `bad` is false at frame 0 (q=0), true
    // at frame 1 (q=1) — the shallowest CEX is depth 1.
    const REACH: &str = "1 sort bitvec 1\n2 zero 1\n3 one 1\n4 state 1 q\n5 init 1 4 2\n\
                         6 next 1 4 3\n7 bad 4\n";
    // `q` init 0, `next q = 0` (stays 0), `bad = q`. Never violated (bounded safe).
    const SAFE: &str = "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 next 1 3 2\n\
                        6 bad 3\n";
    // 64-bit counter: init 0, `next = big + 1`, `bad = (big == 5)`. Reaches 5 at
    // depth 5. The 64-bit cone is OVER the exact engine's 40-bit cap, so only a
    // SAT-based engine (this) decides it — the beyond-cap scale win.
    const WIDE: &str = "1 sort bitvec 64\n2 zero 1\n3 one 1\n4 state 1 big\n5 init 1 4 2\n\
                        6 add 1 4 3\n7 next 1 4 6\n8 constd 1 5\n9 sort bitvec 1\n\
                        10 eq 9 4 8\n11 bad 10\n";

    #[test]
    fn finds_shallowest_cex() {
        assert_eq!(bmc(REACH, 5), BmcOutcome::Violated { depth: 1 });
    }

    #[test]
    fn bad_in_initial_state_is_depth_zero() {
        // `q` init 1, `bad = q` — violated already at frame 0.
        let d0 = "1 sort bitvec 1\n2 zero 1\n3 one 1\n4 state 1 q\n5 init 1 4 3\n\
                  6 next 1 4 3\n7 bad 4\n";
        assert_eq!(bmc(d0, 5), BmcOutcome::Violated { depth: 0 });
    }

    #[test]
    fn no_cex_on_bounded_safe_design() {
        // BMC never claims SAFE — only "no CEX within k".
        assert_eq!(bmc(SAFE, 10), BmcOutcome::NoCexWithin { k: 10 });
    }

    #[test]
    fn decides_beyond_the_exact_40_bit_cap() {
        // A 64-bit reachability the exact BDD engine abstains on (over-cap); native
        // BMC finds the depth-5 counterexample bit-precisely.
        assert_eq!(bmc(WIDE, 10), BmcOutcome::Violated { depth: 5 });
        // And with too small a bound it is honestly bounded, never a wrong SAFE.
        assert_eq!(bmc(WIDE, 3), BmcOutcome::NoCexWithin { k: 3 });
    }

    #[test]
    fn agrees_with_exact_engine_no_spurious_violated() {
        // Differential-oracle cross-check: BMC's `Violated` must NEVER contradict
        // the exact engine (Bruns–Godefroid sound). On in-cap designs the exact
        // engine decides both directions; BMC must (a) find a CEX where exact says
        // reachable, and (b) NEVER report Violated where exact says unreachable —
        // a spurious Violated would be a soundness bug.
        use crate::adapter::btor2::symbolic_bitblast::exact_bad_reachable;
        for (content, name) in [(REACH, "reach"), (SAFE, "safe")] {
            let file = parser::parse(content).expect("parse");
            let bmc = bmc_bad_reachable(&file, 20).expect("bmc runs");
            let exact_reachable = exact_bad_reachable(content).expect("exact decides in-cap");
            if exact_reachable {
                assert!(
                    matches!(bmc, BmcOutcome::Violated { .. }),
                    "{name}: exact says reachable but BMC returned {bmc:?}"
                );
            } else {
                assert!(
                    !matches!(bmc, BmcOutcome::Violated { .. }),
                    "{name}: SOUNDNESS — exact says unreachable but BMC returned {bmc:?}"
                );
            }
        }
    }

    #[test]
    fn no_bad_property_is_an_error() {
        let no_bad = "1 sort bitvec 1\n2 zero 1\n3 state 1 q\n4 init 1 3 2\n5 next 1 3 2\n";
        let file = parser::parse(no_bad).expect("parse");
        assert!(matches!(
            bmc_bad_reachable(&file, 5),
            Err(BmcError::NoBadProperty)
        ));
    }

    #[test]
    fn constraint_can_block_a_counterexample() {
        // `q` init 0, `next = q + 1` (2-bit: 0→1→2→3→0), `bad = (q == 3)`. Without
        // a constraint, bad is reachable at depth 3. Add `constraint = (q != 3)` —
        // wait, that would make bad+constraint jointly UNSAT at the bad frame, so
        // no CEX: the assumption forbids the only violating state.
        // (sort id 1 = bitvec 2, the datapath; sort id 9 = bitvec 1, the bool.)
        let base = "1 sort bitvec 2\n2 zero 1\n3 one 1\n4 state 1 q\n5 init 1 4 2\n\
                    6 add 1 4 3\n7 next 1 4 6\n8 constd 1 3\n9 sort bitvec 1\n\
                    10 eq 9 4 8\n11 bad 10\n";
        assert_eq!(bmc(base, 5), BmcOutcome::Violated { depth: 3 });
        // Same design + `constraint = !(q == 3)` (the negation of bad): the only
        // violating state is excluded, so no reachable bad honours the assumption.
        let constrained = "1 sort bitvec 2\n2 zero 1\n3 one 1\n4 state 1 q\n5 init 1 4 2\n\
                           6 add 1 4 3\n7 next 1 4 6\n8 constd 1 3\n9 sort bitvec 1\n\
                           10 eq 9 4 8\n12 not 9 10\n13 constraint 12\n11 bad 10\n";
        assert_eq!(bmc(constrained, 5), BmcOutcome::NoCexWithin { k: 5 });
    }
}
