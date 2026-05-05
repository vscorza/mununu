use std::collections::{HashMap, VecDeque};
use std::ops::{BitAndAssign, BitOrAssign, Not};
use std::sync::Arc;

use bitvec::prelude::*;
use smallvec::SmallVec;
use thiserror::Error;

use super::{
    Control, Formula, FormulaVarId, Guard, ModalKind, Node, NodeId, NodeOps,
    guard_matches_labels_and_vars, memo::MemoizationCache,
};
use crate::clts::{Clts, IdStorage, LabelId, StateId, Transition};

// Type alias to reduce complexity in function signatures
type TransitionGroupMap<'a, S, L> = HashMap<String, Vec<(&'a Transition<S, L>, usize)>>;

/// Options that control μ-calculus evaluation behaviour.
#[derive(Debug, Clone)]
pub struct EvaluationOptions {
    /// Enable memoisation of visited sub-formulas (skips storing results when fixpoint
    /// bindings are active to avoid stale entries).
    pub use_memoisation: bool,
    /// Enable guard-based symbolic partitions for current/next-state variable checks.
    pub use_partitions: bool,
}

impl Default for EvaluationOptions {
    fn default() -> Self {
        Self {
            use_memoisation: true,
            use_partitions: true,
        }
    }
}

/// Bitset result representing the states that satisfy the evaluated formula.
pub type EvalResult = BitVec<usize, Lsb0>;

/// Records which transition was chosen at each (state, modality) pair
/// during fixpoint evaluation. This constitutes a positional winning
/// strategy on the model-checking game.
///
/// Reference: Bruse, Friedmann & Lange, "Certification for Mu-Calculus
/// with Winning Strategies" (SPIN 2016, arXiv:1401.1693)
#[derive(Debug, Clone, Default)]
pub struct WitnessMap {
    /// `(state_index, diamond_node_id)` → transition index in outgoing list.
    /// For each state where a diamond/existential modality was satisfied, records
    /// which outgoing transition was the witness (the controller's chosen move).
    pub witnesses: HashMap<(usize, NodeId), usize>,

    /// `(state_index, fixpoint_var_id)` → iteration number when the state
    /// entered the fixpoint set. Forms the strategy signature for each state.
    pub iteration_ranks: HashMap<(usize, super::FormulaVarId), usize>,
}

/// A state's strategy signature — its rank tuple under the fixpoint nesting.
/// Used for lexicographic comparison to determine progressive moves.
pub type Signature = Vec<usize>;

impl WitnessMap {
    /// Compute the strategy signature for a state given the formula's fixpoint
    /// nesting order. Returns a rank vector where each entry corresponds to a
    /// fixpoint variable (outermost first).
    ///
    /// For mu-variables: the iteration at which the state entered the fixpoint
    /// (lower = closer to goal = better). States not in the fixpoint get `usize::MAX`.
    ///
    /// For nu-variables: 0 if the state is in the greatest fixpoint, `usize::MAX` if not.
    /// (Being in the nu-fixpoint is "good" — the invariant holds.)
    pub fn signature(
        &self,
        state_idx: usize,
        nesting: &[(super::FormulaVarId, bool)],
    ) -> Signature {
        nesting
            .iter()
            .map(|(var_id, _is_mu)| {
                self.iteration_ranks
                    .get(&(state_idx, *var_id))
                    .copied()
                    .unwrap_or(usize::MAX)
            })
            .collect()
    }

    /// Returns true if `target`'s signature is lexicographically ≤ `source`'s
    /// under the mu/nu ordering. This means the target is at least as progressive
    /// as the source — suitable for a winning strategy move.
    ///
    /// For mu-variables: smaller rank is better (fewer iterations to reach goal).
    /// For nu-variables: smaller rank is better (0 = in fixpoint, MAX = not).
    /// In both cases, the natural ordering (≤) is "at least as good."
    pub fn signature_nonincreasing(
        &self,
        source_idx: usize,
        target_idx: usize,
        nesting: &[(super::FormulaVarId, bool)],
    ) -> bool {
        let src = self.signature(source_idx, nesting);
        let tgt = self.signature(target_idx, nesting);
        tgt <= src
    }

    /// Returns true if `target`'s signature is strictly less than `source`'s
    /// (strict progress). Useful for functional controller extraction where
    /// we want guaranteed liveness progress.
    pub fn signature_decreasing(
        &self,
        source_idx: usize,
        target_idx: usize,
        nesting: &[(super::FormulaVarId, bool)],
    ) -> bool {
        let src = self.signature(source_idx, nesting);
        let tgt = self.signature(target_idx, nesting);
        tgt < src
    }
}

/// Environment that supplies atomic predicate valuations for evaluation.
///
/// Supports both pre-computed predicate bitsets and on-demand evaluation
/// of variable expressions over abstract states.
pub struct Environment {
    state_count: usize,
    predicates: HashMap<String, BitVec<usize, Lsb0>>,
    /// Optional mapping from state indices to abstract states for on-demand evaluation.
    abstract_states: Option<Vec<crate::abstraction::state::AbstractState>>,
}

impl Environment {
    pub fn new(state_count: usize) -> Self {
        Self {
            state_count,
            predicates: HashMap::new(),
            abstract_states: None,
        }
    }

    pub fn with_predicate(mut self, name: impl Into<String>, set: BitVec<usize, Lsb0>) -> Self {
        assert_eq!(
            set.len(),
            self.state_count,
            "predicate length must match state count"
        );
        self.predicates.insert(name.into(), set);
        self
    }

    /// Sets abstract states for on-demand evaluation.
    ///
    /// The abstract states must be in the same order as CLTS states (by index).
    pub fn with_abstract_states(
        mut self,
        states: Vec<crate::abstraction::state::AbstractState>,
    ) -> Self {
        assert_eq!(
            states.len(),
            self.state_count,
            "abstract state count must match state count"
        );
        self.abstract_states = Some(states);
        self
    }

    /// Retrieves a predicate by name.
    ///
    /// Returns pre-computed predicate bitset if available, otherwise None.
    /// For on-demand evaluation, use `evaluate_expression_on_demand()` instead.
    ///
    /// # Coverage Status
    /// Covered by test: `predicate_retrieval`
    pub fn predicate(&self, name: &str) -> Option<&BitVec<usize, Lsb0>> {
        self.predicates.get(name)
    }

    pub fn state_count(&self) -> usize {
        self.state_count
    }

    /// Checks if on-demand evaluation is enabled (abstract states are available).
    pub fn has_abstract_states(&self) -> bool {
        self.abstract_states.is_some()
    }
}

/// Errors produced during μ-calculus evaluation.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EvaluationError {
    #[error("μ-calculus evaluation aborted: {kind} limit exceeded (usage {usage}, limit {limit})")]
    LimitExceeded {
        kind: &'static str,
        usage: u64,
        limit: u64,
    },
}

/// Evaluates `formula` over `clts`, returning the set of satisfying states.
pub fn evaluate<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
) -> Result<EvalResult, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    evaluate_with_options_and_automaton(formula, clts, env, &EvaluationOptions::default())
}

/// Evaluates `formula` using the supplied evaluation options.
///
/// # Coverage Status
/// Covered by tests: `evaluation_with_memoization`, `evaluation_with_guard_partitions`
pub fn evaluate_with_options<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
    options: &EvaluationOptions,
) -> Result<EvalResult, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    evaluate_with_options_and_automaton(formula, clts, env, options)
}

/// Evaluates `formula` using the supplied evaluation options and automaton name.
/// The automaton name is used to resolve guard predicate names.
pub fn evaluate_with_options_and_automaton<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
    options: &EvaluationOptions,
) -> Result<EvalResult, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    assert_eq!(
        clts.state_count(),
        env.state_count(),
        "environment state count does not match CLTS"
    );

    let oob_bits = compute_oob_bits(clts);
    let not_oob_bits = !oob_bits.clone();
    let mut ctx = EvalContext {
        formula,
        clts,
        env,
        options: options.clone(),
        memo: MemoizationCache::default(),
        guard_cache: HashMap::new(),
        expression_eval_cache: HashMap::new(),
        witness_map: None,
        not_oob_bits,
        oob_bits,
    };
    let bindings = HashMap::new();
    let result = ctx.eval_node(formula.root(), &bindings)?;
    Ok(result)
}

/// Evaluates `formula` and additionally records a witness map for strategy extraction.
///
/// For each diamond (existential) modality, records which outgoing transition
/// was the witness. This constitutes a positional winning strategy on the
/// model-checking game (Bruse, Friedmann & Lange, SPIN 2016).
pub fn evaluate_with_witnesses<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
    options: &EvaluationOptions,
) -> Result<(EvalResult, WitnessMap), EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    assert_eq!(
        clts.state_count(),
        env.state_count(),
        "environment state count does not match CLTS"
    );

    let oob_bits = compute_oob_bits(clts);
    let not_oob_bits = !oob_bits.clone();
    let mut ctx = EvalContext {
        formula,
        clts,
        env,
        options: options.clone(),
        memo: MemoizationCache::default(),
        guard_cache: HashMap::new(),
        expression_eval_cache: HashMap::new(),
        witness_map: Some(WitnessMap::default()),
        not_oob_bits,
        oob_bits,
    };
    let bindings = HashMap::new();
    let result = ctx.eval_node(formula.root(), &bindings)?;
    let witnesses = ctx.witness_map.unwrap_or_default();
    Ok((result, witnesses))
}

/// Evaluate `formula` with three-valued (Kleene) semantics, returning a
/// [`TritSet`](super::trit::TritSet) — per-state True / False / Unknown verdict.
///
/// The three-valued evaluator runs alongside the standard BitVec evaluator
/// (it does NOT replace it). Both share the OOB sink convention from
/// `adapter::systemverilog::kripke::OOB_STATE_KEY`. The TritSet path treats OOB
/// states as `Unknown` for every atomic predicate (`must=false, may=true`),
/// propagating the Unknown trit through Boolean and modal connectives via
/// Kleene semantics. Reference: Bruns–Godefroid CONCUR 2000 (generalized model
/// checking), Huth–Jagadeesan–Schmidt ESOP 2001 (modal transition systems).
///
/// This entry point is read-only with respect to existing callers — it does
/// not change the `BitVec` API. Callers that want sound liveness verdicts on
/// OOB-reaching examples can use `verdict_at()` to distinguish definitely-true,
/// definitely-false, and unknown.
pub fn evaluate_tri<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
) -> Result<super::trit::TritSet, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    evaluate_tri_with_options(formula, clts, env, &EvaluationOptions::default())
}

/// Variant of [`evaluate_tri`] that accepts custom evaluation options.
pub fn evaluate_tri_with_options<S, L>(
    formula: &Formula,
    clts: &Clts<S, L>,
    env: &Environment,
    options: &EvaluationOptions,
) -> Result<super::trit::TritSet, EvaluationError>
where
    S: IdStorage,
    L: IdStorage,
{
    assert_eq!(
        clts.state_count(),
        env.state_count(),
        "environment state count does not match CLTS"
    );

    let oob_bits = compute_oob_bits(clts);
    let not_oob_bits = !oob_bits.clone();
    let mut ctx = EvalContext {
        formula,
        clts,
        env,
        options: options.clone(),
        memo: MemoizationCache::default(),
        guard_cache: HashMap::new(),
        expression_eval_cache: HashMap::new(),
        witness_map: None,
        not_oob_bits,
        oob_bits,
    };
    let bindings: HashMap<FormulaVarId, super::trit::TritSet> = HashMap::new();
    ctx.eval_node_tri(formula.root(), &bindings)
}

fn bit_is_set(bits: &BitVec<usize, Lsb0>, idx: usize) -> bool {
    bits.get(idx).map(|bit| *bit).unwrap_or(false)
}

