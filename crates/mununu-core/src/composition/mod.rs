//! Composition primitives for CLTS instances.
//!
//! The implementation intentionally mirrors the semantics documented in
//! `docs/clts_spec.md`: shared alphabet elements synchronise, while independent
//! actions interleave depending on the selected composition mode. The concrete
//! logic will arrive in a later turn; for now we sketch the public API and
//! cover the expected behaviour with failing tests to guide the implementation.

mod controllability;
pub mod hide;
#[cfg(test)]
mod labels;
pub mod minimize;
mod transition;

use crate::clts::{
    Clts, CltsBuilder, CltsResult, DefaultLabelIdx, DefaultStateIdx, LabelControllability, StateId,
    Transition,
};
use crate::composition::controllability::ControllabilityChecker;
use crate::composition::transition::TransitionKeyBuilder;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

/// Key for state variables.
type StateKey = (usize, usize);
/// Key for state pairs.
type StatePairKey = (usize, usize, usize, usize);
/// Key for label pairs.
type LabelPairKey = (usize, usize);

/// Scratch arenas that deduplicate label/variable vectors while composing.
///
/// The product builder frequently merges identical `(left, right)` states and
/// label unions; interning the resulting `Vec<String>` payloads keeps allocator
/// pressure low and allows cheap comparisons via pointer identity.
#[derive(Default)]
struct ProductStateArena {
    // Cache for interned labels.
    intern: RefCell<HashMap<Vec<String>, Rc<Vec<String>>>>,
    // Cache for interned state variables.
    state_cache: RefCell<HashMap<StateKey, Rc<Vec<String>>>>,
    // Cache for unioned state variables.
    state_union_cache: RefCell<HashMap<StatePairKey, Rc<Vec<String>>>>,
    // Cache for unioned label pairs.
    label_union_cache: RefCell<HashMap<LabelPairKey, Rc<Vec<String>>>>,
}

impl ProductStateArena {
    /// Interns a vector of labels and returns a reference-counted vector of strings.
    fn intern_labels(&self, mut labels: Vec<String>) -> Rc<Vec<String>> {
        labels.sort();
        labels.dedup();
        let mut intern = self.intern.borrow_mut();
        if let Some(existing) = intern.get(&labels) {
            return Rc::clone(existing);
        }
        let rc = Rc::new(labels.clone());
        intern.insert(labels, Rc::clone(&rc));
        rc
    }

    /// Returns the labels for a transition.
    ///
    /// This method collects all labels referenced by the transition and returns
    /// a reference-counted vector of strings. The labels are sorted and deduplicated
    /// to ensure a canonical form.
    ///
    /// # Parameters
    ///
    /// * `clts`: The CLTS instance.
    /// * `transition`: The transition.
    ///
    /// # Returns
    ///
    /// A reference-counted vector of strings.
    fn transition_labels(
        &self,
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        transition: &Transition<DefaultStateIdx, DefaultLabelIdx>,
    ) -> Rc<Vec<String>> {
        let mut labels = Vec::new();
        for label_id in transition.labels() {
            if let Some(payload) = clts.label_payload(*label_id) {
                labels.extend(payload.iter().cloned());
            }
        }
        self.intern_labels(labels)
    }

    /// Returns the variables for a state.
    ///
    /// This method collects all variables referenced by the state and returns
    /// a reference-counted vector of strings. The variables are sorted and deduplicated
    /// to ensure a canonical form.
    ///
    /// # Parameters
    ///
    /// * `clts`: The CLTS instance.
    /// * `state`: The state.
    ///
    /// # Returns
    ///
    /// A reference-counted vector of strings.
    fn state_variables(
        &self,
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        state: StateId<DefaultStateIdx>,
    ) -> Rc<Vec<String>> {
        let key = (clts as *const _ as usize, state.index());
        if let Some(existing) = self.state_cache.borrow().get(&key) {
            return Rc::clone(existing);
        }
        let vars = clts.state_variables(state);
        let rc = self.intern_labels(vars);
        self.state_cache.borrow_mut().insert(key, Rc::clone(&rc));
        rc
    }