/// Type alias for grouped transitions by uncontrollable labels.
/// Phase 3 optimization: Maps from label ID set (SmallVec) to (transition, original_index) pairs.
/// This eliminates string conversion overhead by using label IDs directly.
type GroupedTransitions<'b, S, L> =
    HashMap<SmallVec<[LabelId<L>; 4]>, Vec<(&'b Transition<S, L>, usize)>>;

struct EvalContext<'a, S, L>
where
    S: IdStorage,
    L: IdStorage,
{
    formula: &'a Formula,
    clts: &'a Clts<S, L>,
    env: &'a Environment,
    options: EvaluationOptions,
    memo: MemoizationCache,
    guard_cache: HashMap<GuardSignature, Arc<GuardPartitions>>,
    /// Cache for on-demand expression evaluation results.
    /// This is separate from env.expression_cache to allow per-evaluation caching.
    expression_eval_cache: HashMap<String, BitVec<usize, Lsb0>>,
    /// When Some, records transition witnesses for strategy extraction.
    /// None = no overhead; Some = recording witnesses during modal evaluation.
    witness_map: Option<WitnessMap>,
    /// Precomputed `!oob_bits`: the bitset of states whose CLTS valuation does
    /// NOT carry the `$oob$ → "true"` marker. Used to enforce OOB-as-bottom
    /// semantics (Bruns–Godefroid CONCUR 2000 safety projection): every
    /// freshly-allocated bitset (Node::True, predicate_bits, bitwise_not output,
    /// Greatest fixpoint init) is AND-ed with this mask so the OOB sink never
    /// satisfies any positive bitset. Combined with the OOB sink's self-loop
    /// in the adapter, modal `[a]Z` correctly falsifies safety formulas at
    /// any source state with a transition to OOB.
    not_oob_bits: BitVec<usize, Lsb0>,
    /// Precomputed `oob_bits`: the complement of `not_oob_bits`. Used by the
    /// three-valued (TritSet) evaluator to construct `Unknown` cells at OOB
    /// states (must=false, may=true).
    oob_bits: BitVec<usize, Lsb0>,
}

/// Compute the bitset of states whose CLTS valuation contains the
/// `__mununu_oob__ → "true"` out-of-bounds sink marker. Adapters set this
/// marker when a transition would have exited the abstracted domain (see
/// `adapter::systemverilog::kripke::OOB_STATE_KEY`).
fn compute_oob_bits<S, L>(clts: &Clts<S, L>) -> BitVec<usize, Lsb0>
where
    S: IdStorage,
    L: IdStorage,
{
    let mut bits = BitVec::repeat(false, clts.state_count());
    for state_id in clts.states() {
        if let Some(val) = clts.state_valuation(state_id)
            && val.get("__mununu_oob__").map(|s| s.as_str()) == Some("true")
        {
            bits.set(state_id.index(), true);
        }
    }
    bits
}

impl<'a, S, L> EvalContext<'a, S, L>
where
    S: IdStorage,
    L: IdStorage,
{
    fn eval_node(
        &mut self,
        node_id: NodeId,
        bindings: &HashMap<FormulaVarId, BitVec<usize, Lsb0>>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let use_memo = self.options.use_memoisation && bindings.is_empty();
        if use_memo && let Some(clone) = self.memo.get(&node_id) {
            return Ok(clone);
        }

        let store_result = use_memo && !self.formula.node(node_id).is_fixpoint();

        let result = match self.formula.node(node_id) {
            Node::True => self.alloc_bitvec(true)?,
            Node::False => self.alloc_bitvec(false)?,
            Node::Predicate(name) => self.predicate_bits(name)?,
            Node::Variable(var) => self.variable_bits(var, bindings)?,
            Node::Not(inner) => {
                let bits = self.eval_node(*inner, bindings)?;
                self.bitwise_not(bits)?
            }
            Node::And(left, right) => self.bitwise_and(*left, *right, bindings)?,
            Node::Or(left, right) => self.bitwise_or(*left, *right, bindings)?,
            Node::Modal {
                kind,
                guard,
                target,
            } => self.eval_modal(*kind, guard, *target, bindings, node_id)?,
            Node::Mu { var, body } => {
                self.eval_fixpoint(*var, *body, FixpointKind::Least, bindings)?
            }
            Node::Nu { var, body } => {
                self.eval_fixpoint(*var, *body, FixpointKind::Greatest, bindings)?
            }
        };

        if store_result {
            self.memo.insert(node_id, &result);
        }

        Ok(result)
    }

    fn bitwise_not(
        &mut self,
        input: BitVec<usize, Lsb0>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let mut result = self.alloc_bitvec(false)?;
        for (mut out, value) in result.iter_mut().zip(input.iter()) {
            out.set(!value);
        }
        // OOB-as-bottom invariant: bitwise_not flips OOB to true if the input had
        // OOB cleared. Re-mask so OOB stays bottom under negation (avoids the
        // polarity bug where !P would satisfy at OOB but P would not).
        result.bitand_assign(self.not_oob_bits.as_bitslice());
        Ok(result)
    }

    fn bitwise_and(
        &mut self,
        left: NodeId,
        right: NodeId,
        bindings: &HashMap<FormulaVarId, BitVec<usize, Lsb0>>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let lhs = self.eval_node(left, bindings)?;
        let rhs = self.eval_node(right, bindings)?;

        // Clone lhs before modifying to avoid corrupting cached or reused bitsets
        let mut result = self.clone_bitvec(&lhs)?;
        result.as_mut_bitslice().bitand_assign(rhs.as_bitslice());
        Ok(result)
    }

    fn bitwise_or(
        &mut self,
        left: NodeId,
        right: NodeId,
        bindings: &HashMap<FormulaVarId, BitVec<usize, Lsb0>>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let lhs = self.eval_node(left, bindings)?;
        let rhs = self.eval_node(right, bindings)?;
        // Clone lhs before modifying to avoid corrupting cached or reused bitsets
        let mut result = self.clone_bitvec(&lhs)?;
        result.as_mut_bitslice().bitor_assign(rhs.as_bitslice());
        Ok(result)
    }

    fn eval_modal(
        &mut self,
        kind: ModalKind,
        guard: &Guard,
        target: NodeId,
        bindings: &HashMap<FormulaVarId, BitVec<usize, Lsb0>>,
        modal_node_id: NodeId,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let target_set = self.eval_node(target, bindings)?;
        if let Some(bound) = guard.max_steps {
            return self.eval_modal_bounded(kind, guard, &target_set, bound);
        }

        let mut result = self.alloc_bitvec(false)?;
        let guard_parts = if self.options.use_partitions {
            Some(self.guard_partitions(guard))
        } else {
            None
        };

        for state in self.clts.states() {
            let satisfies = match kind {
                ModalKind::Diamond => self.modal_exists(
                    state,
                    guard,
                    &target_set,
                    guard_parts.as_deref(),
                    modal_node_id,
                ),
                ModalKind::Box => self.modal_forall(
                    state,
                    guard,
                    &target_set,
                    guard_parts.as_deref(),
                    modal_node_id,
                ),
            };
            if satisfies {
                result.set(state.index(), true);
                // Record witness: which transition satisfies the modality
                if self.witness_map.is_some() && kind == ModalKind::Diamond {
                    // Find the first outgoing transition whose target is in target_set
                    for (idx, transition) in self.clts.outgoing(state).iter().enumerate() {
                        if self.guard_matches(state, transition, guard)
                            && target_set
                                .get(transition.target().index())
                                .map(|bit| *bit)
                                .unwrap_or(false)
                        {
                            if let Some(ref mut wm) = self.witness_map {
                                wm.witnesses.insert((state.index(), modal_node_id), idx);
                            }
                            break;
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    fn eval_modal_bounded(
        &mut self,
        kind: ModalKind,
        guard: &Guard,
        target_set: &BitVec<usize, Lsb0>,
        bound: u32,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        // When scope is zero steps we still rely on the already-evaluated target set,
        // so we do not need to re-evaluate the target node.
        let mut result = self.alloc_bitvec(false)?;
        for state in self.clts.states() {
            let satisfies = match kind {
                ModalKind::Diamond => self.modal_exists_bounded(state, guard, target_set, bound),
                ModalKind::Box => self.modal_forall_bounded(state, guard, target_set, bound),
            };
            if satisfies {
                result.set(state.index(), true);
            }
        }
        Ok(result)
    }

    /// Extracts the set of label names from a transition's labels.
    /// Returns a sorted vector for consistent grouping.
    fn transition_label_set(&self, transition: &Transition<S, L>) -> Vec<String> {
        let mut labels = Vec::new();
        for label_id in transition.labels() {
            if let Some(payload) = self.clts.label_payload(*label_id) {
                labels.extend(payload.iter().cloned());
            }
        }
        labels.sort();
        labels
    }

    /// Extracts the uncontrollable label IDs from a transition's label set.
    ///
    /// Phase 1 optimization: Uses pre-computed `uncontrollable_alphabet` directly
    /// instead of converting to strings and checking membership.
    ///
    /// Returns a sorted vector of uncontrollable label IDs for use as group keys.
    fn extract_uncontrollable_label_ids(
        &self,
        transition: &Transition<S, L>,
    ) -> smallvec::SmallVec<[crate::clts::LabelId<L>; 4]> {
        use crate::clts::LabelId;
        use smallvec::SmallVec;

        // Epsilon transitions (empty labels) are always uncontrollable
        if transition.labels().is_empty() {
            return SmallVec::new(); // Empty SmallVec represents epsilon
        }

        let mut uncontrollable_ids: SmallVec<[LabelId<L>; 4]> = SmallVec::new();
        for &label_id in transition.labels() {
            if self.clts.is_uncontrollable_label(label_id) {
                uncontrollable_ids.push(label_id);
            }
        }
        // Sort by index for canonical ordering
        uncontrollable_ids.sort_by_key(|id| id.index());
        uncontrollable_ids
    }

    /// Groups transitions by their non-controllable alphabet elements (Skolem paradigm).
    ///
    /// For the Skolem paradigm refinement (Phase 3.5), we group transitions that share
    /// the same **uncontrollable labels** (not all labels). This allows controllable
    /// transitions that "complete" uncontrollable labels to be included in the same group.
    ///
    /// This method now uses pre-computed groups from the CLTS for O(1) access, then filters
    /// by guard conditions.
    ///
    /// Phase 3 optimization: Returns a map from uncontrollable label ID set (SmallVec) to
    /// (transition, index) pairs, eliminating string conversion overhead.
    fn group_transitions_by_uncontrollable_labels<'b>(
        &self,
        transitions: &'b [Transition<S, L>],
        guard: &Guard,
        state: StateId<S>,
        guard_parts: Option<&GuardPartitions>,
    ) -> GroupedTransitions<'b, S, L> {
        // Use pre-computed groups from CLTS
        let precomputed_groups = self
            .clts
            .transitions_grouped_by_uncontrollable_labels(state);

        // Filter by guard and use label ID keys directly (Phase 3 optimization)
        let mut filtered_groups: GroupedTransitions<'b, S, L> = HashMap::new();

        for (uncontrollable_label_ids, transition_indices) in precomputed_groups {
            // Phase 3: Use label ID set directly as key (no string conversion)
            // Note: We need to clone the key since HashMap::insert takes ownership
            let key = uncontrollable_label_ids.clone();

            // Filter transitions by guard and track original indices
            let mut group_transitions: Vec<(&'b Transition<S, L>, usize)> = Vec::new();
            for &idx in transition_indices {
                if idx < transitions.len() {
                    let transition = &transitions[idx];
                    if self.guard_matches(state, transition, guard) {
                        if let Some(parts) = guard_parts
                            && !parts.matches_next(transition.target().index())
                        {
                            continue;
                        }
                        // All transitions are always enabled after unrolling (guards resolved at build time)
                        group_transitions.push((transition, idx));
                    }
                }
            }

            if !group_transitions.is_empty() {
                filtered_groups.insert(key, group_transitions);
            }
        }

        filtered_groups
    }

    fn modal_exists(
        &mut self,
        state: StateId<S>,
        guard: &Guard,
        targets: &BitVec<usize, Lsb0>,
        guard_parts: Option<&GuardPartitions>,
        _modal_node_id: NodeId,
    ) -> bool {
        if let Some(parts) = guard_parts
            && !parts.matches_current(state.index())
        {
            return false;
        }

        let outgoing = self.clts.outgoing(state);

        // Group uncontrollable transitions by their label sets (Skolem paradigm)
        let uncontrollable_groups =
            self.group_transitions_by_uncontrollable_labels(outgoing, guard, state, guard_parts);

        // For diamond operator with Skolem paradigm: we need at least ONE group to have
        // a transition leading to the target (not all groups)
        let mut any_group_satisfies = false;

        // For each group of transitions sharing the same uncontrollable labels (Skolem paradigm),
        // we need to check: for each full label set (including controllable labels), ALL transitions
        // with that label set must satisfy the formula. This ensures that when the system chooses
        // a controllable action, all possible outcomes (nondeterministic choices) satisfy.
        for group in uncontrollable_groups.values() {
            // Sub-group by full label set (not just uncontrollable labels)
            let mut label_set_groups: TransitionGroupMap<'_, S, L> = HashMap::new();
            for (trans, idx) in group {
                let full_label_set = self.transition_label_set(trans);
                let key = full_label_set.join(",");
                label_set_groups.entry(key).or_default().push((trans, *idx));
            }

            // For each sub-group (same full label set), check if it satisfies
            // For diamond: at least one sub-group must satisfy
            //
            // Key semantics: When multiple transitions share the same full label set,
            // they represent nondeterministic choices. For <> (possibility) with Skolem paradigm:
            // there exists a controllable choice (a sub-group) such that ALL states
            // reached through that label set satisfy.
            //
            // However, if a sub-group contains both controllable and uncontrollable transitions
            // with the same label set, we need to check: is there a controllable transition
            // that satisfies? If yes, and if all controllable transitions with that label set
            // satisfy, then the sub-group satisfies (the system can choose the controllable option).
            // If all transitions in a sub-group are uncontrollable, then ALL must satisfy.
            let mut group_has_satisfying_subgroup = false;
            for sub_group in label_set_groups.values() {
                // Check if there are any controllable transitions in this sub-group
                let controllable_transitions: Vec<_> = sub_group
                    .iter()
                    .filter(|(trans, _idx)| trans.is_controllable(self.clts))
                    .collect();
                let uncontrollable_transitions: Vec<_> = sub_group
                    .iter()
                    .filter(|(trans, _idx)| trans.is_uncontrollable(self.clts))
                    .collect();

                if !controllable_transitions.is_empty() {
                    // Sub-group has controllable transitions: check if ALL controllable transitions satisfy
                    // If yes, the system can choose a controllable option that satisfies
                    // (uncontrollable transitions in the same sub-group don't need to satisfy
                    // because the system can choose the controllable option)
                    let all_controllable_satisfy =
                        controllable_transitions.iter().all(|(trans, _idx)| {
                            targets
                                .get(trans.target().index())
                                .map(|bit| *bit)
                                .unwrap_or(false)
                        });
                    if all_controllable_satisfy {
                        group_has_satisfying_subgroup = true;
                        break; // Found a satisfying sub-group for this uncontrollable group
                    }
                } else {
                    // Sub-group has only uncontrollable transitions: ALL must satisfy
                    let all_satisfy = uncontrollable_transitions.iter().all(|(trans, _idx)| {
                        targets
                            .get(trans.target().index())
                            .map(|bit| *bit)
                            .unwrap_or(false)
                    });
                    if all_satisfy {
                        group_has_satisfying_subgroup = true;
                        break; // Found a satisfying sub-group for this uncontrollable group
                    }
                }
            }

            if group_has_satisfying_subgroup {
                any_group_satisfies = true;
                // For diamond operator: if any group has a satisfying sub-group, we can return true
                // (we don't need to check all groups)
                break;
            }
        }

        // For controllable transitions (not in any uncontrollable group), check normally
        // But first, we need to handle the case where multiple transitions share the same
        // full label set (nondeterminism). For <> with Skolem paradigm: when multiple transitions
        // have the same label set, ALL transitions with that label set must satisfy.
        //
        // Phase 1 optimization: No longer need to compute uncontrollable_label_set,
        // use extract_uncontrollable_label_ids directly.

        // Group all transitions (including controllable ones not in uncontrollable groups)
        // by their full label set to handle nondeterminism
        let mut all_transitions_by_label_set: TransitionGroupMap<'_, S, L> = HashMap::new();
        for (idx, transition) in outgoing.iter().enumerate() {
            if !self.guard_matches(state, transition, guard) {
                continue;
            }
            // All transitions are always enabled after unrolling (guards resolved at build time)
            if let Some(parts) = guard_parts
                && !parts.matches_next(transition.target().index())
            {
                continue;
            }
            let full_label_set = self.transition_label_set(transition);
            let key = full_label_set.join(",");
            all_transitions_by_label_set
                .entry(key)
                .or_default()
                .push((transition, idx));
        }

        // Check controllable transitions not in any uncontrollable group
        // For each label set group, if it has multiple transitions, ALL must satisfy
        for transitions_with_same_labels in all_transitions_by_label_set.values() {
            // Check if this label set group is in an uncontrollable group
            // Phase 3: Use label IDs directly as keys (no string conversion)
            let uncontrollable_label_ids_for_key =
                if let Some((first_trans, _)) = transitions_with_same_labels.first() {
                    self.extract_uncontrollable_label_ids(first_trans)
                } else {
                    continue;
                };
            let in_uncontrollable_group = if !uncontrollable_label_ids_for_key.is_empty() {
                // Phase 3: Direct label ID key lookup (no string conversion)
                uncontrollable_groups.contains_key(&uncontrollable_label_ids_for_key)
            } else {
                false
            };

            if !in_uncontrollable_group {
                // This is a purely controllable label set group (or a mixed group where the label
                // was inferred as controllable, so uncontrollable transitions aren't in any group)
                //
                // For <> with Skolem paradigm: when multiple transitions share the same label set,
                // we need to check if there's at least one controllable transition that satisfies.
                // If yes, and if all controllable transitions with that label set satisfy,
                // then the group satisfies (the system can choose the controllable option).
                // If all transitions are uncontrollable, then ALL must satisfy.
                let controllable_in_group: Vec<_> = transitions_with_same_labels
                    .iter()
                    .filter(|(trans, _idx)| trans.is_controllable(self.clts))
                    .collect();
                let uncontrollable_in_group: Vec<_> = transitions_with_same_labels
                    .iter()
                    .filter(|(trans, _idx)| trans.is_uncontrollable(self.clts))
                    .collect();

                if !controllable_in_group.is_empty() {
                    // Group has controllable transitions: check if ALL controllable transitions satisfy
                    let all_controllable_satisfy =
                        controllable_in_group.iter().all(|(trans, _idx)| {
                            targets
                                .get(trans.target().index())
                                .map(|bit| *bit)
                                .unwrap_or(false)
                        });
                    if all_controllable_satisfy {
                        return true; // Found a satisfying controllable label set group
                    }
                } else if !uncontrollable_in_group.is_empty() {
                    // Group has only uncontrollable transitions: ALL must satisfy
                    let all_satisfy = uncontrollable_in_group.iter().all(|(trans, _idx)| {
                        targets
                            .get(trans.target().index())
                            .map(|bit| *bit)
                            .unwrap_or(false)
                    });
                    if all_satisfy {
                        return true;
                    }
                }
            } else {
                // This label set group is in an uncontrollable group
                // The check for this case is already handled above in the uncontrollable_groups loop
                // where we sub-group by full label set and check that all transitions in each
                // sub-group satisfy
            }
        }

        // If any uncontrollable group has a satisfying sub-group, return true
        // (for Skolem paradigm, we need at least one group to satisfy)
        if any_group_satisfies {
            return true;
        }

        // Environment diamond: <ctrl=environment> Φ
        // TRUE if (∃ uncontrollable → Φ) OR (∀ controllable → Φ)
        // "The environment has an uncontrollable escape, or the system is trapped"
        if guard.control == Control::Environment {
            // Check: ∃ uncontrollable transition → targets
            for transition in outgoing.iter() {
                if !transition.is_uncontrollable(self.clts) {
                    continue;
                }
                if !self.guard_matches(state, transition, guard) {
                    continue;
                }
                if let Some(parts) = guard_parts
                    && !parts.matches_next(transition.target().index())
                {
                    continue;
                }
                if targets
                    .get(transition.target().index())
                    .map(|bit| *bit)
                    .unwrap_or(false)
                {
                    return true; // Environment has an uncontrollable escape
                }
            }

            // Check: ∀ controllable transitions → targets (system is trapped)
            let mut ctrl_seen = false;
            let mut all_ctrl_satisfy = true;
            for transition in outgoing.iter() {
                if !transition.is_controllable(self.clts) {
                    continue;
                }
                if !self.guard_matches(state, transition, guard) {
                    continue;
                }
                if let Some(parts) = guard_parts
                    && !parts.matches_next(transition.target().index())
                {
                    continue;
                }
                ctrl_seen = true;
                if !targets
                    .get(transition.target().index())
                    .map(|bit| *bit)
                    .unwrap_or(false)
                {
                    all_ctrl_satisfy = false;
                    break;
                }
            }
            if ctrl_seen && all_ctrl_satisfy {
                return true; // System is trapped: all controllable moves lead to targets
            }
            // No controllable transitions and no uncontrollable escape: vacuously true
            if !ctrl_seen {
                return true;
            }
        }

        // All transitions should have been checked above through:
        // 1. Uncontrollable groups (with sub-grouping by full label set)
        // 2. Controllable transitions grouped by full label set
        // 3. Global label set grouping for nondeterminism
        // 4. Environment diamond (Control::Environment)
        // If we reach here, no group/sub-group satisfied the formula
        false
    }

    fn modal_forall(
        &self,
        state: StateId<S>,
        guard: &Guard,
        targets: &BitVec<usize, Lsb0>,
        guard_parts: Option<&GuardPartitions>,
        _modal_node_id: NodeId,
    ) -> bool {
        if let Some(parts) = guard_parts
            && !parts.matches_current(state.index())
        {
            return true;
        }

        let outgoing = self.clts.outgoing(state);

        match guard.control {
            Control::All => {
                // Group uncontrollable transitions by their label sets (Skolem paradigm)
                let uncontrollable_groups = self.group_transitions_by_uncontrollable_labels(
                    outgoing,
                    guard,
                    state,
                    guard_parts,
                );

                // For each group of uncontrollable transitions, ALL must satisfy
                // Group now contains (transition, index) pairs, so guard predicates are already checked
                for group in uncontrollable_groups.values() {
                    for (trans, _idx) in group {
                        if !targets
                            .get(trans.target().index())
                            .map(|bit| *bit)
                            .unwrap_or(false)
                        {
                            return false;
                        }
                    }
                }

                // For controllable transitions not in any uncontrollable group, all must satisfy
                // Phase 1 optimization: Use extract_uncontrollable_label_ids directly,
                // no need to compute uncontrollable_label_set

                for transition in outgoing.iter() {
                    if transition.is_controllable(self.clts) {
                        if !self.guard_matches(state, transition, guard) {
                            continue;
                        }
                        // All transitions are always enabled after unrolling (guards resolved at build time)
                        if let Some(parts) = guard_parts
                            && !parts.matches_next(transition.target().index())
                        {
                            continue;
                        }
                        // Check if this controllable transition is in an uncontrollable group
                        // Phase 3: Use label IDs directly as keys (no string conversion)
                        let uncontrollable_label_ids =
                            self.extract_uncontrollable_label_ids(transition);
                        let in_uncontrollable_group = if !uncontrollable_label_ids.is_empty() {
                            // Phase 3: Direct label ID key lookup (no string conversion)
                            uncontrollable_groups.contains_key(&uncontrollable_label_ids)
                        } else {
                            false
                        };

                        if !in_uncontrollable_group
                            && !targets
                                .get(transition.target().index())
                                .map(|bit| *bit)
                                .unwrap_or(false)
                        {
                            return false;
                        }
                    }
                }

                // If no uncontrollable groups, check all transitions normally
                if uncontrollable_groups.is_empty() {
                    for transition in outgoing.iter() {
                        if !self.guard_matches(state, transition, guard) {
                            continue;
                        }
                        // All transitions are always enabled after unrolling (guards resolved at build time)
                        if let Some(parts) = guard_parts
                            && !parts.matches_next(transition.target().index())
                        {
                            continue;
                        }
                        if !targets
                            .get(transition.target().index())
                            .map(|bit| *bit)
                            .unwrap_or(false)
                        {
                            return false;
                        }
                    }
                }

                true
            }
            Control::Controllable => {
                // Group uncontrollable transitions by their label sets (Skolem paradigm)
                let uncontrollable_groups = self.group_transitions_by_uncontrollable_labels(
                    outgoing,
                    guard,
                    state,
                    guard_parts,
                );

                // For each group of uncontrollable transitions, ALL must satisfy
                // Group now contains (transition, index) pairs, so guard predicates are already checked
                for group in uncontrollable_groups.values() {
                    for (trans, _idx) in group {
                        if !targets
                            .get(trans.target().index())
                            .map(|bit| *bit)
                            .unwrap_or(false)
                        {
                            return false;
                        }
                    }
                }

                // For controllable transitions, at least one must satisfy
                let mut ctrl_seen = false;
                let mut ctrl_satisfied = false;
                for transition in outgoing {
                    if !self.guard_matches(state, transition, guard) {
                        continue;
                    }
                    let target_ok = targets
                        .get(transition.target().index())
                        .map(|bit| *bit)
                        .unwrap_or(false);
                    if let Some(parts) = guard_parts
                        && !parts.matches_next(transition.target().index())
                    {
                        continue;
                    }
                    if transition.is_controllable(self.clts) {
                        ctrl_seen = true;
                        if target_ok {
                            ctrl_satisfied = true;
                        }
                    }
                    // Uncontrollable transitions already handled in uncontrollable_groups above
                }
                if ctrl_seen { ctrl_satisfied } else { true }
            }
            Control::Environment => {
                // Box with environment perspective: dual of diamond with controllable.
                // [ctrl=environment] Φ = (∀ uncontrollable → Φ) ∧ (¬(∃ controllable → ¬Φ))
                // = all uncontrollable satisfy AND no controllable escapes.
                // Simplified: all uncontrollable → Φ AND (∃ controllable → Φ fails → false)
                // Practically: all matching transitions must satisfy (like Control::All).
                // This case is rare — inversion primarily produces <ctrl=environment>.
                let outgoing = self.clts.outgoing(state);
                for transition in outgoing {
                    if !self.guard_matches(state, transition, guard) {
                        continue;
                    }
                    if let Some(parts) = guard_parts
                        && !parts.matches_next(transition.target().index())
                    {
                        continue;
                    }
                    if !targets
                        .get(transition.target().index())
                        .map(|bit| *bit)
                        .unwrap_or(false)
                    {
                        return false;
                    }
                }
                true
            }
        }
    }

    fn modal_exists_bounded(
        &self,
        state: StateId<S>,
        guard: &Guard,
        targets: &BitVec<usize, Lsb0>,
        bound: u32,
    ) -> bool {
        if bound == 0 {
            return self.guard_zero_step_allowed(guard)
                && self.guard_current_matches(state, guard)
                && bit_is_set(targets, state.index());
        }

        if !self.guard_current_matches(state, guard) {
            return false;
        }

        let outgoing = self.clts.outgoing(state);

        // Group uncontrollable transitions by their label sets (Skolem paradigm)
        let uncontrollable_groups = self.group_transitions_by_uncontrollable_labels(
            outgoing, guard, state, None, // No guard_parts for bounded version
        );

        // For each group of transitions sharing the same uncontrollable labels, check if at least one can satisfy
        // The group may contain both uncontrollable and controllable transitions
        // Within each group, sub-group by full label set and ensure all transitions with same label set satisfy
        let mut any_group_has_satisfying_subgroup = false;
        for group in uncontrollable_groups.values() {
            // Sub-group by full label set (not just uncontrollable labels)
            let mut label_set_groups: TransitionGroupMap<'_, S, L> = HashMap::new();
            for (trans, idx) in group {
                let full_label_set = self.transition_label_set(trans);
                let key = full_label_set.join(",");
                label_set_groups.entry(key).or_default().push((trans, *idx));
            }

            // For each sub-group (same full label set), check if it can satisfy
            // For diamond: at least one sub-group must be able to satisfy
            let mut group_has_satisfying_subgroup = false;
            for sub_group in label_set_groups.values() {
                // Check if there are any controllable transitions in this sub-group
                let controllable_transitions: Vec<_> = sub_group
                    .iter()
                    .filter(|(trans, _idx)| trans.is_controllable(self.clts))
                    .collect();
                let uncontrollable_transitions: Vec<_> = sub_group
                    .iter()
                    .filter(|(trans, _idx)| trans.is_uncontrollable(self.clts))
                    .collect();

                if !controllable_transitions.is_empty() {
                    // Sub-group has controllable transitions: check if ALL controllable transitions can satisfy
                    let all_controllable_can_satisfy =
                        controllable_transitions.iter().all(|(trans, _idx)| {
                            if bit_is_set(targets, trans.target().index()) {
                                return true;
                            }
                            if bound > 1 {
                                self.modal_exists_bounded(trans.target(), guard, targets, bound - 1)
                            } else {
                                false
                            }
                        });
                    if all_controllable_can_satisfy {
                        group_has_satisfying_subgroup = true;
                        break;
                    }
                } else {
                    // Sub-group has only uncontrollable transitions: ALL must be able to satisfy
                    let all_can_satisfy = uncontrollable_transitions.iter().all(|(trans, _idx)| {
                        if bit_is_set(targets, trans.target().index()) {
                            return true;
                        }
                        if bound > 1 {
                            self.modal_exists_bounded(trans.target(), guard, targets, bound - 1)
                        } else {
                            false
                        }
                    });
                    if all_can_satisfy {
                        group_has_satisfying_subgroup = true;
                        break;
                    }
                }
            }

            if group_has_satisfying_subgroup {
                any_group_has_satisfying_subgroup = true;
                break;
            }
        }

        if !any_group_has_satisfying_subgroup && !uncontrollable_groups.is_empty() {
            // For Skolem paradigm: if no group has a satisfying sub-group,
            // the formula is not satisfied
            return false;
        }

        // For controllable transitions not in any uncontrollable group, check normally
        // Phase 3 optimization: Use extract_uncontrollable_label_ids directly with label ID keys

        for transition in outgoing {
            if transition.is_controllable(self.clts) {
                if !self.guard_matches(state, transition, guard) {
                    continue;
                }
                // Check if this controllable transition is in an uncontrollable group
                // Phase 3: Use label IDs directly as keys (no string conversion)
                let uncontrollable_label_ids = self.extract_uncontrollable_label_ids(transition);
                let in_uncontrollable_group = if !uncontrollable_label_ids.is_empty() {
                    // Phase 3: Direct label ID key lookup (no string conversion)
                    uncontrollable_groups.contains_key(&uncontrollable_label_ids)
                } else {
                    false
                };

                if !in_uncontrollable_group {
                    if bit_is_set(targets, transition.target().index()) {
                        return true;
                    }
                    if bound > 1
                        && self.modal_exists_bounded(transition.target(), guard, targets, bound - 1)
                    {
                        return true;
                    }
                }
            }
        }

        // If we have uncontrollable groups, we've already verified they're satisfied
        if !uncontrollable_groups.is_empty() {
            return true;
        }

        // Fallback: if no uncontrollable groups, use original BFS approach
        let depth_limit = bound as usize;
        let mut visited = vec![vec![false; depth_limit + 1]; self.clts.state_count()];
        let mut queue = VecDeque::new();
        queue.push_back((state, 0u32));
        visited[state.index()][0] = true;

        while let Some((current, depth)) = queue.pop_front() {
            if depth > bound {
                continue;
            }
            if depth > 0 && bit_is_set(targets, current.index()) {
                return true;
            }
            if depth == bound {
                continue;
            }
            for transition in self.clts.outgoing(current) {
                if !self.guard_matches(current, transition, guard) {
                    continue;
                }
                let next = transition.target();
                let next_depth = depth + 1;
                if bit_is_set(targets, next.index()) {
                    return true;
                }
                if next_depth <= bound && !visited[next.index()][next_depth as usize] {
                    visited[next.index()][next_depth as usize] = true;
                    if next_depth < bound {
                        queue.push_back((next, next_depth));
                    }
                }
            }
        }

        false
    }

    fn modal_forall_bounded(
        &self,
        state: StateId<S>,
        guard: &Guard,
        targets: &BitVec<usize, Lsb0>,
        bound: u32,
    ) -> bool {
        if bound == 0 {
            return self.guard_zero_step_allowed(guard)
                && self.guard_current_matches(state, guard)
                && bit_is_set(targets, state.index());
        }

        if !self.guard_current_matches(state, guard) {
            return false;
        }

        if matches!(guard.control, Control::Controllable) {
            return self.modal_forall_bounded_controllable(state, guard, targets, bound);
        }

        let outgoing = self.clts.outgoing(state);

        // Group uncontrollable transitions by their label sets (Skolem paradigm)
        let uncontrollable_groups = self.group_transitions_by_uncontrollable_labels(
            outgoing, guard, state, None, // No guard_parts for bounded version
        );

        // For each group of uncontrollable transitions, ALL must satisfy
        // Group now contains (transition, index) pairs, so guard predicates are already checked
        for group in uncontrollable_groups.values() {
            for (trans, _idx) in group {
                if !bit_is_set(targets, trans.target().index()) {
                    return false;
                }
                if bound > 1
                    && !self.modal_forall_bounded(trans.target(), guard, targets, bound - 1)
                {
                    return false;
                }
            }
        }

        // For controllable transitions not in any uncontrollable group, all must satisfy
        // Phase 3 optimization: Use extract_uncontrollable_label_ids with label ID keys
        for transition in outgoing {
            if transition.is_controllable(self.clts) {
                if !self.guard_matches(state, transition, guard) {
                    continue;
                }
                // Check if this controllable transition is in an uncontrollable group
                // Phase 3: Use label IDs directly as keys (no string conversion)
                let uncontrollable_label_ids = self.extract_uncontrollable_label_ids(transition);
                let in_uncontrollable_group = if !uncontrollable_label_ids.is_empty() {
                    // Phase 3: Direct label ID key lookup (no string conversion)
                    uncontrollable_groups.contains_key(&uncontrollable_label_ids)
                } else {
                    false
                };

                if !in_uncontrollable_group {
                    if !bit_is_set(targets, transition.target().index()) {
                        return false;
                    }
                    if bound > 1
                        && !self.modal_forall_bounded(
                            transition.target(),
                            guard,
                            targets,
                            bound - 1,
                        )
                    {
                        return false;
                    }
                }
            }
        }

        // If no uncontrollable groups, use original BFS approach
        if uncontrollable_groups.is_empty() {
            let depth_limit = bound as usize;
            let mut visited = vec![vec![false; depth_limit + 1]; self.clts.state_count()];
            let mut queue = VecDeque::new();
            queue.push_back((state, 0u32));
            visited[state.index()][0] = true;

            while let Some((current, depth)) = queue.pop_front() {
                if depth == bound {
                    continue;
                }
                for transition in self.clts.outgoing(current) {
                    if !self.guard_matches(current, transition, guard) {
                        continue;
                    }
                    let next = transition.target();
                    if !bit_is_set(targets, next.index()) {
                        return false;
                    }
                    let next_depth = depth + 1;
                    if next_depth <= bound && !visited[next.index()][next_depth as usize] {
                        visited[next.index()][next_depth as usize] = true;
                        if next_depth < bound {
                            queue.push_back((next, next_depth));
                        }
                    }
                }
            }
        }

        true
    }

    fn modal_forall_bounded_controllable(
        &self,
        state: StateId<S>,
        guard: &Guard,
        targets: &BitVec<usize, Lsb0>,
        bound: u32,
    ) -> bool {
        let state_count = self.clts.state_count();
        let mut memo = vec![vec![None; (bound + 1) as usize]; state_count];
        self.modal_forall_bounded_controllable_rec(state, guard, targets, bound, &mut memo)
    }

    fn modal_forall_bounded_controllable_rec(
        &self,
        state: StateId<S>,
        guard: &Guard,
        targets: &BitVec<usize, Lsb0>,
        remaining: u32,
        memo: &mut Vec<Vec<Option<bool>>>,
    ) -> bool {
        let idx = state.index();
        if let Some(value) = memo[idx][remaining as usize] {
            return value;
        }

        if !self.guard_current_matches(state, guard) {
            memo[idx][remaining as usize] = Some(false);
            return false;
        }

        let result = if remaining == 0 {
            self.guard_zero_step_allowed(guard) && bit_is_set(targets, idx)
        } else {
            let outgoing = self.clts.outgoing(state);

            // Group uncontrollable transitions by their label sets (Skolem paradigm)
            let uncontrollable_groups = self.group_transitions_by_uncontrollable_labels(
                outgoing, guard, state, None, // No guard_parts for bounded version
            );

            // For each group of uncontrollable transitions, ALL must satisfy
            // Group now contains (transition, index) pairs, so guard predicates are already checked
            for group in uncontrollable_groups.values() {
                for (trans, _idx) in group {
                    let next_ok = bit_is_set(targets, trans.target().index())
                        && self.modal_forall_bounded_controllable_rec(
                            trans.target(),
                            guard,
                            targets,
                            remaining - 1,
                            memo,
                        );
                    if !next_ok {
                        memo[idx][remaining as usize] = Some(false);
                        return false;
                    }
                }
            }

            // For controllable transitions, at least one must satisfy
            let mut ctrl_seen = false;
            let mut ctrl_satisfied = false;
            for transition in outgoing {
                if !self.guard_matches(state, transition, guard) {
                    continue;
                }
                if transition.is_controllable(self.clts) {
                    let next = transition.target();
                    let next_ok = bit_is_set(targets, next.index())
                        && self.modal_forall_bounded_controllable_rec(
                            next,
                            guard,
                            targets,
                            remaining - 1,
                            memo,
                        );
                    ctrl_seen = true;
                    if next_ok {
                        ctrl_satisfied = true;
                    }
                }
                // Uncontrollable transitions already handled in groups above
            }
            if ctrl_seen { ctrl_satisfied } else { true }
        };

        memo[idx][remaining as usize] = Some(result);
        result
    }

    fn guard_current_matches(&self, state: StateId<S>, guard: &Guard) -> bool {
        if !guard.current.required.is_empty() {
            let vars = self.clts.state_variable_bitset(state);
            if guard
                .current
                .required
                .iter()
                .any(|var| !vars.contains(var.as_str()))
            {
                return false;
            }
        }

        if !guard.current.forbidden.is_empty() {
            let vars = self.clts.state_variable_bitset(state);
            if guard
                .current
                .forbidden
                .iter()
                .any(|var| vars.contains(var.as_str()))
            {
                return false;
            }
        }

        true
    }

    fn guard_zero_step_allowed(&self, guard: &Guard) -> bool {
        guard.labels.is_empty() && guard.next.required.is_empty() && guard.next.forbidden.is_empty()
    }

    fn guard_matches(
        &self,
        state: StateId<S>,
        transition: &Transition<S, L>,
        guard: &Guard,
    ) -> bool {
        // Label and variable filters are shared with the parity-game module via
        // the free `guard_matches_labels_and_vars` function. Controllability is
        // handled at the `eval_modal` / `modal_exists` / `modal_forall` level
        // before this method is called, so we do not check it here.
        guard_matches_labels_and_vars(state, transition, guard, self.clts)
    }

    fn eval_fixpoint(
        &mut self,
        var: FormulaVarId,
        body: NodeId,
        kind: FixpointKind,
        bindings: &HashMap<FormulaVarId, BitVec<usize, Lsb0>>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let mut current_set = match kind {
            FixpointKind::Least => self.alloc_bitvec(false)?, // ∅
            FixpointKind::Greatest => self.alloc_bitvec(true)?, // States
        };
        let mut iteration: usize = 0;

        loop {
            iteration += 1;
            let mut next_bindings = bindings.clone();
            next_bindings.insert(var, self.clone_bitvec(&current_set)?);
            let next_set = self.eval_node(body, &next_bindings)?;

            // Record iteration ranks for newly entering states (strategy witness data)
            if self.witness_map.is_some() {
                let state_count = next_set.len();
                for state_idx in 0..state_count {
                    let was_in = current_set.get(state_idx).map(|b| *b).unwrap_or(false);
                    let now_in = next_set.get(state_idx).map(|b| *b).unwrap_or(false);
                    if now_in
                        && !was_in
                        && let Some(ref mut wm) = self.witness_map
                    {
                        wm.iteration_ranks.insert((state_idx, var), iteration);
                    }
                }
            }

            if next_set == current_set {
                return self.clone_bitvec(&next_set);
            }

            current_set = self.clone_bitvec(&next_set)?;
        }
    }

    fn guard_partitions(&mut self, guard: &Guard) -> Arc<GuardPartitions> {
        let signature = GuardSignature::new(guard);
        self.guard_cache
            .entry(signature)
            .or_insert_with(|| Arc::new(GuardPartitions::new(guard, self.clts)))
            .clone()
    }

    fn alloc_bitvec(&mut self, fill: bool) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        let mut bits = BitVec::repeat(fill, self.env.state_count());
        if fill {
            // OOB-as-bottom invariant: an "all-true" allocation must NOT include
            // the OOB sink. Otherwise Greatest fixpoints (Nu) and Node::True would
            // initialize with OOB satisfied, breaking the invariant.
            bits.bitand_assign(self.not_oob_bits.as_bitslice());
        }
        Ok(bits)
    }

    fn clone_bitvec(
        &mut self,
        source: &BitVec<usize, Lsb0>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        Ok(source.clone())
    }

    fn predicate_bits(&mut self, name: &str) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        // First check pre-computed predicates
        if let Some(bits) = self.env.predicate(name) {
            let mut out = self.clone_bitvec(bits)?;
            // OOB-as-bottom invariant: pre-computed bitsets may include OOB if
            // the predicate-map population didn't mask it. Re-mask defensively.
            out.bitand_assign(self.not_oob_bits.as_bitslice());
            return Ok(out);
        }

        // Check cache for on-demand evaluation results (already OOB-masked when stored)
        if let Some(bits) = self.expression_eval_cache.get(name).cloned() {
            return Ok(bits);
        }

        // Try on-demand evaluation if abstract states are available
        if self.env.has_abstract_states()
            && let Some(mut bits) = self.evaluate_expression_on_demand(name)?
        {
            bits.bitand_assign(self.not_oob_bits.as_bitslice());
            let cached = bits.clone();
            self.expression_eval_cache.insert(name.to_string(), cached);
            return Ok(bits);
        }

        // SOUNDNESS: under-approx — unknown predicate assumed false (empty bitset).
        // Conservative for universal (box/nu) modalities: if a property holds with
        // fewer predicates satisfied, it holds with more. Unsound for existential
        // (diamond/mu) modalities: a predicate that should be true but is missing
        // could cause a reachable liveness witness to be missed.
        // (The empty bitset is already OOB-clear; no extra masking needed.)
        self.alloc_bitvec(false)
    }

    /// Evaluates a variable expression on-demand over abstract states.
    ///
    /// This function attempts to parse the predicate name as a guard expression
    /// and evaluate it over all abstract states. Returns None if the predicate
    /// cannot be parsed as a variable expression.
    fn evaluate_expression_on_demand(
        &mut self,
        predicate_name: &str,
    ) -> Result<Option<BitVec<usize, Lsb0>>, EvaluationError> {
        let abstract_states = match &self.env.abstract_states {
            Some(states) => states,
            None => return Ok(None),
        };

        // Try to parse predicate name as a guard expression
        // For now, we'll try to detect variable expressions by checking if they
        // contain comparison operators or are simple variable names
        // In the future, this could be enhanced with a registry of expression-to-predicate mappings
        let guard_expr = Self::try_parse_guard_expression(predicate_name)?;
        let guard_expr = match guard_expr {
            Some(expr) => expr,
            None => return Ok(None), // Not a variable expression
        };

        // Evaluate guard over all states
        let mut result = self.alloc_bitvec(false)?;
        let predicates = HashMap::new(); // No external predicates for guard evaluation

        for (state_idx, abstract_state) in abstract_states.iter().enumerate() {
            // Evaluate guard expression on this abstract state
            let guard_result = crate::abstraction::evaluator::evaluate_guard(
                &guard_expr,
                abstract_state,
                &predicates,
            )
            .map_err(|_e| EvaluationError::LimitExceeded {
                kind: "guard evaluation",
                usage: 0,
                limit: 0,
            })?; // TODO: better error handling - abstraction errors need proper conversion

            // Convert guard result to bitset value
            // Conservative strategy: Maybe -> true
            let should_include = matches!(
                guard_result,
                crate::abstraction::expression::GuardResult::AlwaysTrue
                    | crate::abstraction::expression::GuardResult::Maybe
            );

            if should_include && state_idx < result.len() {
                result.set(state_idx, true);
            }
        }

        Ok(Some(result))
    }

    /// Attempts to parse a predicate name as a guard expression.
    ///
    /// Returns Some(GuardExpr) if the predicate appears to be a variable expression,
    /// None otherwise.
    fn try_parse_guard_expression(
        predicate_name: &str,
    ) -> Result<Option<crate::abstraction::expression::GuardExpr>, EvaluationError> {
        // For now, we use a simple heuristic: check if the predicate name
        // contains comparison operators that suggest it's a variable expression
        // In the future, this could be enhanced with a registry or metadata

        // Common patterns: "x > 5", "x >= 0", "x == true", etc.
        // We'll try to parse it as a guard expression
        use crate::guard::parse_guard;

        // Try parsing as a guard expression
        let (_, parsed_guard) = parse_guard(predicate_name);

        // Convert to abstraction GuardExpr
        let guard_expr = match parsed_guard {
            crate::guard::GuardExpr::True => {
                Some(crate::abstraction::expression::GuardExpr::true_guard())
            }
            crate::guard::GuardExpr::False => {
                Some(crate::abstraction::expression::GuardExpr::false_guard())
            }
            crate::guard::GuardExpr::Predicate(name) => {
                // Single identifier - could be a variable name
                // For now, we'll treat simple identifiers as variable references
                // In a more sophisticated system, we'd check if it's a declared variable
                Some(crate::abstraction::expression::GuardExpr::Predicate(name))
            }
            crate::guard::GuardExpr::Comparison { left, op, right } => {
                // This is definitely a variable expression
                // Parse left and right as expressions
                let left_expr = Self::parse_expr_string(&left)?;
                let right_expr = Self::parse_expr_string(&right)?;
                Some(crate::abstraction::expression::GuardExpr::comparison(
                    left_expr, op, right_expr,
                ))
            }
        };

        Ok(guard_expr)
    }

    /// Parses a string expression into an abstraction Expr.
    fn parse_expr_string(
        expr_str: &str,
    ) -> Result<crate::abstraction::expression::Expr, EvaluationError> {
        use crate::abstraction::expression::Expr;

        let trimmed = expr_str.trim();

        // Try parsing as integer constant
        if let Ok(val) = trimmed.parse::<i64>() {
            return Ok(Expr::constant(val));
        }

        // Try parsing as boolean
        match trimmed {
            "true" => return Ok(Expr::bool(true)),
            "false" => return Ok(Expr::bool(false)),
            _ => {}
        }

        // Otherwise treat as variable
        Ok(Expr::var(trimmed))
    }

    fn variable_bits(
        &mut self,
        var: &FormulaVarId,
        bindings: &HashMap<FormulaVarId, BitVec<usize, Lsb0>>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        if let Some(bits) = bindings.get(var) {
            self.clone_bitvec(bits)
        } else {
            self.alloc_bitvec(false)
        }
    }

    // -----------------------------------------------------------------------
    // Three-valued (Kleene) evaluator — runs alongside the BitVec one.
    // -----------------------------------------------------------------------

    /// Compute the modal-result BitVec given a precomputed target bitset.
    ///
    /// Used by the three-valued evaluator: for each modal node, the trit
    /// evaluator computes the target's `TritSet`, then calls this helper twice
    /// — once with `target.must` and once with `target.may` — and recombines.
    /// Modal operators decompose cleanly into two parallel BitVec evaluations
    /// because they do not mix `must` and `may` (unlike Not).
    ///
    /// Witness recording is intentionally skipped on this path; witnesses are
    /// only meaningful for the BitVec evaluator's positional strategy
    /// extraction.
    fn modal_bits_from_target(
        &mut self,
        kind: ModalKind,
        guard: &Guard,
        target_set: &BitVec<usize, Lsb0>,
    ) -> Result<BitVec<usize, Lsb0>, EvaluationError> {
        if let Some(bound) = guard.max_steps {
            return self.eval_modal_bounded(kind, guard, target_set, bound);
        }

        let mut result = self.alloc_bitvec(false)?;
        let guard_parts = if self.options.use_partitions {
            Some(self.guard_partitions(guard))
        } else {
            None
        };

        // NodeId is only used by modal_exists/modal_forall for an unused
        // `_modal_node_id` parameter (the witness map is consulted by the
        // caller, not here). Pass NodeId(0) as a placeholder.
        let placeholder = NodeId(0);

        for state in self.clts.states() {
            let satisfies = match kind {
                ModalKind::Diamond => self.modal_exists(
                    state,
                    guard,
                    target_set,
                    guard_parts.as_deref(),
                    placeholder,
                ),
                ModalKind::Box => self.modal_forall(
                    state,
                    guard,
                    target_set,
                    guard_parts.as_deref(),
                    placeholder,
                ),
            };
            if satisfies {
                result.set(state.index(), true);
            }
        }
        Ok(result)
    }

    /// Evaluate a formula node under three-valued (Kleene) semantics.
    ///
    /// Mirrors [`Self::eval_node`] but operates on [`super::trit::TritSet`]
    /// instead of `BitVec`. For modal nodes, the target's TritSet is decomposed
    /// into `(must, may)` and each is fed through `modal_bits_from_target`
    /// independently — the parallel-evaluation strategy is sound because modal
    /// operators do not mix polarity. Boolean Not is handled by the `TritSet`
    /// type, which swaps `(must, may)` and complements per Kleene semantics.
    fn eval_node_tri(
        &mut self,
        node_id: NodeId,
        bindings: &HashMap<FormulaVarId, super::trit::TritSet>,
    ) -> Result<super::trit::TritSet, EvaluationError> {
        match self.formula.node(node_id) {
            Node::True => Ok(super::trit::TritSet::all_true(
                self.env.state_count(),
                &self.oob_bits,
            )),
            Node::False => Ok(super::trit::TritSet::all_false(self.env.state_count())),
            Node::Predicate(name) => {
                // predicate_bits already masks OOB out (Phase 3) — that's the
                // must bitset. from_predicate sets OOB in may, giving Unknown
                // at OOB.
                let bits = self.predicate_bits(name)?;
                Ok(super::trit::TritSet::from_predicate(bits, &self.oob_bits))
            }
            Node::Variable(var) => {
                if let Some(t) = bindings.get(var) {
                    Ok(t.clone())
                } else {
                    Ok(super::trit::TritSet::all_false(self.env.state_count()))
                }
            }
            Node::Not(inner) => {
                let t = self.eval_node_tri(*inner, bindings)?;
                Ok(t.not())
            }
            Node::And(left, right) => {
                let l = self.eval_node_tri(*left, bindings)?;
                let r = self.eval_node_tri(*right, bindings)?;
                Ok(l.and(&r))
            }
            Node::Or(left, right) => {
                let l = self.eval_node_tri(*left, bindings)?;
                let r = self.eval_node_tri(*right, bindings)?;
                Ok(l.or(&r))
            }
            Node::Modal {
                kind,
                guard,
                target,
            } => {
                let target_tri = self.eval_node_tri(*target, bindings)?;
                let must_target = target_tri.must_true().clone();
                let may_target = target_tri.may_true().clone();
                let must_bits = self.modal_bits_from_target(*kind, guard, &must_target)?;
                let may_bits = self.modal_bits_from_target(*kind, guard, &may_target)?;
                Ok(super::trit::TritSet::from_parts(must_bits, may_bits))
            }
            Node::Mu { var, body } => {
                self.eval_fixpoint_tri(*var, *body, FixpointKind::Least, bindings)
            }
            Node::Nu { var, body } => {
                self.eval_fixpoint_tri(*var, *body, FixpointKind::Greatest, bindings)
            }
        }
    }

    /// Compute a TritSet fixpoint by Kleene iteration.
    ///
    /// `Least` (μ) starts at the all-False trit set. `Greatest` (ν) starts at
    /// the all-True trit set with OOB held as Unknown. Iterates the body until
    /// both `must` and `may` stabilize.
    fn eval_fixpoint_tri(
        &mut self,
        var: FormulaVarId,
        body: NodeId,
        kind: FixpointKind,
        bindings: &HashMap<FormulaVarId, super::trit::TritSet>,
    ) -> Result<super::trit::TritSet, EvaluationError> {
        let mut current = match kind {
            FixpointKind::Least => super::trit::TritSet::all_false(self.env.state_count()),
            FixpointKind::Greatest => {
                super::trit::TritSet::all_true(self.env.state_count(), &self.oob_bits)
            }
        };
        loop {
            let mut next_bindings = bindings.clone();
            next_bindings.insert(var, current.clone());
            let next = self.eval_node_tri(body, &next_bindings)?;
            if current.eq_set(&next) {
                return Ok(next);
            }
            current = next;
        }
    }
}

enum FixpointKind {
    Least,
    Greatest,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GuardSignature {
    labels: Vec<String>,
    current_required: Vec<String>,
    current_forbidden: Vec<String>,
    next_required: Vec<String>,
    next_forbidden: Vec<String>,
    max_steps: Option<u32>,
}

impl GuardSignature {
    fn new(guard: &Guard) -> Self {
        Self {
            labels: sorted(&guard.labels),
            current_required: sorted(&guard.current.required),
            current_forbidden: sorted(&guard.current.forbidden),
            next_required: sorted(&guard.next.required),
            next_forbidden: sorted(&guard.next.forbidden),
            max_steps: guard.max_steps,
        }
    }
}

#[derive(Debug, Clone)]
struct GuardPartitions {
    current_required: BitVec<usize, Lsb0>,
    current_forbidden: BitVec<usize, Lsb0>,
    next_required: BitVec<usize, Lsb0>,
    next_forbidden: BitVec<usize, Lsb0>,
}

impl GuardPartitions {
    fn new<S, L>(guard: &Guard, clts: &Clts<S, L>) -> Self
    where
        S: IdStorage,
        L: IdStorage,
    {
        let state_count = clts.state_count();
        let mut current_required = BitVec::repeat(true, state_count);
        let mut current_forbidden = BitVec::repeat(true, state_count);
        let mut next_required = BitVec::repeat(true, state_count);
        let mut next_forbidden = BitVec::repeat(true, state_count);

        for state in clts.states() {
            let idx = state.index();
            let vars = clts.state_variable_bitset(state);

            if !guard.current.required.is_empty()
                && guard
                    .current
                    .required
                    .iter()
                    .any(|var| !vars.contains(var.as_str()))
            {
                current_required.set(idx, false);
            }

            if !guard.current.forbidden.is_empty()
                && guard
                    .current
                    .forbidden
                    .iter()
                    .any(|var| vars.contains(var.as_str()))
            {
                current_forbidden.set(idx, false);
            }

            if !guard.next.required.is_empty()
                && guard
                    .next
                    .required
                    .iter()
                    .any(|var| !vars.contains(var.as_str()))
            {
                next_required.set(idx, false);
            }

            if !guard.next.forbidden.is_empty()
                && guard
                    .next
                    .forbidden
                    .iter()
                    .any(|var| vars.contains(var.as_str()))
            {
                next_forbidden.set(idx, false);
            }
        }

        Self {
            current_required,
            current_forbidden,
            next_required,
            next_forbidden,
        }
    }

    fn matches_current(&self, idx: usize) -> bool {
        bit_is_set(&self.current_required, idx) && bit_is_set(&self.current_forbidden, idx)
    }

    fn matches_next(&self, idx: usize) -> bool {
        bit_is_set(&self.next_required, idx) && bit_is_set(&self.next_forbidden, idx)
    }
}

fn sorted(values: &[String]) -> Vec<String> {
    let mut out = values.to_vec();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
    use crate::mu_calculus::parser;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn build_simple_clts() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
        let mut builder = Clts::builder();
        builder.state("s0").initial("s0");
        builder.state("s1");
        builder.state("s2");

        builder.with_variables("s0", ["flag"]);
        builder.with_variables("s2", ["flag"]);

        let tick = builder.labels().intern(["tick"]).unwrap();
        let sync = builder.labels().intern(["sync"]).unwrap();

        let s0 = builder.state_id_or_insert("s0").unwrap();
        let s1 = builder.state_id_or_insert("s1").unwrap();
        let s2 = builder.state_id_or_insert("s2").unwrap();

        builder.set_label_controllability(sync, LabelControllability::Uncontrollable);
        builder.transition_ids(s0, &[tick], s1);
        builder.transition_ids(s1, &[sync], s2);

        builder.build().expect("fixture CLTS builds")
    }

    #[test]
    fn diamond_matches_controllable_transition() -> TestResult {
        let clts = build_simple_clts();
        let formula = parser::parse("< labels = {tick} > true")?;
        let env = Environment::new(clts.state_count());

        let result = evaluate(&formula, &clts, &env)?;
        let s0 = clts.state_id("s0")?;
        let s1 = clts.state_id("s1")?;

        assert!(bit_is_set(&result, s0.index()));
        assert!(!bit_is_set(&result, s1.index()));

        Ok(())
    }

    #[test]
    fn diamond_with_step_bound_finds_goal() -> TestResult {
        let clts = build_simple_clts();
        let s0 = clts.state_id("s0")?;
        let s2 = clts.state_id("s2")?;

        let mut goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        goal.set(s2.index(), true);

        let env = Environment::new(clts.state_count()).with_predicate("goal", goal);

        let within_two = parser::parse("< ( steps <= 2 ) > goal")?;
        let result = evaluate(&within_two, &clts, &env)?;
        assert!(bit_is_set(&result, s0.index()));

        let within_one = parser::parse("< ( steps <= 1 ) > goal")?;
        let result_fail = evaluate(&within_one, &clts, &env)?;
        assert!(!bit_is_set(&result_fail, s0.index()));

        Ok(())
    }

    #[test]
    fn diamond_zero_steps_checks_current_state() -> TestResult {
        let clts = build_simple_clts();
        let s0 = clts.state_id("s0")?;

        let mut goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        goal.set(s0.index(), true);
        let env = Environment::new(clts.state_count()).with_predicate("goal", goal);

        let formula = parser::parse("< ( steps <= 0 ) > goal")?;
        let result = evaluate(&formula, &clts, &env)?;

        assert!(bit_is_set(&result, s0.index()));
        Ok(())
    }

    #[test]
    fn box_controllable_requires_successful_choice() -> TestResult {
        let clts = build_simple_clts();
        let formula = parser::parse("[ ( labels = {tick}, ctrl = controllable ) ] true")?;
        let env = Environment::new(clts.state_count());

        let result = evaluate(&formula, &clts, &env)?;
        let s0 = clts.state_id("s0")?;
        let s1 = clts.state_id("s1")?;

        assert!(bit_is_set(&result, s0.index()));
        assert!(bit_is_set(&result, s1.index()));

        Ok(())
    }

    #[test]
    fn least_fixpoint_stabilises_to_empty_set() -> TestResult {
        let clts = build_simple_clts();
        let formula = parser::parse("mu X. < labels = {tick} > X")?;
        let env = Environment::new(clts.state_count());

        let result = evaluate(&formula, &clts, &env)?;
        let s0 = clts.state_id("s0")?;
        let s1 = clts.state_id("s1")?;

        assert!(!bit_is_set(&result, s0.index()));
        assert!(!bit_is_set(&result, s1.index()));

        Ok(())
    }

    #[test]
    fn skolem_paradigm_groups_uncontrollable_transitions() -> TestResult {
        // Build a CLTS with multiple uncontrollable transitions sharing the same labels
        // and a controllable transition with the same labels
        // This tests the Skolem paradigm: for all non-controllable choices,
        // there exists one controllable choice that satisfies
        let mut builder = Clts::builder();
        builder.state("s0");
        builder.state("s1");
        builder.state("s2");
        builder.state("s3");

        let input_label = builder.labels().intern(["input_signal"])?;

        let s0 = builder.state_id_or_insert("s0").unwrap();
        let s1 = builder.state_id_or_insert("s1").unwrap();
        let s2 = builder.state_id_or_insert("s2").unwrap();
        let s3 = builder.state_id_or_insert("s3").unwrap();

        // Two uncontrollable transitions from s0 sharing the same input label
        builder.set_label_controllability(input_label, LabelControllability::Uncontrollable);
        builder.transition_ids(s0, &[input_label], s1);
        builder.transition_ids(s0, &[input_label], s2);

        // One controllable transition from s0 with the same input label (system can choose)
        // For this to be controllable, we need a different label or make input_label controllable
        // Since we want to test the grouping, we'll add a second controllable label
        let action_label = builder.labels().intern(["action"])?;
        builder.transition_ids(s0, &[input_label, action_label], s3);

        // s3 is the goal state
        let clts = builder.build()?;

        // Create a goal set with only s3
        let mut goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        let s3_id = clts.state_id("s3")?;
        goal.set(s3_id.index(), true);

        let formula = parser::parse("< labels = {input_signal} > goal")?;
        let goal_env = Environment::new(clts.state_count()).with_predicate("goal", goal);

        let result = evaluate(&formula, &clts, &goal_env)?;

        // s0 should satisfy because there exists a controllable transition (s0 -> s3)
        // that satisfies, even though the uncontrollable transitions (s0 -> s1, s0 -> s2) don't
        let s0_id = clts.state_id("s0")?;
        assert!(
            result.get(s0_id.index()).is_some_and(|bit| *bit),
            "s0 should satisfy: exists controllable transition to s3"
        );

        Ok(())
    }

    #[test]
    fn skolem_paradigm_requires_controllable_choice_for_uncontrollable_group() -> TestResult {
        // Test that if all uncontrollable transitions in a group fail,
        // we need at least one controllable transition with the same labels
        let mut builder = Clts::builder();
        builder.state("s0");
        builder.state("s1");
        builder.state("s2");

        let input_label = builder.labels().intern(["input_signal"])?;

        let s0 = builder.state_id_or_insert("s0").unwrap();
        let s1 = builder.state_id_or_insert("s1").unwrap();
        let s2 = builder.state_id_or_insert("s2").unwrap();

        // Two uncontrollable transitions from s0 sharing the same input label
        // Both lead to non-goal states
        builder.set_label_controllability(input_label, LabelControllability::Uncontrollable);
        builder.transition_ids(s0, &[input_label], s1);
        builder.transition_ids(s0, &[input_label], s2);

        // No controllable transition with the same labels
        let clts = builder.build()?;

        // Create a goal set with no states (unreachable)
        let goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());

        let formula = parser::parse("< labels = {input_signal} > goal")?;
        let goal_env = Environment::new(clts.state_count()).with_predicate("goal", goal);

        let result = evaluate(&formula, &clts, &goal_env)?;

        // s0 should NOT satisfy because:
        // 1. Uncontrollable transitions (s0 -> s1, s0 -> s2) don't lead to goal
        // 2. No controllable transition with same labels exists
        let s0_id = clts.state_id("s0")?;
        assert!(
            result.get(s0_id.index()).is_some_and(|bit| !*bit),
            "s0 should not satisfy: no controllable choice available"
        );

        Ok(())
    }

    #[test]
    fn predicate_retrieval() -> TestResult {
        // Test predicate() method coverage
        let clts = build_simple_clts();
        let mut env = Environment::new(clts.state_count());

        let mut pred = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred.set(0, true);
        env = env.with_predicate("test_pred", pred.clone());

        // Test retrieving existing predicate
        let retrieved = env.predicate("test_pred");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), &pred);

        // Test retrieving non-existing predicate
        assert!(env.predicate("nonexistent").is_none());

        Ok(())
    }

    #[test]
    fn evaluation_with_memoization() -> TestResult {
        // Test evaluate_with_options with memoization enabled
        let clts = build_simple_clts();
        let env = Environment::new(clts.state_count());
        let formula = parser::parse("< labels = {tick} > true")?;

        let options = EvaluationOptions {
            use_memoisation: true,
            ..Default::default()
        };

        let eval_result = evaluate_with_options(&formula, &clts, &env, &options)?;
        let s0 = clts.state_id("s0")?;
        assert!(bit_is_set(&eval_result, s0.index()));

        // Second evaluation should use memoization
        let eval_result2 = evaluate_with_options(&formula, &clts, &env, &options)?;
        assert_eq!(eval_result, eval_result2);

        Ok(())
    }

    #[test]
    fn evaluation_with_guard_partitions() -> TestResult {
        // Test evaluate_with_options with guard partitions
        let clts = build_simple_clts();
        let env = Environment::new(clts.state_count());
        let formula = parser::parse("< ( req_cur = {flag} ) > true")?;

        let options = EvaluationOptions {
            use_partitions: true,
            ..Default::default()
        };

        let eval_result = evaluate_with_options(&formula, &clts, &env, &options)?;
        let s0 = clts.state_id("s0")?;

        // States with 'flag' variable that have outgoing transitions should satisfy
        // s0 has flag and has outgoing transition, so it should satisfy
        assert!(bit_is_set(&eval_result, s0.index()));

        // Verify partitions are being used (result should match non-partition version)
        let default_result = evaluate(&formula, &clts, &env)?;
        assert_eq!(eval_result, default_result);

        Ok(())
    }

    #[test]
    fn variable_binding_in_formula() -> TestResult {
        // Test variable evaluation with bindings in fixpoint
        let clts = build_simple_clts();
        let env = Environment::new(clts.state_count());

        // Formula with variable: mu X. (<tick>X || true)
        // This is a least fixpoint that finds states that can eventually reach a state satisfying true
        // Since s0 can transition to s1, and s1 satisfies true, s0 should be in the fixpoint
        let formula = parser::parse("mu X. (< labels = {tick} > X || true)")?;

        let result = evaluate(&formula, &clts, &env)?;
        let s0 = clts.state_id("s0")?;

        // s0 can transition to s1, which satisfies true, so s0 should be in the fixpoint
        assert!(bit_is_set(&result, s0.index()));

        Ok(())
    }

    #[test]
    fn bitwise_operations() -> TestResult {
        // Test bitwise AND, OR, NOT operations
        let clts = build_simple_clts();
        let mut env = Environment::new(clts.state_count());

        let mut pred1 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred1.set(0, true); // s0
        env = env.with_predicate("p1", pred1);

        let mut pred2 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred2.set(2, true); // s2
        env = env.with_predicate("p2", pred2);

        // Test AND: p1 && p2 (should be empty)
        let and_formula = parser::parse("p1 && p2")?;
        let and_result = evaluate(&and_formula, &clts, &env)?;
        assert_eq!(and_result.count_ones(), 0);

        // Test OR: p1 || p2 (should have s0 and s2)
        let or_formula = parser::parse("p1 || p2")?;
        let or_result = evaluate(&or_formula, &clts, &env)?;
        assert!(bit_is_set(&or_result, 0));
        assert!(bit_is_set(&or_result, 2));

        // Test NOT: !p1 (should have s1 and s2)
        let not_formula = parser::parse("!p1")?;
        let not_result = evaluate(&not_formula, &clts, &env)?;
        assert!(!bit_is_set(&not_result, 0));
        assert!(bit_is_set(&not_result, 1));
        assert!(bit_is_set(&not_result, 2));

        Ok(())
    }

    #[test]
    fn greatest_fixpoint_evaluation() -> TestResult {
        // Test nu (greatest fixpoint) evaluation
        let clts = build_simple_clts();
        let env = Environment::new(clts.state_count());

        // nu X. (<tick>X || true) - should include all states reachable via tick
        let formula = parser::parse("nu X. (< labels = {tick} > X || true)")?;
        let result = evaluate(&formula, &clts, &env)?;

        let s0 = clts.state_id("s0")?;
        let s1 = clts.state_id("s1")?;

        // Greatest fixpoint should stabilize to all states
        assert!(bit_is_set(&result, s0.index()));
        assert!(bit_is_set(&result, s1.index()));

        Ok(())
    }

    #[test]
    fn bounded_evaluation_edge_cases() -> TestResult {
        // Test bounded evaluation with various step counts
        let clts = build_simple_clts();
        let mut env = Environment::new(clts.state_count());

        let mut goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        goal.set(2, true); // s2
        env = env.with_predicate("goal", goal);

        let s0 = clts.state_id("s0")?;

        // Test with steps = 0 (should fail, need 2 steps)
        let formula0 = parser::parse("< ( steps <= 0 ) > goal")?;
        let result0 = evaluate(&formula0, &clts, &env)?;
        assert!(!bit_is_set(&result0, s0.index()));

        // Test with steps = 1 (should fail, need 2 steps)
        let formula1 = parser::parse("< ( steps <= 1 ) > goal")?;
        let result1 = evaluate(&formula1, &clts, &env)?;
        assert!(!bit_is_set(&result1, s0.index()));

        // Test with steps = 2 (should succeed)
        let formula2 = parser::parse("< ( steps <= 2 ) > goal")?;
        let result2 = evaluate(&formula2, &clts, &env)?;
        assert!(bit_is_set(&result2, s0.index()));

        Ok(())
    }

    #[test]
    fn modal_with_complex_guards() -> TestResult {
        // Test modal operators with complex guard conditions
        let clts = build_simple_clts();
        let env = Environment::new(clts.state_count());

        // Test diamond with required current variable
        // <req_cur={flag}>true means: can transition from a state with 'flag' variable
        let formula1 = parser::parse("< ( req_cur = {flag} ) > true")?;
        let result1 = evaluate(&formula1, &clts, &env)?;
        let s0 = clts.state_id("s0")?;
        // s0 has flag and has outgoing transition, so it should satisfy
        assert!(bit_is_set(&result1, s0.index()));
        // s2 has flag but no outgoing transitions, so it shouldn't satisfy
        // (the guard requires a transition, not just the variable)

        // Test box with required labels
        let formula2 = parser::parse("[ labels = {tick} ] true")?;
        let result2 = evaluate(&formula2, &clts, &env)?;
        assert!(bit_is_set(&result2, s0.index())); // s0 has tick transition

        Ok(())
    }

    #[test]
    fn skolem_paradigm_with_controllable_alternative() -> TestResult {
        // Test Skolem paradigm: uncontrollable group with controllable alternative
        let mut builder = Clts::builder();
        builder.state("s0").initial("s0");
        builder.state("s1");
        builder.state("s2");

        let input_label = builder.labels().intern(["input"]).unwrap();
        let s0 = builder.state_id_or_insert("s0").unwrap();
        let s1 = builder.state_id_or_insert("s1").unwrap();
        let s2 = builder.state_id_or_insert("s2").unwrap();

        // Uncontrollable transition: s0 -> s1 with input
        builder.set_label_controllability(input_label, LabelControllability::Uncontrollable);
        builder.transition_ids(s0, &[input_label], s1);
        // Controllable transition: s0 -> s2 with same input label + action
        let action_label = builder.labels().intern(["action"])?;
        builder.transition_ids(s0, &[input_label, action_label], s2);

        let clts = builder.build().expect("CLTS builds");
        let env = Environment::new(clts.state_count());

        // Formula: <input>true - should be satisfiable because controllable alternative exists
        let formula = parser::parse("< labels = {input} > true")?;
        let result = evaluate(&formula, &clts, &env)?;

        assert!(bit_is_set(&result, s0.index()));

        Ok(())
    }

    #[test]
    fn skolem_paradigm_two_groups_one_satisfying_controllable() -> TestResult {
        // Test Skolem paradigm with one state, four transitions, two groups
        // Each group shares non-controllable elements, but only one controllable action satisfies
        //
        // Structure:
        // - State s0 (single state)
        // - Group 1 (shares "input_a"):
        //   - s0 -> s1 (uncontrollable, input_a) - does NOT satisfy formula
        //   - s0 -> s2 (controllable, input_a) - DOES satisfy formula
        // - Group 2 (shares "input_b"):
        //   - s0 -> s3 (uncontrollable, input_b) - does NOT satisfy formula
        //   - s0 -> s4 (controllable, input_b) - DOES satisfy formula
        //
        // The formula should be satisfied at s0 because each group has at least one
        // satisfying transition (the controllable ones), following the Skolem paradigm.

        let mut builder = Clts::builder();
        builder.state("s0");
        builder.state("s1"); // Group 1: uncontrollable, does not satisfy
        builder.state("s2"); // Group 1: controllable, satisfies
        builder.state("s3"); // Group 2: uncontrollable, does not satisfy
        builder.state("s4"); // Group 2: controllable, satisfies

        let input_a_label = builder.labels().intern(["input_a"])?;
        let input_b_label_id = builder.labels().intern(["input_b"])?;

        let s0 = builder.state_id_or_insert("s0").unwrap();
        let s1 = builder.state_id_or_insert("s1").unwrap();
        let s2 = builder.state_id_or_insert("s2").unwrap();
        let s3 = builder.state_id_or_insert("s3").unwrap();
        let s4 = builder.state_id_or_insert("s4").unwrap();

        // Group 1: transitions sharing "input_a"
        builder.set_label_controllability(input_a_label, LabelControllability::Uncontrollable);
        builder.transition_ids(s0, &[input_a_label], s1);
        // For controllable alternative, add a second label
        let action_a = builder.labels().intern(["action_a"])?;
        builder.transition_ids(s0, &[input_a_label, action_a], s2);

        // Group 2: transitions sharing "input_b"
        builder.set_label_controllability(input_b_label_id, LabelControllability::Uncontrollable);
        builder.transition_ids(s0, &[input_b_label_id], s3);
        // For controllable alternative, add a second label
        let action_b = builder.labels().intern(["action_b"])?;
        builder.transition_ids(s0, &[input_b_label_id, action_b], s4);

        let clts = builder.build()?;

        // Create goal set: only s2 and s4 satisfy the formula
        let mut goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        let s2_id = clts.state_id("s2")?;
        let s4_id = clts.state_id("s4")?;
        goal.set(s2_id.index(), true);
        goal.set(s4_id.index(), true);

        let goal_env = Environment::new(clts.state_count()).with_predicate("goal", goal);

        // Formula: <input_a>goal || <input_b>goal
        // This should be satisfied at s0 because:
        // - Group 1 (input_a): controllable transition s0->s2 satisfies
        // - Group 2 (input_b): controllable transition s0->s4 satisfies
        let formula = parser::parse("< labels = {input_a} > goal || < labels = {input_b} > goal")?;
        let result = evaluate(&formula, &clts, &goal_env)?;

        let s0_id = clts.state_id("s0")?;
        assert!(
            bit_is_set(&result, s0_id.index()),
            "s0 should satisfy the formula because each group has a satisfying controllable transition"
        );

        // Verify that s1 and s3 (non-satisfying states) are not in the result
        let s1_id = clts.state_id("s1")?;
        let s3_id = clts.state_id("s3")?;
        assert!(!bit_is_set(&result, s1_id.index()));
        assert!(!bit_is_set(&result, s3_id.index()));

        // Also test with a formula that requires BOTH groups to be satisfied
        // Formula: <input_a>goal && <input_b>goal
        // This should also be satisfied at s0 because both groups have satisfying transitions
        let formula_both =
            parser::parse("< labels = {input_a} > goal && < labels = {input_b} > goal")?;
        let result_both = evaluate(&formula_both, &clts, &goal_env)?;
        assert!(
            bit_is_set(&result_both, s0_id.index()),
            "s0 should satisfy the conjunction because both groups have satisfying controllable transitions"
        );

        // Test that if no states satisfy the goal, the formula fails
        let no_goal = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        let no_goal_env = Environment::new(clts.state_count()).with_predicate("no_goal", no_goal);

        let formula_no_goal =
            parser::parse("< labels = {input_a} > no_goal || < labels = {input_b} > no_goal")?;
        let result_no_goal = evaluate(&formula_no_goal, &clts, &no_goal_env)?;
        assert!(
            !bit_is_set(&result_no_goal, s0_id.index()),
            "s0 should NOT satisfy when no states satisfy the goal"
        );

        Ok(())
    }

    #[test]
    fn memoisation_and_partitions_preserve_semantics() -> TestResult {
        let clts = build_simple_clts();
        let env = Environment::new(clts.state_count());

        let formula = parser::parse("< ( req_cur = {flag} ) > true")?;
        let default_result = evaluate(&formula, &clts, &env)?;
        let custom_opts = EvaluationOptions {
            use_memoisation: false,
            use_partitions: false,
        };
        let no_cache_result = evaluate_with_options(&formula, &clts, &env, &custom_opts)?;

        assert_eq!(default_result, no_cache_result);

        let s0 = clts.state_id("s0")?;
        let s1 = clts.state_id("s1")?;

        assert!(bit_is_set(&default_result, s0.index()));
        assert!(!bit_is_set(&default_result, s1.index()));
        Ok(())
    }

    #[test]
    fn greatest_fixpoint_with_box_modality() -> TestResult {
        // Test: nu X. (has_enabled || is_completion) && [] X
        // This should satisfy all states: Start, Do_Work, End
        // This test reproduces the bug where only End satisfies when all should
        let mut builder = Clts::builder();
        builder.state("Start").initial("Start");
        builder.state("Do_Work");
        builder.state("End");
        let tick = builder.labels().intern(["tick"])?;
        builder.transition("Start", &[tick], "Do_Work");
        builder.transition("Do_Work", &[tick], "End");
        let clts = builder.build()?;

        let mut env = Environment::new(clts.state_count());

        // Set up predicates
        let mut has_enabled = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        has_enabled.set(clts.state_id("Start")?.index(), true);
        has_enabled.set(clts.state_id("Do_Work")?.index(), true);
        env = env.with_predicate("has_enabled", has_enabled);

        let mut is_completion = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        is_completion.set(clts.state_id("End")?.index(), true);
        env = env.with_predicate("is_completion", is_completion);

        // Formula: nu X. (has_enabled || is_completion) && [] X
        // Note: Need parentheses to ensure fixpoint binds the entire And expression
        let formula = parser::parse("nu X. ((has_enabled || is_completion) && [] X)")?;
        let result = evaluate(&formula, &clts, &env)?;

        let start_idx = clts.state_id("Start")?.index();
        let do_work_idx = clts.state_id("Do_Work")?.index();
        let end_idx = clts.state_id("End")?.index();

        // All states should satisfy
        assert!(
            result.get(start_idx).map(|b| *b).unwrap_or(false),
            "Start should satisfy"
        );
        assert!(
            result.get(do_work_idx).map(|b| *b).unwrap_or(false),
            "Do_Work should satisfy"
        );
        assert!(
            result.get(end_idx).map(|b| *b).unwrap_or(false),
            "End should satisfy"
        );

        Ok(())
    }

    #[test]
    fn greatest_fixpoint_with_and_and_box_modality() -> TestResult {
        // Test: nu X. (pred1 && pred2) && [] X
        // This tests the same bitset reuse fixes but with AND instead of OR
        // Key: All states must have both predicates AND all successors must satisfy X
        let mut builder = Clts::builder();
        builder.state("Start").initial("Start");
        builder.state("End");
        let tick = builder.labels().intern(["tick"])?;
        builder.transition("Start", &[tick], "End");
        let clts = builder.build()?;

        let mut env = Environment::new(clts.state_count());

        // Set up predicates: both true for Start and End
        let mut pred1 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred1.set(clts.state_id("Start")?.index(), true);
        pred1.set(clts.state_id("End")?.index(), true);
        env = env.with_predicate("pred1", pred1);

        let mut pred2 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred2.set(clts.state_id("Start")?.index(), true);
        pred2.set(clts.state_id("End")?.index(), true);
        env = env.with_predicate("pred2", pred2);

        // Formula: nu X. (pred1 && pred2) && [] X
        // Both Start and End have (pred1 && pred2)
        // Start: [] X means End must satisfy X (which it does)
        // End: [] X is vacuously true (no outgoing transitions)
        // So both should satisfy
        let formula = parser::parse("nu X. ((pred1 && pred2) && [] X)")?;
        let result = evaluate(&formula, &clts, &env)?;

        let start_idx = clts.state_id("Start")?.index();
        let end_idx = clts.state_id("End")?.index();

        // Both should satisfy
        assert!(
            result.get(start_idx).map(|b| *b).unwrap_or(false),
            "Start should satisfy"
        );
        assert!(
            result.get(end_idx).map(|b| *b).unwrap_or(false),
            "End should satisfy"
        );

        Ok(())
    }

    #[test]
    fn least_fixpoint_with_or_and_box_modality() -> TestResult {
        // Test: mu X. (pred1 || pred2) && [] X
        // Tests least fixpoint with OR and box modality
        // Key: States with (pred1 || pred2) AND all successors satisfy X
        let mut builder = Clts::builder();
        builder.state("Start").initial("Start");
        builder.state("End");
        let tick = builder.labels().intern(["tick"])?;
        builder.transition("Start", &[tick], "End");
        let clts = builder.build()?;

        let mut env = Environment::new(clts.state_count());

        // Set up predicates: pred1 for Start, pred2 for End
        let mut pred1 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred1.set(clts.state_id("Start")?.index(), true);
        env = env.with_predicate("pred1", pred1);

        let mut pred2 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred2.set(clts.state_id("End")?.index(), true);
        env = env.with_predicate("pred2", pred2);

        // Formula: mu X. (pred1 || pred2) && [] X
        // Least fixpoint: Start has pred1, End has pred2
        // Start: (pred1 || pred2) = true, [] X means End must satisfy X (which it does)
        // End: (pred1 || pred2) = true (pred2), [] X is vacuously true
        // So both should satisfy
        let formula = parser::parse("mu X. ((pred1 || pred2) && [] X)")?;
        let result = evaluate(&formula, &clts, &env)?;

        let start_idx = clts.state_id("Start")?.index();
        let end_idx = clts.state_id("End")?.index();

        // Both should satisfy
        assert!(
            result.get(start_idx).map(|b| *b).unwrap_or(false),
            "Start should satisfy"
        );
        assert!(
            result.get(end_idx).map(|b| *b).unwrap_or(false),
            "End should satisfy"
        );

        Ok(())
    }

    #[test]
    fn fixpoint_with_nested_bitwise_operations() -> TestResult {
        // Test: nu X. ((pred1 || pred2) && (pred3 || pred4)) && [] X
        // Tests fixpoint with nested bitwise operations to ensure bitset reuse is fixed
        let mut builder = Clts::builder();
        builder.state("Start").initial("Start");
        builder.state("End");
        let tick = builder.labels().intern(["tick"])?;
        builder.transition("Start", &[tick], "End");
        let clts = builder.build()?;

        let mut env = Environment::new(clts.state_count());

        // Set up predicates: Start has pred1 and pred3, End has pred2 and pred4
        let mut pred1 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred1.set(clts.state_id("Start")?.index(), true);
        env = env.with_predicate("pred1", pred1);

        let mut pred2 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred2.set(clts.state_id("End")?.index(), true);
        env = env.with_predicate("pred2", pred2);

        let mut pred3 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred3.set(clts.state_id("Start")?.index(), true);
        env = env.with_predicate("pred3", pred3);

        let mut pred4 = BitVec::<usize, Lsb0>::repeat(false, clts.state_count());
        pred4.set(clts.state_id("End")?.index(), true);
        env = env.with_predicate("pred4", pred4);

        // Formula: nu X. ((pred1 || pred2) && (pred3 || pred4)) && [] X
        // Start: (pred1 || pred2) = true (pred1), (pred3 || pred4) = true (pred3), [] X = true (End satisfies X)
        // End: (pred1 || pred2) = true (pred2), (pred3 || pred4) = true (pred4), [] X = true (vacuously)
        // Both should satisfy
        let formula = parser::parse("nu X. (((pred1 || pred2) && (pred3 || pred4)) && [] X)")?;
        let result = evaluate(&formula, &clts, &env)?;

        let start_idx = clts.state_id("Start")?.index();
        let end_idx = clts.state_id("End")?.index();

        // Both states should satisfy
        assert!(
            result.get(start_idx).map(|b| *b).unwrap_or(false),
            "Start should satisfy"
        );
        assert!(
            result.get(end_idx).map(|b| *b).unwrap_or(false),
            "End should satisfy"
        );

        Ok(())
    }

    #[test]
    fn fixpoint_with_diamond_and_box_modalities() -> TestResult {
        // Test: nu X. (<tick>X && [tick]X)
        // Tests fixpoint with both diamond and box modalities
        // This verifies that bitset reuse is fixed when evaluating both modalities
        // We use a cycle so both states can satisfy the fixpoint
        let mut builder = Clts::builder();
        builder.state("Start").initial("Start");
        builder.state("End");
        let tick = builder.labels().intern(["tick"])?;
        builder.transition("Start", &[tick], "End");
        builder.transition("End", &[tick], "Start");
        let clts = builder.build()?;

        let env = Environment::new(clts.state_count());

        // Formula: nu X. (<tick>X && [tick]X)
        // Greatest fixpoint with cycle: both states can reach each other
        // Start: <tick>X = true (End satisfies X), [tick]X = true (End satisfies X)
        // End: <tick>X = true (Start satisfies X), [tick]X = true (Start satisfies X)
        // Both should satisfy
        let formula = parser::parse("nu X. (< labels = {tick} > X && [ labels = {tick} ] X)")?;
        let result = evaluate(&formula, &clts, &env)?;

        let start_idx = clts.state_id("Start")?.index();
        let end_idx = clts.state_id("End")?.index();

        // Both should satisfy (they form a cycle)
        assert!(
            result.get(start_idx).map(|b| *b).unwrap_or(false),
            "Start should satisfy"
        );
        assert!(
            result.get(end_idx).map(|b| *b).unwrap_or(false),
            "End should satisfy"
        );

        Ok(())
    }
}