    /// Merges the variables for two states.
    ///
    /// This method merges the variables for two states and returns a reference-counted
    /// vector of strings. The variables are sorted and deduplicated to ensure a canonical form.
    ///
    /// # Parameters
    ///
    /// * `left`: The left CLTS instance.
    /// * `l_state`: The left state.
    /// * `right`: The right CLTS instance.
    /// * `r_state`: The right state.
    ///
    /// # Returns
    ///
    /// A reference-counted vector of strings.
    fn merge_state_variables(
        &self,
        left: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        l_state: StateId<DefaultStateIdx>,
        right: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        r_state: StateId<DefaultStateIdx>,
    ) -> Rc<Vec<String>> {
        let key = (
            left as *const _ as usize,
            l_state.index(),
            right as *const _ as usize,
            r_state.index(),
        );
        if let Some(existing) = self.state_union_cache.borrow().get(&key) {
            return Rc::clone(existing);
        }
        let left_vars = self.state_variables(left, l_state);
        let right_vars = self.state_variables(right, r_state);
        let rc = self.union_from_slices(left_vars.as_ref(), right_vars.as_ref());
        self.state_union_cache
            .borrow_mut()
            .insert(key, Rc::clone(&rc));
        rc
    }

    /// Merges the labels for two transitions.
    ///
    /// This method merges the labels for two transitions and returns a reference-counted
    /// vector of strings. The labels are sorted and deduplicated to ensure a canonical form.
    ///
    /// # Parameters
    ///
    /// * `left`: The left transition.
    /// * `right`: The right transition.
    ///
    /// # Returns
    ///
    /// A reference-counted vector of strings.
    fn union_labels(&self, left: &Rc<Vec<String>>, right: &Rc<Vec<String>>) -> Rc<Vec<String>> {
        let left_ptr = Rc::as_ptr(left) as usize;
        let right_ptr = Rc::as_ptr(right) as usize;
        let key = if left_ptr <= right_ptr {
            (left_ptr, right_ptr)
        } else {
            (right_ptr, left_ptr)
        };
        if let Some(existing) = self.label_union_cache.borrow().get(&key) {
            return Rc::clone(existing);
        }
        let rc = self.union_from_slices(left.as_ref(), right.as_ref());
        self.label_union_cache
            .borrow_mut()
            .insert(key, Rc::clone(&rc));
        rc
    }

    /// Merges the labels for two transitions from slices.
    ///
    /// This method merges the labels for two transitions from slices and returns a reference-counted
    /// vector of strings. The labels are sorted and deduplicated to ensure a canonical form.
    ///
    /// # Parameters
    ///
    /// * `left`: The left transition.
    /// * `right`: The right transition.
    ///
    /// # Returns
    ///
    /// A reference-counted vector of strings.
    fn union_from_slices(&self, left: &[String], right: &[String]) -> Rc<Vec<String>> {
        if left.is_empty() && right.is_empty() {
            return self.intern_labels(Vec::new());
        }
        let mut merged = Vec::with_capacity(left.len() + right.len());
        merged.extend(left.iter().cloned());
        merged.extend(right.iter().cloned());
        self.intern_labels(merged)
    }

    /// Intersects the labels with a set of shared labels.
    ///
    /// This method intersects the labels with a set of shared labels and returns a reference-counted
    /// vector of strings. The labels are sorted and deduplicated to ensure a canonical form.
    ///
    /// # Parameters
    ///
    /// * `labels`: The labels.
    /// * `shared`: The shared labels.
    ///
    /// # Returns
    ///
    /// A reference-counted vector of strings.
    fn intersection_with_set(
        &self,
        labels: &Rc<Vec<String>>,
        shared: &BTreeSet<String>,
    ) -> Rc<Vec<String>> {
        let filtered: Vec<String> = labels
            .iter()
            .filter(|label| shared.contains(*label))
            .cloned()
            .collect();
        self.intern_labels(filtered)
    }
}

/// Builder responsible for constructing and interning product states.
#[derive(Default)]
struct ProductStateBuilder {
    state_map:
        HashMap<(StateId<DefaultStateIdx>, StateId<DefaultStateIdx>), StateId<DefaultStateIdx>>,
}

impl ProductStateBuilder {
    /// Ensures the product state `(l_state, r_state)` exists in the builder and
    /// returns its canonical identifier.
    ///
    /// # Parameters
    ///
    /// * `arena`: The product state arena.
    /// * `builder`: The CLTS builder.
    /// * `left`: The left CLTS instance.
    /// * `right`: The right CLTS instance.
    /// * `l_state`: The left state.
    /// * `r_state`: The right state.
    ///
    /// # Returns
    ///
    /// The canonical identifier of the product state.
    fn ensure_state(
        &mut self,
        arena: &ProductStateArena,
        builder: &mut CltsBuilder<DefaultStateIdx, DefaultLabelIdx>,
        left: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        right: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        l_state: StateId<DefaultStateIdx>,
        r_state: StateId<DefaultStateIdx>,
    ) -> StateId<DefaultStateIdx> {
        if let Some(&id) = self.state_map.get(&(l_state, r_state)) {
            return id;
        }

        let left_label = left
            .state_name(l_state)
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("s{}", l_state.index()));
        let right_label = right
            .state_name(r_state)
            .map(|name| name.to_owned())
            .unwrap_or_else(|| format!("s{}", r_state.index()));
        let composed_name = format!("{}|{}", left_label, right_label);

        let state_id = builder
            .state_with_name(composed_name)
            .expect("state identifier overflow");
        if left.initial_states().contains(&l_state) && right.initial_states().contains(&r_state) {
            builder.initial_state_id(state_id);
        }

        let composed_vars = arena.merge_state_variables(left, l_state, right, r_state);
        if !composed_vars.is_empty() {
            builder.with_variables_for_state(state_id, composed_vars.iter().map(|s| s.as_str()));
        }

        self.state_map.insert((l_state, r_state), state_id);
        state_id
    }
}

/// Returns `true` when two sorted label vectors share at least one element.
/// The vectors emitted by the arena are already deduplicated, so a linear merge
/// check keeps the hot path cache-friendly.
///
/// # Parameters
///
/// * `left`: The left labels.
/// * `right`: The right labels.
///
/// # Returns
///
/// `true` when two sorted label vectors share at least one element.
fn labels_have_intersection(left: &[String], right: &[String]) -> bool {
    let mut i = 0;
    let mut j = 0;
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

// (controllability helpers moved to `composition::controllability`)

/// Test-only wrapper around the shared transition-label collector.
#[cfg(test)]
fn transition_label_set(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    transition: &Transition<DefaultStateIdx, DefaultLabelIdx>,
) -> BTreeSet<String> {
    crate::composition::labels::collect_transition_labels(clts, transition)
}

/// Computes the full alphabet referenced by the provided CLTS.
///
/// This delegates to the CLTS-level `alphabet()` helper, which already
/// aggregates symbols across controllable, internal, and uncontrollable
/// alphabets. The result is then normalised into a `BTreeSet` for callers
/// that rely on deterministic ordering.
fn collect_alphabet(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> BTreeSet<String> {
    clts.alphabet().into_iter().collect()
}

/// Registers a product state if it has not been seen and pushes it onto the BFS
/// queue for later processing.
///
/// # Parameters
///
/// * `product`: The product state builder.
/// * `arena`: The product state arena.
/// * `builder`: The CLTS builder.
/// * `left`: The left CLTS instance.
/// * `right`: The right CLTS instance.
/// * `l_state`: The left state.
/// * `r_state`: The right state.
/// * `queue`: The queue.
/// * `discovered`: The discovered states.
///
/// # Returns
///
/// The canonical identifier of the product state.
#[allow(clippy::too_many_arguments)]
fn enqueue_state(
    product: &mut ProductStateBuilder,
    arena: &ProductStateArena,
    builder: &mut CltsBuilder<DefaultStateIdx, DefaultLabelIdx>,
    left: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    right: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    l_state: StateId<DefaultStateIdx>,
    r_state: StateId<DefaultStateIdx>,
    queue: &mut VecDeque<(StateId<DefaultStateIdx>, StateId<DefaultStateIdx>)>,
    discovered: &mut HashSet<(StateId<DefaultStateIdx>, StateId<DefaultStateIdx>)>,
) -> StateId<DefaultStateIdx> {
    let id = product.ensure_state(arena, builder, left, right, l_state, r_state);
    if discovered.insert((l_state, r_state)) {
        queue.push_back((l_state, r_state));
    }
    id
}

/// Composition strategies supported by the CLTS product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionSemantics {
    /// Shared alphabet elements must fire together; independent actions are
    /// collapsed into a single joint step.
    Synchronous,
    /// Shared alphabet elements fire together; independent actions may
    /// interleave in either order as well as proceed jointly.
    ///
    /// SOUNDNESS: over-approx — independent labels can fire freely without
    /// fairness constraints. One side can idle indefinitely while the other
    /// progresses. Sound for safety (extra interleavings are conservative),
    /// unsound for liveness (apparent progress may not exist in reality if
    /// fairness is assumed but not enforced).
    Asynchronous,
    /// Superset semantics combine the union step with both permutations even
    /// when actions are independent.
    Superset,
}

/// Placeholder options structure in case additional flags are required later.
#[derive(Debug, Clone)]
pub struct CompositionOptions {
    pub semantics: CompositionSemantics,
}

impl CompositionOptions {
    /// Creates a new set of composition options bound to the requested
    /// semantics.
    pub fn new(semantics: CompositionSemantics) -> Self {
        Self { semantics }
    }
}

/// Compose two CLTS instances according to the requested semantics.
///
/// The resulting CLTS contains only reachable product states, mirroring the
/// semantics set out in `docs/clts_spec.md`.
///
/// # Parameters
///
/// * `left`: The left CLTS instance.
/// * `right`: The right CLTS instance.
/// * `options`: The composition options.
///
/// # Returns
///
/// The composed CLTS instance.
///
/// # Errors
///
/// Returns an error if the composition is invalid.
pub fn compose(
    left: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    right: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    options: &CompositionOptions,
) -> CltsResult<Clts<DefaultStateIdx, DefaultLabelIdx>> {
    // Validate mutual exclusivity of controllable and internal actions.
    ControllabilityChecker::validate_composition(left, right)?;

    let left_alphabet = collect_alphabet(left);
    let right_alphabet = collect_alphabet(right);
    let shared_alphabet: BTreeSet<String> = left_alphabet
        .intersection(&right_alphabet)
        .cloned()
        .collect();
    let mut product_builder = ProductStateBuilder::default();
    let mut builder = Clts::builder();
    let mut pending_transitions = HashSet::new();
    let mut queue = VecDeque::new();
    let mut discovered = HashSet::new();
    let mut visited = HashSet::new();

    let left_initials: Vec<_> = left.initial_states().iter().copied().collect();
    let right_initials: Vec<_> = right.initial_states().iter().copied().collect();

    // Coverage Status: Covered by test `composition_with_empty_initial_states`
    if left_initials.is_empty() || right_initials.is_empty() {
        // If either side has no initial states, return an empty CLTS.
        return Clts::builder().build();
    }

    let arena = ProductStateArena::default();

    for &l_init in &left_initials {
        for &r_init in &right_initials {
            enqueue_state(
                &mut product_builder,
                &arena,
                &mut builder,
                left,
                right,
                l_init,
                r_init,
                &mut queue,
                &mut discovered,
            );
        }
    }

    // Main composition loop.
    while let Some((l_state, r_state)) = queue.pop_front() {
        if !visited.insert((l_state, r_state)) {
            continue;
        }

        let composed_id =
            product_builder.ensure_state(&arena, &mut builder, left, right, l_state, r_state);

        let left_out = left.outgoing(l_state);
        let right_out = right.outgoing(r_state);

        for lt in left_out.iter() {
            let left_labels = arena.transition_labels(left, lt);
            let left_shared_global = arena.intersection_with_set(&left_labels, &shared_alphabet);
            let mut matched = false;
            let mut left_perm_state: Option<StateId<DefaultStateIdx>> = None;
            let mut right_perm_targets: HashSet<StateId<DefaultStateIdx>> = HashSet::new();

            for rt in right_out.iter() {
                let right_labels = arena.transition_labels(right, rt);
                let right_shared_global =
                    arena.intersection_with_set(&right_labels, &shared_alphabet);
                if left_shared_global != right_shared_global {
                    continue;
                }

                // Fast path: intersect using the arena's sorted vectors so we avoid
                // allocating temporary `BTreeSet`s for every pair of transitions.
                let shared_actual_empty =
                    !labels_have_intersection(left_labels.as_ref(), right_labels.as_ref());
                let union_labels_set = arena.union_labels(&left_labels, &right_labels);

                match options.semantics {
                    CompositionSemantics::Synchronous => {
                        let union_target = enqueue_state(
                            &mut product_builder,
                            &arena,
                            &mut builder,
                            left,
                            right,
                            lt.target(),
                            rt.target(),
                            &mut queue,
                            &mut discovered,
                        );
                        let has_uncontrollable =
                            ControllabilityChecker::composed_has_uncontrollable_labels(
                                lt, rt, left, right,
                            );
                        pending_transitions.insert(TransitionKeyBuilder::create_key(
                            composed_id,
                            union_target,
                            Rc::clone(&union_labels_set),
                            has_uncontrollable,
                        ));
                        matched = true;
                    }
                    CompositionSemantics::Asynchronous => {
                        if shared_actual_empty {
                            let left_target = left_perm_state.unwrap_or_else(|| {
                                let name = enqueue_state(
                                    &mut product_builder,
                                    &arena,
                                    &mut builder,
                                    left,
                                    right,
                                    lt.target(),
                                    r_state,
                                    &mut queue,
                                    &mut discovered,
                                );
                                left_perm_state = Some(name);
                                name
                            });
                            pending_transitions.insert(TransitionKeyBuilder::create_key(
                                composed_id,
                                left_target,
                                Rc::clone(&left_labels),
                                lt.is_uncontrollable(left),
                            ));

                            if right_perm_targets.insert(rt.target()) {
                                let right_target = enqueue_state(
                                    &mut product_builder,
                                    &arena,
                                    &mut builder,
                                    left,
                                    right,
                                    l_state,
                                    rt.target(),
                                    &mut queue,
                                    &mut discovered,
                                );
                                pending_transitions.insert(TransitionKeyBuilder::create_key(
                                    composed_id,
                                    right_target,
                                    Rc::clone(&right_labels),
                                    rt.is_uncontrollable(right),
                                ));
                            }
                            matched = true;
                        } else {
                            let union_target = enqueue_state(
                                &mut product_builder,
                                &arena,
                                &mut builder,
                                left,
                                right,
                                lt.target(),
                                rt.target(),
                                &mut queue,
                                &mut discovered,
                            );
                            let has_uncontrollable =
                                ControllabilityChecker::composed_has_uncontrollable_labels(
                                    lt, rt, left, right,
                                );
                            pending_transitions.insert(TransitionKeyBuilder::create_key(
                                composed_id,
                                union_target,
                                Rc::clone(&union_labels_set),
                                has_uncontrollable,
                            ));
                            matched = true;
                        }
                    }
                    CompositionSemantics::Superset => {
                        // Coverage Status: Superset composition is tested, but edge cases need more coverage
                        // TODO: Add tests for superset composition with various label combinations
                        let union_target = enqueue_state(
                            &mut product_builder,
                            &arena,
                            &mut builder,
                            left,
                            right,
                            lt.target(),
                            rt.target(),
                            &mut queue,
                            &mut discovered,
                        );
                        let has_uncontrollable =
                            ControllabilityChecker::composed_has_uncontrollable_labels(
                                lt, rt, left, right,
                            );
                        pending_transitions.insert(TransitionKeyBuilder::create_key(
                            composed_id,
                            union_target,
                            Rc::clone(&union_labels_set),
                            has_uncontrollable,
                        ));
                        if shared_actual_empty {
                            let left_target = left_perm_state.unwrap_or_else(|| {
                                let name = enqueue_state(
                                    &mut product_builder,
                                    &arena,
                                    &mut builder,
                                    left,
                                    right,
                                    lt.target(),
                                    r_state,
                                    &mut queue,
                                    &mut discovered,
                                );
                                left_perm_state = Some(name);
                                name
                            });
                            pending_transitions.insert(TransitionKeyBuilder::create_key(
                                composed_id,
                                left_target,
                                Rc::clone(&left_labels),
                                lt.is_uncontrollable(left),
                            ));

                            if right_perm_targets.insert(rt.target()) {
                                let right_target = enqueue_state(
                                    &mut product_builder,
                                    &arena,
                                    &mut builder,
                                    left,
                                    right,
                                    l_state,
                                    rt.target(),
                                    &mut queue,
                                    &mut discovered,
                                );
                                pending_transitions.insert(TransitionKeyBuilder::create_key(
                                    composed_id,
                                    right_target,
                                    Rc::clone(&right_labels),
                                    rt.is_uncontrollable(right),
                                ));
                            }
                        }
                        matched = true;
                    }
                }
            }

            if matches!(
                options.semantics,
                CompositionSemantics::Asynchronous | CompositionSemantics::Superset
            ) && !matched
                && left_shared_global.is_empty()
            {
                let left_target = left_perm_state.unwrap_or_else(|| {
                    enqueue_state(
                        &mut product_builder,
                        &arena,
                        &mut builder,
                        left,
                        right,
                        lt.target(),
                        r_state,
                        &mut queue,
                        &mut discovered,
                    )
                });
                pending_transitions.insert(TransitionKeyBuilder::create_key(
                    composed_id,
                    left_target,
                    Rc::clone(&left_labels),
                    lt.is_uncontrollable(left),
                ));
            }
        }

        if matches!(
            options.semantics,
            CompositionSemantics::Asynchronous | CompositionSemantics::Superset
        ) {
            for rt in right_out.iter() {
                let right_labels = arena.transition_labels(right, rt);
                let right_shared_global =
                    arena.intersection_with_set(&right_labels, &shared_alphabet);
                if right_shared_global.is_empty() {
                    let right_target = enqueue_state(
                        &mut product_builder,
                        &arena,
                        &mut builder,
                        left,
                        right,
                        l_state,
                        rt.target(),
                        &mut queue,
                        &mut discovered,
                    );
                    pending_transitions.insert(TransitionKeyBuilder::create_key(
                        composed_id,
                        right_target,
                        Rc::clone(&right_labels),
                        rt.is_uncontrollable(right),
                    ));
                }
            }
        }
    }

    let mut pending_vec: Vec<_> = pending_transitions.into_iter().collect();
    pending_vec.sort_by(|a, b| {
        (
            a.source.index(),
            a.target.index(),
            a.labels.as_slice(),
            a.has_uncontrollable_labels,
        )
            .cmp(&(
                b.source.index(),
                b.target.index(),
                b.labels.as_slice(),
                b.has_uncontrollable_labels,
            ))
    });

    for key in pending_vec {
        let label_id = {
            let labels = builder.labels();
            labels.intern(key.labels.iter().map(|s| s.as_str()))?
        };
        // Set label controllability if the transition has uncontrollable labels
        if key.has_uncontrollable_labels {
            builder.set_label_controllability(label_id, LabelControllability::Uncontrollable);
        }
        builder.transition_ids(key.source, &[label_id], key.target);
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

    fn singleton(
        label: &[&str],
        vars: &[&str],
    ) -> TestResult<Clts<DefaultStateIdx, DefaultLabelIdx>> {
        let mut builder = Clts::builder();
        builder.state("s0");
        builder.initial("s0");
        builder.state("s1");
        if !label.is_empty() {
            let id = builder.labels().intern(label.to_vec())?;
            builder.transition("s0", &[id], "s1");
        }
        if !vars.is_empty() {
            builder.with_variables("s1", vars.to_vec());
        }
        builder.build().map_err(|err| err.into())
    }

    fn outgoing_labels(
        clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
        state: &str,
    ) -> TestResult<Vec<(String, BTreeSet<String>)>> {
        let state_id = clts.state_id(state)?;
        Ok(clts
            .outgoing(state_id)
            .iter()
            .map(|transition| {
                let target = clts
                    .state_name(transition.target())
                    .unwrap_or("?")
                    .to_owned();
                let labels = transition_label_set(clts, transition);
                (target, labels)
            })
            .collect())
    }

    fn label_set(symbols: &[&str]) -> BTreeSet<String> {
        symbols.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn synchronous_composition_merges_shared_alphabet() -> TestResult {
        // Use different controllable labels for each automaton (mutual exclusivity requirement)
        // Shared labels for synchronization should be uncontrollable
        let mut left_builder = Clts::builder();
        let left_req = left_builder.labels().intern(["req"])?;
        left_builder
            .set_label_controllability(left_req, crate::clts::LabelControllability::Uncontrollable);
        left_builder.state("s0").initial("s0");
        left_builder.state("s1");
        left_builder.transition("s0", &[left_req], "s1");
        let left = left_builder.build()?;

        let mut right_builder = Clts::builder();
        let right_req = right_builder.labels().intern(["req"])?;
        let right_ack = right_builder.labels().intern(["ack"])?;
        right_builder.set_label_controllability(
            right_req,
            crate::clts::LabelControllability::Uncontrollable,
        );
        right_builder.state("s0").initial("s0");
        right_builder.state("s1");
        right_builder.transition("s0", &[right_req, right_ack], "s1");
        let right = right_builder.build()?;

        let options = CompositionOptions::new(CompositionSemantics::Synchronous);

        let product = compose(&left, &right, &options)?;
        let arcs = outgoing_labels(&product, "s0|s0")?;
        assert_eq!(arcs.len(), 1);
        assert_eq!(arcs[0].0, "s1|s1");
        assert_eq!(arcs[0].1, label_set(&["ack", "req"]));
        Ok(())
    }

    #[test]
    fn asynchronous_composition_interleaves_independent_actions() -> TestResult {
        // Use different controllable labels for each automaton (mutual exclusivity requirement)
        let left = singleton(&["produce"], &[])?;
        let right = singleton(&["consume"], &[])?;
        let options = CompositionOptions::new(CompositionSemantics::Asynchronous);

        let product = compose(&left, &right, &options)?;
        let arcs = outgoing_labels(&product, "s0|s0")?;
        let map: HashMap<_, _> = arcs.into_iter().collect();
        assert_eq!(map.len(), 2);
        assert_eq!(map["s1|s0"], label_set(&["produce"]));
        assert_eq!(map["s0|s1"], label_set(&["consume"]));
        Ok(())
    }

    #[test]
    fn superset_composition_includes_union_and_permutations() -> TestResult {
        // Use different controllable labels for each automaton (mutual exclusivity requirement)
        let left = singleton(&["a"], &["x"])?;
        let right = singleton(&["b"], &["y"])?;
        let options = CompositionOptions::new(CompositionSemantics::Superset);

        let product = compose(&left, &right, &options)?;
        let arcs = outgoing_labels(&product, "s0|s0")?;
        let map: HashMap<_, _> = arcs.into_iter().collect();
        assert_eq!(map.len(), 3);
        assert_eq!(map["s1|s0"], label_set(&["a"]));
        assert_eq!(map["s0|s1"], label_set(&["b"]));
        assert_eq!(map["s1|s1"], label_set(&["a", "b"]));

        let vars = {
            let id = product.state_id("s1|s1")?;
            let mut collected = product.state_variables(id);
            collected.sort();
            collected
        };
        assert_eq!(vars, vec!["x".to_string(), "y".to_string()]);
        Ok(())
    }

    #[test]
    fn composition_preserves_controllability() -> TestResult {
        // Test that controllability information is preserved during composition
        let mut left_builder = Clts::builder();
        left_builder.state("s0");
        left_builder.initial("s0");
        left_builder.state("s1");
        let s0 = left_builder.state_id_or_insert("s0").unwrap();
        let s1 = left_builder.state_id_or_insert("s1").unwrap();
        let input_label = left_builder.labels().intern(["input"])?;
        left_builder.set_label_controllability(input_label, LabelControllability::Uncontrollable);
        left_builder.transition_ids(s0, &[input_label], s1);
        let left = left_builder.build()?;

        let mut right_builder = Clts::builder();
        right_builder.state("t0");
        right_builder.initial("t0");
        right_builder.state("t1");
        let t0 = right_builder.state_id_or_insert("t0").unwrap();
        let t1 = right_builder.state_id_or_insert("t1").unwrap();
        let output_label = right_builder.labels().intern(["output"])?;
        right_builder.transition_ids(t0, &[output_label], t1);
        let right = right_builder.build()?;

        let options = CompositionOptions::new(CompositionSemantics::Asynchronous);
        let composed = compose(&left, &right, &options)?;

        // Find transitions in the composed CLTS
        let s0_t0 = composed.state_id("s0|t0")?;
        let transitions: Vec<_> = composed.outgoing(s0_t0).iter().collect();

        // The composed CLTS should have transitions, and independent transitions
        // should preserve their controllability
        let has_uncontrollable = transitions.iter().any(|t| t.is_uncontrollable(&composed));
        let has_controllable = transitions.iter().any(|t| t.is_controllable(&composed));

        // Since left has uncontrollable and right has controllable, both should be present
        // in asynchronous composition (they interleave independently)
        assert!(
            has_uncontrollable && has_controllable,
            "Composed CLTS should preserve controllability: found uncontrollable={}, controllable={}",
            has_uncontrollable,
            has_controllable
        );

        Ok(())
    }

    #[test]
    fn composition_skips_unreachable_pairs() -> TestResult {
        let mut left = Clts::builder();
        left.state("s0");
        left.initial("s0");
        left.state("s1");
        left.state("dead");
        let label = left.labels().intern(["tick"])?;
        left.transition("s0", &[label], "s1");
        let left = left.build()?;

        let right = singleton(&[], &[])?;
        let options = CompositionOptions::new(CompositionSemantics::Synchronous);

        let product = compose(&left, &right, &options)?;
        let names: Vec<_> = product
            .states()
            .map(|state| product.state_name(state).unwrap_or("?").to_owned())
            .collect();
        assert!(!names.iter().any(|name| name.contains("dead")));
        Ok(())
    }

    #[test]
    fn composition_with_empty_initial_states() -> TestResult {
        // Test composition when one CLTS has no initial states (line 358)
        let mut left = Clts::builder();
        left.state("s0"); // No initial state set
        let left_clts = left.build()?;

        let mut right = Clts::builder();
        right.state("t0").initial("t0");
        let right_clts = right.build()?;

        let options = CompositionOptions {
            semantics: CompositionSemantics::Synchronous,
        };

        let composed = compose(&left_clts, &right_clts, &options)?;
        // Should return empty CLTS when one side has no initial states
        assert_eq!(composed.state_count(), 0);

        // Test reverse case
        let mut left2 = Clts::builder();
        left2.state("s0").initial("s0");
        let left_clts2 = left2.build()?;

        let mut right2 = Clts::builder();
        right2.state("t0"); // No initial state
        let right_clts2 = right2.build()?;

        let composed2 = compose(&left_clts2, &right_clts2, &options)?;
        assert_eq!(composed2.state_count(), 0);

        Ok(())
    }

    #[test]
    fn superset_composition_handles_label_permutations() -> TestResult {
        // Test superset composition edge cases (lines 486-494, 496-501, 503)
        let mut left = Clts::builder();
        let label_a = left.labels().intern(["a"])?;
        let label_b = left.labels().intern(["b"])?;
        left.state("s0").initial("s0");
        left.state("s1");
        left.transition("s0", &[label_a], "s1");
        left.transition("s0", &[label_b], "s1");
        let left_clts = left.build()?;

        let mut right = Clts::builder();
        let label_c = right.labels().intern(["c"])?;
        right.state("t0").initial("t0");
        right.state("t1");
        right.transition("t0", &[label_c], "t1");
        let right_clts = right.build()?;

        let options = CompositionOptions {
            semantics: CompositionSemantics::Superset,
        };

        let composed = compose(&left_clts, &right_clts, &options)?;
        // Superset composition should include all label combinations
        assert!(composed.state_count() > 0);

        // Verify transitions exist for various label combinations
        // State names use format "left|right"
        let s0_t0 = composed.state_id("s0|t0")?;
        let outgoing = composed.outgoing(s0_t0);
        assert!(!outgoing.is_empty());

        Ok(())
    }

    #[test]
    fn composition_handles_unreachable_states() -> TestResult {
        // Test composition with unreachable states (line 381)
        let mut left = Clts::builder();
        left.state("s0").initial("s0");
        left.state("s1"); // Unreachable from s0
        let left_clts = left.build()?;

        let mut right = Clts::builder();
        right.state("t0").initial("t0");
        right.state("t1"); // Unreachable from t0
        let right_clts = right.build()?;

        let options = CompositionOptions {
            semantics: CompositionSemantics::Synchronous,
        };

        let composed = compose(&left_clts, &right_clts, &options)?;
        // Should only include reachable states (s0|t0)
        assert_eq!(composed.state_count(), 1);
        assert!(composed.state_id("s0|t0").is_ok());
        assert!(composed.state_id("s1|t1").is_err());

        Ok(())
    }
}
